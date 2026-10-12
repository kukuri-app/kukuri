//! metaverse の空間音声(ADR 0067、#1033)。
//!
//! 話者は自分の音声を moq-lite の broadcast(Opus の track と位置の track)として公開し、聞き手の認可要求を
//! 送る側で確かめる(iroh-node の media の受け口)。聞き手は画面が近い順に渡した話者のうち、client budget の
//! 同時 stream 数(既定 16)まで購読する。受けた frame は送信量の上限で確かめてから decode し、既存の距離減衰の
//! 音量で再生する。音声の bytes は memory にだけ置き、docs・blob・SQLite・log へ出さない。Web は扱わない(ADR 0060)。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use kukuri_app_api::AppService;
use kukuri_core::{
    METAVERSE_AUDIO_POSITION_TRACK, METAVERSE_AUDIO_TRACK, MetaverseAudioAccessRequestV1,
    MetaverseBudgetResource, MetaverseBudgetScope, MetaverseResourceRejection,
    MetaverseResourceRejectionReason, Pubkey, SpatialContextV1,
    connection_opening_audio_distance_cm, metaverse_audio_broadcast_path,
    spatial_audio_distance_cm, spatial_audio_gain_milli,
};
use kukuri_iroh_node::{IrohDocsNode, MediaAuthorizer, MediaGrant};
use moq_net::{Timestamp, origin, track};
use n0_future::task::AbortOnDropHandle;
use n0_future::time::{Instant, timeout};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

mod devices;

/// Opus の 1 frame の長さ。再生前の保持の上限は client budget の jitter frame の数を掛ける(既定 8 で 160 ms。
/// ADR 0041、ADR 0067 §4)。
const OPUS_FRAME: Duration = Duration::from_millis(20);
/// 同じ話者への接続の試行の間隔と、試行の記録の上限・期限(ADR 0067 §4)。
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const ATTEMPT_LEDGER_LIMIT: usize = 64;
const ATTEMPT_LEDGER_TTL: Duration = Duration::from_secs(30);
/// 自分の位置を位置の track へ書く間隔の下限(10 Hz)。
const POSITION_INTERVAL: Duration = Duration::from_millis(100);
/// 公開を待つ・catalog を読む上限。
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Opus の設定(声。ADR 0067 §9)。
const OPUS_SAMPLE_RATE: u32 = 48_000;
/// 認可の拒否の code(理由だけを返し、他の参加者の識別子を返さない。ADR 0067 §3)。
const DENIED_ACCESS: &str = "access_denied";
const DENIED_NOT_IN_DOME: &str = "not_in_dome";
const DENIED_INVALID: &str = "invalid_request";
const DENIED_UNAVAILABLE: &str = "unavailable";

/// 画面が渡す Dome(current または ready な隣接)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MetaverseAudioRoom {
    pub spatial_context: SpatialContextV1,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub owner_pubkey: Pubkey,
    pub instance_id: String,
    pub instance_generation: u64,
    pub session_id: String,
}

/// ready な隣接 Dome と、合成距離に使う opening の位置(cm)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MetaverseAudioNeighbor {
    pub room: MetaverseAudioRoom,
    pub speaker_opening: [i64; 3],
    pub listener_opening: [i64; 3],
}

/// 音声を送っている話者(presence の endpoint id)。画面が近い順に並べる。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MetaverseAudioSpeaker {
    pub endpoint_id: String,
    pub instance_id: String,
}

/// 空間音声の状態。`room` が `None` なら、取り込み・送信・購読・再生をすべて止める。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SetMetaverseAudioRequest {
    pub room: Option<MetaverseAudioRoom>,
    pub microphone_enabled: bool,
    pub listener_position: [i64; 3],
    pub neighbors: Vec<MetaverseAudioNeighbor>,
    pub speakers: Vec<MetaverseAudioSpeaker>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MetaverseMicrophoneStatus {
    Off,
    On,
    /// 機器を開けない(OS の許可を含む)。送信は 0 で、mic は off のまま。
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MetaverseAudioStatus {
    pub microphone: MetaverseMicrophoneStatus,
    /// 購読中の話者の数。
    pub listening: u32,
    /// 送信量の上限を超えて捨てた frame の数(この端末が起動してから)。
    pub rejected_frames: u64,
}

impl MetaverseAudioRoom {
    fn same_session(&self, other: &Self) -> bool {
        self.instance_id == other.instance_id
            && self.instance_generation == other.instance_generation
            && self.session_id == other.session_id
    }
}

/// 聞き手の位置と、話者の Dome ごとの opening(gain の計算に使う)。
#[derive(Default)]
struct Listener {
    position: [i64; 3],
    current_instance: String,
    neighbors: HashMap<String, ([i64; 3], [i64; 3])>,
}

impl Listener {
    /// 話者の位置からの gain(0〜1)。current は直線距離、ready な隣接は opening の合成距離。それ以外は 0。
    fn gain(&self, instance_id: &str, speaker: [i64; 3]) -> f32 {
        let distance = if instance_id == self.current_instance {
            spatial_audio_distance_cm(speaker, self.position)
        } else if let Some((speaker_opening, listener_opening)) = self.neighbors.get(instance_id) {
            connection_opening_audio_distance_cm(
                speaker,
                *speaker_opening,
                *listener_opening,
                self.position,
            )
        } else {
            return 0.0;
        };
        f32::from(spatial_audio_gain_milli(distance)) / 1_000.0
    }
}

/// 送信量の検査(ADR 0041 player の 50 frame/s・32 KiB/s)。1 秒分の枠を経過時間に比例して補充するので、正規の
/// 20 ms の frame の到着の揺れとまとめ到着(1 秒分まで)は捨てず、上限を超え続ける分だけを捨てる。
struct FrameBudget {
    at: Option<Instant>,
    frames: f64,
    bytes: f64,
    max_frames: u64,
    max_bytes: u64,
}

impl FrameBudget {
    fn new(max_frames: u64, max_bytes: u64) -> Self {
        Self {
            at: None,
            frames: max_frames as f64,
            bytes: max_bytes as f64,
            max_frames,
            max_bytes,
        }
    }

    fn admit(
        &mut self,
        now: Instant,
        bytes: usize,
    ) -> std::result::Result<(), MetaverseResourceRejection> {
        let elapsed = self
            .at
            .map_or(0.0, |at| now.saturating_duration_since(at).as_secs_f64());
        self.at = Some(now);
        self.frames = (self.frames + elapsed * self.max_frames as f64).min(self.max_frames as f64);
        self.bytes = (self.bytes + elapsed * self.max_bytes as f64).min(self.max_bytes as f64);
        // observed は上限を超えたことだけを示す(1 秒分の枠を使い切った)。
        let rejection = |resource, limit: u64| {
            MetaverseResourceRejection::new(
                MetaverseBudgetScope::Player,
                resource,
                MetaverseResourceRejectionReason::RateExceeded,
                limit.saturating_add(1),
                limit,
            )
        };
        if self.frames < 1.0 {
            return Err(rejection(
                MetaverseBudgetResource::AudioFrameRate,
                self.max_frames,
            ));
        }
        if self.bytes < bytes as f64 {
            return Err(rejection(
                MetaverseBudgetResource::AudioBandwidth,
                self.max_bytes,
            ));
        }
        self.frames -= 1.0;
        self.bytes -= bytes as f64;
        Ok(())
    }
}

/// 購読中の話者。落とすと購読の task を止め、接続を閉じる。
struct Speaker {
    instance_id: String,
    position: Arc<StdMutex<Option<[i64; 3]>>>,
    volume: devices::Volume,
    finished: Arc<AtomicBool>,
    _task: AbortOnDropHandle<()>,
}

/// 自分の音声の公開(mic が on の間)。落とすと取り込みを止め、broadcast を閉じる。
struct Microphone {
    _capture: devices::Capture,
    position: track::Producer,
    started: Instant,
    last_position: Option<([i64; 3], Instant)>,
}

/// 参加中の Dome の音声。落とすと公開・購読・再生をすべて止める。
struct RoomAudio {
    room: MetaverseAudioRoom,
    origin: origin::Producer,
    _origin_driver: AbortOnDropHandle<()>,
    listener: Arc<StdMutex<Listener>>,
    microphone: Option<Microphone>,
    speakers: HashMap<String, Speaker>,
    attempts: VecDeque<(String, Instant)>,
    playback: Option<devices::Playback>,
}

impl RoomAudio {
    fn new(room: MetaverseAudioRoom) -> Self {
        let (origin, driver) = origin::Producer::new(origin::Config::default());
        let driver = n0_future::task::spawn(async move {
            moq_net::time::run(driver).await;
        });
        Self {
            room,
            origin,
            _origin_driver: AbortOnDropHandle::new(driver),
            listener: Arc::default(),
            microphone: None,
            speakers: HashMap::new(),
            attempts: VecDeque::new(),
            playback: None,
        }
    }

    fn broadcast_path(&self) -> String {
        metaverse_audio_broadcast_path(&self.room.instance_id, &self.room.session_id)
    }

    async fn playback(&mut self) -> Result<&devices::Playback> {
        if self.playback.is_none() {
            self.playback = Some(devices::Playback::open().await?);
        }
        self.playback.as_ref().context("playback is not open")
    }

    /// 同じ話者へ 5 秒以内に試していなければ記録して真。記録は 64 件・30 秒まで(古いものから捨てる)。
    fn may_attempt(&mut self, endpoint_id: &str, now: Instant) -> bool {
        self.attempts
            .retain(|(_, at)| now.saturating_duration_since(*at) < ATTEMPT_LEDGER_TTL);
        if self.attempts.iter().any(|(peer, at)| {
            peer == endpoint_id && now.saturating_duration_since(*at) < RETRY_INTERVAL
        }) {
            return false;
        }
        self.attempts.push_back((endpoint_id.to_string(), now));
        while self.attempts.len() > ATTEMPT_LEDGER_LIMIT {
            self.attempts.pop_front();
        }
        true
    }

    fn apply_gains(&self) {
        let listener = self.listener.lock().expect("audio listener poisoned");
        for speaker in self.speakers.values() {
            let gain = speaker
                .position
                .lock()
                .expect("speaker position poisoned")
                .map_or(0.0, |position| {
                    listener.gain(&speaker.instance_id, position)
                });
            speaker.volume.set(gain);
        }
    }
}

/// 端末の空間音声。room の切替・mic・購読を 1 つの lock で直列にする。
#[derive(Default)]
pub(crate) struct MetaverseAudio {
    state: Mutex<Option<RoomAudio>>,
    rejected_frames: Arc<AtomicU64>,
}

impl MetaverseAudio {
    /// 画面の状態を反映する。room が変われば前の room の音声をすべて止めてから始める。
    pub(crate) async fn set(
        &self,
        app: &Arc<AppService>,
        node: Option<Arc<IrohDocsNode>>,
        request: SetMetaverseAudioRequest,
    ) -> MetaverseAudioStatus {
        let mut state = self.state.lock().await;
        let Some(room) = request.room.clone() else {
            *state = None;
            return self.status(None);
        };
        if state.as_ref().is_none_or(|current| current.room != room) {
            // 前の room の公開・購読・再生を止めてから(落とすと閉じる)、新しい room を始める。
            *state = None;
            *state = Some(RoomAudio::new(room));
        }
        let audio = state.as_mut().expect("room audio");
        let microphone = self.apply_microphone(audio, &request).await;
        {
            let mut listener = audio.listener.lock().expect("audio listener poisoned");
            listener.position = request.listener_position;
            listener.current_instance = audio.room.instance_id.clone();
            listener.neighbors = request
                .neighbors
                .iter()
                .map(|neighbor| {
                    (
                        neighbor.room.instance_id.clone(),
                        (neighbor.speaker_opening, neighbor.listener_opening),
                    )
                })
                .collect();
        }
        if let Some(node) = node {
            self.apply_speakers(app, &node, audio, &request).await;
        }
        audio.apply_gains();
        let mut status = self.status(Some(audio));
        if microphone == MetaverseMicrophoneStatus::Unavailable {
            status.microphone = microphone;
        }
        status
    }

    fn status(&self, audio: Option<&RoomAudio>) -> MetaverseAudioStatus {
        MetaverseAudioStatus {
            microphone: if audio.is_some_and(|audio| audio.microphone.is_some()) {
                MetaverseMicrophoneStatus::On
            } else {
                MetaverseMicrophoneStatus::Off
            },
            listening: audio.map_or(0, |audio| audio.speakers.len() as u32),
            rejected_frames: self.rejected_frames.load(Ordering::Relaxed),
        }
    }

    /// mic を利用者の操作に合わせる。機器を開けなければ公開しない(送信 0)。
    async fn apply_microphone(
        &self,
        audio: &mut RoomAudio,
        request: &SetMetaverseAudioRequest,
    ) -> MetaverseMicrophoneStatus {
        if !request.microphone_enabled {
            audio.microphone = None;
            return MetaverseMicrophoneStatus::Off;
        }
        if audio.microphone.is_none() {
            match self.open_microphone(audio).await {
                Ok(microphone) => audio.microphone = Some(microphone),
                Err(error) => {
                    tracing::warn!(%error, "microphone could not be opened");
                    return MetaverseMicrophoneStatus::Unavailable;
                }
            }
        }
        if let Some(microphone) = audio.microphone.as_mut() {
            let now = Instant::now();
            let changed = microphone.last_position.is_none_or(|(position, at)| {
                position != request.listener_position
                    && now.saturating_duration_since(at) >= POSITION_INTERVAL
            });
            if changed {
                let payload = serde_json::to_vec(&request.listener_position).unwrap_or_default();
                let elapsed = now
                    .saturating_duration_since(microphone.started)
                    .as_micros();
                let timestamp = Timestamp::from_micros(u64::try_from(elapsed).unwrap_or(u64::MAX))
                    .unwrap_or(Timestamp::ZERO);
                if microphone.position.write_frame(timestamp, payload).is_ok() {
                    microphone.last_position = Some((request.listener_position, now));
                }
            }
        }
        MetaverseMicrophoneStatus::On
    }

    async fn open_microphone(&self, audio: &mut RoomAudio) -> Result<Microphone> {
        let path = audio.broadcast_path();
        let mut broadcast = audio
            .origin
            .publish(path.as_str(), origin::Route::default())
            .context("failed to publish the speaker broadcast")?;
        let position = broadcast
            .create_track(METAVERSE_AUDIO_POSITION_TRACK, None)
            .context("failed to create the position track")?;
        let playback = audio.playback().await.ok();
        let capture = devices::Capture::open(&mut broadcast, playback).await?;
        Ok(Microphone {
            _capture: capture,
            position,
            started: Instant::now(),
            last_position: None,
        })
    }

    /// 画面の順に、上限まで購読する。順位の外・終わった購読は止める。
    async fn apply_speakers(
        &self,
        app: &Arc<AppService>,
        node: &Arc<IrohDocsNode>,
        audio: &mut RoomAudio,
        request: &SetMetaverseAudioRequest,
    ) {
        let limit = app
            .metaverse_resource_budget()
            .client
            .max_concurrent_audio_streams as usize;
        let rooms: HashMap<&str, &MetaverseAudioRoom> = std::iter::once(&audio.room)
            .chain(request.neighbors.iter().map(|neighbor| &neighbor.room))
            .map(|room| (room.instance_id.as_str(), room))
            .collect();
        let mut wanted: Vec<(&MetaverseAudioSpeaker, MetaverseAudioRoom)> = Vec::new();
        for speaker in &request.speakers {
            if wanted.len() >= limit {
                break;
            }
            let Some(room) = rooms.get(speaker.instance_id.as_str()) else {
                continue;
            };
            if wanted
                .iter()
                .any(|(existing, _)| existing.endpoint_id == speaker.endpoint_id)
            {
                continue;
            }
            wanted.push((speaker, (*room).clone()));
        }
        audio.speakers.retain(|endpoint_id, speaker| {
            !speaker.finished.load(Ordering::Acquire)
                && wanted.iter().any(|(wanted, _)| {
                    &wanted.endpoint_id == endpoint_id && wanted.instance_id == speaker.instance_id
                })
        });
        let now = Instant::now();
        for (speaker, room) in wanted {
            if audio.speakers.contains_key(&speaker.endpoint_id)
                || !audio.may_attempt(&speaker.endpoint_id, now)
            {
                continue;
            }
            let output = match audio.playback().await.and_then(devices::Playback::output) {
                Ok(output) => output,
                Err(error) => {
                    tracing::warn!(%error, "audio output could not be opened");
                    return;
                }
            };
            let position = Arc::new(StdMutex::new(None));
            let finished = Arc::new(AtomicBool::new(false));
            let volume = output.volume();
            let task = n0_future::task::spawn(listen(
                Arc::clone(app),
                Arc::clone(node),
                speaker.endpoint_id.clone(),
                room,
                output,
                Arc::clone(&audio.listener),
                Arc::clone(&position),
                Arc::clone(&finished),
                Arc::clone(&self.rejected_frames),
            ));
            audio.speakers.insert(
                speaker.endpoint_id.clone(),
                Speaker {
                    instance_id: speaker.instance_id.clone(),
                    position,
                    volume,
                    finished,
                    _task: AbortOnDropHandle::new(task),
                },
            );
        }
    }

    /// 話者の端末の受け口が呼ぶ。参加中の Dome への要求だけを、proof と access で確かめて許す。
    async fn grant(
        self: &Arc<Self>,
        app: &Arc<AppService>,
        request: MetaverseAudioAccessRequestV1,
    ) -> std::result::Result<MediaGrant, String> {
        let (room, publish) = {
            let state = self.state.lock().await;
            let Some(audio) = state.as_ref().filter(|audio| {
                audio.room.instance_id == request.instance_id
                    && audio.room.instance_generation == request.instance_generation
                    && audio.room.session_id == request.session_id
            }) else {
                return Err(DENIED_NOT_IN_DOME.into());
            };
            (audio.room.clone(), audio.origin.consume())
        };
        let listener = app
            .authorize_spatial_audio_listener(
                &request.proof,
                &room.spatial_context,
                &room.owner_pubkey,
            )
            .await
            .map_err(|_| DENIED_UNAVAILABLE.to_string())?
            .ok_or_else(|| DENIED_ACCESS.to_string())?;
        let audio = Arc::downgrade(self);
        let app = Arc::downgrade(app);
        Ok(MediaGrant {
            publish,
            revalidate: Arc::new(move || {
                let audio = audio.clone();
                let app = app.clone();
                let room = room.clone();
                let listener = listener.clone();
                Box::pin(async move {
                    let (Some(audio), Some(app)) = (audio.upgrade(), app.upgrade()) else {
                        return false;
                    };
                    let still_here = audio
                        .state
                        .lock()
                        .await
                        .as_ref()
                        .is_some_and(|current| current.room.same_session(&room));
                    still_here
                        && app
                            .spatial_audio_listener_allowed(
                                &room.spatial_context,
                                &room.owner_pubkey,
                                &listener,
                            )
                            .await
                            .unwrap_or(false)
                })
            }),
        })
    }
}

/// iroh-node の media の受け口へ付ける認可。runtime を所有しない(止まった後の要求は拒否する)。
pub(crate) fn media_authorizer(
    audio: Weak<MetaverseAudio>,
    app: Weak<AppService>,
) -> MediaAuthorizer {
    Arc::new(move |_remote, request| {
        let audio = audio.clone();
        let app = app.clone();
        Box::pin(async move {
            let (Some(audio), Some(app)) = (audio.upgrade(), app.upgrade()) else {
                return Err(DENIED_UNAVAILABLE.to_string());
            };
            let request: MetaverseAudioAccessRequestV1 =
                serde_json::from_slice(&request).map_err(|_| DENIED_INVALID.to_string())?;
            audio.grant(&app, request).await
        })
    })
}

/// 1 人の話者を購読し、終わるまで再生する(拒否・切断・公開の終了で終わる)。
#[allow(clippy::too_many_arguments)]
async fn listen(
    app: Arc<AppService>,
    node: Arc<IrohDocsNode>,
    endpoint_id: String,
    room: MetaverseAudioRoom,
    output: devices::Output,
    listener: Arc<StdMutex<Listener>>,
    position: Arc<StdMutex<Option<[i64; 3]>>>,
    finished: Arc<AtomicBool>,
    rejected: Arc<AtomicU64>,
) {
    if let Err(error) = play(
        &app,
        &node,
        &endpoint_id,
        &room,
        output,
        &listener,
        &position,
        &rejected,
    )
    .await
    {
        // 話者の識別子・音声を log に出さない(ADR 0067 §2)。拒否は固定の code だけを出す。
        match error.downcast_ref::<kukuri_iroh_node::MediaDenied>() {
            Some(denied) => {
                let code = [
                    DENIED_ACCESS,
                    DENIED_NOT_IN_DOME,
                    DENIED_INVALID,
                    DENIED_UNAVAILABLE,
                ]
                .into_iter()
                .find(|known| *known == denied.0)
                .unwrap_or("unknown");
                tracing::debug!(code, "spatial audio subscription denied");
            }
            None => tracing::debug!("spatial audio subscription ended"),
        }
    }
    finished.store(true, Ordering::Release);
}

#[allow(clippy::too_many_arguments)]
async fn play(
    app: &AppService,
    node: &IrohDocsNode,
    endpoint_id: &str,
    room: &MetaverseAudioRoom,
    mut output: devices::Output,
    listener: &StdMutex<Listener>,
    position: &StdMutex<Option<[i64; 3]>>,
    rejected: &AtomicU64,
) -> Result<()> {
    let proof = app
        .build_dome_access_proof(room.spatial_context.clone(), room.owner_pubkey.clone())
        .await?;
    let request = serde_json::to_vec(&MetaverseAudioAccessRequestV1 {
        proof,
        instance_id: room.instance_id.clone(),
        instance_generation: room.instance_generation,
        session_id: room.session_id.clone(),
    })?;
    let subscription = node.media_subscribe(endpoint_id, &request).await?;
    let broadcast = timeout(
        SUBSCRIBE_TIMEOUT,
        subscription.broadcast(&metaverse_audio_broadcast_path(
            &room.instance_id,
            &room.session_id,
        )),
    )
    .await
    .context("the speaker did not publish in time")??;
    let mut catalog =
        moq_mux::catalog::Consumer::<()>::new(&broadcast, moq_mux::catalog::CatalogFormat::Hang)
            .await?;
    let catalog = timeout(
        SUBSCRIBE_TIMEOUT,
        moq_mux::catalog::Stream::next(&mut catalog),
    )
    .await
    .context("the speaker catalog did not arrive in time")??
    .context("the speaker catalog ended")?;
    let config = catalog
        .audio
        .renditions
        .get(METAVERSE_AUDIO_TRACK)
        .context("the speaker publishes no audio track")?
        .clone();
    if config.codec != hang::catalog::AudioCodec::Opus
        || config.sample_rate != OPUS_SAMPLE_RATE
        || config.channel_count != 1
    {
        bail!("the speaker audio track is not mono 48 kHz Opus");
    }
    let container = moq_mux::catalog::hang::Container::try_from(&config)
        .map_err(|error| anyhow!("unsupported audio container: {error}"))?;
    let budget = app.metaverse_resource_budget();
    let playout = OPUS_FRAME * budget.client.max_audio_jitter_frames;
    let mut frames = audio_frames(&broadcast, container, playout).await?;
    let mut decoder = moq_audio::decode::Decoder::new(&config, &moq_audio::decode::Config::new())?;
    let mut positions = broadcast
        .track(METAVERSE_AUDIO_POSITION_TRACK)?
        .subscribe(track::Subscription::default())
        .await?;
    let mut admission = FrameBudget::new(
        u64::from(budget.player.max_audio_frames_per_second),
        budget.player.max_audio_bytes_per_second,
    );
    let volume = output.volume();
    loop {
        tokio::select! {
            frame = frames.read() => {
                let Some(frame) = frame? else { return Ok(()) };
                if let Err(rejection) = admission.admit(Instant::now(), frame.payload.len()) {
                    rejected.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(code = %rejection.code(), "spatial audio frame rejected");
                    continue;
                }
                let decoded = decoder.decode(&frame.payload)?;
                output.write(&decoded.samples, playout)?;
            }
            group = positions.recv_group() => {
                let Some(mut group) = group? else { continue };
                if let Ok(Some(frame)) = group.read_frame().await
                    && let Ok(speaker) = serde_json::from_slice::<[i64; 3]>(&frame.payload)
                {
                    *position.lock().expect("speaker position poisoned") = Some(speaker);
                    let gain = listener
                        .lock()
                        .expect("audio listener poisoned")
                        .gain(&room.instance_id, speaker);
                    volume.set(gain);
                }
            }
            _ = subscription.closed() => return Ok(()),
        }
    }
}

/// 話者の音声の track を、group の順を並べ直して時刻順の frame として読む。`playout`(再生前の保持の上限)より
/// 遅れた group は古いものから飛ばす。
async fn audio_frames(
    broadcast: &moq_net::broadcast::Consumer,
    container: moq_mux::catalog::hang::Container,
    playout: Duration,
) -> Result<moq_mux::container::Consumer<moq_mux::catalog::hang::Container>> {
    let track = broadcast
        .track(METAVERSE_AUDIO_TRACK)?
        .subscribe(
            track::Subscription::default()
                .with_priority(hang::catalog::PRIORITY.audio)
                .with_max_age(playout),
        )
        .await?;
    Ok(moq_mux::container::Consumer::new(track, container))
}

#[cfg(test)]
mod tests;
