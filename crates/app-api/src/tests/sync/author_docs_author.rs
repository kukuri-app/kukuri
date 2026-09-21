//! #1239 / ADR 0053 §6: author replica の record を、著者が署名つきで申告した docs author と key の組で読む。
//! 他の名義の record は何件あっても読まないので、上限つきの読み出しを超える数のごみを置かれても隠されない。

use super::shadowing_docs::ShadowingDocsSync;
use super::*;

const ACCOUNT_DOCS_AUTHOR: &str =
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
/// 上限つきの読み出し(同じ key で 8 件)を超える数。
const SHADOWS: usize = 10;

fn app_over(docs_sync: Arc<ShadowingDocsSync>) -> (AppService, Arc<MemoryStore>) {
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

/// `keys` の著者が、docs author を申告した profile を書く。
async fn put_profile(docs_sync: &dyn DocsSync, keys: &KukuriKeys, docs_author: Option<&str>) {
    let envelope = build_profile_envelope_with_docs_author(
        keys,
        &KukuriProfileEnvelopeContentV1 {
            author_pubkey: keys.public_key(),
            name: Some("remote".into()),
            display_name: None,
            about: None,
            picture_asset: None,
        },
        docs_author,
    )
    .expect("profile envelope");
    let profile = parse_profile(&envelope)
        .expect("parse profile")
        .expect("profile");
    persist_profile_doc(docs_sync, &profile, &envelope)
        .await
        .expect("persist profile");
}

/// `keys` の著者が、docs author を申告した follow を書く。
async fn put_follow(docs_sync: &dyn DocsSync, keys: &KukuriKeys, target: &str) {
    let envelope = build_follow_edge_envelope_with_docs_author(
        keys,
        &Pubkey::from(target),
        FollowEdgeStatus::Active,
        Some(ACCOUNT_DOCS_AUTHOR),
    )
    .expect("follow envelope");
    let edge = parse_follow_edge(&envelope)
        .expect("parse follow")
        .expect("follow");
    persist_follow_edge_doc(docs_sync, &edge, &envelope)
        .await
        .expect("persist follow");
}

async fn shadow_many(docs_sync: &ShadowingDocsSync, key: &str) {
    for index in 0..SHADOWS {
        docs_sync
            .shadow(key, serde_json::json!({ "garbage": index }))
            .await;
    }
}

// profile・follow・block・custom reaction の asset の envelope は、docs author を申告する tag を持てる。
#[test]
fn author_envelopes_declare_the_docs_author() {
    let keys = generate_keys();
    let target = Pubkey::from(generate_keys().public_key_hex().as_str());
    let profile = build_profile_envelope_with_docs_author(
        &keys,
        &KukuriProfileEnvelopeContentV1 {
            author_pubkey: keys.public_key(),
            name: None,
            display_name: None,
            about: None,
            picture_asset: None,
        },
        Some(ACCOUNT_DOCS_AUTHOR),
    )
    .expect("profile");
    let follow = build_follow_edge_envelope_with_docs_author(
        &keys,
        &target,
        FollowEdgeStatus::Active,
        Some(ACCOUNT_DOCS_AUTHOR),
    )
    .expect("follow");
    let block = build_block_edge_envelope_with_docs_author(
        &keys,
        &target,
        BlockEdgeStatus::Active,
        Some(ACCOUNT_DOCS_AUTHOR),
    )
    .expect("block");
    let asset = build_custom_reaction_asset_envelope_with_docs_author(
        &keys,
        BlobHash::new("a".repeat(64)),
        "search".into(),
        "image/png".into(),
        1,
        1,
        1,
        Some(ACCOUNT_DOCS_AUTHOR),
    )
    .expect("asset");
    for envelope in [&profile, &follow, &block, &asset] {
        envelope.verify().expect("signed");
        assert_eq!(envelope.docs_author(), Some(ACCOUNT_DOCS_AUTHOR));
    }
    assert!(
        build_follow_edge_envelope_with_docs_author(
            &keys,
            &target,
            FollowEdgeStatus::Active,
            Some("not-a-docs-author"),
        )
        .is_err()
    );
}

// 上限つきの読み出しを超える数のごみが同じ key にあっても、docs author が分かれば、組で 1 件読んで反映する。
// docs author は、署名つきの profile の tag から覚える(同じ回の follow の読み出しから使う)。
#[tokio::test]
async fn a_learned_docs_author_reads_the_edge_behind_any_number_of_shadows() {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author(
        ACCOUNT_DOCS_AUTHOR,
    ));
    let (app, store) = app_over(docs_sync.clone());
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    put_profile(docs_sync.as_ref(), &remote_keys, Some(ACCOUNT_DOCS_AUTHOR)).await;
    put_follow(
        docs_sync.as_ref(),
        &remote_keys,
        local_author_pubkey.as_str(),
    )
    .await;
    let key = stable_key("graph/follows", local_author_pubkey.as_str());
    shadow_many(docs_sync.as_ref(), key.as_str()).await;

    // docs author を知らないうちは、上限つきの読み出しがごみで埋まる(旧 record の best effort)。
    let before = hydrate_author_key(
        &app.services,
        local_author_pubkey.as_str(),
        remote_pubkey.as_str(),
        key.as_str(),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("hydrate before learning");
    assert_eq!(before.reflected, 0);

    hydrate_author_state(
        &app.services,
        local_author_pubkey.as_str(),
        remote_pubkey.as_str(),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("hydrate author state");

    assert_eq!(
        store
            .get_author_docs_author(remote_pubkey.as_str())
            .await
            .expect("docs author")
            .as_deref(),
        Some(ACCOUNT_DOCS_AUTHOR),
        "learned from the signed profile"
    );
    assert!(
        store
            .get_author_relationship(local_author_pubkey.as_str(), remote_pubkey.as_str())
            .await
            .expect("relationship")
            .is_some_and(|row| row.followed_by),
        "the follow of me is read by the docs author and key"
    );
    assert!(
        docs_sync
            .author_reads
            .lock()
            .await
            .contains(&(ACCOUNT_DOCS_AUTHOR.to_string(), key.clone()))
    );
}

// tag の無い envelope(旧 client)からは docs author を覚えない。
#[tokio::test]
async fn an_envelope_without_the_tag_does_not_set_the_docs_author() {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author(
        ACCOUNT_DOCS_AUTHOR,
    ));
    let (app, store) = app_over(docs_sync.clone());
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    put_profile(docs_sync.as_ref(), &remote_keys, None).await;

    hydrate_author_state(
        &app.services,
        app.current_author_pubkey().as_str(),
        remote_pubkey.as_str(),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("hydrate author state");

    assert_eq!(
        store
            .get_author_docs_author(remote_pubkey.as_str())
            .await
            .expect("docs author"),
        None
    );
}

// 自分の custom reaction の asset は、自分の docs author と key の組で読むので、ごみが何件あっても一覧から消えない。
#[tokio::test]
async fn own_custom_reaction_assets_are_read_by_the_own_docs_author() {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author(
        ACCOUNT_DOCS_AUTHOR,
    ));
    let keys = generate_keys();
    let author_pubkey = keys.public_key_hex();
    let replica = author_replica_id(author_pubkey.as_str());
    docs_sync
        .open_replica(&replica)
        .await
        .expect("open replica");
    let key = stable_key("reactions/assets", "asset-1/state");
    let asset = CustomReactionAssetDocV1 {
        asset_id: "asset-1".into(),
        author_pubkey: Pubkey::from(author_pubkey.as_str()),
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
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: key.clone(),
                value: serde_json::to_value(&asset).expect("asset json"),
            },
        )
        .await
        .expect("write asset");
    let other = generate_keys().public_key_hex();
    for _ in 0..SHADOWS {
        docs_sync
            .shadow(
                key.as_str(),
                serde_json::to_value(CustomReactionAssetDocV1 {
                    author_pubkey: Pubkey::from(other.as_str()),
                    ..asset.clone()
                })
                .expect("shadow json"),
            )
            .await;
    }

    let by_author = load_custom_reaction_assets_from_author_replica(
        docs_sync.as_ref(),
        author_pubkey.as_str(),
        Some(ACCOUNT_DOCS_AUTHOR),
    )
    .await
    .expect("assets by docs author");
    assert_eq!(by_author.len(), 1);

    let without = load_custom_reaction_assets_from_author_replica(
        docs_sync.as_ref(),
        author_pubkey.as_str(),
        None,
    )
    .await
    .expect("assets without docs author");
    assert!(
        without.is_empty(),
        "without the docs author, the bounded read is filled by the shadows"
    );
}
