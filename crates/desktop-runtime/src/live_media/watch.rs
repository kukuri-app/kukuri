//! 視聴(ADR 0067 §8): 配信者から子の一覧をたどって空きのある親へ参加し、配信者の署名を確かめた frame を画面・音声へ
//! 渡し、中身を変えずに子へ転送する。親を失ったら配信者から参加し直す(5 秒に 1 回まで。自分の子は付いたまま移る)。
//! 入力が途絶えかけたら子への公開を取り下げ、子は取り下げの間は親が戻るのを待つ(上流の 1 人が抜けても、下流が
//! そろって配信者へ参加し直さない)。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use kukuri_core::{
    LiveMediaAccessRequestV1, LiveMediaVerifier, MediaAccessRequestV1, Pubkey, SpatialContextV1,
    live_media_broadcast_path,
};
use kukuri_iroh_node::MediaSubscription;
use moq_mux::catalog::hang::{Catalog, Container};
use moq_net::origin;
use n0_future::time::{Instant, sleep_until, timeout};

use super::screen::ScreenChunk;
use super::{
    AUDIO_PLAYOUT, Fanout, FanoutLinks, JOIN_STEP_TIMEOUT, LIVE_AUDIO_TRACK, LIVE_VIDEO_TRACK,
    LiveWatchState, MAX_JOIN_STEPS, OPUS_FRAME_MICROS, REJOIN_INTERVAL, STALL, VIDEO_MAX_AGE,
    Watch, micros, micros_of, video_budget,
};
use crate::media_access::{
    DENIED_FULL, DENIED_NOT_LIVE, RateBudget, denial, denial_code, opus_frames, ordered_frames,
    read_catalog,
};
use crate::media_devices as devices;

/// 親から何も届かないとき、親を失ったとみなす(`STALL`)前に子への公開を取り下げる時間。
const RELAY_HOLD: Duration = Duration::from_millis(2_500);
/// 親が公開を取り下げてから戻るのを待つ上限(親が親を失ったとみなし、参加し直し終えるまでの上限)。
const PARENT_RECOVERY: Duration = Duration::from_secs(
    STALL.as_secs()
        + REJOIN_INTERVAL.as_secs()
        + MAX_JOIN_STEPS as u64 * JOIN_STEP_TIMEOUT.as_secs(),
);

/// 1 つの session を視聴し、続けられなくなったら画面へ状態を渡す。
pub(super) async fn watch_session(cx: Watch) {
    let Ok(mut relay) = Relay::new(Arc::clone(&cx.fanout), &cx.session_id) else {
        return cx.screen.end(LiveWatchState::Ended);
    };
    let mut joined_at: Option<Instant> = None;
    let ended = loop {
        if let Some(at) = joined_at {
            sleep_until(at + REJOIN_INTERVAL).await;
        }
        joined_at = Some(Instant::now());
        match attend(&cx, &mut relay).await {
            // 親を失った: 子への公開を取り下げ(子は付いたまま待つ)、配信者から参加し直す。
            Ok(None) => relay.unannounce(),
            Ok(Some(state)) => break state,
            Err(error) => {
                match denial_code(&error) {
                    Some(code) => tracing::debug!(code, "live media subscription denied"),
                    None => tracing::debug!("live media subscription ended"),
                }
                break LiveWatchState::Ended;
            }
        }
    };
    cx.screen.end(ended);
}

/// 配信者から参加し、親を失うまで受ける。親を失ったら `None`、配信者に断られたらその状態。
async fn attend(cx: &Watch, relay: &mut Relay) -> Result<Option<LiveWatchState>> {
    let Some((broadcaster, verifier, context, owner)) = cx
        .app
        .live_media_source(&cx.topic_id, &cx.session_id)
        .await?
    else {
        return Ok(Some(LiveWatchState::NoMedia));
    };
    let (subscription, route) = match join(cx, &broadcaster, &context, &owner).await? {
        Ok(parent) => parent,
        Err(state) => return Ok(Some(state)),
    };
    cx.fanout.set_route(route);
    let path = live_media_broadcast_path(&cx.session_id);
    loop {
        // 親が公開を取り下げている間(親も参加し直している)は、戻るのを待つ。
        let broadcast = tokio::select! {
            broadcast = timeout(PARENT_RECOVERY, subscription.broadcast(&path)) => match broadcast {
                Ok(Ok(broadcast)) => broadcast,
                _ => return Ok(None),
            },
            _ = subscription.closed() => return Ok(None),
        };
        match receive(cx, &subscription, &broadcast, &verifier, relay).await {
            Ok(Parent::Unannounced) => relay.unannounce(),
            Ok(Parent::Lost) | Err(_) => return Ok(None),
        }
    }
}

/// 配信者から空きのある親を探して購読する(満員なら子の一覧から無作為に。16 段まで、1 段 5 秒)。親と、配信者から
/// この端末までの経路を返す。配信者が映像を送っていなければ `NoMedia`、空きが見つからなければ `Full`。
async fn join(
    cx: &Watch,
    broadcaster: &str,
    context: &SpatialContextV1,
    owner: &Pubkey,
) -> Result<std::result::Result<(MediaSubscription, Vec<String>), LiveWatchState>> {
    let me = cx.node.endpoint().id().to_string();
    let mut candidates = vec![broadcaster.to_string()];
    for _ in 0..MAX_JOIN_STEPS {
        if candidates.is_empty() {
            break;
        }
        let candidate = candidates.swap_remove(random_index(candidates.len()));
        let from_broadcaster = candidate == broadcaster;
        // proof は 10 秒で切れるので、段ごとに作る。
        let request = serde_json::to_vec(&MediaAccessRequestV1::Live(LiveMediaAccessRequestV1 {
            proof: cx
                .app
                .build_dome_access_proof(context.clone(), owner.clone())
                .await?,
            topic_id: cx.topic_id.clone(),
            session_id: cx.session_id.clone(),
        }))?;
        let answer = timeout(
            JOIN_STEP_TIMEOUT,
            cx.node.media_subscribe(&candidate, &request),
        )
        .await
        .unwrap_or_else(|_| Err(anyhow!("the media peer did not answer in time")));
        match answer {
            // 自分の子孫(経路に自分を含む)には付かない。
            Ok(subscription) => {
                let mut route = serde_json::from_str::<FanoutLinks>(subscription.detail())
                    .unwrap_or_default()
                    .route;
                if !route.contains(&me) {
                    route.push(me);
                    return Ok(Ok((subscription, route)));
                }
            }
            Err(error) => match denial(&error) {
                Some((DENIED_FULL, detail)) => {
                    let links = serde_json::from_str::<FanoutLinks>(detail).unwrap_or_default();
                    if !links.route.contains(&me) {
                        candidates = links
                            .children
                            .into_iter()
                            .filter(|child| *child != me)
                            .collect();
                    }
                }
                Some((DENIED_NOT_LIVE, _)) if from_broadcaster => {
                    return Ok(Err(LiveWatchState::NoMedia));
                }
                // 配信者に断られた・届かない。
                _ if from_broadcaster => return Err(error),
                // 転送する視聴者が受けられない・届かない: 同じ一覧の別の候補へ。
                _ => {}
            },
        }
    }
    Ok(Err(LiveWatchState::Full))
}

fn random_index(len: usize) -> usize {
    (uuid::Uuid::new_v4().as_u128() % len as u128) as usize
}

/// 親が公開を取り下げたか、親を失ったか。
enum Parent {
    Unannounced,
    Lost,
}

/// 次に届いたもの。
enum Next {
    Video(Option<moq_mux::container::Frame>),
    Audio(Option<moq_mux::container::Frame>),
    Catalog(Option<Box<Catalog>>),
    Unannounced,
    /// 入力が途絶えかけている(子への公開を取り下げる)。
    Hold,
    Lost,
}

/// 親の broadcast を受け、配信者の署名を確かめた frame を画面・音声と子へ渡す。親が取り下げるか、親を失うまで。
async fn receive(
    cx: &Watch,
    subscription: &MediaSubscription,
    broadcast: &moq_net::broadcast::Consumer,
    verifier: &LiveMediaVerifier,
    relay: &mut Relay,
) -> Result<Parent> {
    let (catalog, mut catalogs) = read_catalog(broadcast).await?;
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
    relay.set_catalog(&catalog)?;
    // 途中参加は最新の group から始め(遅れの許容 0 で購読し、前の group を送らせない)、届き始めてから 1 秒を許す。
    let mut video = ordered_frames(
        broadcast,
        LIVE_VIDEO_TRACK,
        container,
        hang::catalog::PRIORITY.video,
        Duration::ZERO,
    )
    .await?;
    let mut joined = false;
    let mut audio = open_audio(broadcast, &catalog).await?;
    let audio_budget = &cx.app.metaverse_resource_budget().player;
    let mut audio_admission = RateBudget::new(
        u64::from(audio_budget.max_audio_frames_per_second),
        audio_budget.max_audio_bytes_per_second,
    );
    let mut video_admission = video_budget(video_config);
    let mut heard_at = Instant::now();
    loop {
        // 次に届いたもの(購読の future を落としてから扱う)。
        let next = {
            let audio_frame = async {
                match audio.as_mut() {
                    Some((frames, _, _)) => frames.read().await,
                    None => std::future::pending().await,
                }
            };
            let relaying = relay.announced();
            tokio::select! {
                frame = video.read() => Next::Video(frame?),
                frame = audio_frame => Next::Audio(frame?),
                catalog = moq_mux::catalog::Stream::next(&mut catalogs) => Next::Catalog(catalog?.map(Box::new)),
                _ = broadcast.closed() => Next::Unannounced,
                _ = subscription.closed() => Next::Lost,
                // 入力が途絶えたら、先に子への公開を取り下げ(子は待つ)、5 秒で親を失ったとみなす。
                _ = sleep_until(heard_at + RELAY_HOLD), if relaying => Next::Hold,
                _ = sleep_until(heard_at + STALL) => Next::Lost,
            }
        };
        match next {
            Next::Lost => return Ok(Parent::Lost),
            Next::Unannounced | Next::Video(None) | Next::Catalog(None) => {
                return Ok(Parent::Unannounced);
            }
            Next::Hold => relay.unannounce(),
            // 配信の途中で source が替わったら上限を合わせ、mic が初めて開いたら音声の track を足す。
            Next::Catalog(Some(catalog)) => {
                if let Some(config) = catalog.video.renditions.get(LIVE_VIDEO_TRACK) {
                    video_admission = video_budget(config);
                }
                relay.set_catalog(&catalog)?;
                if audio.is_none() {
                    audio = open_audio(broadcast, &catalog).await?;
                }
            }
            // 配信者の署名が無い・合わない frame は捨てて、その親から離れる。
            Next::Video(Some(frame)) => {
                let timestamp_us = micros(frame.timestamp);
                let Some(payload) = verifier.verify(LIVE_VIDEO_TRACK, timestamp_us, &frame.payload)
                else {
                    return Ok(Parent::Lost);
                };
                let data = frame.payload.slice_ref(payload);
                heard_at = Instant::now();
                if !joined {
                    video.set_max_age(VIDEO_MAX_AGE);
                    joined = true;
                }
                if video_admission.admit(heard_at, frame.payload.len()).is_ok() {
                    cx.screen.push(ScreenChunk {
                        timestamp_us,
                        keyframe: frame.keyframe,
                        data,
                    });
                    relay.write(false, frame);
                }
            }
            Next::Audio(None) => audio = None,
            Next::Audio(Some(frame)) => {
                let timestamp_us = micros(frame.timestamp);
                let Some(payload) = verifier.verify(LIVE_AUDIO_TRACK, timestamp_us, &frame.payload)
                else {
                    return Ok(Parent::Lost);
                };
                heard_at = Instant::now();
                if audio_admission
                    .admit(heard_at, frame.payload.len())
                    .is_err()
                {
                    continue;
                }
                let (_, decoder, (_, output)) = audio.as_mut().expect("audio is open");
                output
                    .volume()
                    .set(f32::from_bits(cx.gain.load(Ordering::Relaxed)));
                let decoded = decoder.decode(payload)?;
                output.write(&decoded.samples, AUDIO_PLAYOUT)?;
                let heard = timestamp_us.saturating_add(OPUS_FRAME_MICROS);
                cx.screen
                    .set_audio_clock(heard.saturating_sub(micros_of(output.buffered())));
                relay.write(true, frame);
            }
        }
    }
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

/// 転送する視聴者が子へ公開する broadcast(口の origin の `live/{session}`)。配信者の署名を確かめた frame を、中身を
/// 変えずに置く。親を替えても同じ broadcast に置き続ける(子の購読は続く)。
struct Relay {
    fanout: Arc<Fanout>,
    broadcast: moq_net::broadcast::Producer,
    catalog: Option<moq_mux::catalog::Producer>,
    video: Option<moq_mux::container::Producer<Container>>,
    audio: Option<moq_mux::container::Producer<Container>>,
}

impl Relay {
    fn new(fanout: Arc<Fanout>, session_id: &str) -> Result<Self> {
        let broadcast = fanout
            .origin
            .create_broadcast(live_media_broadcast_path(session_id))?;
        Ok(Self {
            fanout,
            broadcast,
            catalog: None,
            video: None,
            audio: None,
        })
    }

    fn announced(&self) -> bool {
        self.fanout.announced.load(Ordering::Acquire)
    }

    /// 親の catalog を写す。映像・音声の track は catalog に現れたときに作る。
    fn set_catalog(&mut self, catalog: &Catalog) -> Result<()> {
        let producer = match &mut self.catalog {
            Some(producer) => producer,
            None => self.catalog.insert(moq_mux::catalog::Producer::new(
                &mut self.broadcast,
                moq_mux::catalog::Config::default(),
            )?),
        };
        producer.mutate(|current| *current = catalog.clone())?;
        if self.video.is_none()
            && let Some(config) = catalog.video.renditions.get(LIVE_VIDEO_TRACK)
        {
            let track = self.broadcast.create_track(
                LIVE_VIDEO_TRACK,
                producer.track_info(hang::catalog::PRIORITY.video),
            )?;
            self.video = Some(moq_mux::container::Producer::new(
                track,
                Container::try_from(config)?,
            ));
        }
        if self.audio.is_none()
            && let Some(config) = catalog.audio.renditions.get(LIVE_AUDIO_TRACK)
        {
            let track = self.broadcast.create_track(
                LIVE_AUDIO_TRACK,
                producer.track_info(hang::catalog::PRIORITY.audio),
            )?;
            self.audio = Some(moq_mux::container::Producer::new(
                track,
                Container::try_from(config)?,
            ));
        }
        Ok(())
    }

    /// 確かめた frame を置き、置けたら子へ公開する(親を替えた直後の keyframe より前の差分などは置けずに捨てる)。
    fn write(&mut self, audio: bool, frame: moq_mux::container::Frame) {
        let producer = if audio {
            self.audio.as_mut()
        } else {
            self.video.as_mut()
        };
        if producer.is_some_and(|producer| producer.write(frame).is_ok())
            && !self.announced()
            && self.broadcast.announce(origin::Route::default()).is_ok()
        {
            self.fanout.announced.store(true, Ordering::Release);
        }
    }

    /// 子への公開を取り下げる(入力が途絶えた。子は取り下げの間、親が戻るのを待つ)。次に置けるのは keyframe から。
    fn unannounce(&mut self) {
        if self.fanout.announced.swap(false, Ordering::AcqRel) {
            self.broadcast.unannounce();
        }
        for producer in [self.video.as_mut(), self.audio.as_mut()]
            .into_iter()
            .flatten()
        {
            let _ = producer.cut(None);
        }
    }
}
