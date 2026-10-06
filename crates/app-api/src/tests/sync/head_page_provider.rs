//! #1624: タイムラインの先頭のページの取得が、provider の応答を待たずに手元の投稿を返すことを固定する。
//!
//! 起動直後は、既知の peer がまだ繋がっていないか応答しない。取得が provider のページの照合(期限 30 秒)を待つと、
//! 手元にある投稿も返せず、画面の定期更新は前の取得が終わるまで次を出さないので、Timeline 列が空のままになった。

use super::range_reconcile::{BASE_TIME, project, put_post_at};
use super::*;

/// 時系列の索引の一覧(`indexes/timeline/`)に `delay` だけ遅れて答える provider。同時に答えている一覧の数の最大を数える。
struct SlowProvider {
    inner: Arc<CountingDocsSync>,
    delay: Duration,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

#[async_trait]
impl DocsSync for SlowProvider {
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
        if !query.prefix.starts_with("indexes/timeline/") {
            return self.inner.query_replica_keys(replica_id, query).await;
        }
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        sleep(self.delay).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
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

    async fn local_docs_author(&self) -> Result<Option<String>> {
        self.inner.local_docs_author().await
    }

    async fn query_replica_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        key: &str,
        policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Option<kukuri_docs_sync::DocRecord>> {
        self.inner
            .query_replica_by_author(replica_id, docs_author, key, policy)
            .await
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

fn contents(view: &TimelineView) -> Vec<&str> {
    view.items
        .iter()
        .map(|post| post.content.as_str())
        .collect()
}

// AC-1: provider が索引の一覧に 2 秒ずつ遅れて答える(scope の 3 つの replica で計 6 秒)間も、先頭のページの取得は
// 猶予の内に手元の投稿を返す。INVAR-1: 猶予を過ぎた照合は背景で続き、反映した投稿は次の取得で出る。
// INVAR-2: 照合が続く間の取得は、provider を重ねて読まない。
#[tokio::test(start_paused = true)]
async fn the_head_page_returns_local_posts_without_waiting_for_a_slow_provider() {
    let topic = TopicId::new("kukuri:topic:head-page-slow-provider");
    let replica = topic_replica_id(topic.as_str());
    let provider_docs = Arc::new(CountingDocsSync::default());
    let author = generate_keys();
    let local = put_post_at(
        provider_docs.as_ref(),
        &replica,
        &author,
        &topic,
        BASE_TIME,
        "local post",
        None,
    )
    .await;
    put_post_at(
        provider_docs.as_ref(),
        &replica,
        &author,
        &topic,
        BASE_TIME + 1,
        "provider post",
        None,
    )
    .await;
    let provider = Arc::new(SlowProvider {
        inner: provider_docs,
        delay: Duration::from_secs(2),
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
    });
    let store = Arc::new(MemoryStore::default());
    project(store.as_ref(), &local, &replica).await;
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let viewer = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        Arc::new(CountingDocsSync::reading_from(provider.clone())),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );

    let first = timeout(
        Duration::from_secs(1),
        viewer.list_timeline(topic.as_str(), None, 20),
    )
    .await
    .expect("the head page does not wait for the provider")
    .expect("timeline");
    assert_eq!(contents(&first), vec!["local post"]);

    sleep(Duration::from_secs(3)).await;
    let while_reading = viewer
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    assert_eq!(contents(&while_reading), vec!["local post"]);

    sleep(Duration::from_secs(4)).await;
    let after = viewer
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    assert_eq!(contents(&after), vec!["provider post", "local post"]);
    assert_eq!(provider.max_in_flight.load(Ordering::SeqCst), 1);
    viewer.shutdown().await;
}
