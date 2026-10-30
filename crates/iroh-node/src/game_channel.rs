//! SDK のゲームの action session の送受信(ADR 0070 §3・§5.2、#1979 AC-1)。認可された session の接続に、最新の
//! 状態を運ぶ unreliable datagram と、確定させる操作を順に全部運ぶ reliable stream を載せる。送り待ちと受け取った
//! packet は session ごとの上限つきの待ち行列に置き、payload の本文は log に出さない。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};

use iroh::endpoint::Connection;
use moq_net::bytes::Bytes;
use n0_future::task::{AbortOnDropHandle, spawn};
use tokio::sync::Notify;

/// datagram の payload の上限。さらに接続の `max_datagram_size` 以下。
pub const GAME_DATAGRAM_MAX_BYTES: usize = 1100;
/// stream の message 1 件の上限。
pub const GAME_STREAM_MESSAGE_MAX_BYTES: usize = 64 * 1024;
/// 送り待ち・受け取った datagram の上限(溢れたら古いものから捨てる)。
const QUEUED_DATAGRAMS: usize = 64;
/// 送り待ち・受け取った stream の message の合計の上限。
const QUEUED_STREAM_BYTES: usize = 256 * 1024;
const CLOSE_PROTOCOL: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameDelivery {
    Datagram,
    Stream,
}

impl GameDelivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Datagram => "datagram",
            Self::Stream => "stream",
        }
    }
}

/// 送らなかった理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameSendRefused {
    /// 上限を超える大きさ。
    TooLarge,
    /// stream の送り待ちが上限に達した。
    Backpressure,
    /// session が閉じている。
    Closed,
}

impl std::fmt::Display for GameSendRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "the game packet was not sent: {self:?}")
    }
}

impl std::error::Error for GameSendRefused {}

#[derive(Debug, PartialEq, Eq)]
pub struct GamePacket {
    pub delivery: GameDelivery,
    pub bytes: Bytes,
}

/// 実データの経路(Direct P2P / Relay Supported P2P / Relay Fallback)と RTT、接続の送受信量。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSessionPath {
    pub path: &'static str,
    pub rtt_ms: u64,
    pub sent_bytes: u64,
    pub received_bytes: u64,
}

#[derive(Default)]
struct Queues {
    out_datagrams: VecDeque<Bytes>,
    out_stream: VecDeque<Bytes>,
    /// 送り待ちと、QUIC へ書いている途中の message の合計。書き終えるまで数え、flow control の待ちを送り手へ返す。
    out_stream_bytes: usize,
    in_datagrams: VecDeque<Bytes>,
    in_stream: VecDeque<Bytes>,
    in_stream_bytes: usize,
}

#[derive(Default)]
struct Shared {
    queues: StdMutex<Queues>,
    datagram_ready: Notify,
    stream_ready: Notify,
    /// 受け取った stream の message が取り出されて空いた。
    stream_space: Notify,
}

impl Shared {
    fn queues(&self) -> std::sync::MutexGuard<'_, Queues> {
        self.queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 1 つの session の送受信。落とすと送受信の task を止める(接続は閉じない)。
pub struct GameSessionChannel {
    connection: Connection,
    shared: Arc<Shared>,
    /// [`Self::close`] で閉じた(host が参加者を外した)。
    closed_here: std::sync::atomic::AtomicBool,
    _tasks: [AbortOnDropHandle<()>; 4],
}

impl GameSessionChannel {
    /// 認可を終えた接続に送受信を載せる。stream は向きごとに 1 本の単方向 stream(長さ 4 bytes の前置き)。
    pub(crate) fn new(connection: Connection) -> Self {
        let shared = Arc::new(Shared::default());
        let (c, s) = (&connection, &shared);
        let _tasks = [
            AbortOnDropHandle::new(spawn(send_datagrams(c.clone(), s.clone()))),
            AbortOnDropHandle::new(spawn(send_stream(c.clone(), s.clone()))),
            AbortOnDropHandle::new(spawn(receive_datagrams(c.clone(), s.clone()))),
            AbortOnDropHandle::new(spawn(receive_stream(c.clone(), s.clone()))),
        ];
        Self {
            connection,
            shared,
            closed_here: Default::default(),
            _tasks,
        }
    }

    pub fn is_open(&self) -> bool {
        self.connection.close_reason().is_none()
    }

    pub(crate) fn closed_here(&self) -> bool {
        self.closed_here.load(std::sync::atomic::Ordering::Acquire)
    }

    /// 理由の code を付けて session を閉じる(相手はその code を受け取る)。
    pub fn close(&self, reason: &str) {
        self.closed_here
            .store(true, std::sync::atomic::Ordering::Release);
        self.connection
            .close(crate::game_session::CLOSE_ENDED.into(), reason.as_bytes());
    }

    /// 送り待ちに置いて即座に戻る。datagram は送り待ちが溢れたら古いものを捨てる。
    pub fn send(&self, delivery: GameDelivery, bytes: &[u8]) -> Result<(), GameSendRefused> {
        if !self.is_open() {
            return Err(GameSendRefused::Closed);
        }
        let mut queues = self.shared.queues();
        match delivery {
            GameDelivery::Datagram => {
                let limit = self
                    .connection
                    .max_datagram_size()
                    .unwrap_or(0)
                    .min(GAME_DATAGRAM_MAX_BYTES);
                if bytes.len() > limit {
                    return Err(GameSendRefused::TooLarge);
                }
                if queues.out_datagrams.len() >= QUEUED_DATAGRAMS {
                    queues.out_datagrams.pop_front();
                }
                queues
                    .out_datagrams
                    .push_back(Bytes::copy_from_slice(bytes));
                drop(queues);
                self.shared.datagram_ready.notify_one();
            }
            GameDelivery::Stream => {
                if bytes.len() > GAME_STREAM_MESSAGE_MAX_BYTES {
                    return Err(GameSendRefused::TooLarge);
                }
                if queues.out_stream_bytes + bytes.len() > QUEUED_STREAM_BYTES {
                    return Err(GameSendRefused::Backpressure);
                }
                queues.out_stream_bytes += bytes.len();
                queues.out_stream.push_back(Bytes::copy_from_slice(bytes));
                drop(queues);
                self.shared.stream_ready.notify_one();
            }
        }
        Ok(())
    }

    /// 受け取った packet を、`fits` が大きさを許す間だけ取り出す(stream の message を先に、届いた順に)。
    /// `accept` が false なら(呼出側の未解放の bytes が上限に達した)、datagram は捨て、stream は読み取りを止めたままにする。
    pub fn take(&self, accept: bool, mut fits: impl FnMut(usize) -> bool) -> Vec<GamePacket> {
        let mut queues = self.shared.queues();
        if !accept {
            queues.in_datagrams.clear();
            return Vec::new();
        }
        let mut packets = Vec::new();
        while let Some(message) = queues.in_stream.front()
            && fits(message.len())
        {
            let bytes = queues.in_stream.pop_front().expect("front exists");
            queues.in_stream_bytes -= bytes.len();
            packets.push(GamePacket {
                delivery: GameDelivery::Stream,
                bytes,
            });
        }
        let freed = !packets.is_empty();
        while let Some(datagram) = queues.in_datagrams.front()
            && fits(datagram.len())
        {
            let bytes = queues.in_datagrams.pop_front().expect("front exists");
            packets.push(GamePacket {
                delivery: GameDelivery::Datagram,
                bytes,
            });
        }
        drop(queues);
        if freed {
            self.shared.stream_space.notify_one();
        }
        packets
    }

    /// 今の経路(選ばれた経路が無い間は None)。選ばれた経路が relay なら Relay Fallback、IP の経路で relay の経路も
    /// 開いていれば(relay で接続を助けた)Relay Supported P2P、IP の経路だけなら Direct P2P。
    pub fn path(&self) -> Option<GameSessionPath> {
        let paths = self.connection.paths();
        let selected = paths.iter().find(|path| path.is_selected())?;
        let relay_open = paths.iter().any(|path| path.is_relay());
        let path = if selected.is_relay() {
            "relay_fallback"
        } else if relay_open {
            "relay_supported_p2p"
        } else {
            "direct_p2p"
        };
        let rtt_ms = selected.rtt().as_millis() as u64;
        let stats = self.connection.stats();
        Some(GameSessionPath {
            path,
            rtt_ms,
            sent_bytes: stats.udp_tx.bytes,
            received_bytes: stats.udp_rx.bytes,
        })
    }
}

async fn send_datagrams(connection: Connection, shared: Arc<Shared>) {
    loop {
        let ready = shared.datagram_ready.notified();
        let next = shared.queues().out_datagrams.pop_front();
        match next {
            Some(datagram) => {
                if connection.send_datagram_wait(datagram).await.is_err() {
                    return;
                }
            }
            None => ready.await,
        }
    }
}

async fn send_stream(connection: Connection, shared: Arc<Shared>) {
    let mut stream = None;
    loop {
        let ready = shared.stream_ready.notified();
        let next = shared.queues().out_stream.pop_front();
        let Some(message) = next else {
            ready.await;
            continue;
        };
        if stream.is_none() {
            stream = connection.open_uni().await.ok();
        }
        let Some(send) = stream.as_mut() else { return };
        let header = (message.len() as u32).to_be_bytes();
        if send.write_all(&header).await.is_err() || send.write_all(&message).await.is_err() {
            return;
        }
        shared.queues().out_stream_bytes -= message.len();
    }
}

async fn receive_datagrams(connection: Connection, shared: Arc<Shared>) {
    while let Ok(datagram) = connection.read_datagram().await {
        let mut queues = shared.queues();
        if queues.in_datagrams.len() >= QUEUED_DATAGRAMS {
            queues.in_datagrams.pop_front();
        }
        queues.in_datagrams.push_back(datagram);
    }
}

/// 受け取った message が上限に達したら読み取りを止め、QUIC の flow control で送り手を待たせる。
async fn receive_stream(connection: Connection, shared: Arc<Shared>) {
    let Ok(mut recv) = connection.accept_uni().await else {
        return;
    };
    loop {
        let mut header = [0; 4];
        if recv.read_exact(&mut header).await.is_err() {
            return;
        }
        let len = u32::from_be_bytes(header) as usize;
        if len > GAME_STREAM_MESSAGE_MAX_BYTES {
            connection.close(CLOSE_PROTOCOL.into(), b"oversized message");
            return;
        }
        let mut message = vec![0; len];
        if recv.read_exact(&mut message).await.is_err() {
            return;
        }
        loop {
            let space = shared.stream_space.notified();
            {
                let mut queues = shared.queues();
                if queues.in_stream.is_empty()
                    || queues.in_stream_bytes + len <= QUEUED_STREAM_BYTES
                {
                    queues.in_stream_bytes += len;
                    queues.in_stream.push_back(message.into());
                    break;
                }
            }
            space.await;
        }
    }
}
