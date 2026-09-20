//! #1239: 購読タスクが replica を走査せず、窓(新しい側の固定件数)の追いつきで反映することを固定する。

use super::range_reconcile::{BASE_TIME, is_projected, put_post_at};
use super::*;
use kukuri_docs_sync::{ReplicaNotice, ReplicaNoticeStream};

/// docs の通知を test から流し込む docs。entry の event は購読側へ届けない(取りこぼした状態を作る)。
#[derive(Clone)]
struct InjectedNoticesDocsSync {
    inner: CountingDocsSync,
    notices: tokio::sync::broadcast::Sender<ReplicaNotice>,
}

impl Default for InjectedNoticesDocsSync {
    fn default() -> Self {
        Self {
            inner: CountingDocsSync::default(),
            notices: tokio::sync::broadcast::channel(16).0,
        }
    }
}

#[async_trait]
impl DocsSync for InjectedNoticesDocsSync {
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
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn subscribe_replica(
        &self,
        _replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        Ok(Box::pin(futures_util::stream::pending()))
    }

    async fn subscribe_replica_notices(
        &self,
        _replica_id: &ReplicaId,
    ) -> Result<ReplicaNoticeStream> {
        let stream = tokio_stream::wrappers::BroadcastStream::new(self.notices.subscribe());
        Ok(Box::pin(futures_util::StreamExt::filter_map(
            stream,
            |item| async move { item.ok().map(Ok) },
        )))
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

fn app_over(docs_sync: Arc<dyn DocsSync>) -> (AppService, Arc<MemoryStore>) {
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

async fn put_posts(
    docs_sync: &dyn DocsSync,
    topic: &TopicId,
    count: usize,
    offset: i64,
) -> Vec<super::range_reconcile::TestPost> {
    let replica = topic_replica_id(topic.as_str());
    let keys = generate_keys();
    let mut posts = Vec::new();
    for index in 0..count {
        posts.push(
            put_post_at(
                docs_sync,
                &replica,
                &keys,
                topic,
                BASE_TIME + offset + index as i64,
                format!("post {index}").as_str(),
                None,
            )
            .await,
        );
    }
    posts
}

// TR-1 / AC-1: 購読タスクの起動が読む docs の量は、replica の投稿の総数に依存しない。replica を走査しない。
#[tokio::test]
async fn subscription_start_reads_a_bounded_window_regardless_of_the_replica_size() {
    let mut counts = Vec::new();
    for size in [300usize, 1_500] {
        let docs_sync = Arc::new(CountingDocsSync::default());
        let topic = TopicId::new(format!("kukuri:topic:catch-up-start-{size}"));
        let posts = put_posts(docs_sync.as_ref(), &topic, size, 0).await;
        let (app, store) = app_over(docs_sync.clone());
        docs_sync.reset_records_returned();
        docs_sync.clear_queries().await;
        app.ensure_topic_subscription(topic.as_str())
            .await
            .expect("subscribe");
        // 起動時の追いつきが終わる(窓の最後の object が反映される)まで待つ。
        let oldest_in_window = &posts[size - 200];
        timeout(Duration::from_secs(20), async {
            while !is_projected(store.as_ref(), oldest_in_window).await {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the window must be projected at start");
        sleep(Duration::from_millis(200)).await;
        assert!(
            !is_projected(store.as_ref(), &posts[0]).await,
            "posts older than the window are left to the range reconcile"
        );
        assert_eq!(
            docs_sync.object_scans().await,
            0,
            "the subscription start must not scan the replica"
        );
        assert!(
            docs_sync
                .queries()
                .await
                .iter()
                .all(|(_, query)| matches!(query, DocQuery::Exact(_))),
            "only key-specified reads are allowed"
        );
        counts.push(docs_sync.records_returned());
        app.shutdown().await;
    }
    assert_eq!(
        counts[0], counts[1],
        "the amount read at start must not grow with the replica: {counts:?}"
    );
}

// TR-2: event を取りこぼした(`Lagged`)後、窓の範囲は追いつく。走査はしない。
#[tokio::test]
async fn lagged_notice_makes_the_window_catch_up_without_a_scan() {
    let docs_sync = Arc::new(InjectedNoticesDocsSync::default());
    let topic = TopicId::new("kukuri:topic:catch-up-lagged");
    let (app, store) = app_over(docs_sync.clone());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe");
    sleep(Duration::from_millis(200)).await;

    // 購読の開始後に届いた投稿。entry の event は購読側へ届いていない。
    let posts = put_posts(docs_sync.as_ref(), &topic, 5, 0).await;
    sleep(Duration::from_millis(300)).await;
    assert!(!is_projected(store.as_ref(), &posts[0]).await);

    docs_sync.inner.clear_queries().await;
    docs_sync
        .notices
        .send(ReplicaNotice::Lagged { missed: 20 })
        .expect("a subscriber is listening");
    timeout(Duration::from_secs(10), async {
        loop {
            let mut all = true;
            for post in &posts {
                all &= is_projected(store.as_ref(), post).await;
            }
            if all {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the window must catch up after a lag");
    assert_eq!(docs_sync.inner.object_scans().await, 0);
    app.shutdown().await;
}

// 同期の区切り(`SyncFinished`)も追いつきの契機になる。短い間隔で続いても、追いつきは 1 回にまとまる。
#[tokio::test]
async fn sync_finished_notices_are_coalesced_into_one_catch_up() {
    let docs_sync = Arc::new(InjectedNoticesDocsSync::default());
    let topic = TopicId::new("kukuri:topic:catch-up-sync-finished");
    let (app, store) = app_over(docs_sync.clone());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe");
    sleep(Duration::from_millis(200)).await;
    let posts = put_posts(docs_sync.as_ref(), &topic, 3, 0).await;

    docs_sync.inner.reset_records_returned();
    for _ in 0..10 {
        docs_sync
            .notices
            .send(ReplicaNotice::SyncFinished)
            .expect("a subscriber is listening");
    }
    timeout(Duration::from_secs(10), async {
        while !is_projected(store.as_ref(), &posts[2]).await {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the window must catch up after a sync");
    let after_first = docs_sync.inner.records_returned();
    // 追いつきの直後に届いた通知のぶんは、最小間隔が過ぎるまで追いつかない。
    for _ in 0..5 {
        docs_sync
            .notices
            .send(ReplicaNotice::SyncFinished)
            .expect("a subscriber is listening");
    }
    sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        docs_sync.inner.records_returned(),
        after_first,
        "ten notices in a burst must not run ten catch-ups"
    );
    app.shutdown().await;
}

// TR-3: 変化の無い replica では、時間が過ぎても購読タスクは docs を読まない。
#[tokio::test]
async fn idle_subscription_does_not_read_docs() {
    let docs_sync = Arc::new(CountingDocsSync::with_assist_peer_ids(vec!["peer-a"]));
    let topic = TopicId::new("kukuri:topic:catch-up-idle");
    put_posts(docs_sync.as_ref(), &topic, 30, 0).await;
    let (app, _store) = app_over(docs_sync.clone());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe");
    sleep(Duration::from_millis(500)).await;

    docs_sync.reset_records_returned();
    docs_sync.clear_queries().await;
    // recovery tick の期限(3 秒)を超えて待つ。docs の支援 peer がいるので、再 sync は促される。
    sleep(Duration::from_millis(
        PUBLIC_TOPIC_RECOVERY_GRACE_MS as u64 + 1_500,
    ))
    .await;
    assert!(
        docs_sync.restarts().await >= 1,
        "the recovery tick still prompts a re-sync"
    );
    assert_eq!(docs_sync.records_returned(), 0, "but it reads nothing");
    assert!(docs_sync.queries().await.is_empty());
    app.shutdown().await;
}
