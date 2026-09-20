//! #1239: ページの範囲の照合が、読めない record・反映できない entry・権限の無い replica・手元に無い本文に
//! 出会っても、表示を失敗させず、禁止された読み出しをしないことを固定する。
//!
//! 独立監査(PR #1247)の再現 test を恒久化したものを含む。

use super::range_reconcile::{
    BASE_TIME, NoScanDocsSync, RangeFixture, cursor_at, is_projected, project, put_post_at,
    range_fixture,
};
use super::*;

/// 投稿として読めない `objects/<id>/state` と、それを指す時系列の索引の entry を置く。
async fn put_unreadable_post(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    created_at: i64,
    object_id: &str,
) {
    for (key, value) in [
        (
            stable_key("objects", &format!("{object_id}/state")),
            serde_json::json!({ "not": "a canonical post header" }),
        ),
        (
            stable_key(
                "indexes/timeline",
                &format!("{created_at:020}-{object_id}/{object_id}"),
            ),
            serde_json::json!({}),
        ),
    ] {
        docs_sync
            .apply_doc_op(replica, DocOp::SetJson { key, value })
            .await
            .expect("write entry");
    }
}

/// state の無い(本体が届いていない投稿と同じ状態の)索引の entry を置く。
async fn put_dangling_index_entry(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    created_at: i64,
    object_id: &str,
) {
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "indexes/timeline",
                    &format!("{created_at:020}-{object_id}/{object_id}"),
                ),
                value: serde_json::json!({}),
            },
        )
        .await
        .expect("write index entry");
}

fn item_counts(results: &[Result<TimelineView>]) -> Vec<std::result::Result<usize, String>> {
    results
        .iter()
        .map(|result| {
            result
                .as_ref()
                .map(|view| view.items.len())
                .map_err(|error| format!("{error:#}"))
        })
        .collect()
}

async fn list_repeatedly(
    fixture: &RangeFixture,
    cursor: Option<TimelineCursor>,
    limit: usize,
    times: usize,
) -> Vec<std::result::Result<usize, String>> {
    let mut results = Vec::new();
    for _ in 0..times {
        results.push(
            fixture
                .app
                .list_timeline(fixture.topic.as_str(), cursor.clone(), limit)
                .await,
        );
    }
    item_counts(&results)
}

// ADR 0052 §2: 読めない state が 1 件あっても、行のあるページの取得は失敗しない(独立監査の blocker の再現)。
#[tokio::test]
async fn unreadable_state_record_does_not_fail_a_non_empty_head_page() {
    let fixture = range_fixture("unreadable-head", 3, |_| true).await;
    let replica = topic_replica_id(fixture.topic.as_str());
    put_unreadable_post(
        fixture.docs_sync.as_ref(),
        &replica,
        BASE_TIME + 10,
        "e".repeat(64).as_str(),
    )
    .await;
    let outcomes = list_repeatedly(&fixture, None, 20, 3).await;
    fixture.app.shutdown().await;
    assert!(
        outcomes.iter().all(|outcome| *outcome == Ok(3)),
        "every listing of a non-empty page must succeed: {outcomes:?}"
    );
}

#[tokio::test]
async fn unreadable_state_record_does_not_fail_a_non_empty_cursor_page() {
    let fixture = range_fixture("unreadable-cursor", 30, |_| true).await;
    let replica = topic_replica_id(fixture.topic.as_str());
    // cursor にする投稿のすぐ古い側(同じ秒で object id が最小)に、読めない state を置く。
    put_unreadable_post(
        fixture.docs_sync.as_ref(),
        &replica,
        fixture.posts[20].created_at,
        "0".repeat(64).as_str(),
    )
    .await;
    let outcomes = list_repeatedly(&fixture, Some(cursor_at(&fixture.posts[20])), 10, 2).await;
    fixture.app.shutdown().await;
    assert!(
        outcomes.iter().all(|outcome| *outcome == Ok(10)),
        "every listing of a non-empty cursor page must succeed: {outcomes:?}"
    );
}

#[tokio::test]
async fn unreadable_state_record_does_not_fail_a_non_empty_thread() {
    let (app, store, docs_sync, _blobs) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:range-unreadable-thread");
    let replica = topic_replica_id(topic.as_str());
    let root = persist_test_post(
        docs_sync.as_ref(),
        Some(store.as_ref()),
        &generate_keys(),
        &topic,
        PayloadRef::InlineText {
            text: "root".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    let bad = "e".repeat(64);
    for (key, value) in [
        (
            stable_key("objects", &format!("{bad}/state")),
            serde_json::json!({ "not": "a header" }),
        ),
        (
            stable_key(
                "indexes/thread",
                &format!("{}/{:020}-{bad}/{bad}", root.id.as_str(), 1_900_000_000_i64),
            ),
            serde_json::json!({}),
        ),
    ] {
        docs_sync
            .apply_doc_op(&replica, DocOp::SetJson { key, value })
            .await
            .expect("write entry");
    }
    let mut results = Vec::new();
    for _ in 0..2 {
        results.push(
            app.list_thread(topic.as_str(), root.id.as_str(), None, 20)
                .await,
        );
    }
    let outcomes = item_counts(&results);
    app.shutdown().await;
    assert!(
        outcomes.iter().all(|outcome| *outcome == Ok(1)),
        "a non-empty thread must be listed: {outcomes:?}"
    );
}

// AC-4: 反映できない entry(本体が届いていない、投稿として読めない)が 1 ページぶん以上続いても、その先の
// 投稿へ遡れる。以前の全件走査は、反映できる object をすべて反映していた。
#[tokio::test]
async fn a_run_of_unresolvable_entries_does_not_block_older_history() {
    let posts = 40usize;
    // 新しい側の 5 件だけが projection にある。
    let fixture = range_fixture("unresolvable-run", posts, |index| index >= posts - 5).await;
    let replica = topic_replica_id(fixture.topic.as_str());
    let oldest_projected = &fixture.posts[posts - 5];
    // projection にある最古の投稿と、その 1 秒前の投稿のあいだ(同じ秒の小さい object id)に、反映できない
    // entry を 30 件置く。ページの件数(10 件)より多い。
    for index in 0..30usize {
        let id = format!("{index:064x}");
        if index % 2 == 0 {
            put_unreadable_post(
                fixture.docs_sync.as_ref(),
                &replica,
                oldest_projected.created_at,
                id.as_str(),
            )
            .await;
        } else {
            put_dangling_index_entry(
                fixture.docs_sync.as_ref(),
                &replica,
                oldest_projected.created_at,
                id.as_str(),
            )
            .await;
        }
    }
    let page = fixture
        .app
        .list_timeline(
            fixture.topic.as_str(),
            Some(cursor_at(oldest_projected)),
            10,
        )
        .await
        .expect("older page");
    let contents = page
        .items
        .iter()
        .map(|item| item.content.clone())
        .collect::<Vec<_>>();
    fixture.app.shutdown().await;
    assert_eq!(
        contents,
        ((posts - 15)..(posts - 5))
            .rev()
            .map(|index| format!("post {index}"))
            .collect::<Vec<_>>(),
        "the page skips the unresolvable entries and shows the older posts"
    );
}

// 反映できない entry が 1 回の上限(200 件)を超えて続くときは、読み進めた位置から次の照合が続ける。
// 1 回の照合が読む量は上限のままで、照合を重ねれば先へ進む。
#[tokio::test]
async fn a_long_run_of_unresolvable_entries_is_passed_over_several_bounded_checks() {
    let posts = 12usize;
    let fixture = range_fixture("unresolvable-long-run", posts, |index| index >= posts - 2).await;
    let replica = topic_replica_id(fixture.topic.as_str());
    let oldest_projected = &fixture.posts[posts - 2];
    for index in 0..450usize {
        put_dangling_index_entry(
            fixture.docs_sync.as_ref(),
            &replica,
            oldest_projected.created_at,
            format!("{index:064x}").as_str(),
        )
        .await;
    }
    let cursor = cursor_at(oldest_projected);
    fixture.docs_sync.reset_records_returned();
    let first = fixture
        .app
        .reconcile_timeline_range(
            fixture.topic.as_str(),
            &TimelineScope::Public,
            Some(&cursor),
            10,
        )
        .await
        .expect("first check");
    assert_eq!(first, 0, "the first check only reads unresolvable entries");
    // 同じ秒の読み出しは 1 回 512 件まで。読む件数を 4 倍ずつ増やすので、1 回の照合の読み直しは 3 回まで。
    assert!(
        fixture.docs_sync.records_returned() <= 512 * 4,
        "one check reads a bounded number of keys: {}",
        fixture.docs_sync.records_returned()
    );
    assert!(!is_projected(fixture.store.as_ref(), &fixture.posts[posts - 3]).await);

    // 台帳の間隔を待つ代わりに、同じ範囲の次の照合を直接呼べるよう、間隔の経過を模す。
    let mut hydrated = 0usize;
    for _ in 0..4 {
        fixture
            .app
            .services
            .range_checks
            .expire_all_for_test()
            .await;
        hydrated += fixture
            .app
            .reconcile_timeline_range(
                fixture.topic.as_str(),
                &TimelineScope::Public,
                Some(&cursor),
                10,
            )
            .await
            .expect("following check");
    }
    fixture.app.shutdown().await;
    assert_eq!(
        hydrated,
        posts - 2,
        "the following checks reach the older posts"
    );
}

// INVAR-2: 照合は `LocalOnly`。remote を待つ docs でも、照合は待たされない。
#[tokio::test]
async fn reconcile_does_not_wait_for_remote_docs() {
    let docs_sync = Arc::new(HangingRemoteOnMissDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let app = app_with_hanging_remote_docs(
        store.clone(),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:range-local-only");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let mut posts = Vec::new();
    for index in 0..6usize {
        let object = put_post_at(
            docs_sync.as_ref(),
            &replica,
            &author_keys,
            &topic,
            BASE_TIME + index as i64,
            format!("post {index}").as_str(),
            None,
        )
        .await;
        if index >= 3 {
            project(store.as_ref(), &object, &replica).await;
        }
        posts.push(object);
    }
    let head = timeout(
        Duration::from_secs(5),
        app.reconcile_timeline_range(topic.as_str(), &TimelineScope::Public, None, 20),
    )
    .await
    .expect("the head reconcile must not wait for a remote fetch")
    .expect("reconcile");
    assert_eq!(head, 3);
    let thread = timeout(
        Duration::from_secs(5),
        app.reconcile_thread(topic.as_str(), &posts[5].object_id),
    )
    .await
    .expect("the thread reconcile must not wait for a remote fetch")
    .expect("reconcile thread");
    assert_eq!(thread, 0);
    app.shutdown().await;
}

/// 本文 blob が「手元には無いが、remote からは取れる」状態を表す blob service。`fetch_blob` の回数を数える。
#[derive(Clone, Default)]
struct RemoteOnlyBlobService {
    inner: MemoryBlobService,
    fetches: Arc<std::sync::atomic::AtomicUsize>,
}

impl RemoteOnlyBlobService {
    fn fetches(&self) -> usize {
        self.fetches.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl BlobService for RemoteOnlyBlobService {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.fetch_blob(hash).await
    }

    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }

    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.inner.blob_status(hash).await
    }

    async fn local_blob_status(&self, _hash: &BlobHash) -> Result<BlobStatus> {
        Ok(BlobStatus::Missing)
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

struct BlobBodyFixture {
    app: AppService,
    store: Arc<MemoryStore>,
    docs_sync: Arc<MemoryDocsSync>,
    blobs: Arc<RemoteOnlyBlobService>,
    topic: TopicId,
    replica: ReplicaId,
    object: CanonicalPostHeader,
    envelope: KukuriEnvelope,
}

/// 本文が blob(手元に無いが remote からは取れる)の投稿を 1 件、docs にだけ持つ fixture。
async fn blob_body_fixture(name: &str) -> BlobBodyFixture {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let blobs = Arc::new(RemoteOnlyBlobService::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blobs.clone(),
        generate_keys(),
    );
    let topic = TopicId::new(format!("kukuri:topic:range-blob-body-{name}").as_str());
    let replica = topic_replica_id(topic.as_str());
    let stored = blobs
        .put_blob(b"blob body".to_vec(), "text/plain")
        .await
        .expect("put blob");
    let envelope = build_post_envelope_with_payload_in_channel(
        &generate_keys(),
        &topic,
        PayloadRef::BlobText {
            hash: stored.hash.clone(),
            mime: "text/plain".into(),
            bytes: stored.bytes,
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("envelope");
    let object = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    BlobBodyFixture {
        app,
        store,
        docs_sync,
        blobs,
        topic,
        replica,
        object,
        envelope,
    }
}

// 照合は 1 回に多数の object を反映するので、手元に無い本文を remote へ取りに行かない(表示を待たせない)。
#[tokio::test]
async fn reconcile_does_not_fetch_a_body_blob_that_is_not_local() {
    let fixture = blob_body_fixture("reconcile").await;
    persist_post_object(
        fixture.docs_sync.as_ref(),
        &fixture.replica,
        fixture.object.clone(),
        fixture.envelope.clone(),
    )
    .await
    .expect("persist");
    let hydrated = fixture
        .app
        .reconcile_timeline_range(fixture.topic.as_str(), &TimelineScope::Public, None, 20)
        .await
        .expect("reconcile");
    assert_eq!(hydrated, 1);
    assert_eq!(
        fixture.blobs.fetches(),
        0,
        "the reconcile must not call fetch_blob for a body that is not local"
    );
    let row = ObjectProjectionStore::get_object_projection(
        fixture.store.as_ref(),
        &fixture.object.object_id,
    )
    .await
    .expect("projection")
    .expect("row");
    assert!(row.content.is_none(), "the body is recovered later");
    fixture.app.shutdown().await;
}

// 利用者の操作の対象と community index の解決は object を 1 件ずつ反映するので、本文を台帳の内で remote から取る。
// 表示や bookmark の内容が、本文の無いままにならない(独立監査の指摘)。
#[tokio::test]
async fn community_index_resolution_shows_the_body_of_an_unprojected_post() {
    let fixture = blob_body_fixture("community-index").await;
    persist_post_object(
        fixture.docs_sync.as_ref(),
        &fixture.replica,
        fixture.object.clone(),
        fixture.envelope.clone(),
    )
    .await
    .expect("persist");
    let response = fixture
        .app
        .resolve_community_index_posts(vec![CommunityIndexPostResolveInput {
            key: "entry".into(),
            topic: fixture.topic.as_str().to_string(),
            object_id: fixture.object.object_id.as_str().to_string(),
            author_pubkey: fixture.envelope.pubkey.as_str().to_string(),
            channel_ref: ChannelRef::Public,
        }])
        .await
        .expect("resolve");
    let content = response.entries[0]
        .post
        .as_ref()
        .map(|post| post.content.clone());
    fixture.app.shutdown().await;
    assert_eq!(content.as_deref(), Some("blob body"));
}

// docs の event の個別反映は、本文の取得を台帳(`MissingBodyLedger`)に通す。取得に失敗した直後の表示が、
// 同じ本文をもう一度取りに行かない。
#[tokio::test]
async fn event_hydration_and_the_following_listing_share_the_missing_body_schedule() {
    let fixture = blob_body_fixture("event-ledger").await;
    // 購読を始めてから投稿を書く(docs の event で反映される)。
    fixture
        .app
        .list_timeline(fixture.topic.as_str(), None, 20)
        .await
        .expect("empty timeline");
    sleep(Duration::from_millis(150)).await;
    // remote からも取れない本文にする。
    let missing = BlobHash::new("f".repeat(64));
    let mut object = fixture.object.clone();
    object.payload_ref = PayloadRef::BlobText {
        hash: missing,
        mime: "text/plain".into(),
        bytes: 9,
    };
    persist_post_object(
        fixture.docs_sync.as_ref(),
        &fixture.replica,
        object.clone(),
        fixture.envelope.clone(),
    )
    .await
    .expect("persist");
    let projected = timeout(Duration::from_secs(3), async {
        loop {
            if ObjectProjectionStore::get_object_projection(
                fixture.store.as_ref(),
                &object.object_id,
            )
            .await
            .expect("projection")
            .is_some()
            {
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .is_ok();
    assert!(projected, "the docs event must project the post");
    let after_event = fixture.blobs.fetches();
    assert_eq!(after_event, 1, "the event tries the body once");
    for _ in 0..3 {
        fixture
            .app
            .list_timeline(fixture.topic.as_str(), None, 20)
            .await
            .expect("timeline");
    }
    sleep(Duration::from_millis(150)).await;
    let fetches = fixture.blobs.fetches();
    fixture.app.shutdown().await;
    assert_eq!(
        fetches, after_event,
        "the listing right after a failed attempt must wait for the ledger schedule"
    );
}

// 1 回の照合が読む索引の entry 数は、`limit` が大きくても上限(200 件)で抑えられる。
#[tokio::test]
async fn reconcile_caps_the_entries_read_per_call() {
    let fixture = range_fixture("cap", 400, |_| false).await;
    let hydrated = fixture
        .app
        .reconcile_timeline_range(
            fixture.topic.as_str(),
            &TimelineScope::Public,
            None,
            100_000,
        )
        .await
        .expect("reconcile");
    fixture.app.shutdown().await;
    assert_eq!(hydrated, 200);
}

// thread の照合は、古い側の 512 件までを読む。
#[tokio::test]
async fn thread_reconcile_caps_the_entries_read_per_call() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-thread-cap");
    let replica = topic_replica_id(topic.as_str());
    let root = persist_test_post(
        docs_sync.as_ref(),
        Some(store.as_ref()),
        &generate_keys(),
        &topic,
        PayloadRef::InlineText {
            text: "root".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    // 返信の state は書かず、thread の索引だけを 600 件置く(照合が読む key の件数だけを見る)。
    for index in 0..600usize {
        let id = format!("{index:064x}");
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key(
                        "indexes/thread",
                        &format!(
                            "{}/{:020}-{id}/{id}",
                            root.id.as_str(),
                            1_900_000_000 + index as i64
                        ),
                    ),
                    value: serde_json::json!({}),
                },
            )
            .await
            .expect("write thread index");
    }
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    docs_sync.reset_records_returned();
    let hydrated = app
        .reconcile_thread(topic.as_str(), &root.id)
        .await
        .expect("reconcile thread");
    app.shutdown().await;
    assert_eq!(hydrated, 0);
    // 索引の key を 512 件まで読む。state の無い object の key 指定の読み出しは 0 件を返す。
    assert_eq!(docs_sync.records_returned(), 512);
}

// 遡ったページ(cursor つき)は、projection が尽きていなくても途中の欠けが埋まる。
#[tokio::test]
async fn cursor_page_with_gaps_is_filled_even_when_the_projection_is_not_exhausted() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(NoScanDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-gaps");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let mut posts = Vec::new();
    for index in 0..60usize {
        let object = put_post_at(
            docs_sync.as_ref(),
            &replica,
            &author_keys,
            &topic,
            BASE_TIME + index as i64,
            format!("post {index}").as_str(),
            None,
        )
        .await;
        // 偶数番目だけが projection にある(途中が欠けている)。
        if index % 2 == 0 {
            project(store.as_ref(), &object, &replica).await;
        }
        posts.push(object);
    }
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let page = app
        .list_timeline(topic.as_str(), Some(cursor_at(&posts[40])), 10)
        .await
        .expect("older page");
    let contents = page
        .items
        .iter()
        .map(|item| item.content.clone())
        .collect::<Vec<_>>();
    app.shutdown().await;
    assert_eq!(
        contents,
        (30..40)
            .rev()
            .map(|index| format!("post {index}"))
            .collect::<Vec<_>>()
    );
}

// 先頭のページは、projection が尽きていれば(行が残っていても)照合する。
#[tokio::test]
async fn short_head_page_is_filled_when_the_projection_is_exhausted() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(NoScanDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-short-head");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    for index in 0..12usize {
        let object = put_post_at(
            docs_sync.as_ref(),
            &replica,
            &author_keys,
            &topic,
            BASE_TIME + index as i64,
            format!("post {index}").as_str(),
            None,
        )
        .await;
        if index >= 9 {
            project(store.as_ref(), &object, &replica).await;
        }
    }
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let page = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("head page");
    app.shutdown().await;
    assert_eq!(page.items.len(), 12);
}

// thread は、ページが空でなくても照合する(途中の返信が欠けうる)。
#[tokio::test]
async fn thread_with_missing_replies_is_filled_even_when_the_page_is_not_empty() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(NoScanDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-thread-gaps");
    let author_keys = generate_keys();
    let root = persist_test_post(
        docs_sync.as_ref(),
        Some(store.as_ref()),
        &author_keys,
        &topic,
        PayloadRef::InlineText {
            text: "root".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    for index in 0..4usize {
        let projection: Option<&dyn ProjectionStore> = if index % 2 == 0 {
            Some(store.as_ref())
        } else {
            None
        };
        persist_test_post(
            docs_sync.as_ref(),
            projection,
            &author_keys,
            &topic,
            PayloadRef::InlineText {
                text: format!("reply {index}"),
            },
            Vec::new(),
            Some(&root),
        )
        .await;
    }
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let thread = app
        .list_thread(topic.as_str(), root.id.as_str(), None, 20)
        .await
        .expect("thread");
    app.shutdown().await;
    assert_eq!(thread.items.len(), 5, "root and four replies");
}
