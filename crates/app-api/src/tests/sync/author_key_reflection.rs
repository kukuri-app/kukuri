//! #1239: author replica の反映(profile・follow・block・follow の通知の起点・custom reaction の asset)が、
//! replica を走査せず、読む量が follow・block の総数に依存しないことを固定する。

use super::shadowing_docs::ShadowingDocsSync;
use super::*;
use crate::service::author_state_support::AUTHOR_EDGE_KEYS;

/// `author_keys` の author replica に、`count` 件の follow edge(相手は毎回新しい author)を書く。
async fn put_follow_edges(docs_sync: &dyn DocsSync, author_keys: &KukuriKeys, count: usize) {
    for _ in 0..count {
        put_follow_edge(
            docs_sync,
            author_keys,
            generate_keys().public_key_hex().as_str(),
        )
        .await;
    }
}

async fn put_follow_edge(docs_sync: &dyn DocsSync, author_keys: &KukuriKeys, target: &str) {
    let envelope =
        build_follow_edge_envelope(author_keys, &Pubkey::from(target), FollowEdgeStatus::Active)
            .expect("build follow edge");
    let edge = parse_follow_edge(&envelope)
        .expect("parse follow edge")
        .expect("follow edge");
    persist_follow_edge_doc(docs_sync, &edge, &envelope)
        .await
        .expect("persist follow edge doc");
}

fn counting_app(docs_sync: Arc<CountingDocsSync>) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    (app, store)
}

fn assert_no_prefix_read(queries: &[(String, DocQuery)], operation: &str) {
    let prefix_reads = queries
        .iter()
        .filter(|(_, query)| matches!(query, DocQuery::Prefix(_)))
        .collect::<Vec<_>>();
    assert!(
        prefix_reads.is_empty(),
        "{operation} must not read a whole prefix of the author replica: {prefix_reads:?}"
    );
}

// 起動時と追いつきの反映は、follow の総数が上限を超えても、読む量が同じ(上限の件数だけ読む)。
#[tokio::test]
async fn author_state_reads_are_bounded_by_the_edge_key_limit() {
    let mut counts = Vec::new();
    for edges in [AUTHOR_EDGE_KEYS + 20, AUTHOR_EDGE_KEYS + 300] {
        let docs_sync = Arc::new(CountingDocsSync::default());
        let (app, store) = counting_app(docs_sync.clone());
        let remote_keys = generate_keys();
        let remote_pubkey = remote_keys.public_key_hex();
        put_follow_edges(docs_sync.as_ref(), &remote_keys, edges).await;
        docs_sync.clear_queries().await;
        docs_sync.reset_records_returned();

        let reflected = hydrate_author_state(
            &app.services,
            app.current_author_pubkey().as_str(),
            remote_pubkey.as_str(),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate author state");

        assert_eq!(reflected, AUTHOR_EDGE_KEYS, "reflects up to the key limit");
        assert_no_prefix_read(&docs_sync.queries().await, "hydrate_author_state");
        assert_eq!(
            store
                .list_follow_edges_by_subject(remote_pubkey.as_str())
                .await
                .expect("follow edges")
                .len(),
            AUTHOR_EDGE_KEYS
        );
        counts.push(docs_sync.records_returned());
    }
    assert_eq!(
        counts[0], counts[1],
        "docs records read must not depend on the number of follow edges"
    );
}

// 自分の custom reaction の asset は、同じ key を別の名義が書いても一覧から消えない。
#[tokio::test]
async fn a_shadowed_custom_reaction_asset_is_still_listed() {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author("f".repeat(64)));
    let keys = generate_keys();
    let author_pubkey = keys.public_key_hex();
    let replica = author_replica_id(author_pubkey.as_str());
    let asset = |author: &str| CustomReactionAssetDocV1 {
        asset_id: "asset-1".into(),
        author_pubkey: Pubkey::from(author),
        blob_hash: BlobHash::new("a".repeat(64)),
        search_key: String::new(),
        mime: "image/png".into(),
        bytes: 1,
        width: 1,
        height: 1,
        created_at: 1,
        updated_at: 1,
        envelope_id: EnvelopeId::from("asset-envelope"),
    };
    docs_sync
        .open_replica(&replica)
        .await
        .expect("open replica");
    let key = stable_key("reactions/assets", "asset-1/state");
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: key.clone(),
                value: serde_json::to_value(asset(author_pubkey.as_str())).expect("asset json"),
            },
        )
        .await
        .expect("write asset");
    let other = generate_keys().public_key_hex();
    docs_sync
        .shadow(
            key.as_str(),
            serde_json::to_value(asset(other.as_str())).expect("shadow json"),
        )
        .await;

    let assets = load_custom_reaction_assets_from_author_replica(
        docs_sync.as_ref(),
        author_pubkey.as_str(),
        None,
    )
    .await
    .expect("assets");

    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].author_pubkey.as_str(), author_pubkey);
}

async fn put_follow_edge_with_status(
    docs_sync: &dyn DocsSync,
    author_keys: &KukuriKeys,
    target: &str,
    status: FollowEdgeStatus,
) -> KukuriEnvelope {
    let envelope = build_follow_edge_envelope(author_keys, &Pubkey::from(target), status)
        .expect("build follow edge");
    let edge = parse_follow_edge(&envelope)
        .expect("parse follow edge")
        .expect("follow edge");
    persist_follow_edge_doc(docs_sync, &edge, &envelope)
        .await
        .expect("persist follow edge doc");
    envelope
}

// envelope の key に別の正しい envelope が置かれても、id の一致する envelope を返す。
#[tokio::test]
async fn the_envelope_fetch_returns_the_envelope_with_the_requested_id() {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author("f".repeat(64)));
    let remote_keys = generate_keys();
    let replica = author_replica_id(remote_keys.public_key_hex().as_str());
    let wanted = put_follow_edge_with_status(
        docs_sync.as_ref(),
        &remote_keys,
        generate_keys().public_key_hex().as_str(),
        FollowEdgeStatus::Active,
    )
    .await;
    let other = put_follow_edge_with_status(
        docs_sync.as_ref(),
        &remote_keys,
        generate_keys().public_key_hex().as_str(),
        FollowEdgeStatus::Active,
    )
    .await;
    docs_sync
        .shadow(
            stable_key("envelopes", wanted.id.as_str()).as_str(),
            serde_json::to_value(&other).expect("envelope json"),
        )
        .await;

    let fetched = fetch_author_envelope_by_id(
        docs_sync.as_ref(),
        &replica,
        &wanted.id,
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("fetch")
    .expect("envelope");

    assert_eq!(fetched.id, wanted.id);
}
