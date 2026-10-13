//! live 配信の映像・音声(ADR 0067 §8・§9、#1034 AC-2a)。
//!
//! 配信者の端末は、画面が VP8 に圧縮した映像の chunk と mic の音声(moq-audio)を、同じ clock の timestamp で
//! 1 つの broadcast に置く。最初の keyframe が届くまで broadcast を公開しない(取り込みが拒否されたら送信 0)。
//! 直接の視聴者は 4 人まで(超えたら満員)。視聴者の端末は配信者の端末へ購読し、音声を Rust で再生し、映像の
//! chunk を送りかけ 1 秒分までで画面へ渡す(音声の再生時刻を添える)。音声と映像の bytes は memory にだけ置き、
//! docs・blob・SQLite・log へ出さない。Web は扱わない(ADR 0060)。

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use kukuri_app_api::AppService;
use kukuri_core::{
    LiveMediaAccessRequestV1, MediaAccessRequestV1, Pubkey, SpatialContextV1,
    live_media_broadcast_path,
};
use kukuri_iroh_node::{IrohDocsNode, MediaGrant, media_origin_config};
use moq_mux::catalog::hang::{Catalog, Container};
use moq_net::bytes::Bytes;
use moq_net::{Timestamp, origin};
use n0_future::task::AbortOnDropHandle;
use n0_future::time::{Instant, timeout};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};

use crate::media_access::{
    DENIED_ACCESS, DENIED_FULL, DENIED_NOT_LIVE, DENIED_UNAVAILABLE, RateBudget, SUBSCRIBE_TIMEOUT,
    denial_code, opus_frames, ordered_frames, read_catalog,
};
use crate::media_devices as devices;

const LIVE_AUDIO_TRACK: &str = "audio";
const LIVE_VIDEO_TRACK: &str = "video";
/// 配信者が直接送る視聴者の上限(ADR 0067 §4・§8)。
const MAX_DIRECT_VIEWERS: usize = 4;
/// 画面へ送りかけの映像の上限(1 秒分。ADR 0067 §4)。
const SCREEN_QUEUE_CHUNKS: usize = 30;
/// 視聴の購読の最大遅延(映像。ADR 0067 §4)と、音声の再生前の保持の上限(ADR 0041 client の既定 8 × 20 ms)。
const VIDEO_MAX_AGE: Duration = Duration::from_secs(1);
const AUDIO_PLAYOUT: Duration = Duration::from_millis(160);
/// Opus の 1 frame の長さ(μs)。
const OPUS_FRAME_MICROS: u64 = 20_000;
/// 画面が次の映像を待つ上限(届かなければ空を返し、画面はまた待つ)。
const SCREEN_WAIT: Duration = Duration::from_secs(1);

/// 配信する映像(ADR 0067 D7。1 つの配信で同時に送るのはどちらか 1 つ)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum LiveVideoSource {
    Camera,
    Screen,
}

/// 映像の取り込み・圧縮の上限(ADR 0067 D8)。画面はこの値で取り込み・encode する。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LiveVideoLimits {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate: u32,
}

impl LiveVideoSource {
    pub fn limits(self) -> LiveVideoLimits {
        match self {
            Self::Camera => LiveVideoLimits {
                width: 1280,
                height: 720,
                framerate: 30,
                bitrate: 1_500_000,
            },
            Self::Screen => LiveVideoLimits {
                width: 1920,
                height: 1080,
                framerate: 15,
                bitrate: 2_500_000,
            },
        }
    }
}

/// 配信を始める・切り替える(画面が camera・画面の取り込みを始める前と、source・mic を変えたときに呼ぶ)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SetLiveBroadcastRequest {
    pub topic: String,
    pub session_id: String,
    pub source: LiveVideoSource,
    pub microphone: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LiveBroadcastStatus {
    /// mic が開いているか(求めても開けなければ false で、映像だけを送る)。
    pub microphone: bool,
    pub limits: LiveVideoLimits,
}

/// 視聴の開始・停止、配信の停止の対象。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LiveMediaRequest {
    pub topic: String,
    pub session_id: String,
}

/// 視聴の状態(画面へ映像と一緒に code で渡す)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveWatchState {
    Watching,
    /// 配信者が直接送る視聴者が上限に達している(視聴者どうしの転送は #1034 AC-2b)。
    Full,
    /// media の無い session、または配信者がまだ映像を送っていない。
    NoMedia,
    /// 終わった・止まった・届かなくなった。
    Ended,
}

impl LiveWatchState {
    fn code(self) -> u8 {
        match self {
            Self::Watching => 0,
            Self::Full => 1,
            Self::NoMedia => 2,
            Self::Ended => 3,
        }
    }
}

/// 端末の live の配信と視聴。
#[derive(Default)]
pub(crate) struct LiveMedia {
    broadcasts: Mutex<HashMap<String, Broadcast>>,
    viewers: Mutex<HashMap<String, Viewer>>,
}

/// 配信中の session。落とすと broadcast を閉じ、mic を止め、視聴者の接続を終える。
struct Broadcast {
    topic_id: String,
    origin: origin::Producer,
    broadcast: moq_net::broadcast::Producer,
    announced: bool,
    catalog: moq_mux::catalog::Producer,
    video: moq_mux::container::Producer<Container, hang::catalog::VideoConfig>,
    /// 最後に置いた映像の timestamp(μs)。
    last_video_us: u64,
    source: LiveVideoSource,
    viewers: Arc<AtomicUsize>,
    /// 開くのを待つ間も映像を置けるよう、broadcast の一覧と別に lock する。
    microphone: Arc<Mutex<Microphone>>,
    _origin_driver: AbortOnDropHandle<()>,
}

/// 配信の mic。一度開いた後は、止めても track と catalog を残す(視聴者の購読はそのまま続く)。
#[derive(Default)]
struct Microphone {
    capture: Option<devices::Capture>,
    on: bool,
}

impl Microphone {
    async fn set(
        &mut self,
        enabled: bool,
        broadcast: &mut moq_net::broadcast::Producer,
        catalog: moq_mux::catalog::Producer,
    ) -> bool {
        if enabled == self.on {
            return self.on;
        }
        self.on = if !enabled {
            if let Some(capture) = &self.capture {
                capture.stop();
            }
            false
        } else {
            let opened = match self.capture.as_mut() {
                Some(capture) => capture.start().await,
                None => devices::Capture::open(broadcast, catalog, LIVE_AUDIO_TRACK, None)
                    .await
                    .map(|capture| self.capture = Some(capture)),
            };
            if let Err(error) = &opened {
                tracing::warn!(%error, "live microphone could not be opened");
            }
            opened.is_ok()
        };
        self.on
    }
}

impl Broadcast {
    /// 公開前の broadcast。
    fn new(topic_id: String, session_id: &str, source: LiveVideoSource) -> Result<Self> {
        let (origin, driver) = origin::Producer::new(media_origin_config());
        let origin_driver = n0_future::task::spawn(async move {
            moq_net::time::run(driver).await;
        });
        let mut broadcast = origin.create_broadcast(live_media_broadcast_path(session_id))?;
        let catalog =
            moq_mux::catalog::Producer::new(&mut broadcast, moq_mux::catalog::Config::default())?;
        let track = broadcast.create_track(
            LIVE_VIDEO_TRACK,
            catalog.track_info(hang::catalog::PRIORITY.video),
        )?;
        let video = catalog.video(
            track,
            Container::Legacy(moq_mux::container::Kind::Video),
            video_config(source.limits()),
        )?;
        Ok(Self {
            topic_id,
            origin,
            broadcast,
            announced: false,
            catalog,
            video,
            last_video_us: 0,
            source,
            viewers: Arc::default(),
            microphone: Arc::default(),
            _origin_driver: AbortOnDropHandle::new(origin_driver),
        })
    }
}

/// 直接の視聴者 1 人分の枠。落とすと空く。
struct ViewerSlot(Arc<AtomicUsize>);

impl ViewerSlot {
    fn take(viewers: &Arc<AtomicUsize>) -> Option<Self> {
        viewers
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_DIRECT_VIEWERS).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(viewers)))
    }
}

impl Drop for ViewerSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 視聴中の session。落とすと購読と再生を止める。
struct Viewer {
    screen: Arc<ScreenQueue>,
    _task: AbortOnDropHandle<()>,
}

/// 画面へ送りかけの映像(1 秒分まで。超えたら次の keyframe まで捨てる)と、視聴の状態・音声の再生時刻。
#[derive(Default)]
struct ScreenQueue {
    state: StdMutex<ScreenState>,
    ready: Notify,
}

#[derive(Default)]
struct ScreenState {
    chunks: VecDeque<ScreenChunk>,
    waiting_keyframe: bool,
    ended: Option<LiveWatchState>,
    /// 最後に再生へ渡した音声の、聞こえる位置の timestamp(μs)と、渡した時刻。音声がまだ無ければ None。
    audio_clock: Option<(u64, Instant)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScreenChunk {
    timestamp_us: u64,
    keyframe: bool,
    data: Bytes,
}

impl ScreenQueue {
    fn push(&self, chunk: ScreenChunk) {
        let mut state = self.state.lock().expect("screen queue poisoned");
        if chunk.keyframe {
            if state.waiting_keyframe || state.chunks.len() >= SCREEN_QUEUE_CHUNKS {
                state.chunks.clear();
            }
            state.waiting_keyframe = false;
        } else if state.waiting_keyframe {
            return;
        } else if state.chunks.len() >= SCREEN_QUEUE_CHUNKS {
            state.waiting_keyframe = true;
            return;
        }
        state.chunks.push_back(chunk);
        drop(state);
        self.ready.notify_one();
    }

    fn set_audio_clock(&self, timestamp_us: u64) {
        self.state
            .lock()
            .expect("screen queue poisoned")
            .audio_clock = Some((timestamp_us, Instant::now()));
    }

    fn end(&self, state: LiveWatchState) {
        self.state
            .lock()
            .expect("screen queue poisoned")
            .ended
            .get_or_insert(state);
        self.ready.notify_one();
    }

    /// 送りかけの映像を全部取る。無ければ `SCREEN_WAIT` まで待つ。音声の再生時刻は、最後に渡した位置から経過時間で
    /// 進める(mic が止まっても映像を止めない)。音声がまだ無ければ 0。
    async fn take(&self) -> (LiveWatchState, u64, Vec<ScreenChunk>) {
        let deadline = Instant::now() + SCREEN_WAIT;
        loop {
            let ready = self.ready.notified();
            {
                let mut state = self.state.lock().expect("screen queue poisoned");
                if !state.chunks.is_empty() || state.ended.is_some() || Instant::now() >= deadline {
                    let chunks = state.chunks.drain(..).collect();
                    let watch = state.ended.unwrap_or(LiveWatchState::Watching);
                    let clock = state.audio_clock.map_or(0, |(timestamp_us, at)| {
                        timestamp_us.saturating_add(micros_of(at.elapsed()))
                    });
                    return (watch, clock, chunks);
                }
            }
            let _ = timeout(deadline.saturating_duration_since(Instant::now()), ready).await;
        }
    }
}

/// 画面へ渡す形: state(1)・音声の再生時刻(8)・chunk の数(2)、chunk ごとに timestamp(8)・keyframe(1)・長さ(4)・bytes。
/// 数値は little endian。
fn encode_screen_batch(
    watch: LiveWatchState,
    audio_clock_us: u64,
    chunks: &[ScreenChunk],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        11 + chunks
            .iter()
            .map(|chunk| 13 + chunk.data.len())
            .sum::<usize>(),
    );
    out.push(watch.code());
    out.extend_from_slice(&audio_clock_us.to_le_bytes());
    out.extend_from_slice(&(chunks.len() as u16).to_le_bytes());
    for chunk in chunks {
        out.extend_from_slice(&chunk.timestamp_us.to_le_bytes());
        out.push(u8::from(chunk.keyframe));
        out.extend_from_slice(&(chunk.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&chunk.data);
    }
    out
}

impl LiveMedia {
    /// 配信を始める、または配信中の source と mic を切り替える。broadcast は最初の keyframe で公開する。
    pub(crate) async fn set_broadcast(
        &self,
        app: &Arc<AppService>,
        request: SetLiveBroadcastRequest,
    ) -> Result<LiveBroadcastStatus> {
        app.live_media_scope(&request.topic, &request.session_id)
            .await?
            .context("only the owner of a live session can broadcast it")?;
        let limits = request.source.limits();
        let (microphone, mut broadcast, catalog) = {
            let mut broadcasts = self.broadcasts.lock().await;
            let current = match broadcasts.entry(request.session_id) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let broadcast = Broadcast::new(request.topic, entry.key(), request.source)?;
                    entry.insert(broadcast)
                }
            };
            if current.source != request.source {
                current.video.set(video_config(limits))?;
                current.source = request.source;
            }
            (
                Arc::clone(&current.microphone),
                current.broadcast.clone(),
                current.catalog.clone(),
            )
        };
        let microphone = microphone
            .lock()
            .await
            .set(request.microphone, &mut broadcast, catalog)
            .await;
        Ok(LiveBroadcastStatus { microphone, limits })
    }

    /// 画面が圧縮した映像の chunk を置く。`age` は取り込みから今までの時間(timestamp を取り込み時に合わせる)。
    pub(crate) async fn push_video(
        &self,
        session_id: &str,
        keyframe: bool,
        age: Duration,
        data: Vec<u8>,
    ) -> Result<()> {
        let mut broadcasts = self.broadcasts.lock().await;
        let current = broadcasts
            .get_mut(session_id)
            .context("the live session is not being broadcast")?;
        // IPC の遅れの揺れで前の frame より前にならないようにする(巻き戻ると視聴者の track が止まる)。
        let timestamp_us = micros(current.catalog.clock().now())
            .saturating_sub(micros_of(age))
            .max(current.last_video_us.saturating_add(1));
        let written = current.video.write(moq_mux::container::Frame {
            timestamp: Timestamp::from_micros(timestamp_us)?,
            duration: None,
            payload: data.into(),
            keyframe,
        });
        // 最初の keyframe の前の frame は置けない(途中参加と同じく keyframe から始める)。
        if written.is_err() && !keyframe {
            return Ok(());
        }
        written?;
        current.last_video_us = timestamp_us;
        if !current.announced {
            current.broadcast.announce(origin::Route::default())?;
            current.announced = true;
        }
        Ok(())
    }

    /// 配信を止める(broadcast を閉じ、mic を止める)。
    pub(crate) async fn stop_broadcast(&self, session_id: &str) {
        self.broadcasts.lock().await.remove(session_id);
    }

    /// 配信者の端末の受け口が呼ぶ。配信中の自分の session への要求だけを、proof と access で確かめて許す。
    pub(crate) async fn grant(
        self: &Arc<Self>,
        app: &Arc<AppService>,
        request: LiveMediaAccessRequestV1,
    ) -> std::result::Result<MediaGrant, String> {
        let (publish, viewers) = {
            let broadcasts = self.broadcasts.lock().await;
            let Some(current) = broadcasts
                .get(&request.session_id)
                .filter(|current| current.topic_id == request.topic_id && current.announced)
            else {
                return Err(DENIED_NOT_LIVE.into());
            };
            (current.origin.consume(), Arc::clone(&current.viewers))
        };
        // 満員なら docs を読む前に断る(待っている視聴者の試し直しを軽くする)。枠は認可の後に取る。
        if viewers.load(Ordering::Acquire) >= MAX_DIRECT_VIEWERS {
            return Err(DENIED_FULL.into());
        }
        let (context, owner) = app
            .live_media_scope(&request.topic_id, &request.session_id)
            .await
            .map_err(|_| DENIED_UNAVAILABLE.to_string())?
            .ok_or_else(|| DENIED_NOT_LIVE.to_string())?;
        let viewer = app
            .authorize_media_listener(&request.proof, &context, &owner)
            .await
            .map_err(|_| DENIED_UNAVAILABLE.to_string())?
            .ok_or_else(|| DENIED_ACCESS.to_string())?;
        let slot = ViewerSlot::take(&viewers).ok_or_else(|| DENIED_FULL.to_string())?;
        let live = Arc::downgrade(self);
        let app = Arc::downgrade(app);
        Ok(MediaGrant {
            publish,
            revalidate: Arc::new(move || {
                // 枠は認可の間だけ持つ(接続が終われば grant と一緒に落ちて空く)。
                let _slot = &slot;
                Box::pin(still_allowed(
                    live.clone(),
                    app.clone(),
                    request.topic_id.clone(),
                    request.session_id.clone(),
                    context.clone(),
                    owner.clone(),
                    viewer.clone(),
                ))
            }),
        })
    }

    /// 視聴を始める(同じ session の前の視聴は止めて始め直す。満員・映像の無い間は画面が 5 秒ごとに呼び直す)。状態は
    /// `next_video` で画面へ渡す。
    pub(crate) async fn watch(
        &self,
        app: &Arc<AppService>,
        node: Option<Arc<IrohDocsNode>>,
        request: LiveMediaRequest,
    ) -> Result<()> {
        let node = node.context("the iroh stack is not running")?;
        let mut viewers = self.viewers.lock().await;
        let screen = Arc::new(ScreenQueue::default());
        let task = n0_future::task::spawn(watch_session(
            Arc::clone(app),
            node,
            request.topic,
            request.session_id.clone(),
            Arc::clone(&screen),
        ));
        viewers.insert(
            request.session_id,
            Viewer {
                screen,
                _task: AbortOnDropHandle::new(task),
            },
        );
        Ok(())
    }

    /// 画面が呼ぶ。送りかけの映像を取り、`encode_screen_batch` の形で返す。視聴していなければ終わった状態。
    pub(crate) async fn next_video(&self, session_id: &str) -> Vec<u8> {
        let screen = self
            .viewers
            .lock()
            .await
            .get(session_id)
            .map(|viewer| Arc::clone(&viewer.screen));
        match screen {
            Some(screen) => {
                let (watch, audio_clock_us, chunks) = screen.take().await;
                encode_screen_batch(watch, audio_clock_us, &chunks)
            }
            None => encode_screen_batch(LiveWatchState::Ended, 0, &[]),
        }
    }

    /// 視聴を止める(購読と再生を止める)。
    pub(crate) async fn stop_watching(&self, session_id: &str) {
        self.viewers.lock().await.remove(session_id);
    }
}

fn micros(timestamp: Timestamp) -> u64 {
    u64::try_from(timestamp.as_micros()).unwrap_or(u64::MAX)
}

fn micros_of(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// 受ける側の映像の上限(ADR 0067 §4): catalog の source(画面は 15 fps)の frame 数と bitrate の 1.25 倍を 1 秒分。
fn video_budget(config: &hang::catalog::VideoConfig) -> RateBudget {
    let screen = LiveVideoSource::Screen.limits();
    let limits = if config.framerate == Some(f64::from(screen.framerate)) {
        screen
    } else {
        LiveVideoSource::Camera.limits()
    };
    RateBudget::new(
        u64::from(limits.framerate),
        u64::from(limits.bitrate) * 5 / 4 / 8,
    )
}

fn video_config(limits: LiveVideoLimits) -> hang::catalog::VideoConfig {
    let mut config = hang::catalog::VideoConfig::new(hang::catalog::VideoCodec::VP8);
    config.coded_width = Some(limits.width);
    config.coded_height = Some(limits.height);
    config.framerate = Some(f64::from(limits.framerate));
    config.bitrate = Some(u64::from(limits.bitrate));
    config.optimize_for_latency = Some(true);
    config
}

/// 配信者の端末が 5 秒ごとに確かめる: まだ配信中で、session が終わっておらず、視聴者の資格がある。
async fn still_allowed(
    live: Weak<LiveMedia>,
    app: Weak<AppService>,
    topic_id: String,
    session_id: String,
    context: SpatialContextV1,
    owner: Pubkey,
    viewer: Pubkey,
) -> bool {
    let (Some(live), Some(app)) = (live.upgrade(), app.upgrade()) else {
        return false;
    };
    if !live.broadcasts.lock().await.contains_key(&session_id) {
        return false;
    }
    matches!(
        app.live_media_scope(&topic_id, &session_id).await,
        Ok(Some(_))
    ) && app
        .media_listener_allowed(&context, &owner, &viewer)
        .await
        .unwrap_or(false)
}

/// 1 つの session を視聴し、終わったら画面へ状態を渡す。
async fn watch_session(
    app: Arc<AppService>,
    node: Arc<IrohDocsNode>,
    topic_id: String,
    session_id: String,
    screen: Arc<ScreenQueue>,
) {
    let ended = match play(&app, &node, &topic_id, &session_id, &screen).await {
        Ok(state) => state,
        Err(error) => match denial_code(&error) {
            Some(DENIED_FULL) => LiveWatchState::Full,
            Some(code) => {
                tracing::debug!(code, "live media subscription denied");
                LiveWatchState::Ended
            }
            None => {
                tracing::debug!("live media subscription ended");
                LiveWatchState::Ended
            }
        },
    };
    screen.end(ended);
}

async fn play(
    app: &AppService,
    node: &IrohDocsNode,
    topic_id: &str,
    session_id: &str,
    screen: &ScreenQueue,
) -> Result<LiveWatchState> {
    let Some((endpoint_id, context, owner)) = app.live_media_source(topic_id, session_id).await?
    else {
        return Ok(LiveWatchState::NoMedia);
    };
    let proof = app.build_dome_access_proof(context, owner).await?;
    let request = serde_json::to_vec(&MediaAccessRequestV1::Live(LiveMediaAccessRequestV1 {
        proof,
        topic_id: topic_id.to_string(),
        session_id: session_id.to_string(),
    }))?;
    let subscription = match node.media_subscribe(&endpoint_id, &request).await {
        Ok(subscription) => subscription,
        Err(error) if denial_code(&error) == Some(DENIED_NOT_LIVE) => {
            return Ok(LiveWatchState::NoMedia);
        }
        Err(error) => return Err(error),
    };
    let broadcast = timeout(
        SUBSCRIBE_TIMEOUT,
        subscription.broadcast(&live_media_broadcast_path(session_id)),
    )
    .await
    .context("the broadcaster did not publish in time")??;
    let (catalog, mut catalogs) = read_catalog(&broadcast).await?;
    let video_config = catalog
        .video
        .renditions
        .get(LIVE_VIDEO_TRACK)
        .context("the broadcast has no video track")?;
    if video_config.codec != hang::catalog::VideoCodec::VP8 {
        bail!("the broadcast video is not VP8");
    }
    let container = Container::try_from(video_config)
        .map_err(|error| anyhow!("unsupported video container: {error}"))?;
    // 途中参加は最新の group から始め(遅れの許容 0 で購読し、前の group を送らせない)、届き始めてから 1 秒を許す。
    let mut video = ordered_frames(
        &broadcast,
        LIVE_VIDEO_TRACK,
        container,
        hang::catalog::PRIORITY.video,
        Duration::ZERO,
    )
    .await?;
    let mut joined = false;
    let mut audio = open_audio(&broadcast, &catalog).await?;
    let audio_budget = &app.metaverse_resource_budget().player;
    let mut audio_admission = RateBudget::new(
        u64::from(audio_budget.max_audio_frames_per_second),
        audio_budget.max_audio_bytes_per_second,
    );
    let mut video_admission = video_budget(video_config);
    loop {
        // 次に届いたもの(購読の future を落としてから扱う)。
        let next = {
            let audio_frame = async {
                match audio.as_mut() {
                    Some((frames, _, _)) => frames.read().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                frame = video.read() => Next::Video(frame?),
                frame = audio_frame => Next::Audio(frame?),
                catalog = moq_mux::catalog::Stream::next(&mut catalogs) => Next::Catalog(catalog?.map(Box::new)),
                _ = subscription.closed() => Next::Closed,
            }
        };
        match next {
            Next::Video(None) | Next::Catalog(None) | Next::Closed => {
                return Ok(LiveWatchState::Ended);
            }
            // 配信の途中で source が替わったら上限を合わせ、mic が初めて開いたら音声の track を足す。
            Next::Catalog(Some(catalog)) => {
                if let Some(config) = catalog.video.renditions.get(LIVE_VIDEO_TRACK) {
                    video_admission = video_budget(config);
                }
                if audio.is_none() {
                    audio = open_audio(&broadcast, &catalog).await?;
                }
            }
            Next::Video(Some(frame)) => {
                if !joined {
                    video.set_max_age(VIDEO_MAX_AGE);
                    joined = true;
                }
                if video_admission
                    .admit(Instant::now(), frame.payload.len())
                    .is_ok()
                {
                    screen.push(ScreenChunk {
                        timestamp_us: micros(frame.timestamp),
                        keyframe: frame.keyframe,
                        data: frame.payload,
                    });
                }
            }
            Next::Audio(None) => audio = None,
            Next::Audio(Some(frame)) => {
                if audio_admission
                    .admit(Instant::now(), frame.payload.len())
                    .is_err()
                {
                    continue;
                }
                let (_, decoder, (_, output)) = audio.as_mut().expect("audio is open");
                let decoded = decoder.decode(&frame.payload)?;
                output.write(&decoded.samples, AUDIO_PLAYOUT)?;
                let heard = micros(frame.timestamp).saturating_add(OPUS_FRAME_MICROS);
                screen.set_audio_clock(heard.saturating_sub(micros_of(output.buffered())));
            }
        }
    }
}

enum Next {
    Video(Option<moq_mux::container::Frame>),
    Audio(Option<moq_mux::container::Frame>),
    Catalog(Option<Box<Catalog>>),
    Closed,
}

/// catalog に音声の track があれば、購読して再生の出力を開く。
async fn open_audio(
    broadcast: &moq_net::broadcast::Consumer,
    catalog: &Catalog,
) -> Result<
    Option<(
        moq_mux::container::Consumer<Container>,
        moq_audio::decode::Decoder,
        (devices::Playback, devices::Output),
    )>,
> {
    if !catalog.audio.renditions.contains_key(LIVE_AUDIO_TRACK) {
        return Ok(None);
    }
    let (frames, decoder) =
        opus_frames(broadcast, catalog, LIVE_AUDIO_TRACK, AUDIO_PLAYOUT).await?;
    let playback = devices::Playback::open().await?;
    let output = playback.output()?;
    Ok(Some((frames, decoder, (playback, output))))
}

#[cfg(test)]
mod tests;
