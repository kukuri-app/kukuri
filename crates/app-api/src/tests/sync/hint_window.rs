//! #1567 AC-1: lease の task が 1 つの窓(`HINT_WINDOW`)で読む content hint は `HINT_WINDOW_BUDGET` 件までで、超えた分は
//! peer の学習も docs の読みもせずに捨て、窓の終わりに 1 回の読み直し(新しい側の窓と session)で回収する。
//! 判定は所要時間ではなく provider への読みの回数で行い、hint の数 N を増やしても、hint の時点の読みの回数と窓の終わりの
//! 読み直しの回数が増えないことを確かめる。

use super::range_reconcile::{BASE_TIME, TestPost, is_projected, put_post_at};
use super::*;
use crate::service::scope_receive::{HINT_WINDOW, HINT_WINDOW_BUDGET, until_next_bucket};

struct Fixture {
    /// 列の lease を持ち続ける(落とすと task が止まる)。
    _app: AppService,
    store: Arc<MemoryStore>,
    /// 手元の docs。空で、provider を読む。
    docs: Arc<CountingDocsSync>,
    /// 投稿を持つ provider。読みの回数を数える。
    provider: Arc<CountingDocsSync>,
    transport: Arc<StaticTransport>,
    topic: TopicId,
    replica: ReplicaId,
    author: KukuriKeys,
}

/// task の日の境界の読み直し(UTC の日付の切れ目)が、この test の窓に入らないようにする。境界まで 2 分未満なら、
/// 境界を実時間で過ぎてから始める(1 日のうちの 2 分だけ待つ)。
fn hold_off_the_day_boundary() {
    let remaining = until_next_bucket(Utc::now().timestamp());
    if remaining < Duration::from_secs(120) {
        std::thread::sleep(remaining + Duration::from_secs(1));
    }
}

/// topic の列を開き、lease の task の起動時の読み直しを終えてから返す。投稿はまだ無い。
async fn fixture(name: &str) -> Fixture {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let provider = Arc::new(CountingDocsSync::default());
    let docs = Arc::new(CountingDocsSync::reading_from(provider.clone()));
    let topic = TopicId::new(format!("kukuri:topic:hint-window-{name}").as_str());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport.clone(),
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    display_topic(&app, topic.as_str())
        .await
        .expect("open the topic column");
    let fixture = Fixture {
        _app: app,
        store,
        docs,
        provider,
        transport,
        replica: topic_replica_id(topic.as_str()),
        topic,
        author: generate_keys(),
    };
    fixture.barrier().await;
    fixture
}

/// 送った hint が task に取り出されるまで待つ。
async fn wait_until_consumed(sender: &broadcast::Sender<HintEnvelope>) {
    timeout(Duration::from_secs(30), async {
        while !sender.is_empty() {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the lease task must consume the hints");
}

impl Fixture {
    fn presence_marker(&self, dropped_before: u64) -> HintEnvelope {
        HintEnvelope {
            hint: GossipHint::LivePresence {
                topic_id: self.topic.clone(),
                session_id: "barrier".into(),
                author: Pubkey::from("a".repeat(64)),
                ttl_ms: 1,
            },
            received_at: 0,
            source_peer: String::new(),
            dropped_before,
        }
    }

    /// 短命種の hint を 1 件送り、task がそれを取り出すまで待つ。task は 1 件ずつ直列に処理するので、それより前の hint の
    /// 処理と、窓の終わりの読み直しは終わっている。
    async fn barrier(&self) {
        let sender = self.transport.hint_sender(&self.topic).await;
        sender
            .send(self.presence_marker(0))
            .expect("the topic subscription listens for hints");
        wait_until_consumed(&sender).await;
    }

    /// 投稿を provider に置く(手元の docs には無い)。古い順。
    async fn put_posts(&self, count: usize) -> Vec<TestPost> {
        let mut posts = Vec::with_capacity(count);
        for index in 0..count {
            posts.push(
                put_post_at(
                    self.provider.as_ref(),
                    &self.replica,
                    &self.author,
                    &self.topic,
                    BASE_TIME + index as i64,
                    format!("post {index}").as_str(),
                    None,
                )
                .await,
            );
        }
        posts
    }

    /// 投稿の hint を古い順に送る。task が追いつくのを待ちながら送り、transport の双子の捨て口(容量 64)では落とさせない。
    async fn publish_post_hints(&self, posts: &[TestPost]) {
        let sender = self.transport.hint_sender(&self.topic).await;
        for (index, post) in posts.iter().enumerate() {
            self.transport
                .publish_hint(
                    &channel_hint_topic_for(self.topic.as_str(), None),
                    GossipHint::TopicObjectsChanged {
                        topic_id: self.topic.clone(),
                        objects: vec![HintObjectRef {
                            object_id: post.object_id.as_str().to_string(),
                            object_kind: "post".into(),
                            docs_author: None,
                            sent_at: None,
                        }],
                    },
                )
                .await
                .expect("publish hint");
            if index % 16 == 15 {
                wait_until_consumed(&sender).await;
            }
        }
        self.barrier().await;
    }

    async fn projected(&self, posts: &[TestPost]) -> usize {
        let mut projected = 0;
        for post in posts {
            if is_projected(self.store.as_ref(), post).await {
                projected += 1;
            }
        }
        projected
    }
}

/// N 件の hint を送ったときの、hint の時点と窓の終わりの読みの回数と、反映の結果。
struct Observed {
    /// hint の時点に provider から読んだ record の query の回数。
    hint_time_reads: usize,
    /// hint の時点に手元の docs が学習した peer の回数。
    learned_peers: usize,
    /// 窓を進めた後の、provider への key 一覧の回数。
    window_key_queries: usize,
    /// 窓を進めた後の、provider から読んだ record の query の回数。
    window_reads: usize,
    /// 窓を進めた後に projection にある投稿の数。
    projected_after_window: usize,
}

async fn observe(hints: usize) -> Observed {
    let fixture = fixture(format!("burst-{hints}").as_str()).await;
    let posts = fixture.put_posts(hints).await;
    fixture.provider.clear_queries().await;
    let keys_before = fixture.provider.key_queries();
    let learned_before = fixture.docs.learned_peers();

    fixture.publish_post_hints(&posts).await;
    let hint_time_reads = fixture.provider.queries().await.len();
    let learned_peers = fixture.docs.learned_peers() - learned_before;
    assert_eq!(
        fixture.provider.key_queries(),
        keys_before,
        "hints read their target by key and never page the index"
    );

    // 窓を進める。溢れた窓だけ、終わりに 1 回読み直す。
    sleep(HINT_WINDOW + Duration::from_millis(1)).await;
    fixture.barrier().await;
    let window_key_queries = fixture.provider.key_queries() - keys_before;
    let window_reads = fixture.provider.queries().await.len() - hint_time_reads;
    let projected_after_window = fixture.projected(&posts).await;
    Observed {
        hint_time_reads,
        learned_peers,
        window_key_queries,
        window_reads,
        projected_after_window,
    }
}

// AC-1: 1 窓の反映は B 件まで。N を B の 1 倍・10 倍・20 倍にしても、hint の時点の読みと peer の学習は増えず、
// 溢れた窓の終わりの読み直し(key 一覧と record の読み)の回数も N に依らない。溢れが無い窓は docs を読まない。
#[tokio::test(start_paused = true)]
async fn hint_reads_per_window_are_bounded_and_the_overflow_is_reread_once() {
    hold_off_the_day_boundary();
    let budget = HINT_WINDOW_BUDGET;
    let within = observe(budget).await;
    let over_by_one = observe(budget + 1).await;
    let ten_times = observe(budget * 10).await;
    let twenty_times = observe(budget * 20).await;

    assert!(
        within.hint_time_reads > 0,
        "the hints within the budget are read"
    );
    for (name, observed) in [
        ("B+1", &over_by_one),
        ("10B", &ten_times),
        ("20B", &twenty_times),
    ] {
        assert_eq!(
            observed.hint_time_reads, within.hint_time_reads,
            "{name}: reads at hint time must not grow with the number of hints"
        );
        assert_eq!(
            observed.learned_peers, budget,
            "{name}: peers are learned only for the hints that were applied"
        );
    }
    assert_eq!(within.learned_peers, budget);

    assert_eq!(
        within.window_key_queries, 0,
        "a window without overflow reads nothing"
    );
    assert_eq!(
        within.window_reads, 0,
        "a window without overflow reads nothing"
    );
    assert_eq!(within.projected_after_window, budget);

    assert!(
        over_by_one.window_key_queries > 0,
        "an overflowed window rereads the scope"
    );
    assert_eq!(
        over_by_one.projected_after_window,
        budget + 1,
        "the one dropped hint is recovered by the reread"
    );

    assert_eq!(
        ten_times.window_key_queries, twenty_times.window_key_queries,
        "the reread pages the index a fixed number of times"
    );
    assert_eq!(
        ten_times.window_reads, twenty_times.window_reads,
        "the reread hydrates at most the window, not the dropped hints"
    );
    assert_eq!(
        ten_times.projected_after_window, twenty_times.projected_after_window,
        "the recovered rows are bounded by the window"
    );
    assert!(ten_times.projected_after_window > budget);
}

// AC-1: transport の購読 stream が取りこぼした印(`dropped_before`)だけでも、窓の終わりに 1 回読み直す。印の無い窓は読まない。
#[tokio::test(start_paused = true)]
async fn a_transport_drop_marker_triggers_one_reread_at_the_end_of_the_window() {
    hold_off_the_day_boundary();
    let fixture = fixture("dropped-before").await;
    let posts = fixture.put_posts(3).await;
    fixture.provider.clear_queries().await;
    let keys_before = fixture.provider.key_queries();

    sleep(HINT_WINDOW + Duration::from_millis(1)).await;
    fixture.barrier().await;
    assert_eq!(
        fixture.provider.key_queries(),
        keys_before,
        "no marker, no reread"
    );
    assert!(fixture.provider.queries().await.is_empty());
    assert_eq!(fixture.projected(&posts).await, 0);

    let sender = fixture.transport.hint_sender(&fixture.topic).await;
    sender
        .send(fixture.presence_marker(1))
        .expect("the topic subscription listens for hints");
    wait_until_consumed(&sender).await;
    assert!(
        fixture.provider.queries().await.is_empty(),
        "the marker itself reads nothing before the window ends"
    );

    sleep(HINT_WINDOW + Duration::from_millis(1)).await;
    fixture.barrier().await;
    assert!(
        fixture.provider.key_queries() > keys_before,
        "the marker makes the window reread"
    );
    assert_eq!(fixture.projected(&posts).await, posts.len());
}
