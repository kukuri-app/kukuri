//! #1816: session の hint で session を読むとき、反映済みと同じ版を返す読み先で読みを終えず、残りの読み先から新しい版を
//! 探す(手元に保持した他の参加者の版と同じ扱い)。

use super::hint_window::{hold_off_the_day_boundary, presence_marker, wait_until_consumed};
use super::hydration_integrity_sessions::{
    game_manifest, live_manifest, write_game_room, write_live_session,
};
use super::*;
use kukuri_core::{
    GameRoomManifestBlobV1, GameRoomStatus, LiveSessionManifestBlobV1, LiveSessionStatus,
};
use kukuri_transport::{HintStream, SeedPeer};

/// 読み先は、いつもこの順(先に古い版を持つ視聴者、次に配信者)。
const PEERS: [&str; 2] = ["viewer", "host"];

struct FixedReadPeers(Arc<StaticTransport>);

#[async_trait]
impl HintTransport for FixedReadPeers {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<HintStream> {
        self.0.subscribe_hints(topic).await
    }

    async fn unsubscribe_hints(&self, topic: &TopicId) -> Result<()> {
        self.0.unsubscribe_hints(topic).await
    }

    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()> {
        self.0.publish_hint(topic, hint).await
    }

    async fn topic_read_candidates(&self, _: &TopicId) -> Result<Vec<SeedPeer>> {
        Ok(PEERS
            .iter()
            .map(|peer| SeedPeer {
                endpoint_id: (*peer).into(),
                addr_hint: None,
            })
            .collect())
    }
}

struct Fixture {
    _app: AppService,
    store: Arc<MemoryStore>,
    transport: Arc<StaticTransport>,
    viewer: Arc<CountingDocsSync>,
    host: Arc<CountingDocsSync>,
    blobs: Arc<MemoryBlobService>,
    topic: TopicId,
    replica: ReplicaId,
    owner: KukuriKeys,
}

/// topic の列を開いた視聴者。読み先の視聴者と配信者の docs は、test が session を置く。
async fn fixture(name: &str) -> Fixture {
    hold_off_the_day_boundary();
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let viewer = Arc::new(CountingDocsSync::default());
    let host = Arc::new(CountingDocsSync::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let topic = TopicId::new(format!("kukuri:topic:session-newer-provider-{name}").as_str());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        Arc::new(FixedReadPeers(transport.clone())),
        Arc::new(CountingDocsSync::reading_from_peers(HashMap::from([
            (PEERS[0].to_string(), viewer.clone() as Arc<dyn DocsSync>),
            (PEERS[1].to_string(), host.clone() as Arc<dyn DocsSync>),
        ]))),
        blobs.clone(),
        generate_keys(),
    );
    display_topic(&app, topic.as_str())
        .await
        .expect("open the topic column");
    Fixture {
        _app: app,
        store,
        transport,
        viewer,
        host,
        blobs,
        replica: topic_replica_id(topic.as_str()),
        topic,
        owner: generate_keys(),
    }
}

impl Fixture {
    /// 配信者に結び付いた session の id(作った時刻は今)。
    fn session_id(&self, prefix: &str) -> String {
        format!(
            "{prefix}-{}-{}",
            Utc::now().timestamp_millis(),
            &self.owner.public_key_hex()[..16]
        )
    }

    /// session の hint を送り、task がそれを処理し終えるまで待つ。
    async fn hint(&self, session_id: &str, object_kind: &str, sent_at: i64) {
        self.transport
            .publish_hint(
                &channel_hint_topic_for(self.topic.as_str(), None),
                GossipHint::SessionChanged {
                    topic_id: self.topic.clone(),
                    session_id: session_id.into(),
                    object_kind: object_kind.into(),
                    sent_at: Some(sent_at),
                },
            )
            .await
            .expect("publish hint");
        let sender = self.transport.hint_sender(&self.topic).await;
        sender
            .send(presence_marker(&self.topic, 0))
            .expect("the topic subscription listens for hints");
        wait_until_consumed(&sender).await;
    }

    async fn put_live(&self, docs: &CountingDocsSync, manifest: &LiveSessionManifestBlobV1) {
        write_live_session(
            docs,
            self.blobs.as_ref(),
            &self.replica,
            Some(&self.owner),
            manifest,
        )
        .await;
    }

    async fn put_game(&self, docs: &CountingDocsSync, manifest: &GameRoomManifestBlobV1) {
        write_game_room(
            docs,
            self.blobs.as_ref(),
            &self.replica,
            Some(&self.owner),
            manifest,
        )
        .await;
    }

    fn projection(&self) -> &dyn ProjectionStore {
        self.store.as_ref()
    }
}

// AC-1: 先の読み先(視聴者)が反映済みと同じ版(配信中)を返しても、後の読み先(配信者)の新しい版(終了)を一覧に反映する。
#[tokio::test]
async fn a_hinted_live_session_is_read_past_a_provider_that_returns_the_projected_version() {
    let f = fixture("live").await;
    let session_id = f.session_id("live");
    let status = async || {
        f.projection()
            .get_live_session(f.topic.as_str(), &session_id)
            .await
            .expect("projection")
            .map(|row| row.status)
    };
    let live = live_manifest(
        &session_id,
        &f.topic,
        None,
        &f.owner.public_key_hex(),
        LiveSessionStatus::Live,
    );
    f.put_live(&f.viewer, &live).await;
    f.put_live(&f.host, &live).await;
    f.hint(&session_id, "live-session", 1).await;
    assert_eq!(status().await, Some(LiveSessionStatus::Live));

    // 配信者が session を終える。視聴者は前の版のまま。
    let ended = LiveSessionManifestBlobV1 {
        revision: 2,
        status: LiveSessionStatus::Ended,
        ended_at: Some(Utc::now().timestamp_millis()),
        ..live
    };
    f.put_live(&f.host, &ended).await;
    f.hint(&session_id, "live-session", 2).await;
    assert_eq!(status().await, Some(LiveSessionStatus::Ended));
}

// AC-1(game room): 同じく、後の読み先の新しい版(得点の版 2・終了)を一覧に反映する。
#[tokio::test]
async fn a_hinted_game_room_is_read_past_a_provider_that_returns_the_projected_version() {
    let f = fixture("game").await;
    let room_id = f.session_id("game");
    let status = async || {
        f.projection()
            .get_game_room(f.topic.as_str(), &room_id)
            .await
            .expect("projection")
            .map(|row| row.status)
    };
    let waiting = game_manifest(&room_id, &f.topic, None, &f.owner.public_key_hex());
    f.put_game(&f.viewer, &waiting).await;
    f.put_game(&f.host, &waiting).await;
    f.hint(&room_id, "game-session", 1).await;
    assert_eq!(status().await, Some(GameRoomStatus::Waiting));

    let ended = GameRoomManifestBlobV1 {
        score_revision: Some(2),
        status: GameRoomStatus::Ended,
        updated_at: Utc::now().timestamp_millis(),
        ..waiting
    };
    f.put_game(&f.host, &ended).await;
    f.hint(&room_id, "game-session", 2).await;
    assert_eq!(status().await, Some(GameRoomStatus::Ended));
}
