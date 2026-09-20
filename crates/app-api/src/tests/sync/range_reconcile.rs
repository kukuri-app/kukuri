//! #1239: タイムラインと thread の取得が、replica を走査せずに、ページの範囲を時系列の索引と照合して
//! 欠けを埋めることを固定する。判定は所要時間ではなく、docs の query が返した record(または key)の数で行う。

use super::*;

pub(super) const BASE_TIME: i64 = 1_700_000_000;

/// test の投稿。行と cursor は、署名つき envelope から作った header と同じ値になる。
pub(super) struct TestPost {
    header: CanonicalPostHeader,
    pub(super) envelope: KukuriEnvelope,
}

impl std::ops::Deref for TestPost {
    type Target = CanonicalPostHeader;

    fn deref(&self) -> &CanonicalPostHeader {
        &self.header
    }
}

pub(super) struct RangeFixture {
    pub(super) app: AppService,
    pub(super) store: Arc<MemoryStore>,
    pub(super) docs_sync: Arc<CountingDocsSync>,
    pub(super) topic: TopicId,
    /// 古い順。`created_at` は `BASE_TIME + index`。
    pub(super) posts: Vec<TestPost>,
}

/// 1 秒に 1 件ずつの投稿を docs に置く。`projected` が真の位置だけ projection にも入れる。
/// 購読タスクを起動しないので、反映するのは照合だけになる。
pub(super) async fn range_fixture(
    name: &str,
    posts: usize,
    projected: impl Fn(usize) -> bool,
) -> RangeFixture {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let topic = TopicId::new(format!("kukuri:topic:range-{name}-{posts}").as_str());
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let mut objects = Vec::new();
    for index in 0..posts {
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
        if projected(index) {
            project(store.as_ref(), &object, &replica).await;
        }
        objects.push(object);
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
    docs_sync.clear_queries().await;
    RangeFixture {
        app,
        store,
        docs_sync,
        topic,
        posts: objects,
    }
}

/// `created_at` を指定して署名した投稿を docs に書く(索引の key も同じ時刻になる)。
/// 反映は署名つき envelope から行を作るので(#1248)、時刻は envelope に入れて署名する。
pub(super) async fn put_post_at(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    keys: &KukuriKeys,
    topic: &TopicId,
    created_at: i64,
    text: &str,
    reply_to: Option<&KukuriEnvelope>,
) -> TestPost {
    let template = build_post_envelope_with_payload_in_channel(
        keys,
        topic,
        PayloadRef::InlineText { text: text.into() },
        Vec::new(),
        Vec::new(),
        reply_to,
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("post envelope");
    let content: serde_json::Value =
        serde_json::from_str(template.content.as_str()).expect("envelope content");
    let envelope = kukuri_core::sign_envelope_json_at(
        keys,
        template.kind.clone(),
        template.tags.clone(),
        &content,
        created_at,
    )
    .expect("sign at the given time");
    let header = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    assert_eq!(header.created_at, created_at);
    persist_post_object(docs_sync, replica, header.clone(), envelope.clone())
        .await
        .expect("persist post");
    TestPost { header, envelope }
}

pub(super) async fn project(store: &MemoryStore, post: &TestPost, replica: &ReplicaId) {
    let content = match &post.payload_ref {
        PayloadRef::InlineText { text } => Some(text.clone()),
        PayloadRef::BlobText { .. } => None,
    };
    ObjectProjectionStore::put_object_projection(
        store,
        verified_projection_row(&post.envelope, replica, content),
    )
    .await
    .expect("put projection");
}

pub(super) async fn is_projected(store: &MemoryStore, object: &CanonicalPostHeader) -> bool {
    ObjectProjectionStore::get_object_projection(store, &object.object_id)
        .await
        .expect("projection")
        .is_some()
}

pub(super) fn cursor_at(object: &CanonicalPostHeader) -> TimelineCursor {
    TimelineCursor {
        created_at: object.created_at,
        object_id: object.object_id.clone(),
    }
}

// TR-5 / AC-2: 遡ったページの範囲だけを索引から読み、projection に無い object だけを key 指定で反映する。
// 読む docs の record 数は、replica の大きさに依存しない。prefix の読み出し(走査)は 1 回も行わない。
#[tokio::test]
async fn older_page_is_filled_from_the_time_index_with_a_bounded_number_of_reads() {
    let limit = 20usize;
    let mut counts = Vec::new();
    for posts in [300usize, 1_500] {
        // 新しい側の 50 件だけが projection にある。
        let fixture = range_fixture("older", posts, |index| index >= posts - 50).await;
        let oldest_projected = &fixture.posts[posts - 50];
        let hydrated = fixture
            .app
            .reconcile_timeline_range(
                fixture.topic.as_str(),
                &TimelineScope::Public,
                Some(&cursor_at(oldest_projected)),
                limit,
            )
            .await
            .expect("reconcile");
        assert_eq!(hydrated, limit, "posts={posts}");
        for index in (posts - 50 - limit)..(posts - 50) {
            assert!(
                is_projected(fixture.store.as_ref(), &fixture.posts[index]).await,
                "posts={posts}: post {index} is inside the requested range"
            );
        }
        assert!(
            !is_projected(
                fixture.store.as_ref(),
                &fixture.posts[posts - 50 - limit - 1]
            )
            .await,
            "posts={posts}: nothing beyond the requested range is read"
        );
        assert!(
            fixture
                .docs_sync
                .queries()
                .await
                .iter()
                .all(|(_, query)| matches!(query, DocQuery::Exact(_))),
            "posts={posts}: the reconcile must not scan a prefix"
        );
        counts.push(fixture.docs_sync.records_returned());
    }
    assert_eq!(
        counts[0], counts[1],
        "docs records read must not depend on the replica size: {counts:?}"
    );
    // 索引の key が limit 件、投稿の state が limit 件(取り下げの key は存在しないので 0 件)。
    assert!(counts[0] <= limit * 3, "{counts:?}");
}

// AC-2: projection が空でも、先頭のページは新しい側の limit 件だけを読む(replica は走査しない)。
#[tokio::test]
async fn head_page_reads_only_the_newest_entries() {
    let limit = 20usize;
    let mut counts = Vec::new();
    for posts in [300usize, 1_500] {
        let fixture = range_fixture("head", posts, |_| false).await;
        let hydrated = fixture
            .app
            .reconcile_timeline_range(fixture.topic.as_str(), &TimelineScope::Public, None, limit)
            .await
            .expect("reconcile");
        assert_eq!(hydrated, limit);
        assert!(is_projected(fixture.store.as_ref(), &fixture.posts[posts - 1]).await);
        assert!(is_projected(fixture.store.as_ref(), &fixture.posts[posts - limit]).await);
        assert!(!is_projected(fixture.store.as_ref(), &fixture.posts[posts - limit - 1]).await);
        counts.push(fixture.docs_sync.records_returned());
    }
    assert_eq!(counts[0], counts[1], "{counts:?}");
}

// 同じ範囲の照合は、間隔が空くまで繰り返さない(先頭ページは数秒ごとに取得されうる)。
#[tokio::test]
async fn the_same_range_is_not_checked_again_within_the_interval() {
    let fixture = range_fixture("ledger", 60, |_| true).await;
    let reconcile = || {
        fixture.app.reconcile_timeline_range(
            fixture.topic.as_str(),
            &TimelineScope::Public,
            None,
            20,
        )
    };
    reconcile().await.expect("first reconcile");
    // 1 回目は索引の key だけを読む(全件が projection にあるので state は読まない)。未来の時刻と形の違う key の
    // ための余裕ぶん、`limit` より多くの key を読むが、上限は固定。
    let first = fixture.docs_sync.records_returned();
    assert!((20..=20 + 64).contains(&first), "first={first}");
    fixture.docs_sync.reset_records_returned();
    for _ in 0..3 {
        reconcile().await.expect("repeated reconcile");
    }
    assert_eq!(
        fixture.docs_sync.records_returned(),
        0,
        "the same range is not read again within the interval"
    );
}

// INVAR-1: projection にある行でも、取り下げの event を取りこぼしていれば、照合が取り下げを反映して本文を伏せる。
#[tokio::test]
async fn reconcile_applies_a_withdrawal_that_was_never_delivered_as_an_event() {
    let (app, store, docs_sync, _blobs) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:range-withdrawal");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let envelope = persist_test_post(
        docs_sync.as_ref(),
        Some(store.as_ref()),
        &author_keys,
        &topic,
        PayloadRef::InlineText {
            text: "to be withdrawn".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    let withdrawal = build_post_withdrawal_envelope(
        &author_keys,
        &envelope,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("withdrawal");
    persist_post_withdrawal(docs_sync.as_ref(), &replica, &envelope.id, &withdrawal)
        .await
        .expect("persist withdrawal");

    let hydrated = app
        .reconcile_timeline_range(topic.as_str(), &TimelineScope::Public, None, 20)
        .await
        .expect("reconcile");
    assert_eq!(hydrated, 1);
    let projection_store: &dyn ProjectionStore = store.as_ref();
    assert!(
        projection_store
            .get_post_withdrawal(&envelope.id)
            .await
            .expect("withdrawal row")
            .is_some()
    );
    let row = ObjectProjectionStore::get_object_projection(store.as_ref(), &envelope.id)
        .await
        .expect("projection")
        .expect("row");
    assert_eq!(row.content.as_deref(), Some(""));
}

// INVAR-2 / TR-12: 参加していない private channel の範囲は照合しない(replica を読まない)。
#[tokio::test]
async fn private_channel_range_is_not_read_without_membership() {
    let fixture = range_fixture("private", 5, |_| true).await;
    let outcome = fixture
        .app
        .reconcile_timeline_range(
            fixture.topic.as_str(),
            &TimelineScope::Channel {
                channel_id: ChannelId::new("channel-not-joined"),
            },
            None,
            20,
        )
        .await;
    assert!(outcome.is_err());
    assert_eq!(fixture.docs_sync.records_returned(), 0);
    assert!(fixture.docs_sync.queries().await.is_empty());
}

// thread は途中の返信が欠けうる。thread の索引と照合して、欠けている返信だけを反映する。
#[tokio::test]
async fn thread_reconcile_fills_missing_replies_without_scanning() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-thread");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    // thread と無関係な投稿を多数置く(thread の照合が読む量に影響しない)。
    for index in 0..200usize {
        put_post_at(
            docs_sync.as_ref(),
            &replica,
            &author_keys,
            &topic,
            BASE_TIME + index as i64,
            "noise",
            None,
        )
        .await;
    }
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
    let mut replies = Vec::new();
    for index in 0..6usize {
        // 偶数番目の返信だけが projection にある。
        let projection: Option<&dyn ProjectionStore> = if index % 2 == 0 {
            Some(store.as_ref())
        } else {
            None
        };
        replies.push(
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
            .await,
        );
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
    docs_sync.clear_queries().await;

    let hydrated = app
        .reconcile_thread(topic.as_str(), &root.id)
        .await
        .expect("reconcile thread");
    assert_eq!(hydrated, 3);
    for reply in &replies {
        assert!(
            ObjectProjectionStore::get_object_projection(store.as_ref(), &reply.id)
                .await
                .expect("projection")
                .is_some()
        );
    }
    assert!(
        docs_sync
            .queries()
            .await
            .iter()
            .all(|(_, query)| matches!(query, DocQuery::Exact(_))),
        "the thread reconcile must not scan a prefix"
    );
    // thread の索引の key が 7 件(root と返信 6 件)、欠けていた返信の state が 3 件。
    assert_eq!(docs_sync.records_returned(), 10);
}

/// `objects/` などの prefix の読み出し(全件走査)を失敗させる docs。key 指定と、key だけの上限つきの
/// 読み出しは通す。取得が走査に頼っていないことを示す。
#[derive(Clone, Default)]
pub(super) struct NoScanDocsSync {
    inner: MemoryDocsSync,
}

#[async_trait]
impl DocsSync for NoScanDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        if !matches!(query, DocQuery::Exact(_)) {
            anyhow::bail!("replica scans are disabled in this test: {query:?}");
        }
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<Vec<kukuri_docs_sync::DocKeyEntry>> {
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

// TR-5: `list_timeline` と `list_thread` は、replica の走査が一切できない docs でも、空の projection から
// 先頭のページと、遡ったページを組み立てられる。
#[tokio::test]
async fn timeline_and_thread_pages_are_built_without_any_replica_scan() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(NoScanDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-no-scan");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let mut posts = Vec::new();
    for index in 0..45usize {
        posts.push(
            put_post_at(
                docs_sync.as_ref(),
                &replica,
                &author_keys,
                &topic,
                BASE_TIME + index as i64,
                format!("post {index}").as_str(),
                None,
            )
            .await,
        );
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

    let first = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("first page");
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.content.clone())
            .collect::<Vec<_>>(),
        (25..45)
            .rev()
            .map(|index| format!("post {index}"))
            .collect::<Vec<_>>()
    );
    let second = app
        .list_timeline(topic.as_str(), first.next_cursor.clone(), 20)
        .await
        .expect("second page");
    assert_eq!(
        second
            .items
            .iter()
            .map(|item| item.content.clone())
            .collect::<Vec<_>>(),
        (5..25)
            .rev()
            .map(|index| format!("post {index}"))
            .collect::<Vec<_>>()
    );
    let third = app
        .list_timeline(topic.as_str(), second.next_cursor.clone(), 20)
        .await
        .expect("third page");
    assert_eq!(third.items.len(), 5);
    assert!(third.next_cursor.is_none());

    let thread = app
        .list_thread(topic.as_str(), posts[44].object_id.as_str(), None, 20)
        .await
        .expect("thread");
    assert_eq!(thread.items.len(), 1);
    app.shutdown().await;
}
