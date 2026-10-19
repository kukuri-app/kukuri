//! live 配信の映像・音声(ADR 0067 §8・§9、#1034)。
//!
//! 配信者の端末は、画面が VP8 に圧縮した映像の chunk と mic の音声(moq-audio)に session ごとの鍵で frame ごとに
//! 署名し、同じ clock の timestamp で 1 つの broadcast に置く。最初の keyframe が届くまで broadcast を公開しない
//! (取り込みが拒否されたら送信 0)。配信者が直接送るのは 4 人まで、受けた視聴者は確かめた frame を中身を変えずに
//! 2 人まで転送する(`watch`)。視聴者は音声を Rust で再生し、映像の chunk を送りかけ 1 秒分までで画面へ渡す(音声の
//! 再生時刻を添える。`screen`)。音声と映像の bytes は memory にだけ置き、docs・blob・SQLite・log へ出さない。Web は
//! 扱わない(ADR 0060)。

mod screen;
mod watch;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard, Weak};
use std::time::Duration;

use anyhow::{Context, Result};
use iroh::EndpointId;
use kukuri_app_api::AppService;
use kukuri_core::{
    LiveMediaAccessRequestV1, LiveMediaSigner, Pubkey, SpatialContextV1, live_media_broadcast_path,
};
use kukuri_iroh_node::{IrohDocsNode, MediaGrant, media_origin_config};
use moq_mux::catalog::hang::Container;
use moq_net::{Timestamp, origin};
use n0_future::task::AbortOnDropHandle;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::media_access::{
    DENIED_ACCESS, DENIED_ANCESTOR, DENIED_FULL, DENIED_NOT_LIVE, DENIED_UNAVAILABLE, RateBudget,
    ordered_frames,
};
use crate::media_devices as devices;
use screen::{ScreenQueue, encode_screen_batch};

const LIVE_AUDIO_TRACK: &str = "audio";
const LIVE_VIDEO_TRACK: &str = "video";
/// 配信者が直接送る視聴者と、視聴者が転送する相手の上限(ADR 0067 §4・§8)。
const MAX_DIRECT_VIEWERS: usize = 4;
const MAX_FORWARDED_VIEWERS: usize = 2;
/// 参加の問い合わせの上限(ADR 0067 §4): 段数と 1 段の上限。経路はこの段数と配信者の分までを持つ。
const MAX_JOIN_STEPS: usize = 16;
const JOIN_STEP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ROUTE: usize = MAX_JOIN_STEPS + 1;
/// 参加し直す間隔の下限と、親から何も届かなければ親を失ったとみなす時間(ADR 0067 §8)。
const REJOIN_INTERVAL: Duration = Duration::from_secs(5);
const STALL: Duration = Duration::from_secs(5);
/// 視聴の購読の最大遅延(映像。ADR 0067 §4)と、音声の再生前の保持の上限(ADR 0041 client の既定 8 × 20 ms)。
const VIDEO_MAX_AGE: Duration = Duration::from_secs(1);
const AUDIO_PLAYOUT: Duration = Duration::from_millis(160);
/// Opus の 1 frame の長さ(μs)。
const OPUS_FRAME_MICROS: u64 = 20_000;

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

/// 視聴中の live の音量(#1839 AC-4)。0〜1。この端末だけの設定で、共有しない。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LiveVolumeRequest {
    pub session_id: String,
    pub volume: f32,
}

/// 視聴の状態(画面へ映像と一緒に code で渡す)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveWatchState {
    Watching,
    /// 配信者と転送する視聴者のどこにも空きが無い(参加の問い合わせが 16 段で尽きた)。
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

/// この端末で作っていない live session の配信の拒否(manifest の配信者の端末が別。2026-10-13 のユーザー判断)。
#[derive(Debug)]
pub struct LiveBroadcastElsewhere;

impl std::fmt::Display for LiveBroadcastElsewhere {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("this live session is broadcast from the device that created it")
    }
}

impl std::error::Error for LiveBroadcastElsewhere {}

/// runtime が持つ live の配信と視聴。moq-lite の型はブラウザでは Send でない(main thread だけで動かす)ので、Web では
/// 作った thread だけから触れる包みに入れて runtime を Send・Sync に保つ(Web の build は thread を使わない。ADR 0060 §2)。
#[cfg(not(target_family = "wasm"))]
pub(crate) type RuntimeLiveMedia = LiveMedia;
#[cfg(target_family = "wasm")]
pub(crate) type RuntimeLiveMedia = send_wrapper::SendWrapper<LiveMedia>;

pub(crate) fn runtime_live_media() -> RuntimeLiveMedia {
    #[cfg(not(target_family = "wasm"))]
    return LiveMedia::default();
    #[cfg(target_family = "wasm")]
    send_wrapper::SendWrapper::new(LiveMedia::default())
}

/// 端末の live の配信と視聴。
#[derive(Default)]
pub(crate) struct LiveMedia {
    broadcasts: Mutex<HashMap<String, Broadcast>>,
    viewers: Mutex<HashMap<String, Viewer>>,
}

/// 配信中の session。落とすと broadcast を閉じ、mic を止め、視聴者の接続を評価し直しで終える。
struct Broadcast {
    fanout: Arc<Fanout>,
    broadcast: moq_net::broadcast::Producer,
    catalog: moq_mux::catalog::Producer,
    video: moq_mux::container::Producer<Container, hang::catalog::VideoConfig>,
    /// 最後に置いた映像の timestamp(μs)。
    last_video_us: u64,
    source: LiveVideoSource,
    signer: LiveMediaSigner,
    /// 開くのを待つ間も映像を置けるよう、broadcast の一覧と別に lock する。
    microphone: Arc<Mutex<Microphone>>,
}

/// 配信の mic。一度開いた後は、止めても track と catalog を残す(視聴者の購読はそのまま続く)。
#[derive(Default)]
struct Microphone {
    /// 機器と、その音声に署名して配信の track へ写す task。
    capture: Option<(devices::Capture, AbortOnDropHandle<()>)>,
    on: bool,
}

impl Microphone {
    async fn set(
        &mut self,
        enabled: bool,
        broadcast: &mut moq_net::broadcast::Producer,
        catalog: &moq_mux::catalog::Producer,
        signer: LiveMediaSigner,
    ) -> bool {
        if enabled == self.on {
            return self.on;
        }
        self.on = if !enabled {
            if let Some((capture, _)) = &self.capture {
                capture.stop();
            }
            false
        } else {
            let opened = match self.capture.as_mut() {
                Some((capture, _)) => capture.start().await,
                None => open_microphone(broadcast, catalog, signer)
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

/// mic を開く。moq-audio は配信と同じ clock で非公開の broadcast へ書き、その音声に署名して配信の track へ写す。
async fn open_microphone(
    broadcast: &mut moq_net::broadcast::Producer,
    catalog: &moq_mux::catalog::Producer,
    signer: LiveMediaSigner,
) -> Result<(devices::Capture, AbortOnDropHandle<()>)> {
    let mut private = moq_net::broadcast::Info::new().produce();
    let private_catalog = moq_mux::catalog::Producer::new(
        &mut private,
        moq_mux::catalog::Config::default().with_clock(catalog.clock()),
    )?;
    let capture = devices::Capture::open(
        &mut private,
        private_catalog.clone(),
        LIVE_AUDIO_TRACK,
        None,
    )
    .await?;
    let signing = sign_audio(&private, &private_catalog, broadcast, catalog, signer).await?;
    Ok((capture, signing))
}

/// 非公開の broadcast の音声を読み、配信者の署名を付けて配信の音声の track へ写す(ADR 0067 §8)。
async fn sign_audio(
    private: &moq_net::broadcast::Producer,
    private_catalog: &moq_mux::catalog::Producer,
    broadcast: &mut moq_net::broadcast::Producer,
    catalog: &moq_mux::catalog::Producer,
    signer: LiveMediaSigner,
) -> Result<AbortOnDropHandle<()>> {
    let config = private_catalog
        .snapshot()
        .audio
        .renditions
        .get(LIVE_AUDIO_TRACK)
        .cloned()
        .context("the microphone has no audio track")?;
    let mut frames = ordered_frames(
        &private.consume(),
        LIVE_AUDIO_TRACK,
        Container::try_from(&config)?,
        hang::catalog::PRIORITY.audio,
        AUDIO_PLAYOUT,
    )
    .await?;
    let track = broadcast.create_track(
        LIVE_AUDIO_TRACK,
        catalog.track_info(hang::catalog::PRIORITY.audio),
    )?;
    let mut signed = catalog.audio(track, Container::try_from(&config)?, config)?;
    Ok(AbortOnDropHandle::new(n0_future::task::spawn(async move {
        while let Ok(Some(mut frame)) = frames.read().await {
            frame.payload = signer
                .sign(LIVE_AUDIO_TRACK, micros(frame.timestamp), &frame.payload)
                .into();
            let _ = signed.write(frame);
        }
    })))
}

impl Broadcast {
    /// 公開前の broadcast。直接の視聴者への経路は配信者(`endpoint_id`)だけ。
    fn new(
        topic_id: String,
        session_id: &str,
        source: LiveVideoSource,
        endpoint_id: String,
        signer: LiveMediaSigner,
    ) -> Result<Self> {
        let fanout = Fanout::new(topic_id, MAX_DIRECT_VIEWERS, vec![endpoint_id]);
        let mut broadcast = fanout
            .origin
            .create_broadcast(live_media_broadcast_path(session_id))?;
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
            fanout,
            broadcast,
            catalog,
            video,
            last_video_us: 0,
            source,
            signer,
            microphone: Arc::default(),
        })
    }
}

/// 子へ映像・音声を公開する口(配信者の端末と、転送する視聴者。ADR 0067 §8)。落とすと、子の接続は評価し直しで閉じる。
struct Fanout {
    topic_id: String,
    origin: origin::Producer,
    /// 直接の子の上限(配信者 4、転送する視聴者 2)。
    limit: usize,
    /// 子へ公開しているか(配信者は最初の keyframe から。転送する視聴者は親から受けている間)。
    announced: AtomicBool,
    links: StdMutex<FanoutLinks>,
    _driver: AbortOnDropHandle<()>,
}

/// 配信者からこの端末までの経路(この端末を含む)と直接の子(endpoint id)。参加の応答に添える(許可は経路だけ)。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct FanoutLinks {
    route: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children: Vec<String>,
}

impl Fanout {
    #[cfg_attr(
        target_family = "wasm",
        expect(
            clippy::arc_with_non_send_sync,
            reason = "Web の moq-lite の型は main thread だけで使う"
        )
    )]
    fn new(topic_id: String, limit: usize, route: Vec<String>) -> Arc<Self> {
        let (origin, driver) = origin::Producer::new(media_origin_config());
        let driver = n0_future::task::spawn(async move {
            moq_net::time::run(driver).await;
        });
        Arc::new(Self {
            topic_id,
            origin,
            limit,
            announced: AtomicBool::new(false),
            links: StdMutex::new(FanoutLinks {
                route,
                children: Vec::new(),
            }),
            _driver: AbortOnDropHandle::new(driver),
        })
    }

    fn links(&self) -> MutexGuard<'_, FanoutLinks> {
        self.links.lock().expect("fanout links poisoned")
    }

    /// 経路を付け替える(転送する視聴者が親を変えた)。最も近い祖先から `MAX_ROUTE` 件までを持つ。
    fn set_route(&self, mut route: Vec<String>) {
        route.drain(..route.len().saturating_sub(MAX_ROUTE));
        self.links().route = route;
    }

    /// 子の枠を取る。満員なら、満員の応答に添える子の一覧と経路。
    fn admit(self: &Arc<Self>, child: String) -> std::result::Result<ChildSlot, String> {
        let mut links = self.links();
        if links.children.len() >= self.limit {
            return Err(serde_json::to_string(&*links).unwrap_or_default());
        }
        links.children.push(child.clone());
        Ok(ChildSlot {
            fanout: Arc::downgrade(self),
            child,
        })
    }
}

/// 子 1 人分の枠。落とすと空く(接続が終われば grant と一緒に落ちる)。
struct ChildSlot {
    fanout: Weak<Fanout>,
    child: String,
}

impl Drop for ChildSlot {
    fn drop(&mut self) {
        if let Some(fanout) = self.fanout.upgrade() {
            let mut links = fanout.links();
            if let Some(at) = links.children.iter().position(|child| *child == self.child) {
                links.children.swap_remove(at);
            }
        }
    }
}

/// 視聴中の session。落とすと購読・再生・転送を止める。
struct Viewer {
    screen: Arc<ScreenQueue>,
    /// 音量(f32 の bits)。視聴の task が音声の出力へ渡す。
    gain: Arc<AtomicU32>,
    /// 転送の口(視聴の task が持ち、task が終われば子の接続は評価し直しで閉じる)。
    fanout: Weak<Fanout>,
    _task: AbortOnDropHandle<()>,
}

/// 視聴の task が使うもの。
struct Watch {
    app: Arc<AppService>,
    node: Arc<IrohDocsNode>,
    topic_id: String,
    session_id: String,
    screen: Arc<ScreenQueue>,
    fanout: Arc<Fanout>,
    gain: Arc<AtomicU32>,
}

impl LiveMedia {
    /// 配信を始める、または配信中の source と mic を切り替える(`endpoint_id` はこの端末)。broadcast は最初の keyframe
    /// で公開する。
    pub(crate) async fn set_broadcast(
        &self,
        app: &Arc<AppService>,
        endpoint_id: Option<String>,
        request: SetLiveBroadcastRequest,
    ) -> Result<LiveBroadcastStatus> {
        let (_, _, media_endpoint_id) = app
            .live_media_scope(&request.topic, &request.session_id)
            .await?
            .context("only the owner of a live session can broadcast it")?;
        // 視聴者は manifest の端末へ購読し、manifest の鍵で署名を確かめるので、この端末で作った session だけを配信する。
        let Some(endpoint_id) = endpoint_id.filter(|id| media_endpoint_id.as_ref() == Some(id))
        else {
            return Err(LiveBroadcastElsewhere.into());
        };
        let limits = request.source.limits();
        let (microphone, mut broadcast, catalog, signer) = {
            let mut broadcasts = self.broadcasts.lock().await;
            let current = match broadcasts.entry(request.session_id) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let signer = app.live_media_signer(entry.key())?;
                    let broadcast = Broadcast::new(
                        request.topic,
                        entry.key(),
                        request.source,
                        endpoint_id,
                        signer,
                    )?;
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
                current.signer.clone(),
            )
        };
        let microphone = microphone
            .lock()
            .await
            .set(request.microphone, &mut broadcast, &catalog, signer)
            .await;
        Ok(LiveBroadcastStatus { microphone, limits })
    }

    /// 画面が圧縮した映像の chunk に署名して置く。`age` は取り込みから今までの時間(timestamp を取り込み時に合わせる)。
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
            payload: current
                .signer
                .sign(LIVE_VIDEO_TRACK, timestamp_us, &data)
                .into(),
            keyframe,
        });
        // 最初の keyframe の前の frame は置けない(途中参加と同じく keyframe から始める)。
        if written.is_err() && !keyframe {
            return Ok(());
        }
        written?;
        current.last_video_us = timestamp_us;
        if !current.fanout.announced.load(Ordering::Acquire) {
            current.broadcast.announce(origin::Route::default())?;
            current.fanout.announced.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// 配信を止める(broadcast を閉じ、mic を止める)。
    pub(crate) async fn stop_broadcast(&self, session_id: &str) {
        self.broadcasts.lock().await.remove(session_id);
    }

    /// 受け口が呼ぶ(配信者の端末と、転送する視聴者)。子へ公開している session への要求だけを、要求した端末が祖先で
    /// ないこと・proof・access で確かめて許す。満員なら子の一覧と経路を添えて断る(ADR 0067 §8)。
    #[cfg_attr(
        target_family = "wasm",
        expect(
            clippy::arc_with_non_send_sync,
            reason = "Web の moq-lite の型は main thread だけで使う"
        )
    )]
    pub(crate) async fn grant(
        &self,
        app: &Arc<AppService>,
        remote: EndpointId,
        request: LiveMediaAccessRequestV1,
    ) -> std::result::Result<MediaGrant, String> {
        let fanout = self
            .fanout(&request.topic_id, &request.session_id)
            .await
            .ok_or_else(|| DENIED_NOT_LIVE.to_string())?;
        let remote = remote.to_string();
        if fanout.links().route.contains(&remote) {
            return Err(DENIED_ANCESTOR.into());
        }
        let (_, _, context, owner) = app
            .live_media_source(&request.topic_id, &request.session_id)
            .await
            .map_err(|_| DENIED_UNAVAILABLE.to_string())?
            .ok_or_else(|| DENIED_NOT_LIVE.to_string())?;
        let viewer = app
            .authorize_media_listener(&request.proof, &context, &owner)
            .await
            .map_err(|_| DENIED_UNAVAILABLE.to_string())?
            .ok_or_else(|| DENIED_ACCESS.to_string())?;
        // 子の一覧は資格を確かめた相手にだけ返す。
        let slot = fanout
            .admit(remote)
            .map_err(|links| format!("{DENIED_FULL}\n{links}"))?;
        let detail = serde_json::to_string(&FanoutLinks {
            route: fanout.links().route.clone(),
            children: Vec::new(),
        })
        .unwrap_or_default();
        let publish = fanout.origin.consume();
        let fanout = Arc::downgrade(&fanout);
        let app = Arc::downgrade(app);
        Ok(MediaGrant {
            publish,
            detail,
            revalidate: Arc::new(move || {
                // 枠は認可の間だけ持つ(接続が終われば grant と一緒に落ちて空く)。
                let _slot = &slot;
                Box::pin(still_allowed(
                    fanout.clone(),
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

    /// 子へ公開している session の口(配信中なら配信者の、視聴中なら転送の)。
    async fn fanout(&self, topic_id: &str, session_id: &str) -> Option<Arc<Fanout>> {
        let broadcast = self
            .broadcasts
            .lock()
            .await
            .get(session_id)
            .map(|current| Arc::clone(&current.fanout));
        let fanout = match broadcast {
            Some(fanout) => Some(fanout),
            None => self
                .viewers
                .lock()
                .await
                .get(session_id)
                .and_then(|viewer| viewer.fanout.upgrade()),
        };
        fanout.filter(|fanout| {
            fanout.topic_id == topic_id && fanout.announced.load(Ordering::Acquire)
        })
    }

    /// 視聴を始める(同じ session の前の視聴は止めて始め直す)。状態は `next_video` で画面へ渡す。
    pub(crate) async fn watch(
        &self,
        app: &Arc<AppService>,
        node: Option<Arc<IrohDocsNode>>,
        request: LiveMediaRequest,
    ) -> Result<()> {
        let node = node.context("the iroh stack is not running")?;
        let mut viewers = self.viewers.lock().await;
        let screen = Arc::new(ScreenQueue::default());
        let gain = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let fanout = Fanout::new(request.topic.clone(), MAX_FORWARDED_VIEWERS, Vec::new());
        let viewer_fanout = Arc::downgrade(&fanout);
        let task = n0_future::task::spawn(watch::watch_session(Watch {
            app: Arc::clone(app),
            node,
            topic_id: request.topic,
            session_id: request.session_id.clone(),
            screen: Arc::clone(&screen),
            fanout,
            gain: Arc::clone(&gain),
        }));
        viewers.insert(
            request.session_id,
            Viewer {
                screen,
                gain,
                fanout: viewer_fanout,
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

    /// 視聴中の session の音量を変える(0〜1)。視聴していなければ何もしない。
    pub(crate) async fn set_volume(&self, session_id: &str, volume: f32) {
        if let Some(viewer) = self.viewers.lock().await.get(session_id) {
            viewer
                .gain
                .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// 視聴を止める(購読・再生・転送を止める)。
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

/// 受ける側の映像の上限(ADR 0067 §4): catalog の source(画面は 15 fps)の frame 数と bitrate のそれぞれ 1.25 倍を 1 秒分。
/// 配信者の間引きは取り込みの間隔の 0.8 倍まで通すので、frame 数も 1.25 倍(切り上げ)まで受ける。
fn video_budget(config: &hang::catalog::VideoConfig) -> RateBudget {
    let screen = LiveVideoSource::Screen.limits();
    let limits = if config.framerate == Some(f64::from(screen.framerate)) {
        screen
    } else {
        LiveVideoSource::Camera.limits()
    };
    RateBudget::new(
        (u64::from(limits.framerate) * 5).div_ceil(4),
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

/// 子の資格を 5 秒ごとに確かめる: 口がまだあり(配信中・視聴中)、session が終わっておらず、視聴者の資格がある。
async fn still_allowed(
    fanout: Weak<Fanout>,
    app: Weak<AppService>,
    topic_id: String,
    session_id: String,
    context: SpatialContextV1,
    owner: Pubkey,
    viewer: Pubkey,
) -> bool {
    let (Some(_fanout), Some(app)) = (fanout.upgrade(), app.upgrade()) else {
        return false;
    };
    matches!(
        app.live_media_source(&topic_id, &session_id).await,
        Ok(Some(_))
    ) && app
        .media_listener_allowed(&context, &owner, &viewer)
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
