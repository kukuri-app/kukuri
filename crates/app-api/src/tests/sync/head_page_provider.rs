//! #1624: タイムラインの先頭のページの取得が、起動より前に手元へ反映した投稿だけなら provider の応答を待たずに返し、
//! それ以外は従来どおり provider のページを待つことを固定する。
//!
//! 起動直後は、既知の peer がまだ繋がっていないか応答しない。取得が provider のページの照合(期限 30 秒)を待つと、
//! 手元にある投稿も返せず、画面の定期更新は前の取得が終わるまで次を出さないので、Timeline 列が空のままになった。

use super::range_reconcile::{BASE_TIME, TestPost, project, put_post_at};
use super::range_reconcile_faults::put_dangling_index_entry;
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

/// provider は `posts` を古い順に 1 秒ずつ新しく持ち、索引の一覧に 2 秒ずつ遅れて答える(scope の 3 つの replica で
/// 計 6 秒)。viewer は古い側の `local` 件だけを、起動より前に反映した投稿として手元に持つ。
async fn viewer_of_slow_provider(
    topic: &TopicId,
    posts: &[String],
    local: usize,
) -> (AppService, Arc<SlowProvider>, Vec<TestPost>) {
    let replica = topic_replica_id(topic.as_str());
    let provider_docs = Arc::new(CountingDocsSync::default());
    let author = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let mut stored = Vec::with_capacity(posts.len());
    for (index, content) in posts.iter().enumerate() {
        let created_at = BASE_TIME + index as i64;
        let post = put_post_at(
            provider_docs.as_ref(),
            &replica,
            &author,
            topic,
            created_at,
            content,
            None,
        )
        .await;
        if index < local {
            project(store.as_ref(), &post, &replica).await;
        }
        stored.push(post);
    }
    let provider = Arc::new(SlowProvider {
        inner: provider_docs,
        delay: Duration::from_secs(2),
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
    });
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
    (viewer, provider, stored)
}

// AC-1: 起動より前に手元へ反映した投稿だけなら、provider が遅れて答える間も、先頭のページの取得は猶予の内に
// 手元の投稿を返す。
// INVAR-1: 猶予を過ぎた照合は背景で続き、反映した投稿は次の取得で出る。
// INVAR-2: 照合が続く間の取得は、provider を重ねて読まない。
#[tokio::test(start_paused = true)]
async fn the_head_page_returns_local_posts_without_waiting_for_a_slow_provider() {
    let topic = TopicId::new("kukuri:topic:head-page-slow-provider");
    let posts = ["local post".to_owned(), "provider post".to_owned()];
    let (viewer, provider, _) = viewer_of_slow_provider(&topic, &posts, 1).await;

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

// INVAR-5(#1239 AC-4): 背景の照合が猶予を過ぎた取得と、照合が続く間の取得も、前回の照合が数えた「本体が手元に無い
// 投稿の数」を返す(修正前の、台帳の間隔の内の取得と同じ)。数を落とすと、画面の表示が出たり消えたりする。
#[tokio::test(start_paused = true)]
async fn the_head_page_keeps_the_last_unavailable_count_while_the_provider_is_slow() {
    let topic = TopicId::new("kukuri:topic:head-page-unavailable");
    let (viewer, provider, _) =
        viewer_of_slow_provider(&topic, &["local post".to_owned()], 1).await;
    let replica = topic_replica_id(topic.as_str());
    for index in 0..3usize {
        put_dangling_index_entry(
            provider.inner.as_ref(),
            &replica,
            BASE_TIME + 1,
            format!("{index:064x}").as_str(),
        )
        .await;
    }
    let unavailable = async || {
        viewer
            .list_timeline(topic.as_str(), None, 20)
            .await
            .expect("timeline")
            .unavailable_count
    };

    unavailable().await;
    sleep(Duration::from_secs(7)).await;
    assert_eq!(unavailable().await, 3, "the finished check counted them");
    viewer.services.range_checks.expire_all_for_test().await;
    assert_eq!(unavailable().await, 3, "past the grace");
    sleep(Duration::from_secs(3)).await;
    assert_eq!(unavailable().await, 3, "while the check continues");
    viewer.shutdown().await;
}

/// provider が 21 件を持ち、手元にはこの起動の間に反映した新しい側の `filled` 件だけがある viewer の、先頭のページ。
async fn head_page_while_filling(name: &str, filled: usize) -> TimelineView {
    let topic = TopicId::new(format!("kukuri:topic:head-page-{name}"));
    let posts = (0..21)
        .map(|index| format!("post {index}"))
        .collect::<Vec<_>>();
    let (viewer, _, stored) = viewer_of_slow_provider(&topic, &posts, 0).await;
    let replica = topic_replica_id(topic.as_str());
    for post in stored.iter().rev().take(filled) {
        let PayloadRef::InlineText { text } = &post.payload_ref else {
            unreachable!("an inline post");
        };
        let mut row = verified_projection_row(&post.envelope, &replica, Some(text.clone()));
        row.derived_at = viewer.services.started_at_ms + 1;
        viewer
            .services
            .projection_store
            .put_object_projection(row)
            .await
            .expect("put projection");
    }
    let page = viewer
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    viewer.shutdown().await;
    page
}

// INVAR-4: 手元に投稿の無い先頭のページ(新しい端末・初めて開く scope)と、この起動の間に反映した投稿を含む先頭の
// ページ(lease の読み直しなど別の経路が手元を埋めている途中)は、従来どおり provider の照合を待ち、provider のページと
// 続きの位置を同じ応答で返す。途中の状態を先に返すと、画面はそれを最初のページにし、後の取得を「新しい投稿」として
// 保留して、続きのページを読めなかった(web-e2e の fallback)。
#[tokio::test(start_paused = true)]
async fn a_head_page_without_earlier_posts_waits_for_the_provider_page_and_its_cursor() {
    for (name, filled) in [("empty", 0), ("filling", 5)] {
        let page = head_page_while_filling(name, filled).await;
        assert_eq!(page.items.len(), 20, "{name}");
        assert_eq!(contents(&page)[0], "post 20", "{name}");
        assert!(
            page.next_cursor.is_some(),
            "{name}: the next page is reachable"
        );
    }
}
