//! media の受け口(`/kukuri/media/1`)の認可と、受ける側で共用する部品(ADR 0067 §3・§4)。
//!
//! 認可要求の種類ごとに、metaverse の空間音声か live の配信へ振り分ける。拒否は理由の code だけを返し、他の参加者の
//! 識別子と media を返さない。Web は metaverse を扱わない(ADR 0060)ので、空間音声の要求は断る。

use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use kukuri_app_api::AppService;
use kukuri_core::MediaAccessRequestV1;
use kukuri_iroh_node::{MediaAuthorizer, MediaDenied};
use moq_mux::catalog::hang::{Catalog, Container};
use moq_net::track;
use n0_future::time::{Instant, timeout};

use crate::live_media::RuntimeLiveMedia;
use crate::media_devices::OPUS_SAMPLE_RATE;
#[cfg(not(target_family = "wasm"))]
use crate::metaverse_audio::MetaverseAudio;

pub(crate) const DENIED_ACCESS: &str = "access_denied";
pub(crate) const DENIED_NOT_IN_DOME: &str = "not_in_dome";
pub(crate) const DENIED_INVALID: &str = "invalid_request";
pub(crate) const DENIED_UNAVAILABLE: &str = "unavailable";
/// 直接の子が上限に達している(ADR 0067 §4)。live は改行の後に子の一覧と経路が続く(§8)。
pub(crate) const DENIED_FULL: &str = "full";
/// 配信していない・終わった live session(転送する視聴者は、入力が途絶えている間も)。
pub(crate) const DENIED_NOT_LIVE: &str = "not_live";
/// 要求した端末が、転送する視聴者の祖先(循環の防止。ADR 0067 §8)。
pub(crate) const DENIED_ANCESTOR: &str = "ancestor";
const KNOWN_DENIALS: [&str; 7] = [
    DENIED_ACCESS,
    DENIED_NOT_IN_DOME,
    DENIED_INVALID,
    DENIED_UNAVAILABLE,
    DENIED_FULL,
    DENIED_NOT_LIVE,
    DENIED_ANCESTOR,
];
/// 公開を待つ・catalog を読む上限。
pub(crate) const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(10);

/// iroh-node の media の受け口へ付ける認可。runtime を所有しない(止まった後の要求は拒否する)。
pub(crate) fn media_authorizer(
    #[cfg(not(target_family = "wasm"))] audio: Weak<MetaverseAudio>,
    live: Weak<RuntimeLiveMedia>,
    app: Weak<AppService>,
) -> MediaAuthorizer {
    Arc::new(move |remote, request| {
        #[cfg(not(target_family = "wasm"))]
        let audio = audio.clone();
        let (live, app) = (live.clone(), app.clone());
        Box::pin(async move {
            let app = app
                .upgrade()
                .ok_or_else(|| DENIED_UNAVAILABLE.to_string())?;
            match serde_json::from_slice::<MediaAccessRequestV1>(&request) {
                #[cfg(not(target_family = "wasm"))]
                Ok(MediaAccessRequestV1::MetaverseAudio(request)) => match audio.upgrade() {
                    Some(audio) => audio.grant(&app, request).await,
                    None => Err(DENIED_UNAVAILABLE.to_string()),
                },
                #[cfg(target_family = "wasm")]
                Ok(MediaAccessRequestV1::MetaverseAudio(_)) => Err(DENIED_UNAVAILABLE.to_string()),
                Ok(MediaAccessRequestV1::Live(request)) => match live.upgrade() {
                    Some(live) => live.grant(&app, remote, request).await,
                    None => Err(DENIED_UNAVAILABLE.to_string()),
                },
                Err(_) => Err(DENIED_INVALID.to_string()),
            }
        })
    })
}

/// 購読の拒否の code(既知のものだけ。相手の識別子と media を log に出さないため。ADR 0067 §2)と添え書き。
pub(crate) fn denial(error: &anyhow::Error) -> Option<(&'static str, &str)> {
    let denied = error.downcast_ref::<MediaDenied>()?;
    let (code, detail) = denied.0.split_once('\n').unwrap_or((&denied.0, ""));
    let code = KNOWN_DENIALS
        .into_iter()
        .find(|known| *known == code)
        .unwrap_or("unknown");
    Some((code, detail))
}

pub(crate) fn denial_code(error: &anyhow::Error) -> Option<&'static str> {
    denial(error).map(|(code, _)| code)
}

/// 受ける側の送信量の検査(ADR 0041 player・ADR 0067 §4)。1 秒分の枠を経過時間に比例して補充するので、正規の
/// frame の到着の揺れとまとめ到着(1 秒分まで)は捨てず、上限を超え続ける分だけを捨てる。
pub(crate) struct RateBudget {
    at: Option<Instant>,
    frames: f64,
    bytes: f64,
    max_frames: u64,
    max_bytes: u64,
}

/// 超えた上限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RateExceeded {
    Frames,
    Bytes,
}

impl RateBudget {
    pub(crate) fn new(max_frames: u64, max_bytes: u64) -> Self {
        Self {
            at: None,
            frames: max_frames as f64,
            bytes: max_bytes as f64,
            max_frames,
            max_bytes,
        }
    }

    // 上限の値を示すのは metaverse の空間音声の拒否だけ(Web は metaverse を扱わない)。
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn max_frames(&self) -> u64 {
        self.max_frames
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub(crate) fn admit(
        &mut self,
        now: Instant,
        bytes: usize,
    ) -> std::result::Result<(), RateExceeded> {
        let elapsed = self
            .at
            .map_or(0.0, |at| now.saturating_duration_since(at).as_secs_f64());
        self.at = Some(now);
        self.frames = (self.frames + elapsed * self.max_frames as f64).min(self.max_frames as f64);
        self.bytes = (self.bytes + elapsed * self.max_bytes as f64).min(self.max_bytes as f64);
        if self.frames < 1.0 {
            return Err(RateExceeded::Frames);
        }
        if self.bytes < bytes as f64 {
            return Err(RateExceeded::Bytes);
        }
        self.frames -= 1.0;
        self.bytes -= bytes as f64;
        Ok(())
    }
}

/// 送る側の catalog を待って読む。続く更新は返した stream から読める。
pub(crate) async fn read_catalog(
    broadcast: &moq_net::broadcast::Consumer,
) -> Result<(Catalog, moq_mux::catalog::Consumer)> {
    let mut catalogs =
        moq_mux::catalog::Consumer::<()>::new(broadcast, moq_mux::catalog::CatalogFormat::Hang)
            .await?;
    let catalog = timeout(
        SUBSCRIBE_TIMEOUT,
        moq_mux::catalog::Stream::next(&mut catalogs),
    )
    .await
    .context("the catalog did not arrive in time")??
    .context("the catalog ended")?;
    Ok((catalog, catalogs))
}

/// catalog の `track_name` の Opus(mono 48 kHz)を、group の順を並べ直して時刻順に読む frame と decoder。
/// `max_age`(再生前の保持の上限)より遅れた group は古いものから飛ばす。
pub(crate) async fn opus_frames(
    broadcast: &moq_net::broadcast::Consumer,
    catalog: &Catalog,
    track_name: &str,
    max_age: Duration,
) -> Result<(
    moq_mux::container::Consumer<Container>,
    moq_audio::decode::Decoder,
)> {
    let config = catalog
        .audio
        .renditions
        .get(track_name)
        .context("no audio track")?;
    if config.codec != hang::catalog::AudioCodec::Opus
        || config.sample_rate != OPUS_SAMPLE_RATE
        || config.channel_count != 1
    {
        bail!("the audio track is not mono 48 kHz Opus");
    }
    let container = Container::try_from(config)
        .map_err(|error| anyhow!("unsupported audio container: {error}"))?;
    let decoder = moq_audio::decode::Decoder::new(config, &moq_audio::decode::Config::new())?;
    Ok((
        ordered_frames(
            broadcast,
            track_name,
            container,
            hang::catalog::PRIORITY.audio,
            max_age,
        )
        .await?,
        decoder,
    ))
}

/// track の frame を、group の順を並べ直して時刻順に読む。`max_age` より遅れた group は古いものから飛ばす。
pub(crate) async fn ordered_frames(
    broadcast: &moq_net::broadcast::Consumer,
    track_name: &str,
    container: Container,
    priority: u8,
    max_age: Duration,
) -> Result<moq_mux::container::Consumer<Container>> {
    let track = broadcast
        .track(track_name)?
        .subscribe(
            track::Subscription::default()
                .with_priority(priority)
                .with_max_age(max_age),
        )
        .await?;
    Ok(moq_mux::container::Consumer::new(track, container))
}
