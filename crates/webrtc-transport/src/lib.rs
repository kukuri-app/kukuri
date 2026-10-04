//! iroh の QUIC パケットを WebRTC DataChannel で運ぶ Custom Transport（ADR 0057）。
//!
//! 接続交渉（SDP の受け渡し）は呼び出し側が行う。この crate は session の作成と、DataChannel での
//! datagram の送受信と、資源の上限を持つ。native は str0m、browser は web-sys の `RTCPeerConnection` を使う。
//! 相手の本人確認は QUIC が行い、DataChannel の相手から届いた bytes はそのまま QUIC へ渡すだけにする。

use std::{
    collections::HashMap,
    fmt, io,
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

use anyhow::{Context as _, Result, bail, ensure};
use bytes::Bytes;
use iroh::{
    EndpointId,
    endpoint::transports::{CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit},
};
use iroh_base::CustomAddr;
use tokio::sync::{mpsc, oneshot};

#[cfg(target_family = "wasm")]
mod browser;
#[cfg(target_family = "wasm")]
use browser as backend;
#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
use native as backend;
mod signaling;

pub use signaling::{
    DemandHooks, MAX_ATTEMPTS, MAX_NEGOTIATIONS, NEGOTIATION_DEADLINE, Rejected, Rejection,
    SIGNALING_ALPN, STUN_PORT, Signaling, SignalingStats,
};

/// `CustomAddr` の transport id（ASCII の `KKWR`）。kukuri の中だけで使う。
pub const TRANSPORT_ID: u64 = 0x4B4B_5752;
/// 1 つの transport が同時に持つ session の上限。
pub const MAX_SESSIONS: usize = 16;
/// session ごとの送信の待ち（datagram の数）。
pub const SEND_QUEUE_DATAGRAMS: usize = 64;
/// transport 全体の受信の待ち（datagram の数）。
pub const RECV_QUEUE_DATAGRAMS: usize = 256;
/// 1 datagram の上限。DataChannel の相互運用で安全な大きさ。
pub const MAX_DATAGRAM_BYTES: usize = 16 * 1024;
/// 受け取る SDP の上限。
pub const MAX_SDP_BYTES: usize = 16 * 1024;
/// 受け取る SDP の ICE の候補の数の上限。
pub const MAX_SDP_CANDIDATES: usize = 32;
/// 受け取る SDP の ICE の候補 1 行の上限。
pub const MAX_CANDIDATE_BYTES: usize = 512;
/// DataChannel の未送信 bytes がこれに達したら、低水位を下回るまで送る datagram を捨てる。
pub const BUFFERED_HIGH_BYTES: usize = 1024 * 1024;
/// 送信を再開する未送信 bytes。
pub const BUFFERED_LOW_BYTES: usize = 256 * 1024;

/// session の event の channel。session の上限の 2 倍を超えるので溢れない。
const EVENT_QUEUE: usize = 64;
/// iroh は Endpoint の GSO の batch 数を全 transport の最小値にするので、UDP を妨げない値にする。
const MAX_TRANSMIT_SEGMENTS: usize = 64;
const CHANNEL_LABEL: &str = "kukuri-quic/1";
const CHANNEL_STREAM_ID: u16 = 0;

/// session の識別子。乱数で、秘密・account 由来の値を含まない。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId([u8; 16]);

impl SessionId {
    fn generate() -> Result<Self> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("session id: {error}"))?;
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..4] {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// session を始めた側か、受けた側か。両端の `CustomAddr` を区別する。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Offerer = 0,
    Answerer = 1,
}

impl Role {
    fn other(self) -> Self {
        match self {
            Self::Offerer => Self::Answerer,
            Self::Answerer => Self::Offerer,
        }
    }
}

fn session_addr(session: SessionId, role: Role) -> CustomAddr {
    let mut data = [0u8; 17];
    data[..16].copy_from_slice(session.as_bytes());
    data[16] = role as u8;
    CustomAddr::from_parts(TRANSPORT_ID, &data)
}

fn parse_session_addr(addr: &CustomAddr) -> Option<(SessionId, Role)> {
    if addr.id() != TRANSPORT_ID {
        return None;
    }
    let data: &[u8; 17] = addr.data().try_into().ok()?;
    let role = match data[16] {
        0 => Role::Offerer,
        1 => Role::Answerer,
        _ => return None,
    };
    let mut session = [0u8; 16];
    session.copy_from_slice(&data[..16]);
    Some((SessionId(session), role))
}

/// 受け取る SDP の大きさと候補の数を検査する。
fn check_remote_sdp(sdp: &str) -> Result<()> {
    ensure!(
        sdp.len() <= MAX_SDP_BYTES,
        "sdp exceeds {MAX_SDP_BYTES} bytes"
    );
    let mut candidates = 0usize;
    for line in sdp.lines() {
        if line.starts_with("a=candidate:") {
            candidates += 1;
            ensure!(
                line.len() <= MAX_CANDIDATE_BYTES,
                "ice candidate exceeds {MAX_CANDIDATE_BYTES} bytes"
            );
        }
    }
    ensure!(
        candidates <= MAX_SDP_CANDIDATES,
        "sdp has more than {MAX_SDP_CANDIDATES} ice candidates"
    );
    Ok(())
}

/// session の状態の変化。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// DataChannel が開いた。`addr` へ QUIC を送れる。
    Opened {
        session: SessionId,
        remote: EndpointId,
        addr: CustomAddr,
    },
    /// session が閉じ、資源を解放した。
    Closed { session: SessionId, reason: String },
}

/// 診断と試験のための数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportStats {
    pub sessions: usize,
    pub running_backends: usize,
    pub dropped_send: u64,
    pub dropped_recv: u64,
    pub max_buffered_bytes: u64,
    /// custom path で受け取り、QUIC へ渡した bytes（経路ごとの実データの判別。#1422）。
    pub received_bytes: u64,
}

/// transport の設定。
#[derive(Clone, Debug)]
pub struct WebRtcConfig {
    /// native の ICE の socket を bind する IP。host の候補になる。未指定なら既定の経路の IP を使う。
    #[cfg(not(target_family = "wasm"))]
    pub bind_ip: std::net::IpAddr,
}

enum Control {
    Answer(String, oneshot::Sender<Result<()>>),
}

struct SessionEntry {
    remote: EndpointId,
    role: Role,
    open: bool,
    out: mpsc::Sender<Bytes>,
    control: mpsc::Sender<Control>,
}

struct Incoming {
    data: Bytes,
    remote: CustomAddr,
    local: CustomAddr,
}

/// backend に渡す session の開始の入力。
struct Start {
    session: SessionId,
    role: Role,
    remote_offer: Option<String>,
    /// STUN の送信先（`host:port`）。空なら送らない。
    stun: Vec<String>,
    reply: oneshot::Sender<Result<String>>,
    out: mpsc::Receiver<Bytes>,
    control: mpsc::Receiver<Control>,
    shared: Arc<Shared>,
}

struct Shared {
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    config: WebRtcConfig,
    sessions: Mutex<HashMap<SessionId, SessionEntry>>,
    recv_tx: mpsc::Sender<Incoming>,
    recv_rx: Mutex<Option<mpsc::Receiver<Incoming>>>,
    events_tx: mpsc::Sender<SessionEvent>,
    events_rx: Mutex<Option<mpsc::Receiver<SessionEvent>>>,
    local_addrs: n0_watcher::Watchable<Vec<CustomAddr>>,
    running_backends: AtomicUsize,
    dropped_send: AtomicU64,
    dropped_recv: AtomicU64,
    max_buffered: AtomicU64,
    received: AtomicU64,
    #[cfg(test)]
    test_send_loss: (AtomicU64, AtomicU64),
}

impl Shared {
    /// 試験で、送る datagram の `every` 個に 1 個を捨てる（E6。native の試験だけが使う）。
    #[cfg(all(test, not(target_family = "wasm")))]
    fn set_test_send_loss(&self, every: u64) {
        self.test_send_loss.0.store(every, Ordering::Relaxed);
    }

    fn test_drops_send(&self) -> bool {
        #[cfg(test)]
        {
            let every = self.test_send_loss.0.load(Ordering::Relaxed);
            if every > 0 {
                return (self.test_send_loss.1.fetch_add(1, Ordering::Relaxed) + 1)
                    .is_multiple_of(every);
            }
        }
        false
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<SessionId, SessionEntry>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn publish_local_addrs(&self, sessions: &HashMap<SessionId, SessionEntry>) {
        let addrs = sessions
            .iter()
            .filter(|(_, entry)| entry.open)
            .map(|(session, entry)| session_addr(*session, entry.role))
            .collect();
        let _ = self.local_addrs.set(addrs);
    }

    /// DataChannel が開いた。閉じた後に届いた callback では何もしない。
    fn opened(&self, session: SessionId) {
        let mut sessions = self.sessions();
        let Some(entry) = sessions.get_mut(&session) else {
            return;
        };
        entry.open = true;
        let event = SessionEvent::Opened {
            session,
            remote: entry.remote,
            addr: session_addr(session, entry.role.other()),
        };
        self.publish_local_addrs(&sessions);
        drop(sessions);
        let _ = self.events_tx.try_send(event);
    }

    /// 受け取った datagram を QUIC へ渡す。待ちが満杯なら捨てる（UDP の損失と同じ扱い）。
    fn deliver(&self, session: SessionId, data: Bytes) {
        let role = match self.sessions().get(&session) {
            Some(entry) if entry.open => entry.role,
            _ => return,
        };
        let len = data.len() as u64;
        if data.len() > MAX_DATAGRAM_BYTES
            || self
                .recv_tx
                .try_send(Incoming {
                    data,
                    remote: session_addr(session, role.other()),
                    local: session_addr(session, role),
                })
                .is_err()
        {
            self.dropped_recv.fetch_add(1, Ordering::Relaxed);
        } else {
            self.received.fetch_add(len, Ordering::Relaxed);
        }
    }

    fn note_dropped_send(&self) {
        self.dropped_send.fetch_add(1, Ordering::Relaxed);
    }

    fn note_buffered(&self, bytes: usize) {
        self.max_buffered.fetch_max(bytes as u64, Ordering::Relaxed);
    }

    fn backend_started(&self) {
        self.running_backends.fetch_add(1, Ordering::Relaxed);
    }

    /// backend が終わった。session の資源を解放し、`Closed` を 1 回だけ出す。
    fn backend_finished(&self, session: SessionId, reason: String) {
        let mut sessions = self.sessions();
        sessions.remove(&session);
        self.publish_local_addrs(&sessions);
        drop(sessions);
        self.running_backends.fetch_sub(1, Ordering::Relaxed);
        let _ = self
            .events_tx
            .try_send(SessionEvent::Closed { session, reason });
    }
}

/// QUIC over WebRTC DataChannel の transport。`Builder::add_custom_transport` へ渡す。
pub struct WebRtcTransport {
    shared: Arc<Shared>,
}

impl fmt::Debug for WebRtcTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebRtcTransport")
            .field("stats", &self.stats())
            .finish()
    }
}

impl WebRtcTransport {
    pub fn new(config: WebRtcConfig) -> Arc<Self> {
        let (recv_tx, recv_rx) = mpsc::channel(RECV_QUEUE_DATAGRAMS);
        let (events_tx, events_rx) = mpsc::channel(EVENT_QUEUE);
        Arc::new(Self {
            shared: Arc::new(Shared {
                config,
                sessions: Mutex::new(HashMap::new()),
                recv_tx,
                recv_rx: Mutex::new(Some(recv_rx)),
                events_tx,
                events_rx: Mutex::new(Some(events_rx)),
                local_addrs: n0_watcher::Watchable::new(Vec::new()),
                running_backends: AtomicUsize::new(0),
                dropped_send: AtomicU64::new(0),
                dropped_recv: AtomicU64::new(0),
                max_buffered: AtomicU64::new(0),
                received: AtomicU64::new(0),
                #[cfg(test)]
                test_send_loss: (AtomicU64::new(0), AtomicU64::new(0)),
            }),
        })
    }

    /// session の event の受け口。最初の 1 回だけ返す。
    pub fn take_events(&self) -> Option<mpsc::Receiver<SessionEvent>> {
        self.shared
            .events_rx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub fn stats(&self) -> TransportStats {
        TransportStats {
            sessions: self.shared.sessions().len(),
            running_backends: self.shared.running_backends.load(Ordering::Relaxed),
            dropped_send: self.shared.dropped_send.load(Ordering::Relaxed),
            dropped_recv: self.shared.dropped_recv.load(Ordering::Relaxed),
            max_buffered_bytes: self.shared.max_buffered.load(Ordering::Relaxed),
            received_bytes: self.shared.received.load(Ordering::Relaxed),
        }
    }

    /// 相手へ送る offer を作る。候補は SDP に含める（trickle しない）。`stun` は STUN の送信先。
    pub async fn offer(&self, remote: EndpointId, stun: &[String]) -> Result<(SessionId, String)> {
        let session = SessionId::generate()?;
        let sdp = self
            .start(remote, session, Role::Offerer, None, stun)
            .await?;
        Ok((session, sdp))
    }

    /// 相手の offer を受け、返す answer を作る。
    pub async fn answer(
        &self,
        remote: EndpointId,
        session: SessionId,
        offer: &str,
        stun: &[String],
    ) -> Result<String> {
        check_remote_sdp(offer)?;
        self.start(
            remote,
            session,
            Role::Answerer,
            Some(offer.to_string()),
            stun,
        )
        .await
    }

    /// 自分の offer への answer を適用する。
    pub async fn accept_answer(&self, session: SessionId, answer: &str) -> Result<()> {
        check_remote_sdp(answer)?;
        let control = match self.shared.sessions().get(&session) {
            Some(entry) if entry.role == Role::Offerer => entry.control.clone(),
            Some(_) => bail!("session {session:?} did not make the offer"),
            None => bail!("session {session:?} is not open"),
        };
        let (ack, done) = oneshot::channel();
        control
            .send(Control::Answer(answer.to_string(), ack))
            .await
            .map_err(|_| anyhow::anyhow!("session {session:?} is closed"))?;
        done.await
            .map_err(|_| anyhow::anyhow!("session {session:?} is closed"))?
    }

    /// session を閉じる。資源は backend が解放し、`Closed` を出す。
    pub fn close(&self, session: SessionId) {
        let mut sessions = self.shared.sessions();
        sessions.remove(&session);
        self.shared.publish_local_addrs(&sessions);
    }

    async fn start(
        &self,
        remote: EndpointId,
        session: SessionId,
        role: Role,
        remote_offer: Option<String>,
        stun: &[String],
    ) -> Result<String> {
        let (out_tx, out) = mpsc::channel(SEND_QUEUE_DATAGRAMS);
        let (control_tx, control) = mpsc::channel(1);
        {
            let mut sessions = self.shared.sessions();
            ensure!(
                sessions.len() < MAX_SESSIONS,
                "webrtc sessions are at the limit of {MAX_SESSIONS}"
            );
            ensure!(
                !sessions.contains_key(&session),
                "session {session:?} already exists"
            );
            sessions.insert(
                session,
                SessionEntry {
                    remote,
                    role,
                    open: false,
                    out: out_tx,
                    control: control_tx,
                },
            );
        }
        let (reply, sdp) = oneshot::channel();
        self.shared.backend_started();
        backend::spawn(Start {
            session,
            role,
            remote_offer,
            stun: stun.to_vec(),
            reply,
            out,
            control,
            shared: Arc::clone(&self.shared),
        });
        match sdp.await {
            Ok(Ok(sdp)) => Ok(sdp),
            Ok(Err(error)) => {
                self.close(session);
                Err(error)
            }
            Err(_) => {
                self.close(session);
                Err(anyhow::anyhow!("webrtc session {session:?} ended"))
            }
        }
        .with_context(|| format!("start webrtc session {session:?}"))
    }
}

impl Drop for WebRtcTransport {
    fn drop(&mut self) {
        self.shared.sessions().clear();
    }
}

impl CustomTransport for WebRtcTransport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        let recv = self
            .shared
            .recv_rx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| io::Error::other("webrtc transport is already bound"))?;
        Ok(Box::new(WebRtcEndpoint {
            shared: Arc::clone(&self.shared),
            recv,
        }))
    }
}

struct WebRtcEndpoint {
    shared: Arc<Shared>,
    recv: mpsc::Receiver<Incoming>,
}

impl fmt::Debug for WebRtcEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WebRtcEndpoint")
    }
}

impl CustomEndpoint for WebRtcEndpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.shared.local_addrs.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        Arc::new(WebRtcSender {
            shared: Arc::clone(&self.shared),
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        let mut filled = 0;
        while filled < bufs.len() {
            let packet = match self.recv.poll_recv(cx) {
                Poll::Ready(Some(packet)) => packet,
                Poll::Ready(None) if filled == 0 => {
                    return Poll::Ready(Err(io::Error::other("webrtc transport closed")));
                }
                Poll::Ready(None) | Poll::Pending => break,
            };
            let len = packet.data.len();
            if len > bufs[filled].len() {
                self.shared.dropped_recv.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            bufs[filled][..len].copy_from_slice(&packet.data);
            metas[filled].len = len;
            metas[filled].stride = len;
            recv_infos[filled] = RecvInfo::new(packet.remote, Some(packet.local));
            filled += 1;
        }
        if filled == 0 {
            Poll::Pending
        } else {
            Poll::Ready(Ok(filled))
        }
    }

    fn max_transmit_segments(&self) -> NonZeroUsize {
        NonZeroUsize::new(MAX_TRANSMIT_SEGMENTS).unwrap_or(NonZeroUsize::MIN)
    }
}

struct WebRtcSender {
    shared: Arc<Shared>,
}

impl fmt::Debug for WebRtcSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WebRtcSender")
    }
}

impl WebRtcSender {
    fn open_session(&self, dst: &CustomAddr) -> Option<mpsc::Sender<Bytes>> {
        let (session, role) = parse_session_addr(dst)?;
        match self.shared.sessions().get(&session) {
            Some(entry) if entry.open && entry.role == role.other() => Some(entry.out.clone()),
            _ => None,
        }
    }
}

impl CustomSender for WebRtcSender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        self.open_session(addr).is_some()
    }

    /// 送信の待ちが満杯なら捨てて `Ok` を返す。iroh は `Pending` の datagram も捨てるので、
    /// ここで数えてから捨てる（ADR 0057 §4）。QUIC は損失として再送する。
    fn poll_send(
        &self,
        _cx: &mut Context,
        dst: &CustomAddr,
        _src: Option<&CustomAddr>,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        let segment = transmit
            .segment_size
            .unwrap_or(transmit.contents.len())
            .max(1);
        let Some(out) = self.open_session(dst) else {
            for _ in transmit.contents.chunks(segment) {
                self.shared.note_dropped_send();
            }
            return Poll::Ready(Ok(()));
        };
        for datagram in transmit.contents.chunks(segment) {
            self.send_datagram(&out, datagram);
        }
        Poll::Ready(Ok(()))
    }
}

impl WebRtcSender {
    /// 1 datagram を session の送信の待ちへ入れる。入らなければ数えて捨てる。
    fn send_datagram(&self, out: &mpsc::Sender<Bytes>, datagram: &[u8]) {
        if datagram.len() > MAX_DATAGRAM_BYTES
            || out.try_send(Bytes::copy_from_slice(datagram)).is_err()
        {
            self.shared.note_dropped_send();
        }
    }
}

#[cfg(all(test, target_family = "wasm"))]
mod browser_tests;
#[cfg(feature = "test-signaling")]
pub mod signaling_fixture;
#[cfg(test)]
mod test_support;
#[cfg(all(test, not(target_family = "wasm")))]
mod tests;
