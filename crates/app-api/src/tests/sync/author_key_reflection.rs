//! #1239: author replica の反映(profile・follow・block・follow の通知の起点・custom reaction の asset)が、
//! replica を走査せず、読む量が follow・block の総数に依存しないことを固定する。

use super::shadowing_docs::ShadowingDocsSync;
use super::subscription_catch_up::InjectedNoticesDocsSync;
use super::*;
use crate::service::author_state_support::AUTHOR_EDGE_KEYS;
use kukuri_docs_sync::ReplicaNotice;

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

// docs の event は、その key だけを読んで反映する。同じ edge をもう一度反映しても、変化は 0 件。
// 対象外の key は何も読まない。
#[tokio::test]
async fn an_author_doc_event_reads_only_its_key() {
    let mut counts = Vec::new();
    for edges in [10usize, 400] {
        let docs_sync = Arc::new(CountingDocsSync::default());
        let (app, store) = counting_app(docs_sync.clone());
        let local_author_pubkey = app.current_author_pubkey();
        let remote_keys = generate_keys();
        let remote_pubkey = remote_keys.public_key_hex();
        put_follow_edges(docs_sync.as_ref(), &remote_keys, edges).await;
        put_follow_edge(
            docs_sync.as_ref(),
            &remote_keys,
            local_author_pubkey.as_str(),
        )
        .await;
        docs_sync.clear_queries().await;
        docs_sync.reset_records_returned();

        let key = stable_key("graph/follows", local_author_pubkey.as_str());
        let first = hydrate_author_key(
            &app.services,
            local_author_pubkey.as_str(),
            remote_pubkey.as_str(),
            key.as_str(),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate the follow key");
        assert_eq!((first.reflected, first.changed), (1, 1));
        assert_no_prefix_read(&docs_sync.queries().await, "hydrate_author_key");
        counts.push(docs_sync.records_returned());
        let relationship = store
            .get_author_relationship(local_author_pubkey.as_str(), remote_pubkey.as_str())
            .await
            .expect("relationship")
            .expect("relationship row");
        assert!(relationship.followed_by, "the relationship is rebuilt");

        let again = hydrate_author_key(
            &app.services,
            local_author_pubkey.as_str(),
            remote_pubkey.as_str(),
            key.as_str(),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate the same key again");
        assert_eq!((again.reflected, again.changed), (1, 0));

        docs_sync.reset_records_returned();
        let unrelated = hydrate_author_key(
            &app.services,
            local_author_pubkey.as_str(),
            remote_pubkey.as_str(),
            "profile/posts/unrelated",
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("an unrelated key");
        assert_eq!((unrelated.reflected, unrelated.changed), (0, 0));
        assert_eq!(
            docs_sync.records_returned(),
            0,
            "an unrelated key reads nothing"
        );
    }
    assert_eq!(
        counts[0], counts[1],
        "docs records read by one event must not depend on the number of follow edges"
    );
}

// follow の通知の起点は、自分を指す follow の key 1 件だけを読む。
#[tokio::test]
async fn the_follow_notification_baseline_reads_only_the_local_author_key() {
    let docs_sync = Arc::new(CountingDocsSync::default());
    let (app, _) = counting_app(docs_sync.clone());
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let replica = author_replica_id(remote_keys.public_key_hex().as_str());
    put_follow_edges(docs_sync.as_ref(), &remote_keys, 300).await;
    put_follow_edge(
        docs_sync.as_ref(),
        &remote_keys,
        local_author_pubkey.as_str(),
    )
    .await;
    docs_sync.clear_queries().await;
    docs_sync.reset_records_returned();

    let baseline = snapshot_follow_notification_baseline(
        docs_sync.as_ref(),
        &replica,
        local_author_pubkey.as_str(),
        None,
    )
    .await
    .expect("baseline");

    assert_eq!(docs_sync.records_returned(), 1);
    assert_no_prefix_read(&docs_sync.queries().await, "the follow baseline");
    let event = remote_doc_event(
        docs_sync.as_ref(),
        &replica,
        stable_key("graph/follows", local_author_pubkey.as_str()),
    )
    .await;
    assert!(
        baseline.contains(&event),
        "the existing follow of me is in the baseline"
    );
}

// 購読タスクが entry の event を取りこぼしても(取りこぼしの通知・同期の区切り)、上限つきの追いつきが反映する。
#[tokio::test]
async fn the_author_subscription_catches_up_after_missed_events() {
    for notice in [
        ReplicaNotice::Lagged { missed: 3 },
        ReplicaNotice::SyncFinished,
        ReplicaNotice::ContentReady,
    ] {
        let docs_sync = Arc::new(InjectedNoticesDocsSync::default());
        let store = Arc::new(MemoryStore::default());
        let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
        let app = app_service_from_dependencies(
            store.clone(),
            store.clone(),
            transport.clone(),
            transport,
            docs_sync.clone(),
            Arc::new(MemoryBlobService::default()),
            generate_keys(),
        );
        let local_author_pubkey = app.current_author_pubkey();
        let remote_keys = generate_keys();
        let remote_pubkey = remote_keys.public_key_hex();
        app.spawn_author_subscription(remote_pubkey.as_str())
            .await
            .expect("subscribe the author");
        sleep(Duration::from_millis(200)).await;
        // 購読の開始の後に届いた edge。entry の event は購読側へ届かない(取りこぼした状態)。
        put_follow_edge(
            docs_sync.as_ref(),
            &remote_keys,
            local_author_pubkey.as_str(),
        )
        .await;
        assert!(
            store
                .list_follow_edges_by_subject(remote_pubkey.as_str())
                .await
                .expect("follow edges")
                .is_empty()
        );

        docs_sync.notices.send(notice.clone()).expect("send notice");
        timeout(Duration::from_secs(10), async {
            loop {
                let relationship = store
                    .get_author_relationship(local_author_pubkey.as_str(), remote_pubkey.as_str())
                    .await
                    .expect("relationship");
                if relationship.is_some_and(|row| row.followed_by) {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{notice:?} leads to a catch-up of the author state"));
        app.shutdown().await;
    }
}

// entry の event は、その key を反映する(追いつきを待たない)。
#[tokio::test]
async fn the_author_subscription_reflects_an_entry_event_by_its_key() {
    let docs_sync = Arc::new(InjectedNoticesDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let replica = author_replica_id(remote_pubkey.as_str());
    app.spawn_author_subscription(remote_pubkey.as_str())
        .await
        .expect("subscribe the author");
    sleep(Duration::from_millis(200)).await;
    put_follow_edge(
        docs_sync.as_ref(),
        &remote_keys,
        local_author_pubkey.as_str(),
    )
    .await;
    let event = remote_doc_event(
        docs_sync.as_ref(),
        &replica,
        stable_key("graph/follows", local_author_pubkey.as_str()),
    )
    .await;
    docs_sync
        .notices
        .send(ReplicaNotice::Entry(event))
        .expect("send entry");

    timeout(Duration::from_secs(5), async {
        loop {
            if !store
                .list_follow_edges_by_subject(remote_pubkey.as_str())
                .await
                .expect("follow edges")
                .is_empty()
            {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the entry event reflects its key");
    let notifications = app.list_notifications().await.expect("notifications");
    assert_eq!(notifications.len(), 1, "a new follow of me is notified");
    app.shutdown().await;
}

fn shadowing_app(docs_sync: Arc<ShadowingDocsSync>) -> (AppService, Arc<MemoryStore>) {
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

// 同じ key を別の名義(docs author の昇順で先に並ぶ)が書いても、正しい edge を反映する(独立監査 B-1)。
// 誰でも書ける replica なので、先頭の record だけを見ると、ごみを置くだけで follow・block を隠せる。
#[tokio::test]
async fn a_shadowed_edge_key_still_reflects_the_valid_record() {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author("f".repeat(64)));
    let (app, store) = shadowing_app(docs_sync.clone());
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    put_follow_edge(
        docs_sync.as_ref(),
        &remote_keys,
        local_author_pubkey.as_str(),
    )
    .await;
    let key = stable_key("graph/follows", local_author_pubkey.as_str());
    docs_sync
        .shadow(key.as_str(), serde_json::json!({ "garbage": true }))
        .await;

    let by_event = hydrate_author_key(
        &app.services,
        local_author_pubkey.as_str(),
        remote_pubkey.as_str(),
        key.as_str(),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("hydrate the shadowed key");
    assert_eq!(by_event.reflected, 1, "the valid record behind the shadow");
    assert!(
        store
            .get_author_relationship(local_author_pubkey.as_str(), remote_pubkey.as_str())
            .await
            .expect("relationship")
            .is_some_and(|row| row.followed_by)
    );
    assert_eq!(
        hydrate_author_state(
            &app.services,
            local_author_pubkey.as_str(),
            remote_pubkey.as_str(),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate author state"),
        1
    );
}

// 相手の follow が上限を超え、自分を指す follow が key の順で窓の外にあっても、追いつきは自分を指す follow を
// 反映する(独立監査 B-2)。
#[tokio::test]
async fn the_catch_up_reads_the_follow_of_me_beyond_the_edge_key_window() {
    let docs_sync = Arc::new(CountingDocsSync::default());
    // 自分の pubkey が key の順で後ろに並ぶ鍵を選ぶ。
    let local_keys = std::iter::repeat_with(generate_keys)
        .find(|keys| keys.public_key_hex().starts_with('f'))
        .expect("a key sorted last");
    let local_author_pubkey = local_keys.public_key_hex();
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        local_keys,
    );
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let mut before_me = 0;
    while before_me < AUTHOR_EDGE_KEYS + 10 {
        let target = generate_keys().public_key_hex();
        if target < local_author_pubkey {
            put_follow_edge(docs_sync.as_ref(), &remote_keys, target.as_str()).await;
            before_me += 1;
        }
    }
    put_follow_edge(
        docs_sync.as_ref(),
        &remote_keys,
        local_author_pubkey.as_str(),
    )
    .await;

    catch_up_author_state(
        &app.services,
        local_author_pubkey.as_str(),
        remote_pubkey.as_str(),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("catch up");

    assert!(
        store
            .get_author_relationship(local_author_pubkey.as_str(), remote_pubkey.as_str())
            .await
            .expect("relationship")
            .is_some_and(|row| row.followed_by),
        "the follow of me is reflected even beyond the key window"
    );
}

// 追いつきは、自分を指す follow・block の key だけを読む。相手の follow が何件でも、key の一覧も prefix も読まず、
// 読む量は同じ(AGENTS.md: ユースケース上ユーザーが必要としない限り同期・復旧はしない)。
#[tokio::test]
async fn the_catch_up_reads_only_the_keys_pointing_to_me() {
    let mut counts = Vec::new();
    for edges in [AUTHOR_EDGE_KEYS + 20, AUTHOR_EDGE_KEYS + 300] {
        let docs_sync = Arc::new(CountingDocsSync::default());
        let (app, store) = counting_app(docs_sync.clone());
        let local_author_pubkey = app.current_author_pubkey();
        let remote_keys = generate_keys();
        let remote_pubkey = remote_keys.public_key_hex();
        put_follow_edges(docs_sync.as_ref(), &remote_keys, edges).await;
        put_follow_edge(
            docs_sync.as_ref(),
            &remote_keys,
            local_author_pubkey.as_str(),
        )
        .await;
        docs_sync.clear_queries().await;
        docs_sync.reset_records_returned();

        let outcome = catch_up_author_state(
            &app.services,
            local_author_pubkey.as_str(),
            remote_pubkey.as_str(),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("catch up");

        assert_eq!(outcome.reflected, 1, "only the follow of me");
        let queries = docs_sync.queries().await;
        assert!(
            queries
                .iter()
                .all(|(_, query)| matches!(query, DocQuery::Exact(_))),
            "the catch-up reads keys only by exact match: {queries:?}"
        );
        assert_eq!(
            store
                .list_follow_edges_by_subject(remote_pubkey.as_str())
                .await
                .expect("follow edges")
                .len(),
            1,
            "the other follows are not read"
        );
        counts.push(docs_sync.records_returned());
    }
    assert_eq!(
        counts[0], counts[1],
        "docs records read by the catch-up must not depend on the number of follow edges"
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

async fn record_value(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    key: &str,
) -> serde_json::Value {
    let record = docs_sync
        .query_replica(replica, DocQuery::Exact(key.to_string()))
        .await
        .expect("query")
        .into_iter()
        .next()
        .expect("record");
    serde_json::from_slice(&record.value).expect("json")
}

// 同じ key に、古い正しい record(別の名義)と新しい正しい record があるとき、新しい状態を反映する
// (古い envelope を指す record を後から置く再送で、状態を巻き戻せない)。
#[tokio::test]
async fn the_newest_valid_edge_wins_over_a_replayed_older_record() {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author("f".repeat(64)));
    let (app, store) = shadowing_app(docs_sync.clone());
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let replica = author_replica_id(remote_pubkey.as_str());
    let key = stable_key("graph/follows", local_author_pubkey.as_str());
    put_follow_edge_with_status(
        docs_sync.as_ref(),
        &remote_keys,
        local_author_pubkey.as_str(),
        FollowEdgeStatus::Active,
    )
    .await;
    let older = record_value(docs_sync.as_ref(), &replica, key.as_str()).await;
    sleep(Duration::from_millis(1_100)).await;
    put_follow_edge_with_status(
        docs_sync.as_ref(),
        &remote_keys,
        local_author_pubkey.as_str(),
        FollowEdgeStatus::Revoked,
    )
    .await;
    docs_sync.shadow(key.as_str(), older).await;

    hydrate_author_key(
        &app.services,
        local_author_pubkey.as_str(),
        remote_pubkey.as_str(),
        key.as_str(),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("hydrate");

    let edges = store
        .list_follow_edges_by_subject(remote_pubkey.as_str())
        .await
        .expect("edges");
    assert_eq!(edges.len(), 1);
    assert_eq!(
        edges[0].status,
        FollowEdgeStatus::Revoked,
        "the newest state"
    );
}

// 別の key の正しい record を、この key に置いても反映しない(key と相手の一致)。
#[tokio::test]
async fn a_valid_edge_record_under_another_key_is_ignored() {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author("f".repeat(64)));
    let (app, store) = shadowing_app(docs_sync.clone());
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let replica = author_replica_id(remote_pubkey.as_str());
    let other = generate_keys().public_key_hex();
    put_follow_edge(docs_sync.as_ref(), &remote_keys, other.as_str()).await;
    let other_record = record_value(
        docs_sync.as_ref(),
        &replica,
        stable_key("graph/follows", other.as_str()).as_str(),
    )
    .await;
    let key = stable_key("graph/follows", local_author_pubkey.as_str());
    docs_sync.shadow(key.as_str(), other_record).await;

    let outcome = hydrate_author_key(
        &app.services,
        local_author_pubkey.as_str(),
        remote_pubkey.as_str(),
        key.as_str(),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("hydrate");

    assert_eq!(outcome.reflected, 0);
    assert!(
        store
            .list_follow_edges_by_subject(remote_pubkey.as_str())
            .await
            .expect("edges")
            .is_empty()
    );
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

// 自分の replica で event を取りこぼしても、自分の follow をすべて読み直すことはしない。購読の開始時の窓
// (`AUTHOR_EDGE_KEYS` 件)より多くは入らない(端末間のアカウント同期は未実装で、ユーザーは全件を必要としない)。
#[tokio::test]
async fn a_lag_on_the_own_replica_does_not_read_every_own_follow() {
    let docs_sync = Arc::new(InjectedNoticesDocsSync::default());
    let local_keys = generate_keys();
    let local_author_pubkey = local_keys.public_key_hex();
    put_follow_edges(docs_sync.as_ref(), &local_keys, AUTHOR_EDGE_KEYS + 40).await;
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        local_keys,
    );
    app.spawn_author_subscription(local_author_pubkey.as_str())
        .await
        .expect("subscribe myself");
    let own_edges = || async {
        store
            .list_follow_edges_by_subject(local_author_pubkey.as_str())
            .await
            .expect("own follow edges")
            .len()
    };
    timeout(Duration::from_secs(10), async {
        while own_edges().await < AUTHOR_EDGE_KEYS {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the subscription start reads the window");

    for notice in [
        ReplicaNotice::Lagged { missed: 600 },
        ReplicaNotice::SyncFinished,
        ReplicaNotice::ContentReady,
    ] {
        docs_sync.notices.send(notice).expect("send notice");
    }
    sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        own_edges().await,
        AUTHOR_EDGE_KEYS,
        "only the window of the subscription start is read"
    );
    app.shutdown().await;
}
