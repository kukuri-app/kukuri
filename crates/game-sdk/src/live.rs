//! Live の映像（ADR 0070 §8.3、#1982）。配信は Unity の readback の RGBA を受け、worker の thread で I420 にして VP8 に
//! 圧縮し、runtime の配信へ置く。送り待ちは 1 枚で、新しい frame が来たら古い frame を捨てる。視聴は runtime の送りかけの
//! chunk を復号し、音声の再生時刻に合う最も新しい frame を session ごとに 1 枚だけ持ち、poll で `live_video_frame` と
//! 詰めた I420 の bytes として渡す。

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex, Weak},
    time::{Duration, Instant},
};

use kukuri_desktop_runtime::{DesktopRuntime, LiveMediaRequest};

use crate::vp8::{Decoder, Encoder, Frame, rgba_bottom_up_to_i420};

/// 配信の上限（既存の受信側の camera の上限。ADR 0067 D8）。
pub const MAX_WIDTH: u32 = 1280;
pub const MAX_HEIGHT: u32 = 720;
const FRAMERATE: u32 = 30;
const BITRATE_KBPS: u32 = 1_500;
/// keyframe の間隔（desktop・Web の配信と同じ 2 秒）。
const KEYFRAME_INTERVAL: Duration = Duration::from_secs(2);
/// frame が届かなくなった配信の worker を止めるまでの時間（次の frame で起こし直す）。
const IDLE: Duration = Duration::from_secs(5);
/// 映像を音声より先に出してよい上限と、復号して描く前に持つ frame の上限（desktop・Web と同じ）。
const VIDEO_LEAD_US: u64 = 125_000;
const DECODED_FRAMES: usize = 30;

type Events = Arc<dyn Fn(Vec<u8>) + Send + Sync>;
pub(crate) type Shown = (u64, u64, Frame);

pub(crate) struct Live {
    handle: tokio::runtime::Handle,
    events: Events,
    encoders: Mutex<HashMap<String, Arc<Slot>>>,
    viewers: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    /// 視聴中の session の、まだ poll していない最新の frame（timestamp・そのときの音声の再生時刻・画像）。
    frames: Mutex<HashMap<String, Shown>>,
}

/// 配信の worker への送り待ち（1 枚）。
#[derive(Default)]
struct Slot {
    frame: Mutex<Option<Captured>>,
    ready: Condvar,
}

struct Captured {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    at: Instant,
}

/// 配信の映像の bytes の拒否。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VideoRefused {
    Invalid,
    NotBroadcasting,
}

impl Live {
    pub(crate) fn new(handle: tokio::runtime::Handle, events: Events) -> Arc<Self> {
        Arc::new(Self {
            handle,
            events,
            encoders: Mutex::default(),
            viewers: Mutex::default(),
            frames: Mutex::default(),
        })
    }

    /// `[幅 u16][高さ u16][下の行からの RGBA]` を配信の送り待ちに置く（待たない）。配信を始める前と止めた後の frame は断る。
    pub(crate) fn push_video(
        self: &Arc<Self>,
        runtime: Arc<DesktopRuntime>,
        session_id: &str,
        bytes: &[u8],
    ) -> Result<(), VideoRefused> {
        let (Some(width), Some(height)) = (
            bytes
                .get(0..2)
                .map(|b| u32::from(u16::from_le_bytes([b[0], b[1]]))),
            bytes
                .get(2..4)
                .map(|b| u32::from(u16::from_le_bytes([b[0], b[1]]))),
        ) else {
            return Err(VideoRefused::Invalid);
        };
        if !runtime.is_live_broadcasting(session_id) {
            return Err(VideoRefused::NotBroadcasting);
        }
        if width == 0
            || height == 0
            || width > MAX_WIDTH
            || height > MAX_HEIGHT
            || bytes.len() != 4 + (width * height * 4) as usize
        {
            return Err(VideoRefused::Invalid);
        }
        let slot = {
            let mut encoders = lock(&self.encoders);
            match encoders.get(session_id) {
                Some(slot) => slot.clone(),
                None => {
                    let slot = Arc::new(Slot::default());
                    encoders.insert(session_id.to_owned(), slot.clone());
                    let live = Arc::downgrade(self);
                    let (session, worker_slot) = (session_id.to_owned(), slot.clone());
                    std::thread::Builder::new()
                        .name("kukuri-live-encode".into())
                        .spawn(move || encode(live, runtime, session, worker_slot))
                        .map_err(|_| VideoRefused::Invalid)?;
                    slot
                }
            }
        };
        *lock(&slot.frame) = Some(Captured {
            width,
            height,
            rgba: bytes[4..].to_vec(),
            at: Instant::now(),
        });
        slot.ready.notify_one();
        Ok(())
    }

    /// 視聴を始めた session の映像を復号し始める（同じ session の前の復号は止めて始め直す）。
    pub(crate) fn watch(
        self: &Arc<Self>,
        runtime: Arc<DesktopRuntime>,
        topic: String,
        session_id: String,
    ) {
        let task = self.handle.spawn(view(
            Arc::downgrade(self),
            runtime,
            topic,
            session_id.clone(),
        ));
        if let Some(previous) = lock(&self.viewers).insert(session_id, task) {
            previous.abort();
        }
    }

    pub(crate) fn stop_watching(&self, session_id: &str) {
        if let Some(task) = lock(&self.viewers).remove(session_id) {
            task.abort();
        }
        lock(&self.frames).remove(session_id);
    }

    /// poll できる frame を、`fits`（JSON の event と bytes の大きさ）が許す分だけ取り出す。
    pub(crate) fn take_frames(&self, mut fits: impl FnMut(usize) -> bool) -> Vec<(String, Shown)> {
        let mut frames = lock(&self.frames);
        let ready: Vec<String> = frames
            .iter()
            .filter(|(_, (_, _, frame))| fits(frame.i420.len()))
            .map(|(session, _)| session.clone())
            .collect();
        ready
            .into_iter()
            .filter_map(|session| frames.remove(&session).map(|shown| (session, shown)))
            .collect()
    }

    fn emit(&self, event: serde_json::Value) {
        (self.events)(serde_json::to_vec(&event).expect("live event encodes"));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 配信の worker。送り待ちの frame を圧縮して runtime の配信へ置く。配信が止まる（置けない）か、`IDLE` の間 frame が届か
/// なければ終わる。
fn encode(live: Weak<Live>, runtime: Arc<DesktopRuntime>, session_id: String, slot: Arc<Slot>) {
    let handle = match live.upgrade() {
        Some(live) => live.handle.clone(),
        None => return,
    };
    let mut encoder: Option<Encoder> = None;
    let mut last_keyframe = Instant::now();
    let mut last_frame: Option<Instant> = None;
    let started = Instant::now();
    loop {
        let captured = {
            let guard = lock(&slot.frame);
            let (mut guard, _) = slot
                .ready
                .wait_timeout_while(guard, IDLE, |frame| frame.is_none())
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.take()
        };
        let Some(captured) = captured else { break };
        // 上限の frame rate を超える分は送らない（desktop・Web と同じく間隔の 8 割を下限にする）。
        if last_frame.is_some_and(|last| {
            captured.at.duration_since(last) < Duration::from_secs(1) * 4 / (5 * FRAMERATE)
        }) {
            continue;
        }
        last_frame = Some(captured.at);
        let size = (captured.width, captured.height);
        let mut keyframe = last_keyframe.elapsed() >= KEYFRAME_INTERVAL;
        if encoder
            .as_ref()
            .is_none_or(|encoder| encoder.size() != size)
        {
            encoder = Encoder::new(size.0, size.1, BITRATE_KBPS);
            keyframe = true;
        }
        let Some(encoder) = encoder.as_mut() else {
            break;
        };
        if keyframe {
            last_keyframe = Instant::now();
        }
        let i420 = rgba_bottom_up_to_i420(&captured.rgba, size.0, size.1);
        let pts = captured.at.duration_since(started).as_micros() as i64;
        for (chunk, key) in encoder.encode(&i420, pts, keyframe) {
            let pushed = handle.block_on(runtime.push_live_video(
                &session_id,
                key,
                captured.at.elapsed(),
                chunk,
            ));
            if pushed.is_err() {
                lock(&slot.frame).take();
                return remove(&live, &session_id, &slot);
            }
        }
    }
    remove(&live, &session_id, &slot);
}

/// 終わった worker の送り待ちを外す（同じ session の新しい worker のものは残す）。
fn remove(live: &Weak<Live>, session_id: &str, slot: &Arc<Slot>) {
    if let Some(live) = live.upgrade() {
        let mut encoders = lock(&live.encoders);
        if encoders
            .get(session_id)
            .is_some_and(|current| Arc::ptr_eq(current, slot))
        {
            encoders.remove(session_id);
        }
    }
}

/// 視聴の状態の code（runtime の `encode_screen_batch` の順）の名前。
fn state_name(code: u8) -> &'static str {
    match code {
        0 => "watching",
        1 => "full",
        2 => "no_media",
        _ => "ended",
    }
}

/// 視聴の状態の code・音声の再生時刻・chunk（timestamp と中身）。
type Batch<'a> = (u8, u64, Vec<(u64, &'a [u8])>);

/// runtime の送りかけの映像（`encode_screen_batch` の形）を読む: state・音声の再生時刻・chunk（timestamp・中身）。
fn parse_batch(batch: &[u8]) -> Option<Batch<'_>> {
    let state = *batch.first()?;
    let clock = u64::from_le_bytes(batch.get(1..9)?.try_into().ok()?);
    let count = u16::from_le_bytes(batch.get(9..11)?.try_into().ok()?);
    let mut at = 11;
    let mut chunks = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let timestamp = u64::from_le_bytes(batch.get(at..at + 8)?.try_into().ok()?);
        let len = u32::from_le_bytes(batch.get(at + 9..at + 13)?.try_into().ok()?) as usize;
        chunks.push((timestamp, batch.get(at + 13..at + 13 + len)?));
        at += 13 + len;
    }
    Some((state, clock, chunks))
}

/// 視聴の task。chunk を順に復号し、音声の再生時刻（無ければ最新）に合う frame を渡す。状態が変われば
/// `live_watch_status` を出し、終われば止まる。
async fn view(live: Weak<Live>, runtime: Arc<DesktopRuntime>, topic: String, session_id: String) {
    let Some(mut decoder) = Decoder::new() else {
        return;
    };
    let request = LiveMediaRequest {
        topic,
        session_id: session_id.clone(),
    };
    let mut state = None;
    let mut decoded: VecDeque<(u64, Frame)> = VecDeque::new();
    loop {
        let Ok(batch) = runtime.next_live_video(request.clone()).await else {
            return;
        };
        let Some(live) = live.upgrade() else { return };
        let Some((code, clock, chunks)) = parse_batch(&batch) else {
            return;
        };
        if state != Some(code) {
            state = Some(code);
            live.emit(serde_json::json!({
                "type": "live_watch_status", "session_id": session_id, "state": state_name(code),
            }));
        }
        for (timestamp, chunk) in chunks {
            if let Some(frame) = decoder.decode(chunk) {
                if decoded.len() == DECODED_FRAMES {
                    decoded.pop_front();
                }
                decoded.push_back((timestamp, frame));
            }
        }
        // 描く frame: 音声の再生時刻に進みの上限を足した時刻までで最も新しいもの（それより前は捨てる）。
        let due = decoded
            .iter()
            .rposition(|(timestamp, _)| clock == 0 || *timestamp <= clock + VIDEO_LEAD_US);
        if let Some(due) = due {
            let (timestamp, frame) = decoded.drain(..=due).next_back().expect("due is in range");
            lock(&live.frames).insert(session_id.clone(), (timestamp, clock, frame));
        }
        if state_name(code) == "ended" {
            lock(&live.viewers).remove(&session_id);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_from_the_runtime_are_read_in_order() {
        let mut batch = vec![0u8];
        batch.extend_from_slice(&42u64.to_le_bytes());
        batch.extend_from_slice(&2u16.to_le_bytes());
        for (timestamp, data) in [(10u64, &b"ab"[..]), (20, &b"c"[..])] {
            batch.extend_from_slice(&timestamp.to_le_bytes());
            batch.push(1);
            batch.extend_from_slice(&(data.len() as u32).to_le_bytes());
            batch.extend_from_slice(data);
        }
        let (state, clock, chunks) = parse_batch(&batch).unwrap();
        assert_eq!((state, clock), (0, 42));
        assert_eq!(chunks, vec![(10, &b"ab"[..]), (20, &b"c"[..])]);
        assert!(parse_batch(&batch[..batch.len() - 1]).is_none());
    }
}
