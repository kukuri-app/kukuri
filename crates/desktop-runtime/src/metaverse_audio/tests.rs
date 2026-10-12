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
    let app = Arc::new(AppService::from_handles(
        kukuri_app_api::ServiceHandles::new(
            store.clone(),
            store,
            transport.clone(),
            transport,
            Arc::new(MemoryDocsSync::default()),
            Arc::new(MemoryBlobService::default()),
            keys,
        ),
    ));
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

fn request(
    room: Option<MetaverseAudioRoom>,
    speakers: &[(&str, &str)],
) -> SetMetaverseAudioRequest {
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
        let mut frame = 0_u64;
        loop {
            tick.tick().await;
            frame += 1;
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

/// 聞き手の(最初の)話者の購読が終わったか(拒否・切断・公開の終了)。
async fn subscription_ended(audio: &MetaverseAudio) -> bool {
    let state = audio.state.lock().await;
    state
        .as_ref()
        .and_then(|room| room.speakers.values().next())
        .is_some_and(|speaker| speaker.finished.load(Ordering::Acquire))
}

/// 以後の再生が無い(300 ms の間に書いた sample が増えない)。
async fn stays_silent(audio: &MetaverseAudio) -> bool {
    let before = written(audio).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    written(audio).await == before
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

    /// 話者が世代 1 の Dome に入り、聞き手(原点)から 400 cm の位置で話す。
    async fn speaker_speaks(&self) -> AbortOnDropHandle<()> {
        self.speaker
            .set(
                &self.speaker_app,
                Some(self.speaker_node.clone()),
                request(Some(room(&self.owner, 1)), &[]),
            )
            .await;
        speak(&self.speaker, [0, 0, 400]).await
    }

    /// 聞き手が `generation` の Dome で話者を聞く。
    async fn listen(&self, generation: u64) -> MetaverseAudioStatus {
        let speaker_id = self.speaker_id();
        self.listener
            .set(
                &self.listener_app,
                Some(self.listener_node.clone()),
                request(
                    Some(room(&self.owner, generation)),
                    &[(&speaker_id, "dome-a")],
                ),
            )
            .await
    }

    /// 聞き手の退出(購読・再生・試行の記録をすべて捨てる)。
    async fn listener_leaves(&self) -> MetaverseAudioStatus {
        self.listener
            .set(&self.listener_app, None, request(None, &[]))
            .await
    }

    /// 聞き手の proof つきの認可要求(話者の世代 1 の Dome)。
    async fn access_request(&self) -> Vec<u8> {
        serde_json::to_vec(&MetaverseAudioAccessRequestV1 {
            proof: self
                .listener_app
                .build_dome_access_proof(room(&self.owner, 1).spatial_context, self.owner.clone())
                .await
                .expect("proof"),
            instance_id: "dome-a".into(),
            instance_generation: 1,
            session_id: "dome-a".into(),
        })
        .expect("request")
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
    let at = |micros: u64| start + Duration::from_micros(micros);
    // 正規の 20 ms の frame は、到着の揺れ(最大 2 ms 早い)と 180 ms 分のまとめ到着があっても 10 秒間捨てない。
    let mut budget = FrameBudget::new(50, 32 * 1024);
    for index in 0..500_u64 {
        let arrival = if (100..109).contains(&index) {
            108 * 20_000
        } else {
            index * 20_000 + 2_000 - index % 3 * 1_000
        };
        budget.admit(at(arrival), 100).expect("a regular frame");
    }
    // 上限を超え続ける送り手(60 frame/s を 10 秒)は、1 秒分の枠と毎秒 50 frame を超える分だけ捨てる。
    let mut budget = FrameBudget::new(50, 32 * 1024);
    let mut admitted = 0;
    let mut rejection = None;
    for index in 0..600_u64 {
        match budget.admit(at(index * 16_667), 100) {
            Ok(()) => admitted += 1,
            Err(error) => rejection = Some(error),
        }
    }
    assert!((545..=551).contains(&admitted), "admitted {admitted}");
    assert_eq!(
        rejection.expect("excess frames").code(),
        "METAVERSE_PLAYER_AUDIO_FRAME_RATE_RATE_EXCEEDED"
    );
    // bytes も同じ: 1 秒分(32 KiB)を使い切った直後の frame は捨てる。
    let mut budget = FrameBudget::new(50, 32 * 1024);
    budget
        .admit(start, 32 * 1024)
        .expect("a full second of bytes");
    let rejection = budget.admit(start, 1).expect_err("over the byte budget");
    assert_eq!(
        rejection.code(),
        "METAVERSE_PLAYER_AUDIO_BANDWIDTH_RATE_EXCEEDED"
    );
}

/// 同じ話者の frame がまとめて・順序を入れ替えて届いても、上限(160 ms)の中なら時刻順に並べ直して読み、上限より
/// 遅れた分は捨てる。旧経路(届いた順の即時再生)では到着順に再生していた(修正前の再現は PR に記録)。
#[tokio::test]
async fn metaverse_audio_plays_bursty_and_reordered_frames_in_timestamp_order() {
    let broadcast = moq_net::broadcast::Info::new().produce();
    let track = broadcast
        .create_track(METAVERSE_AUDIO_TRACK, None)
        .expect("audio track");
    let consumer = broadcast.consume();
    let container = moq_mux::catalog::hang::Container::Legacy(moq_mux::container::Kind::Audio);
    let mut frames = audio_frames(&consumer, container, OPUS_FRAME * 8)
        .await
        .expect("frames");
    let write = |sequence: u64| {
        let mut group = track.create_group(group::Info { sequence }).expect("group");
        hang::container::Frame {
            timestamp: Timestamp::from_micros(sequence * 20_000).expect("timestamp"),
            payload: vec![u8::try_from(sequence).expect("small")].into(),
        }
        .write_to(&mut group)
        .expect("frame");
        group.finish().expect("finish");
    };
    let mut read = async |count: usize| {
        let mut order = Vec::new();
        for _ in 0..count {
            let frame = timeout(Duration::from_secs(5), frames.read())
                .await
                .expect("frame in time")
                .expect("read")
                .expect("frame");
            order.push(frame.payload[0]);
        }
        order
    };
    // 180 ms 相当(9 frame)をまとめて、group の順を入れ替えて書く。
    for sequence in [0_u64, 2, 1, 4, 3, 5, 8, 6, 7] {
        write(sequence);
    }
    assert_eq!(read(9).await, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);
    // group 9〜19 が届かないまま 160 ms より先(group 20〜30)へ進むと、待たずに先へ進み(上限より遅れた group は
    // 古いものから飛ばす)、後から届いた古い group 10 は読まない。
    for sequence in 20..=30 {
        write(sequence);
    }
    let mut latest = Vec::new();
    while latest.last() != Some(&30) {
        latest.extend(read(1).await);
    }
    assert!(
        latest[0] >= 20 && latest.windows(2).all(|pair| pair[0] < pair[1]),
        "{latest:?}"
    );
    write(10);
    track.finish().expect("finish track");
    assert!(
        timeout(Duration::from_secs(5), frames.read())
            .await
            .expect("end in time")
            .expect("read")
            .is_none(),
        "the late group is dropped"
    );
}

#[tokio::test]
async fn metaverse_audio_starts_only_after_explicit_toggle_and_device_open() {
    let network = FakeNetwork::default();
    let (app, owner) = app(generate_keys(), &network);
    let audio = MetaverseAudio::default();
    // 操作の前は off。
    let status = audio
        .set(&app, None, request(Some(room(&owner, 1)), &[]))
        .await;
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
        timeout(
            Duration::from_millis(300),
            room_audio.origin.consume().routed(path.as_str())
        )
        .await
        .ok()
        .flatten()
        .is_none(),
        "nothing is published without an open device"
    );
    drop(state);
    // 再起動の後(新しい状態)は off から始まる。
    let restarted = MetaverseAudio::default();
    let status = restarted
        .set(&app, None, request(Some(room(&owner, 1)), &[]))
        .await;
    assert_eq!(status.microphone, MetaverseMicrophoneStatus::Off);
}

/// 退出・失効・世代の変更で購読と再生が止まり、前の世代の音声を新しい世代へ書かない。offline grace は画面が room を
/// 外す(`useMetaverseRoomSession` の試験)ので、ここでは退出と同じ入力になる。再起動の後に mic が off から始まる
/// ことは `metaverse_audio_starts_only_after_explicit_toggle_and_device_open` が確かめる。
#[tokio::test]
async fn metaverse_audio_stops_on_leave_access_change_and_restart() {
    let pair = pair().await;
    let voice = pair.speaker_speaks().await;
    assert_eq!(pair.listen(1).await.listening, 1);
    assert!(
        eventually(async || written(&pair.listener).await > 0).await,
        "the allowed listener plays the speaker"
    );
    // 距離減衰: 聞き手(原点)から 400 cm → 0.25。
    assert!(eventually(async || volume(&pair.listener).await == Some(0.25)).await);

    // 世代の変更: 前の世代の購読と再生を捨てる。話者は新しい世代に居ないので拒否され、新しい世代には何も書かない。
    pair.listen(2).await;
    assert!(eventually(async || subscription_ended(&pair.listener).await).await);
    assert_eq!(
        written(&pair.listener).await,
        0,
        "nothing from generation 1 plays in generation 2"
    );

    // 話者の退出: 送る側が閉じ、聞き手の再生が止まる。
    pair.listen(1).await;
    assert!(eventually(async || written(&pair.listener).await > 0).await);
    pair.speaker
        .set(&pair.speaker_app, None, request(None, &[]))
        .await;
    assert!(
        eventually(async || subscription_ended(&pair.listener).await).await,
        "the subscription ends after the speaker leaves"
    );
    assert!(
        stays_silent(&pair.listener).await,
        "no playback after the speaker leaves"
    );
    drop(voice);

    // 話者が戻ると、聞き手の同じ要求の送り直し(画面は 5 秒ごと)で、終わった購読を試し直す。
    let _voice = pair.speaker_speaks().await;
    let stopped = written(&pair.listener).await;
    assert!(
        eventually(async || {
            pair.listen(1).await;
            written(&pair.listener).await > stopped
        })
        .await,
        "the same request retries the ended subscription"
    );

    // 失効: owner(ここでは話者)が聞き手を block すると、送る側の評価し直し(5 秒ごと)で閉じる。
    pair.speaker_app
        .block_author(pair.listener_pubkey.as_str())
        .await
        .expect("block");
    assert!(
        eventually(async || subscription_ended(&pair.listener).await).await,
        "the speaker closes the revoked listener"
    );
    assert!(
        stays_silent(&pair.listener).await,
        "no playback after the access is revoked"
    );

    // 聞き手の退出で、購読と再生をすべて回収する。
    assert_eq!(pair.listener_leaves().await.listening, 0);
    assert!(pair.listener.state.lock().await.is_none());
    pair.shutdown().await;
}

#[tokio::test]
async fn metaverse_audio_subscribe_is_denied_without_dome_access() {
    let pair = pair().await;
    let owner = pair.owner.clone();
    let speaker_id = pair.speaker_id();
    let _voice = pair.speaker_speaks().await;

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
        denied
            .downcast_ref::<kukuri_iroh_node::MediaDenied>()
            .map(|denied| denied.0.as_str()),
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
        denied
            .downcast_ref::<kukuri_iroh_node::MediaDenied>()
            .map(|denied| denied.0.as_str()),
        Some(DENIED_ACCESS)
    );

    // owner(ここでは話者)が聞き手を block していれば拒否し、聞き手は再生しない。
    pair.speaker_app
        .block_author(pair.listener_pubkey.as_str())
        .await
        .expect("block");
    pair.listen(1).await;
    assert!(eventually(async || subscription_ended(&pair.listener).await).await);
    assert_eq!(
        written(&pair.listener).await,
        0,
        "a blocked listener hears nothing"
    );
    pair.shutdown().await;
}

#[tokio::test]
async fn metaverse_audio_budget_rejects_and_bounds_ledgers() {
    let pair = pair().await;
    // 聞き手: 画面が近い順に 16 人の 10 倍を渡しても、同時に購読・接続を試すのは client budget の 16 人まで(先頭から)。
    let endpoints: Vec<String> = (0..160_u8)
        .map(|index| {
            iroh::SecretKey::from_bytes(&[index + 1; 32])
                .public()
                .to_string()
        })
        .collect();
    let speakers: Vec<(&str, &str)> = endpoints
        .iter()
        .map(|endpoint| (endpoint.as_str(), "dome-a"))
        .collect();
    let status = pair
        .listener
        .set(
            &pair.listener_app,
            Some(pair.listener_node.clone()),
            request(Some(room(&pair.owner, 1)), &speakers),
        )
        .await;
    assert_eq!(status.listening, 16);
    {
        let state = pair.listener.state.lock().await;
        let room_audio = state.as_ref().expect("room");
        assert!(
            room_audio
                .speakers
                .keys()
                .all(|endpoint| endpoints[..16].contains(endpoint))
        );
        assert_eq!(room_audio.attempts.len(), 16);
    }
    // 同じ話者へは 5 秒に 1 回まで、試行の記録は 64 件・30 秒まで。
    {
        let mut state = pair.listener.state.lock().await;
        let room_audio = state.as_mut().expect("room");
        let now = Instant::now();
        assert!(!room_audio.may_attempt(&endpoints[0], now));
        for index in 0..100 {
            room_audio.may_attempt(&format!("peer-{index}"), now);
        }
        assert_eq!(room_audio.attempts.len(), ATTEMPT_LEDGER_LIMIT);
        assert!(room_audio.may_attempt(&endpoints[0], now + RETRY_INTERVAL));
        // 30 秒たつと、`now` の 64 件は消え、5 秒後の 1 件と新しい 1 件だけが残る。
        assert!(room_audio.may_attempt("late", now + ATTEMPT_LEDGER_TTL));
        assert_eq!(room_audio.attempts.len(), 2);
    }
    pair.listener_leaves().await;

    // 話者: 同時に送る相手は 64 まで。65 番目は busy で断り(access の拒否ではない)、空きができれば受ける。
    let _voice = pair.speaker_speaks().await;
    let speaker_id = pair.speaker_id();
    let access = pair.access_request().await;
    let mut sessions = Vec::new();
    for _ in 0..kukuri_iroh_node::MEDIA_MAX_INBOUND_SESSIONS {
        sessions.push(
            pair.listener_node
                .media_subscribe(&speaker_id, &access)
                .await
                .expect("within the limit"),
        );
    }
    let busy = pair
        .listener_node
        .media_subscribe(&speaker_id, &access)
        .await
        .err()
        .expect("busy over the limit");
    assert!(
        busy.downcast_ref::<kukuri_iroh_node::MediaDenied>()
            .is_none()
    );
    sessions.pop();
    assert!(
        eventually(async || {
            let access = pair.access_request().await;
            pair.listener_node
                .media_subscribe(&speaker_id, &access)
                .await
                .is_ok()
        })
        .await,
        "a freed slot accepts the next listener"
    );
    drop(sessions);
    pair.shutdown().await;
}

#[tokio::test]
async fn metaverse_audio_mute_drops_before_decode() {
    let pair = pair().await;
    let _voice = pair.speaker_speaks().await;
    pair.listen(1).await;
    assert!(eventually(async || written(&pair.listener).await > 0).await);
    // mute した話者は画面が候補から外す。購読を止め、以後の decode・再生は 0(参加・presence は変えない)。
    let status = pair
        .listener
        .set(
            &pair.listener_app,
            Some(pair.listener_node.clone()),
            request(Some(room(&pair.owner, 1)), &[]),
        )
        .await;
    assert_eq!(status.listening, 0);
    assert!(stays_silent(&pair.listener).await);
    pair.shutdown().await;
}
