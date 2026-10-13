//! 画面へ渡す映像(ADR 0067 §4・§9): 送りかけは 1 秒分まで、音声の再生時刻を添える。

use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use moq_net::bytes::Bytes;
use n0_future::time::{Instant, timeout};
use tokio::sync::Notify;

use super::{LiveWatchState, micros_of};

/// 画面へ送りかけの映像の上限(1 秒分。ADR 0067 §4)。
const SCREEN_QUEUE_CHUNKS: usize = 30;
/// 画面が次の映像を待つ上限(届かなければ空を返し、画面はまた待つ)。
const SCREEN_WAIT: Duration = Duration::from_secs(1);

/// 画面へ送りかけの映像(1 秒分まで。超えたら次の keyframe まで捨てる)と、視聴の状態・音声の再生時刻。
#[derive(Default)]
pub(super) struct ScreenQueue {
    pub(super) state: StdMutex<ScreenState>,
    ready: Notify,
}

#[derive(Default)]
pub(super) struct ScreenState {
    pub(super) chunks: VecDeque<ScreenChunk>,
    pub(super) waiting_keyframe: bool,
    ended: Option<LiveWatchState>,
    /// 最後に再生へ渡した音声の、聞こえる位置の timestamp(μs)と、渡した時刻。音声がまだ無ければ None。
    audio_clock: Option<(u64, Instant)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScreenChunk {
    pub(super) timestamp_us: u64,
    pub(super) keyframe: bool,
    pub(super) data: Bytes,
}

impl ScreenQueue {
    pub(super) fn push(&self, chunk: ScreenChunk) {
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

    pub(super) fn set_audio_clock(&self, timestamp_us: u64) {
        self.state
            .lock()
            .expect("screen queue poisoned")
            .audio_clock = Some((timestamp_us, Instant::now()));
    }

    pub(super) fn end(&self, state: LiveWatchState) {
        self.state
            .lock()
            .expect("screen queue poisoned")
            .ended
            .get_or_insert(state);
        self.ready.notify_one();
    }

    /// 送りかけの映像を全部取る。無ければ `SCREEN_WAIT` まで待つ。音声の再生時刻は、最後に渡した位置から経過時間で
    /// 進める(mic が止まっても映像を止めない)。音声がまだ無ければ 0。
    pub(super) async fn take(&self) -> (LiveWatchState, u64, Vec<ScreenChunk>) {
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
pub(super) fn encode_screen_batch(
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
