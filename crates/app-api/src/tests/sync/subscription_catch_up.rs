//! #1239: 購読タスクが replica を走査せず、窓(新しい側の固定件数)の追いつきで反映することを固定する。

use super::range_reconcile::{BASE_TIME, is_projected, put_post_at};
use super::*;
use kukuri_docs_sync::{ReplicaNotice, ReplicaNoticeStream};

/// docs の通知を test から流し込む docs。entry の event は購読側へ届けない(取りこぼした状態を作る)。
#[derive(Clone)]
pub(super) struct InjectedNoticesDocsSync {
    inner: CountingDocsSync,
    pub(super) notices: tokio::sync::broadcast::Sender<ReplicaNotice>,
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

    async fn query_replica_keys_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner
            .query_replica_keys_by_author(replica_id, docs_author, query)
            .await
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

// 購読タスクで同じことを確かめる。空振りの追いつきを 3 回重ねた後(次の期限は約 12 秒先)に、取りこぼした投稿と
// `Lagged` が届く。最小間隔(3 秒)に余裕を足した 6 秒の内に反映されることを期待する。
#[tokio::test]
async fn a_lag_after_empty_catch_ups_is_reflected_within_the_minimum_interval() {
    let docs_sync = Arc::new(InjectedNoticesDocsSync::default());
    let topic = TopicId::new("kukuri:topic:catch-up-idle-then-lag");
    let (app, store) = app_over(docs_sync.clone());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe");
    sleep(Duration::from_millis(200)).await;
    // 空振りの追いつきを 3 回(0 秒、約 3 秒後、約 9 秒後)。
    for wait_ms in [1_500u64, 4_000, 7_000] {
        docs_sync
            .notices
            .send(ReplicaNotice::SyncFinished)
            .expect("a subscriber is listening");
        sleep(Duration::from_millis(wait_ms)).await;
    }
    let posts = put_posts(docs_sync.as_ref(), &topic, 3, 0).await;
    docs_sync
        .notices
        .send(ReplicaNotice::Lagged { missed: 20 })
        .expect("a subscriber is listening");
    timeout(Duration::from_secs(6), async {
        while !is_projected(store.as_ref(), &posts[2]).await {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("a catch-up after a lag must not wait for the interval stretched by empty runs");
    app.shutdown().await;
}

// INVAR-1: 取り下げの event を取りこぼしても、窓の中の投稿なら、`Lagged` の後の追いつきが取り下げを反映する。
// 起動時の追いつきも、projection に既にある投稿の取り下げを確かめる。
#[tokio::test]
async fn missed_withdrawal_in_the_window_is_applied_after_a_lag_and_at_start() {
    let docs_sync = Arc::new(InjectedNoticesDocsSync::default());
    let topic = TopicId::new("kukuri:topic:catch-up-withdrawal");
    let replica = topic_replica_id(topic.as_str());
    let author = generate_keys();
    let (app, store) = app_over(docs_sync.clone());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe");
    sleep(Duration::from_millis(200)).await;
    let post = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &author,
        &topic,
        BASE_TIME,
        "to be withdrawn",
        None,
    )
    .await;
    super::range_reconcile::project(store.as_ref(), &post, &replica).await;
    let withdrawal = build_post_withdrawal_envelope(
        &author,
        &post.envelope,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("withdrawal");
    persist_post_withdrawal(docs_sync.as_ref(), &replica, &post.envelope.id, &withdrawal)
        .await
        .expect("persist withdrawal");
    docs_sync
        .notices
        .send(ReplicaNotice::Lagged { missed: 1 })
        .expect("a subscriber is listening");
    let projection_store: &dyn ProjectionStore = store.as_ref();
    timeout(Duration::from_secs(8), async {
        while projection_store
            .get_post_withdrawal(&post.envelope.id)
            .await
            .expect("withdrawal row")
            .is_none()
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("a missed withdrawal is applied by the catch-up after a lag");
    app.shutdown().await;

    // 起動時: 別の store(投稿だけ反映済み)で購読を始める。
    let (restarted, restarted_store) = app_over(docs_sync.clone());
    super::range_reconcile::project(restarted_store.as_ref(), &post, &replica).await;
    restarted
        .ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe");
    let projection_store: &dyn ProjectionStore = restarted_store.as_ref();
    timeout(Duration::from_secs(8), async {
        while projection_store
            .get_post_withdrawal(&post.envelope.id)
            .await
            .expect("withdrawal row")
            .is_none()
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the catch-up at start checks the withdrawal of a projected post");
    restarted.shutdown().await;
}

fn index_event(
    topic: &TopicId,
    post: &super::range_reconcile::TestPost,
    source_peer: Option<&str>,
) -> ReplicaNotice {
    ReplicaNotice::Entry(kukuri_docs_sync::DocEvent {
        replica_id: topic_replica_id(topic.as_str()),
        key: stable_key(
            "indexes/timeline",
            &format!(
                "{}/{}",
                timeline_sort_key(post.created_at, &post.object_id),
                post.object_id.as_str()
            ),
        ),
        content_hash: String::new(),
        source_peer: source_peer.map(str::to_string),
        docs_author: None,
    })
}

// 相手から届いた索引の entry は個別反映の対象ではない(0 件)。指す object が projection に無ければ追いつきを
// 依頼し、既にあれば依頼しない。反映済みの投稿の envelope の entry と、自分が書いた entry(`source_peer` なし)も依頼しない。
#[tokio::test]
async fn a_remote_index_entry_requests_a_catch_up_only_for_an_unprojected_object() {
    let docs_sync = Arc::new(InjectedNoticesDocsSync::default());
    let topic = TopicId::new("kukuri:topic:catch-up-index-entry");
    let replica = topic_replica_id(topic.as_str());
    let (app, store) = app_over(docs_sync.clone());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe");
    sleep(Duration::from_millis(200)).await;
    let posts = put_posts(docs_sync.as_ref(), &topic, 2, 0).await;
    super::range_reconcile::project(store.as_ref(), &posts[0], &replica).await;

    // 自分が書いた entry と、反映済みの object を指す索引の entry・envelope の entry: 追いつかない。
    docs_sync.inner.reset_records_returned();
    let projected_envelope = ReplicaNotice::Entry(kukuri_docs_sync::DocEvent {
        replica_id: replica.clone(),
        key: stable_key(
            "objects",
            &format!("{}/envelope", posts[0].object_id.as_str()),
        ),
        content_hash: String::new(),
        source_peer: Some("peer-a".into()),
        docs_author: None,
    });
    for notice in [
        index_event(&topic, &posts[1], None),
        index_event(&topic, &posts[0], Some("peer-a")),
        projected_envelope,
    ] {
        docs_sync
            .notices
            .send(notice)
            .expect("a subscriber is listening");
    }
    sleep(Duration::from_millis(4_500)).await;
    assert!(
        !is_projected(store.as_ref(), &posts[1]).await,
        "no event may run a catch-up"
    );
    // 相手の envelope の entry は、通知の判定が envelope を 1 回読む。追いつき(窓の読み出し)は読まない。
    assert!(
        docs_sync.inner.records_returned() <= 1,
        "no event may run a catch-up"
    );

    // 相手から届いた、反映されていない object を指す索引の entry: 追いつく。
    docs_sync
        .notices
        .send(index_event(&topic, &posts[1], Some("peer-a")))
        .expect("a subscriber is listening");
    timeout(Duration::from_secs(6), async {
        while !is_projected(store.as_ref(), &posts[1]).await {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("a remote index entry for an unprojected object must be caught up");
    app.shutdown().await;
}
