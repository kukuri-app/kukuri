//! SDK のゲームの音（Unity の出力音。mic の音は Unity が混ぜて渡す。ADR 0070 §8.3、#1982）を、配信の音声の track にする。
//! 最初の音が届いたときに track を作り、mic と同じく非公開の broadcast で Opus に圧縮してから署名して写す。送り待ちは
//! 200 ms 分までで、溢れたら古いものから捨てる。ゲームの音を送る配信は mic を開かない（同じ track の名前を使う）。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::Result;
use kukuri_core::LiveMediaSigner;
use moq_audio::{Frame, Layout, encode};
use moq_net::Timestamp;
use n0_future::task::AbortOnDropHandle;
use tokio::sync::Notify;

use super::{LIVE_AUDIO_TRACK, micros, sign_audio};
use crate::media_devices::OPUS_SAMPLE_RATE;

const QUEUE_LIMIT: Duration = Duration::from_millis(200);
const OPUS_BITRATE_BPS: u64 = 24_000;

/// ゲームの音の送り待ち。`push` は Unity の audio thread から呼ばれ、待たない。
pub(crate) struct GameAudio {
    clock: moq_mux::Clock,
    queue: StdMutex<Queue>,
    arrived: Notify,
}

#[derive(Default)]
struct Queue {
    /// 最初に届いた音の sample rate。以後この rate の音だけを受ける。
    rate: Option<u32>,
    /// 届いた時刻（μs、配信の clock）と mono の sample。
    chunks: VecDeque<(u64, Vec<f32>)>,
    samples: usize,
}

impl GameAudio {
    /// 送り待ちと、それを配信の track へ写す task。task は最初の音が届くまで track を作らない。
    pub(super) fn start(
        broadcast: moq_net::broadcast::Producer,
        catalog: moq_mux::catalog::Producer,
        signer: LiveMediaSigner,
    ) -> (Arc<Self>, AbortOnDropHandle<()>) {
        let audio = Arc::new(Self {
            clock: catalog.clock(),
            queue: StdMutex::default(),
            arrived: Notify::new(),
        });
        let task = n0_future::task::spawn(forward(audio.clone(), broadcast, catalog, signer));
        (audio, AbortOnDropHandle::new(task))
    }

    /// interleave した `channels` の sample を mono にして送り待ちに置く。rate が最初の音と違えば置かない。
    pub(crate) fn push(&self, sample_rate: u32, channels: u16, interleaved: &[f32]) -> bool {
        if sample_rate == 0 || channels == 0 {
            return false;
        }
        let mono: Vec<f32> = interleaved
            .chunks(usize::from(channels))
            .map(|frame| frame.iter().sum::<f32>() / f32::from(channels))
            .collect();
        let span = mono.len() as u64 * 1_000_000 / u64::from(sample_rate);
        let at = micros(self.clock.now()).saturating_sub(span);
        let mut queue = self.queue.lock().expect("game audio queue poisoned");
        if *queue.rate.get_or_insert(sample_rate) != sample_rate {
            return false;
        }
        let limit = (u128::from(sample_rate) * QUEUE_LIMIT.as_millis() / 1_000) as usize;
        queue.samples += mono.len();
        queue.chunks.push_back((at, mono));
        while queue.samples > limit
            && let Some((_, dropped)) = queue.chunks.pop_front()
        {
            queue.samples -= dropped.len();
        }
        drop(queue);
        self.arrived.notify_one();
        true
    }

    fn take(&self) -> (Option<u32>, Vec<(u64, Vec<f32>)>) {
        let mut queue = self.queue.lock().expect("game audio queue poisoned");
        queue.samples = 0;
        (queue.rate, queue.chunks.drain(..).collect())
    }
}

async fn forward(
    audio: Arc<GameAudio>,
    mut broadcast: moq_net::broadcast::Producer,
    catalog: moq_mux::catalog::Producer,
    signer: LiveMediaSigner,
) {
    let mut output = None;
    loop {
        audio.arrived.notified().await;
        let (Some(rate), chunks) = audio.take() else {
            continue;
        };
        if output.is_none() {
            match open(rate, &mut broadcast, &catalog, signer.clone()).await {
                Ok(opened) => output = Some(opened),
                Err(error) => {
                    tracing::warn!(%error, "live game audio could not be published");
                    return;
                }
            }
        }
        let Some((producer, _, _)) = output.as_mut() else {
            continue;
        };
        for (at, samples) in chunks {
            let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            if let Ok(timestamp) = Timestamp::from_micros(at) {
                let _ = producer.write(&Frame::new(data.into(), timestamp));
            }
        }
    }
}

/// 非公開の broadcast に Opus の track を作り、署名して配信の音声の track へ写す task を起こす（mic と同じ形）。
async fn open(
    rate: u32,
    broadcast: &mut moq_net::broadcast::Producer,
    catalog: &moq_mux::catalog::Producer,
    signer: LiveMediaSigner,
) -> Result<(
    encode::Producer,
    moq_net::broadcast::Producer,
    AbortOnDropHandle<()>,
)> {
    let mut private = moq_net::broadcast::Info::new().produce();
    let private_catalog = moq_mux::catalog::Producer::new(
        &mut private,
        moq_mux::catalog::Config::default().with_clock(catalog.clock()),
    )?;
    let mut options = encode::Options::default();
    options.track = Some(LIVE_AUDIO_TRACK.to_string());
    options.settings = encode::Settings::new(OPUS_SAMPLE_RATE, Layout::Mono);
    options.settings.bitrate = Some(moq_net::bandwidth::Rate::from_bps(OPUS_BITRATE_BPS));
    let producer = encode::Producer::new(
        &mut private,
        private_catalog.clone(),
        encode::Input::new(rate, Layout::Mono),
        &options,
    )?;
    let signing = sign_audio(&private, &private_catalog, broadcast, catalog, signer).await?;
    Ok((producer, private, signing))
}
