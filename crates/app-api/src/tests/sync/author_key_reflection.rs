//! #1239: author replica の反映(profile・follow・block・follow の通知の起点・custom reaction の asset)が、
//! replica を走査せず、読む量が follow・block の総数に依存しないことを固定する。

use super::subscription_catch_up::InjectedNoticesDocsSync;
use super::*;
use crate::service::profile_docs_support::AUTHOR_EDGE_KEYS;
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
