//! #1813: session の hint の時点で読み先に無かった session を、lease の task が間を置いて上限つきで読み直す。
//! 判定は provider への読みの回数と projection で行う(時刻は止めた tokio の時計で進める)。

use super::hint_window::{hold_off_the_day_boundary, presence_marker, wait_until_consumed};
use super::hydration_integrity_sessions::{live_manifest, write_live_session};
use super::*;
use crate::service::scope_receive::{SESSION_RETRY_CAPACITY, SESSION_RETRY_DELAYS};
use kukuri_core::LiveSessionStatus;
use kukuri_transport::{HintStream, SeedPeer};
use std::sync::atomic::AtomicUsize;

/// 読み先の近隣(作った端末と視聴者 4 人)。近隣が 5 人のとき、読み先はこのうち 4 人。
const NEIGHBORS: [&str; 5] = ["creator", "viewer-1", "viewer-2", "viewer-3", "viewer-4"];

/// hint は `StaticTransport` で配り、読み先は `NEIGHBORS` から 4 人を、選ぶたびに 4 つ先の位置から返す(近隣が
/// 変わらないときの `topic_read_candidates` と同じ回り方)。
struct RotatingReadPeers {
    hints: Arc<StaticTransport>,
    next: AtomicUsize,
}

#[async_trait]
impl HintTransport for RotatingReadPeers {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<HintStream> {
        self.hints.subscribe_hints(topic).await
    }

    async fn unsubscribe_hints(&self, topic: &TopicId) -> Result<()> {
        self.hints.unsubscribe_hints(topic).await
    }

    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()> {
        self.hints.publish_hint(topic, hint).await
    }

    async fn topic_read_candidates(&self, _: &TopicId) -> Result<Vec<SeedPeer>> {
        let start = self.next.fetch_add(4, std::sync::atomic::Ordering::SeqCst);
        Ok((start..start + 4)
            .map(|index| SeedPeer {
                endpoint_id: NEIGHBORS[index % NEIGHBORS.len()].into(),
                addr_hint: None,
            })
            .collect())
    }
}

struct Fixture {
    app: AppService,
    store: Arc<MemoryStore>,
    /// 読み先。session は test が後から置く。読みの回数を数える。
    provider: Arc<CountingDocsSync>,
    blobs: Arc<MemoryBlobService>,
    transport: Arc<StaticTransport>,
    /// 読み先を近隣から順に回して選ぶとき(`fixture_rotating`)の、選ぶ位置。
    rotation: Option<Arc<RotatingReadPeers>>,
    topic: TopicId,
}

/// topic の列を開き、lease の task の起動時の読み直しを終えてから返す。手元の docs は空で、provider を読む。
async fn fixture(name: &str) -> Fixture {
    fixture_with(name, false).await
}

/// `rotating` なら、読み先は `NEIGHBORS` から順に回して選び、provider はその先頭(作った端末)だけ。
async fn fixture_with(name: &str, rotating: bool) -> Fixture {
    hold_off_the_day_boundary();
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let provider = Arc::new(CountingDocsSync::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let topic = TopicId::new(format!("kukuri:topic:session-hint-retry-{name}").as_str());
    let rotation = rotating.then(|| {
        Arc::new(RotatingReadPeers {
            hints: transport.clone(),
            next: AtomicUsize::new(0),
        })
    });
    let (docs, hints): (CountingDocsSync, Arc<dyn HintTransport>) = match &rotation {
        None => (
            CountingDocsSync::reading_from(provider.clone()),
            transport.clone(),
        ),
        Some(rotation) => (
            CountingDocsSync::reading_from_peers(
                NEIGHBORS
                    .iter()
                    .map(|peer| {
                        let docs: Arc<dyn DocsSync> = if *peer == NEIGHBORS[0] {
                            provider.clone()
                        } else {
                            Arc::new(CountingDocsSync::default())
                        };
                        (peer.to_string(), docs)
                    })
                    .collect(),
            ),
            rotation.clone(),
        ),
    };
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        hints,
        Arc::new(docs),
        blobs.clone(),
        generate_keys(),
    );
    display_topic(&app, topic.as_str())
        .await
        .expect("open the topic column");
    let fixture = Fixture {
        app,
        store,
        provider,
        blobs,
        transport,
        rotation,
        topic,
    };
    fixture.barrier().await;
    fixture
}

impl Fixture {
    /// 短命種の hint を 1 件送り、task がそれより前の hint を処理し終えるまで待つ。
    async fn barrier(&self) {
        let sender = self.transport.hint_sender(&self.topic).await;
        sender
            .send(presence_marker(&self.topic, 0))
            .expect("the topic subscription listens for hints");
        wait_until_consumed(&sender).await;
    }

    /// live session の hint を送る。`sent_at` を変えると、同じ session の続く hint になる。
    async fn hint(&self, session_id: &str, sent_at: i64) {
        self.transport
            .publish_hint(
                &channel_hint_topic_for(self.topic.as_str(), None),
                GossipHint::SessionChanged {
                    topic_id: self.topic.clone(),
                    session_id: session_id.into(),
                    object_kind: "live-session".into(),
                    sent_at: Some(sent_at),
                },
            )
            .await
            .expect("publish hint");
    }

    /// provider から、この session の state を読んだ回数。
    async fn reads(&self, session_id: &str) -> usize {
        let state = DocQuery::Exact(stable_key("sessions/live", &format!("{session_id}/state")));
        self.provider
            .queries()
            .await
            .iter()
            .filter(|(_, query)| *query == state)
            .count()
    }

    async fn listed(&self, session_id: &str) -> bool {
        let projection_store: &dyn ProjectionStore = self.store.as_ref();
        projection_store
            .get_live_session(self.topic.as_str(), session_id)
            .await
            .expect("projection")
            .is_some()
    }

    /// 最初の読み直し(と少しの余裕)のうちに一覧に出るか。
    async fn listed_by_the_first_retry(&self, session_id: &str) -> bool {
        timeout(SESSION_RETRY_DELAYS[0] + Duration::from_secs(1), async {
            while !self.listed(session_id).await {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    /// 読み先に、`owner` の署名つきの session を置く。
    async fn put_session(&self, owner: &KukuriKeys, session_id: &str) {
        write_live_session(
            self.provider.as_ref(),
            self.blobs.as_ref(),
            &topic_replica_id(self.topic.as_str()),
            Some(owner),
            &live_manifest(
                session_id,
                &self.topic,
                None,
                &owner.public_key_hex(),
                LiveSessionStatus::Live,
            ),
        )
        .await;
    }
}

/// 配信者と、その配信者に結び付いた session の id(作った時刻は今。読む replica は今作った session と同じ)。
fn owned_session() -> (KukuriKeys, String) {
    let owner = generate_keys();
    let session_id = format!(
        "live-{}-{}",
        Utc::now().timestamp_millis(),
        &owner.public_key_hex()[..16]
    );
    (owner, session_id)
}

// AC-1(T1): hint の時点で読み先に無かった session を、最初の読み直し(2 秒後)で一覧に出し、画面の読み直しの印
// (`last_sync_ts`)を進める。INVAR-2: 読めた後は読み直さない。
#[tokio::test(start_paused = true)]
async fn a_session_missing_at_hint_time_is_listed_by_the_first_retry() {
    let f = fixture("late").await;
    let (owner, session_id) = owned_session();
    f.hint(&session_id, 1).await;
    f.barrier().await;
    let per_read = f.reads(&session_id).await;
    assert!(per_read > 0, "the hint reads the session");
    assert!(!f.listed(&session_id).await);

    f.put_session(&owner, &session_id).await;
    f.app.last_sync_ts.set(0).await;
    assert!(
        f.listed_by_the_first_retry(&session_id).await,
        "the first retry lists the session"
    );
    assert!(f.app.last_sync_ts.get().await > Some(0));
    assert_eq!(f.reads(&session_id).await, 2 * per_read);

    sleep(SESSION_RETRY_DELAYS.iter().sum::<Duration>()).await;
    f.barrier().await;
    assert_eq!(
        f.reads(&session_id).await,
        2 * per_read,
        "a listed session is not read again"
    );
}

// AC-1 の原因(#1813)の再現: 読み先は近隣 5 人から 4 人を順に回して選ぶので、hint の時点の読みは、session を持つ
// 唯一の端末(作った端末)を外しうる。選ぶ位置がどこから始まっても、最初の読み直しまでに一覧に出る(1 回の読みの
// locator と replica ごとに選び直すと、選ぶ位置が 1 回の読みで一周し、読み直しも同じ 4 人に聞いて読めない)。
#[tokio::test(start_paused = true)]
async fn a_session_is_listed_by_the_first_retry_from_any_read_rotation() {
    let f = fixture_with("rotation", true).await;
    let rotation = f.rotation.as_ref().expect("the read peers rotate");
    for start in 0..NEIGHBORS.len() {
        let (owner, session_id) = owned_session();
        f.put_session(&owner, &session_id).await;
        rotation
            .next
            .store(start, std::sync::atomic::Ordering::SeqCst);
        f.hint(&session_id, 1).await;
        assert!(
            f.listed_by_the_first_retry(&session_id).await,
            "the first retry lists the session (the rotation starts at {start})"
        );
    }
}

// INVAR-2: 読み直しを待つ間に同じ session の続く hint で読めたら、その読み直しをやめる。
#[tokio::test(start_paused = true)]
async fn a_session_read_by_a_later_hint_is_not_retried() {
    let f = fixture("later-hint").await;
    let (owner, session_id) = owned_session();
    f.hint(&session_id, 1).await;
    f.barrier().await;
    let per_read = f.reads(&session_id).await;
    f.put_session(&owner, &session_id).await;
    f.hint(&session_id, 2).await;
    f.barrier().await;
    assert!(
        f.listed(&session_id).await,
        "the later hint lists the session"
    );

    sleep(SESSION_RETRY_DELAYS.iter().sum::<Duration>()).await;
    f.barrier().await;
    assert_eq!(
        f.reads(&session_id).await,
        2 * per_read,
        "the pending retry of a listed session is dropped"
    );
}

// INVAR-1(T2): 読めない session の読み直しは 1 件につき 3 回まで、待たせるのは task ごとに 16 件まで(17 件目は読み
// 直さない)。同じ session の続く hint は、読み直しを足さない。
#[tokio::test(start_paused = true)]
async fn retries_are_bounded_per_session_and_per_task() {
    let f = fixture("bounded").await;
    let ids = (0..=SESSION_RETRY_CAPACITY)
        .map(|index| format!("live-1-{index:016x}"))
        .collect::<Vec<_>>();
    // 同じ session の続く hint は、待ちに空きがあるうちに送る(空きが無くて足さないのと区別する)。
    f.hint(&ids[0], 0).await;
    for (index, session_id) in ids.iter().enumerate() {
        f.hint(session_id, index as i64 + 1).await;
    }
    f.barrier().await;
    let per_read = f.reads(&ids[1]).await;
    assert!(per_read > 0, "the hints read the sessions");
    assert_eq!(f.reads(&ids[0]).await, 2 * per_read);

    // 読み直しは前の読みから 2・4・8 秒後(hint から 2・6・14 秒後)。各回の直後に数え、3 回目の後は 1 分読まない。
    for (retried, delay) in SESSION_RETRY_DELAYS.iter().enumerate() {
        sleep(*delay + Duration::from_millis(500)).await;
        f.barrier().await;
        assert_eq!(f.reads(&ids[0]).await, (3 + retried) * per_read);
        for session_id in &ids[1..SESSION_RETRY_CAPACITY] {
            assert_eq!(f.reads(session_id).await, (2 + retried) * per_read);
        }
    }
    sleep(Duration::from_secs(60)).await;
    f.barrier().await;
    let retries = SESSION_RETRY_DELAYS.len();
    assert_eq!(f.reads(&ids[0]).await, (2 + retries) * per_read);
    for session_id in &ids[1..SESSION_RETRY_CAPACITY] {
        assert_eq!(f.reads(session_id).await, (1 + retries) * per_read);
    }
    assert_eq!(
        f.reads(&ids[SESSION_RETRY_CAPACITY]).await,
        per_read,
        "a hint beyond the capacity is not retried"
    );
}
