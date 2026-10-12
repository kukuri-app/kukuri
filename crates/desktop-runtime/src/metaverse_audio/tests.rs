//! ADR 0067 の #1033 の contract。実 iroh の 2 node の間で、機器を使わずに話者の音声を流して確かめる。

use super::*;
use kukuri_blob_service::MemoryBlobService;
use kukuri_core::{KukuriKeys, TopicId, generate_keys};
use kukuri_docs_sync::MemoryDocsSync;
use kukuri_store::MemoryStore;
use kukuri_transport::{FakeNetwork, FakeTransport};
use moq_net::group;

const TOPIC: &str = "kukuri:topic:spatial-audio";

/// 試験の端末の app と、その account の公開鍵。
fn app(keys: KukuriKeys, network: &FakeNetwork) -> (Arc<AppService>, Pubkey) {
    let pubkey = keys.public_key();
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new(pubkey.as_str(), network.clone()));
    let app = Arc::new(AppService::from_handles(kukuri_app_api::ServiceHandles::new(
        store.clone(),
        store,
        transport.clone(),
        transport,
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        keys,
    )));
    (app, pubkey)
}

fn room(owner: &Pubkey, generation: u64) -> MetaverseAudioRoom {
    MetaverseAudioRoom {
        spatial_context: SpatialContextV1::Topic {
            topic_id: TopicId::new(TOPIC),
        },
        owner_pubkey: owner.clone(),
        instance_id: "dome-a".into(),
        instance_generation: generation,
        session_id: "dome-a".into(),
    }
}

fn request(room: Option<MetaverseAudioRoom>, speakers: &[(&str, &str)]) -> SetMetaverseAudioRequest {
    SetMetaverseAudioRequest {
        room,
        microphone_enabled: false,
        listener_position: [0, 0, 0],
        neighbors: Vec::new(),
        speakers: speakers
            .iter()
            .map(|(endpoint_id, instance_id)| MetaverseAudioSpeaker {
                endpoint_id: (*endpoint_id).into(),
                instance_id: (*instance_id).into(),
            })
            .collect(),
    }
}

/// 試験の話者: mic の代わりに、参加中の room の broadcast へ 20 ms ごとに Opus を書き、位置を 1 回書く。
async fn speak(audio: &MetaverseAudio, position: [i64; 3]) -> AbortOnDropHandle<()> {
    let (mut broadcast, mut position_track) = {
        let state = audio.state.lock().await;
        let room = state.as_ref().expect("speaker room");
        let broadcast = room
            .origin
            .publish(room.broadcast_path().as_str(), origin::Route::default())
            .expect("publish");
        let position_track = broadcast
            .create_track(METAVERSE_AUDIO_POSITION_TRACK, None)
            .expect("position track");
        (broadcast, position_track)
    };
    let catalog =
        moq_mux::catalog::Producer::new(&mut broadcast, moq_mux::catalog::Config::default())
            .expect("catalog");
    let mut options = moq_audio::encode::Options::default();
    options.track = Some(METAVERSE_AUDIO_TRACK.into());
    options.settings = moq_audio::encode::Settings::new(OPUS_SAMPLE_RATE, moq_audio::Layout::Mono);
    let mut producer = moq_audio::encode::Producer::new(
        &mut broadcast,
        catalog,
        moq_audio::encode::Input::new(OPUS_SAMPLE_RATE, moq_audio::Layout::Mono),
        &options,
    )
    .expect("encoder");
    AbortOnDropHandle::new(n0_future::task::spawn(async move {
        let _broadcast = broadcast;
        let mut tick = n0_future::time::interval(Duration::from_millis(20));
        for frame in 0_u64.. {
            tick.tick().await;
            let _ = position_track.write_frame(
                Timestamp::from_micros(frame * 20_000).expect("timestamp"),
                serde_json::to_vec(&position).expect("position"),
            );
            let samples: Vec<u8> = (0..960)
                .map(|index| ((index as f32) * 0.05).sin() * 0.25)
                .flat_map(f32::to_le_bytes)
                .collect();
            let _ = producer.write(&moq_audio::Frame::new(
                samples.into(),
                Timestamp::from_micros(frame * 20_000).expect("timestamp"),
            ));
        }
    }))
}

async fn written(audio: &MetaverseAudio) -> usize {
    let state = audio.state.lock().await;
    state
        .as_ref()
        .and_then(|room| room.playback.as_ref())
        .map_or(0, |playback| {
            playback
                .outputs
                .lock()
                .expect("outputs")
                .iter()
                .map(|output| output.written.lock().expect("written").len())
                .sum()
        })
}

async fn volume(audio: &MetaverseAudio) -> Option<f32> {
    let state = audio.state.lock().await;
    let room = state.as_ref()?;
    let speaker = room.speakers.values().next()?;
    *speaker.volume.0.lock().expect("volume")
}

async fn eventually(mut condition: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..150 {
        if condition().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

struct Pair {
    speaker_node: Arc<IrohDocsNode>,
    listener_node: Arc<IrohDocsNode>,
    speaker_app: Arc<AppService>,
    listener_app: Arc<AppService>,
    owner: Pubkey,
    listener_pubkey: Pubkey,
    speaker: Arc<MetaverseAudio>,
    listener: Arc<MetaverseAudio>,
}

async fn pair() -> Pair {
    let network = FakeNetwork::default();
    let (speaker_app, owner) = app(generate_keys(), &network);
    let (listener_app, listener_pubkey) = app(generate_keys(), &network);
    let speaker_node = IrohDocsNode::memory().await.expect("speaker node");
    let listener_node = IrohDocsNode::memory().await.expect("listener node");
    listener_node
        .discovery()
        .add_endpoint_info(speaker_node.endpoint().addr());
    let speaker = Arc::new(MetaverseAudio::default());
    speaker_node.install_media_authorizer(Some(media_authorizer(
        Arc::downgrade(&speaker),
        Arc::downgrade(&speaker_app),
    )));
    Pair {
        speaker_node,
        listener_node,
        speaker_app,
        listener_app,
        owner,
        listener_pubkey,
        speaker,
        listener: Arc::new(MetaverseAudio::default()),
    }
}

impl Pair {
    fn speaker_id(&self) -> String {
        self.speaker_node.endpoint().id().to_string()
    }

    async fn shutdown(self) {
        let _ = self.listener_node.shutdown().await;
        let _ = self.speaker_node.shutdown().await;
    }
}

#[test]
fn metaverse_audio_gain_uses_opening_distance_for_ready_neighbors() {
    let listener = Listener {
        position: [0, 0, 200],
        current_instance: "dome-a".into(),
        neighbors: HashMap::from([("dome-b".to_string(), ([300, 0, 400], [0, 0, 0]))]),
    };
    // current: 直線距離 400 cm → 1 m / 4 m。
    assert_eq!(listener.gain("dome-a", [0, 0, 600]), 0.25);
    // 1 m 以内は 1。
    assert_eq!(listener.gain("dome-a", [0, 0, 250]), 1.0);
    // ready な隣接: 話者 → 話者側 opening(500) + 聞き手側 opening → 聞き手(200) = 700 cm → 1 m / 7 m。
    assert_eq!(listener.gain("dome-b", [0, 0, 0]), 0.142);
    // ready でない隣接(一覧に無い)は再生しない。
    assert_eq!(listener.gain("dome-c", [0, 0, 0]), 0.0);
}

#[test]
fn metaverse_audio_frame_budget_rejects_excess_frames_and_bytes() {
    let start = Instant::now();
    let mut budget = FrameBudget::new(50, 32 * 1024);
    for _ in 0..50 {
        budget.admit(start, 100).expect("within the frame budget");
    }
    let rejection = budget.admit(start, 100).expect_err("the 51st frame in one second");
    assert_eq!(rejection.code(), "METAVERSE_PLAYER_AUDIO_FRAME_RATE_RATE_EXCEEDED");
    // 窓が進めば受け付ける。bytes の上限も同じ窓で数える。
    let later = start + Duration::from_secs(1);
    budget.admit(later, 32 * 1024).expect("a full window of bytes");
    let rejection = budget.admit(later, 1).expect_err("over the byte budget");
    assert_eq!(rejection.code(), "METAVERSE_PLAYER_AUDIO_BANDWIDTH_RATE_EXCEEDED");
}

/// 同じ話者の frame がまとめて・順序を入れ替えて届いても、上限(160 ms)の中なら時刻順に並べ直して読む。
/// 旧経路(届いた順の即時再生)では到着順に再生していた(修正前の再現は PR に記録)。
#[tokio::test]
async fn metaverse_audio_plays_bursty_and_reordered_frames_in_timestamp_order() {
    let mut broadcast = moq_net::broadcast::Info::new().produce();
    let track = broadcast
        .create_track(METAVERSE_AUDIO_TRACK, None)
        .expect("audio track");
    let consumer = broadcast.consume();
    let container = moq_mux::catalog::hang::Container::Legacy(moq_mux::container::Kind::Audio);
    let mut frames = audio_frames(&consumer, container).await.expect("frames");
    // 180 ms 相当(9 frame)をまとめて、group の順を入れ替えて書く。
    for sequence in [0_u64, 2, 1, 4, 3, 5, 8, 6, 7] {
        let mut group = track
            .create_group(group::Info { sequence })
            .expect("group");
        hang::container::Frame {
            timestamp: Timestamp::from_micros(sequence * 20_000).expect("timestamp"),
            payload: vec![u8::try_from(sequence).expect("small")].into(),
        }
        .write_to(&mut group)
        .expect("frame");
        group.finish().expect("finish");
    }
    track.finish().expect("finish track");
    let mut order = Vec::new();
    while let Some(frame) = timeout(Duration::from_secs(5), frames.read())
        .await
        .expect("frame in time")
        .expect("read")
    {
        order.push(frame.payload[0]);
    }
    assert_eq!(order, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);
}

#[tokio::test]
async fn metaverse_audio_starts_only_after_explicit_toggle_and_device_open() {
    let network = FakeNetwork::default();
    let (app, owner) = app(generate_keys(), &network);
    let audio = MetaverseAudio::default();
    // 操作の前は off。
    let status = audio.set(&app, None, request(Some(room(&owner, 1)), &[])).await;
    assert_eq!(status.microphone, MetaverseMicrophoneStatus::Off);
    // 機器を開けない build では、mic を on にしても開始を表示せず、broadcast を公開しない(送信 0)。
    let mut on = request(Some(room(&owner, 1)), &[]);
    on.microphone_enabled = true;
    let status = audio.set(&app, None, on).await;
    assert_eq!(status.microphone, MetaverseMicrophoneStatus::Unavailable);
    let state = audio.state.lock().await;
    let room_audio = state.as_ref().expect("room");
    assert!(room_audio.microphone.is_none());
    let path = room_audio.broadcast_path();
    assert!(
        timeout(Duration::from_millis(300), room_audio.origin.consume().routed(path.as_str()))
            .await
            .ok()
            .flatten()
            .is_none(),
        "nothing is published without an open device"
    );
    drop(state);
    // 再起動の後(新しい状態)は off から始まる。
    let restarted = MetaverseAudio::default();
    let status = restarted.set(&app, None, request(Some(room(&owner, 1)), &[])).await;
    assert_eq!(status.microphone, MetaverseMicrophoneStatus::Off);
}

#[tokio::test]
async fn metaverse_audio_reaches_an_allowed_listener_and_stops_on_leave() {
    let pair = pair().await;
    let owner = pair.owner.clone();
    let speaker_id = pair.speaker_id();
    pair.speaker
        .set(&pair.speaker_app, Some(pair.speaker_node.clone()), request(Some(room(&owner, 1)), &[]))
        .await;
    let _voice = speak(&pair.speaker, [0, 0, 400]).await;
    let status = pair
        .listener
        .set(
            &pair.listener_app,
            Some(pair.listener_node.clone()),
            request(Some(room(&owner, 1)), &[(&speaker_id, "dome-a")]),
        )
        .await;
    assert_eq!(status.listening, 1);
    assert!(
        eventually(async || written(&pair.listener).await > 0).await,
        "the allowed listener plays the speaker"
    );
    // 距離減衰: 聞き手(原点)から 400 cm → 0.25。
    assert!(eventually(async || volume(&pair.listener).await == Some(0.25)).await);

    // 話者が退出すると、送る側の評価し直しで閉じ、聞き手の再生が止まる。
    pair.speaker.set(&pair.speaker_app, None, request(None, &[])).await;
    assert!(
        eventually(async || {
            let state = pair.listener.state.lock().await;
            state
                .as_ref()
                .and_then(|room| room.speakers.values().next())
                .is_some_and(|speaker| speaker.finished.load(Ordering::Acquire))
        })
        .await,
        "the subscription ends after the speaker leaves"
    );
    let stopped = written(&pair.listener).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(written(&pair.listener).await, stopped, "no playback after the stop");
    // 聞き手の退出で、購読と再生をすべて回収する。
    let status = pair.listener.set(&pair.listener_app, None, request(None, &[])).await;
    assert_eq!(status.listening, 0);
    assert!(pair.listener.state.lock().await.is_none());
    pair.shutdown().await;
}

#[tokio::test]
async fn metaverse_audio_subscribe_is_denied_without_dome_access() {
    let pair = pair().await;
    let owner = pair.owner.clone();
    let speaker_id = pair.speaker_id();
    pair.speaker
        .set(&pair.speaker_app, Some(pair.speaker_node.clone()), request(Some(room(&owner, 1)), &[]))
        .await;
    let _voice = speak(&pair.speaker, [0, 0, 100]).await;

    // 話者が参加していない世代の Dome への要求は拒否する。
    let stale = MetaverseAudioAccessRequestV1 {
        proof: pair
            .listener_app
            .build_dome_access_proof(room(&owner, 2).spatial_context, owner.clone())
            .await
            .expect("proof"),
        instance_id: "dome-a".into(),
        instance_generation: 2,
        session_id: "dome-a".into(),
    };
    let denied = pair
        .listener_node
        .media_subscribe(&speaker_id, &serde_json::to_vec(&stale).expect("request"))
        .await
        .err()
        .expect("denied");
    assert_eq!(
        denied.downcast_ref::<kukuri_iroh_node::MediaDenied>().map(|denied| denied.0.as_str()),
        Some(DENIED_NOT_IN_DOME)
    );

    // 別の owner に束縛した proof は拒否する(理由の code だけで、音声と識別子は返らない)。
    let other_owner: Pubkey = generate_keys().public_key();
    let unbound = MetaverseAudioAccessRequestV1 {
        proof: pair
            .listener_app
            .build_dome_access_proof(room(&owner, 1).spatial_context, other_owner)
            .await
            .expect("proof"),
        instance_id: "dome-a".into(),
        instance_generation: 1,
        session_id: "dome-a".into(),
    };
    let denied = pair
        .listener_node
        .media_subscribe(&speaker_id, &serde_json::to_vec(&unbound).expect("request"))
        .await
        .err()
        .expect("denied");
    assert_eq!(
        denied.downcast_ref::<kukuri_iroh_node::MediaDenied>().map(|denied| denied.0.as_str()),
        Some(DENIED_ACCESS)
    );

    // owner(ここでは話者)が聞き手を block していれば拒否し、聞き手は再生しない。
    pair.speaker_app
        .block_author(pair.listener_pubkey.as_str())
        .await
        .expect("block");
    pair.listener
        .set(
            &pair.listener_app,
            Some(pair.listener_node.clone()),
            request(Some(room(&owner, 1)), &[(&speaker_id, "dome-a")]),
        )
        .await;
    assert!(
        eventually(async || {
            let state = pair.listener.state.lock().await;
            state
                .as_ref()
                .and_then(|room| room.speakers.values().next())
                .is_some_and(|speaker| speaker.finished.load(Ordering::Acquire))
        })
        .await
    );
    assert_eq!(written(&pair.listener).await, 0, "a blocked listener hears nothing");
    pair.shutdown().await;
}

#[tokio::test]
async fn metaverse_audio_budget_rejects_and_bounds_ledgers() {
    let network = FakeNetwork::default();
    let (app, owner) = app(generate_keys(), &network);
    let node = IrohDocsNode::memory().await.expect("node");
    let audio = MetaverseAudio::default();
    // 画面が 20 人を渡しても、同時に購読するのは client budget の 16 人まで(近い順の先頭)。
    let endpoints: Vec<String> = (0..20_u8)
        .map(|index| iroh::SecretKey::from_bytes(&[index + 1; 32]).public().to_string())
        .collect();
    let speakers: Vec<(&str, &str)> = endpoints
        .iter()
        .map(|endpoint| (endpoint.as_str(), "dome-a"))
        .collect();
    let status = audio
        .set(&app, Some(node.clone()), request(Some(room(&owner, 1)), &speakers))
        .await;
    assert_eq!(status.listening, 16);
    {
        let state = audio.state.lock().await;
        let room_audio = state.as_ref().expect("room");
        assert!(room_audio
            .speakers
            .keys()
            .all(|endpoint| endpoints[..16].contains(endpoint)));
        assert_eq!(room_audio.attempts.len(), 16);
    }
    // 同じ話者へは 5 秒に 1 回まで、試行の記録は 64 件まで。
    let mut state = audio.state.lock().await;
    let room_audio = state.as_mut().expect("room");
    let now = Instant::now();
    assert!(!room_audio.may_attempt(&endpoints[0], now));
    for index in 0..100 {
        room_audio.may_attempt(&format!("peer-{index}"), now);
    }
    assert_eq!(room_audio.attempts.len(), ATTEMPT_LEDGER_LIMIT);
    assert!(room_audio.may_attempt(&endpoints[0], now + RETRY_INTERVAL));
    drop(state);
    let _ = node.shutdown().await;
}

#[tokio::test]
async fn metaverse_audio_mute_drops_before_decode() {
    let pair = pair().await;
    let owner = pair.owner.clone();
    let speaker_id = pair.speaker_id();
    pair.speaker
        .set(&pair.speaker_app, Some(pair.speaker_node.clone()), request(Some(room(&owner, 1)), &[]))
        .await;
    let _voice = speak(&pair.speaker, [0, 0, 100]).await;
    pair.listener
        .set(
            &pair.listener_app,
            Some(pair.listener_node.clone()),
            request(Some(room(&owner, 1)), &[(&speaker_id, "dome-a")]),
        )
        .await;
    assert!(eventually(async || written(&pair.listener).await > 0).await);
    // mute した話者は画面が候補から外す。購読を止め、以後の decode・再生は 0(参加・presence は変えない)。
    let status = pair
        .listener
        .set(
            &pair.listener_app,
            Some(pair.listener_node.clone()),
            request(Some(room(&owner, 1)), &[]),
        )
        .await;
    assert_eq!(status.listening, 0);
    let muted = written(&pair.listener).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(written(&pair.listener).await, muted);
    pair.shutdown().await;
}
