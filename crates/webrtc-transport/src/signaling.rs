//! 専用 ALPN での接続交渉（ADR 0057 §7・§9、#1422 W10）。
//!
//! 既存の iroh の接続（relay 等）の上で、1 本の bi stream に要求（版・宛先・session id・offer の SDP）と
//! 応答（answer の SDP か拒否の理由）を 1 往復させる。DataChannel が開いたら、両端で相手への接続へ
//! custom path を足す（`Endpoint::add_remote_addrs`）。session は QUIC の TLS で認証された相手の
//! EndpointId・session id・世代へ束縛し、要求の中の送り手の値は使わない。
//!
//! 交渉を始めるのは `DemandHooks` を渡した端（IP の transport を持たないブラウザ）だけで、相手との iroh の
//! 接続（既存の需要の owner が張る）がある間だけ試す。session は相手ごとに 1 本で、需要の接続がすべて
//! 閉じたら閉じる。

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt,
    future::Future,
    sync::{Arc, Mutex, MutexGuard, OnceLock, Weak},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayUrl, TransportAddr,
    endpoint::{AfterHandshakeOutcome, Connection, EndpointHooks, WeakConnectionHandle},
    protocol::{AcceptError, ProtocolHandler},
};
use iroh_base::CustomAddr;
use tokio::sync::broadcast;

use crate::{MAX_SDP_BYTES, MAX_SESSIONS, SessionEvent, SessionId, WebRtcTransport};

/// 接続交渉の ALPN。版は要求の先頭の 1 byte でも確かめる。
pub const SIGNALING_ALPN: &[u8] = b"/kukuri/webrtc-signal/1";
/// 両方向あわせて同時に進める交渉の上限。
pub const MAX_NEGOTIATIONS: usize = 4;
/// 交渉の開始から DataChannel が開くまでの期限。失敗・喪失の後に次を試すまでの間隔にも使う。
pub const NEGOTIATION_DEADLINE: Duration = Duration::from_secs(15);
/// 1 回の需要の間に、相手ごとに交渉を試す回数。
pub const MAX_ATTEMPTS: u32 = 3;
/// STUN の宛先の port。relay と同じ host で Community Node の基盤が動かす（ADR 0057 §6）。
pub const STUN_PORT: u16 = 3478;

const VERSION: u8 = 1;
const REQUEST_HEADER_BYTES: usize = 1 + 32 + 16;
const ANSWER: u8 = 0;
const REJECT: u8 = 1;

/// 相手が交渉を断った理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// 版が違う。
    Version,
    /// 宛先が相手自身でない。
    WrongPeer,
    /// 同時の交渉が上限に達している。
    Busy,
    /// 同時に始めた（または開いた session がある）ので、相手との既存の session を使う。
    Glare,
    /// offer の SDP が上限・形式を満たさない。
    InvalidOffer,
    /// 世代が終わった。
    Closed,
}

impl Rejection {
    fn code(self) -> u8 {
        self as u8 + 1
    }

    fn from_code(code: u8) -> Option<Self> {
        [
            Self::Version,
            Self::WrongPeer,
            Self::Busy,
            Self::Glare,
            Self::InvalidOffer,
            Self::Closed,
        ]
        .into_iter()
        .find(|rejection| rejection.code() == code)
    }
}

/// 交渉が断られた（`anyhow::Error` から `downcast_ref` で取り出せる）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejected(pub Rejection);

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "webrtc negotiation rejected: {:?}", self.0)
    }
}

impl std::error::Error for Rejected {}

/// 診断と試験のための数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SignalingStats {
    /// 需要の表にいる相手の数。
    pub demand_peers: usize,
    /// 需要から始めた交渉の延べ数。
    pub attempts: u64,
}

/// 相手への需要。
#[derive(Default)]
struct Demand {
    /// 生きた iroh の接続の数。
    live: usize,
    /// この需要を駆動する task の印（0 は無し）。試行を使い切った後も、需要が終わるか再開まで残す。
    driver: u64,
}

#[derive(Default)]
struct State {
    generation: u64,
    /// 進行中の交渉の数（両方向）。
    active: usize,
    /// 自分が始めた交渉の相手（相手ごとに 1 つ）。
    outgoing: HashSet<EndpointId>,
    /// この交渉が作った session と、その相手・世代・開いた custom のアドレス。閉じたら消す。
    sessions: HashMap<SessionId, (EndpointId, u64, Option<CustomAddr>)>,
    /// 交渉を始める端の、相手ごとの需要（`MAX_SESSIONS` 件まで）。
    demand: HashMap<EndpointId, Demand>,
    attempts: u64,
    drivers: u64,
}

impl State {
    /// 今の世代の、相手との session。
    fn session_with(&self, remote: EndpointId) -> Option<SessionId> {
        self.sessions
            .iter()
            .find(|(_, (peer, opened_in, _))| *peer == remote && *opened_in == self.generation)
            .map(|(session, _)| *session)
    }

    /// 今の世代の、相手との開いた session。
    fn open_session_with(&self, remote: EndpointId) -> Option<SessionId> {
        self.sessions
            .iter()
            .find(|(_, (peer, opened_in, addr))| {
                *peer == remote && *opened_in == self.generation && addr.is_some()
            })
            .map(|(session, _)| *session)
    }

    /// 需要を駆動する新しい印を付けて返す。
    fn new_driver(&mut self, remote: EndpointId) -> Option<u64> {
        self.drivers += 1;
        let driver = self.drivers;
        self.demand.get_mut(&remote)?.driver = driver;
        Some(driver)
    }
}

/// session の開閉（開いたら custom のアドレス、閉じたら `None`）。
type Change = (SessionId, EndpointId, Option<CustomAddr>);

/// 需要の接続を交渉へ渡す Endpoint の hook。同じ値を `Builder::hooks` と `Signaling::new` へ渡す。
#[derive(Clone, Debug, Default)]
pub struct DemandHooks(Arc<OnceLock<Weak<Signaling>>>);

impl EndpointHooks for DemandHooks {
    fn after_handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> impl Future<Output = AfterHandshakeOutcome> + Send + 'a {
        if connection.alpn() != SIGNALING_ALPN
            && let Some(signaling) = self.0.get().and_then(Weak::upgrade)
        {
            tracing::debug!("KD hook {} alpn={}", connection.remote_id().fmt_short(), String::from_utf8_lossy(connection.alpn()));
            signaling.demand(connection.remote_id(), connection.weak_handle());
        }
        async { AfterHandshakeOutcome::accept() }
    }
}

/// 接続交渉の owner。`Router` へ `SIGNALING_ALPN` で登録する。
pub struct Signaling {
    this: Weak<Signaling>,
    transport: Arc<WebRtcTransport>,
    endpoint: Endpoint,
    deadline: Duration,
    state: Mutex<State>,
    changes: broadcast::Sender<Change>,
}

impl fmt::Debug for Signaling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signaling").finish_non_exhaustive()
    }
}

/// 交渉中の枠。drop で返す。
struct Slot<'a>(&'a Signaling, Option<EndpointId>);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.release(self.1);
    }
}

/// STUN の送信先。Endpoint が今使っている relay の host の `STUN_PORT`（ADR 0057 §6）。relay が無ければ送らない。
fn stun_servers<'a>(relays: impl IntoIterator<Item = &'a RelayUrl>) -> Vec<String> {
    relays
        .into_iter()
        .filter_map(|url| url.host_str().map(|host| format!("{host}:{STUN_PORT}")))
        .collect()
}

impl Signaling {
    /// `transport` の event を受け取り、`endpoint`（`transport` を登録した Endpoint）へ custom path を足す。
    /// `demand`（`endpoint` に登録した hook）を渡すと、需要の接続のある相手と交渉を始める。
    pub fn new(
        transport: Arc<WebRtcTransport>,
        endpoint: Endpoint,
        demand: Option<&DemandHooks>,
        deadline: Duration,
    ) -> Result<Arc<Self>> {
        let mut events = transport
            .take_events()
            .context("the transport events are already taken")?;
        let (changes, _) = broadcast::channel(crate::EVENT_QUEUE);
        let signaling = Arc::new_cyclic(|this| Self {
            this: this.clone(),
            transport,
            endpoint,
            deadline,
            state: Mutex::new(State::default()),
            changes,
        });
        if let Some(hooks) = demand {
            let _ = hooks.0.set(Arc::downgrade(&signaling));
        }
        tracing::debug!("KD me {}", signaling.endpoint.id().fmt_short());
        let weak = Arc::downgrade(&signaling);
        n0_future::task::spawn(async move {
            while let Some(event) = events.recv().await {
                let Some(signaling) = Weak::upgrade(&weak) else {
                    break;
                };
                signaling.on_event(event).await;
            }
        });
        Ok(signaling)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn stats(&self) -> SignalingStats {
        let state = self.state();
        SignalingStats {
            demand_peers: state.demand.len(),
            attempts: state.attempts,
        }
    }

    async fn add_path(&self, remote: EndpointId, addr: CustomAddr) {
        self.endpoint
            .add_remote_addrs(remote, BTreeSet::from([TransportAddr::Custom(addr)]))
            .await;
    }

    async fn on_event(&self, event: SessionEvent) {
        match event {
            SessionEvent::Opened {
                session,
                remote,
                addr,
            } => {
                let current = {
                    let mut state = self.state();
                    let generation = state.generation;
                    match state.sessions.get_mut(&session) {
                        Some((peer, opened_in, open))
                            if *peer == remote && *opened_in == generation =>
                        {
                            *open = Some(addr.clone());
                            true
                        }
                        _ => false,
                    }
                };
                tracing::debug!("KD opened {} s={} current={current}", remote.fmt_short(), kd(&session));
                if current {
                    self.add_path(remote, addr.clone()).await;
                    let _ = self.changes.send((session, remote, Some(addr)));
                }
            }
            SessionEvent::Closed { session, .. } => {
                let removed = self.state().sessions.remove(&session);
                tracing::debug!("KD closed s={} known={:?}", kd(&session), removed.as_ref().map(|(r, ..)| r.fmt_short().to_string()));
                if let Some((remote, ..)) = removed {
                    let _ = self.changes.send((session, remote, None));
                }
            }
        }
    }

    /// 世代を終え、この交渉が作った session をすべて閉じる（pagehide・freeze・offline・account の切替・停止）。
    /// 古い世代の応答では session を作らない。
    pub fn reset(&self) {
        tracing::debug!("KD reset");
        let sessions: Vec<_> = {
            let mut state = self.state();
            state.generation += 1;
            for demand in state.demand.values_mut() {
                demand.driver = 0;
            }
            state.sessions.drain().collect()
        };
        for (session, (remote, ..)) in sessions {
            self.transport.close(session);
            let _ = self.changes.send((session, remote, None));
        }
    }

    /// 生きた需要の接続が残り、今の世代の session が無い相手とだけ交渉し直す（可視・online・pageshow・resume）。
    /// 開いている session は閉じない（閉じると、交渉し直すまで relay の経路になるため）。
    pub fn resume(&self) {
        tracing::debug!("KD resume");
        let drivers: Vec<_> = {
            let mut state = self.state();
            let peers: Vec<_> = state
                .demand
                .keys()
                .copied()
                .filter(|peer| state.session_with(*peer).is_none())
                .collect();
            peers
                .into_iter()
                .filter_map(|peer| Some((peer, state.new_driver(peer)?)))
                .collect()
        };
        for (peer, driver) in drivers {
            self.drive(peer, driver);
        }
    }

    /// 相手との iroh の接続ができた（`DemandHooks`）。接続が閉じるまで需要に数える。
    fn demand(&self, remote: EndpointId, connection: WeakConnectionHandle) {
        let closed = connection.closed();
        let (driver, open) = {
            let mut state = self.state();
            // 表が満杯なら載せず、既存の経路を使う。
            if !state.demand.contains_key(&remote) && state.demand.len() >= MAX_SESSIONS {
                return;
            }
            let open = state
                .open_session_with(remote)
                .and_then(|session| state.sessions[&session].2.clone());
            let demand = state.demand.entry(remote).or_default();
            demand.live += 1;
            let start = demand.driver == 0;
            tracing::debug!("KD demand {} live={} start={start} open={}", remote.fmt_short(), demand.live, open.is_some());
            (start.then(|| state.new_driver(remote)).flatten(), open)
        };
        let Some(this) = self.this.upgrade() else {
            return;
        };
        let ended = this.clone();
        n0_future::task::spawn(async move {
            closed.await;
            ended.demand_ended(remote);
        });
        // 開いている session の custom path を、新しい接続にも足す。
        if let Some(addr) = open {
            let this = this.clone();
            n0_future::task::spawn(async move { this.add_path(remote, addr).await });
        }
        if let Some(driver) = driver {
            this.drive(remote, driver);
        }
    }

    /// 需要の接続が 1 本閉じた。最後の 1 本なら表から消し、相手との session を閉じる。
    fn demand_ended(&self, remote: EndpointId) {
        let sessions: Vec<_> = {
            let mut state = self.state();
            let Some(demand) = state.demand.get_mut(&remote) else {
                return;
            };
            demand.live -= 1;
            tracing::debug!("KD demand_ended {} live={}", remote.fmt_short(), demand.live);
            if demand.live > 0 {
                return;
            }
            state.demand.remove(&remote);
            state
                .sessions
                .iter()
                .filter(|(_, (peer, ..))| *peer == remote)
                .map(|(session, _)| *session)
                .collect()
        };
        for session in sessions {
            tracing::debug!("KD demand_ended closes {} s={}", remote.fmt_short(), kd(&session));
            self.transport.close(session);
        }
    }

    fn drive(&self, remote: EndpointId, driver: u64) {
        if let Some(this) = self.this.upgrade() {
            n0_future::task::spawn(async move { this.run_demand(remote, driver).await });
        }
    }

    /// 需要が続き、この task が駆動役の間、相手との交渉を `MAX_ATTEMPTS` 回まで試す。相手との session
    /// （相手が始めたものを含む）があれば、閉じるまで待つ。閉じた・失敗した後は期限の分だけ待つ。
    async fn run_demand(&self, remote: EndpointId, driver: u64) {
        let mut attempts = 0;
        loop {
            let existing = {
                let mut state = self.state();
                if state
                    .demand
                    .get(&remote)
                    .is_none_or(|demand| demand.driver != driver)
                {
                    return;
                }
                let existing = state.session_with(remote);
                if existing.is_none() {
                    if attempts == MAX_ATTEMPTS {
                        return;
                    }
                    attempts += 1;
                    state.attempts += 1;
                }
                existing
            };
            tracing::debug!("KD run_demand {} existing={:?} attempts={attempts}", remote.fmt_short(), existing.as_ref().map(kd));
            let session = match existing {
                Some(session) => Some(session),
                None => match self.connect(self.route(remote).await).await {
                    Ok(session) => Some(session),
                    Err(error) => {
                        tracing::debug!("KD connect failed {} {error:#}", remote.fmt_short());
                        None
                    }
                },
            };
            if let Some(session) = session {
                // 交渉の途中で需要が終わっていたら（登録の前で `demand_ended` が閉じられない）、ここで閉じる。
                if !self.state().demand.contains_key(&remote) {
                    tracing::debug!("KD run_demand closes after demand ended {} s={}", remote.fmt_short(), kd(&session));
                    self.transport.close(session);
                    return;
                }
                self.closed(session).await;
            }
            n0_future::time::sleep(self.deadline).await;
        }
    }

    /// 相手への既存の経路（relay・IP）。custom のアドレスは使わない。
    async fn route(&self, remote: EndpointId) -> EndpointAddr {
        let addrs = self
            .endpoint
            .remote_info(remote)
            .await
            .into_iter()
            .flat_map(|info| info.into_addrs())
            .map(|addr| addr.into_addr())
            .filter(|addr| !addr.is_custom());
        EndpointAddr::from_parts(remote, addrs)
    }

    /// `session` が閉じるまで待つ。
    async fn closed(&self, session: SessionId) {
        let mut changes = self.changes.subscribe();
        while self.state().sessions.contains_key(&session) {
            if let Err(broadcast::error::RecvError::Closed) = changes.recv().await {
                return;
            }
        }
    }

    /// 交渉の枠を取り、今の世代を返す。`outgoing` は自分が始める交渉の相手。
    fn reserve(&self, outgoing: Option<EndpointId>) -> Result<(Slot<'_>, u64)> {
        let mut state = self.state();
        if state.active >= MAX_NEGOTIATIONS
            || outgoing.is_some_and(|remote| state.outgoing.contains(&remote))
        {
            bail!(Rejected(Rejection::Busy));
        }
        state.active += 1;
        if let Some(remote) = outgoing {
            state.outgoing.insert(remote);
        }
        Ok((Slot(self, outgoing), state.generation))
    }

    fn release(&self, outgoing: Option<EndpointId>) {
        let mut state = self.state();
        state.active -= 1;
        if let Some(remote) = outgoing {
            state.outgoing.remove(&remote);
        }
    }

    /// session を世代へ登録する。世代が終わっていたら閉じる。
    fn track(&self, session: SessionId, remote: EndpointId, generation: u64) -> Result<()> {
        let mut state = self.state();
        if state.generation != generation {
            drop(state);
            self.transport.close(session);
            bail!(Rejected(Rejection::Closed));
        }
        state.sessions.insert(session, (remote, generation, None));
        Ok(())
    }

    /// `session`（無ければ `remote` の任意の session）が開くまで待つ。期限は呼び出し側。
    async fn opened(
        mut changes: broadcast::Receiver<Change>,
        remote: EndpointId,
        session: Option<SessionId>,
    ) -> Result<SessionId> {
        loop {
            match changes.recv().await {
                Ok((id, peer, Some(_))) if peer == remote && session.is_none_or(|s| s == id) => {
                    return Ok(id);
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => bail!("signaling stopped"),
            }
        }
    }

    /// `remote` との交渉を始め、DataChannel が開いたら、その session を返す。
    /// `remote` は既存の経路（relay 等）で届く宛先。失敗・期限切れでは session を残さない。
    pub(crate) async fn connect(&self, remote: EndpointAddr) -> Result<SessionId> {
        let peer = remote.id;
        let (_slot, generation) = self.reserve(Some(peer))?;
        let opened = self.changes.subscribe();
        let mut session = None;
        let result = n0_future::time::timeout(self.deadline, async {
            let (id, offer) = self
                .transport
                .offer(peer, &stun_servers(self.endpoint.addr().relay_urls()))
                .await?;
            session = Some(id);
            self.track(id, peer, generation)?;
            let response = self.exchange(remote, id, &offer).await?;
            tracing::debug!("KD connect {} s={} response={:?}", peer.fmt_short(), kd(&id), response.as_ref().map(|_| "answer"));
            match response {
                Ok(answer) => {
                    self.transport.accept_answer(id, &answer).await?;
                    Self::opened(opened, peer, Some(id)).await
                }
                // 相手が始めた交渉の session（開いていればそれ）を使う。
                Err(Rejection::Glare) => {
                    self.transport.close(id);
                    session = None;
                    let open = self.state().open_session_with(peer);
                    match open {
                        Some(open) => Ok(open),
                        None => Self::opened(opened, peer, None).await,
                    }
                }
                Err(rejection) => bail!(Rejected(rejection)),
            }
        })
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("webrtc negotiation timed out")));
        if result.is_err()
            && let Some(id) = session
        {
            self.state().sessions.remove(&id);
            self.transport.close(id);
        }
        result
    }

    async fn exchange(
        &self,
        remote: EndpointAddr,
        session: SessionId,
        offer: &str,
    ) -> Result<std::result::Result<String, Rejection>> {
        let to = remote.id;
        tracing::debug!("KD exchange dial {} addrs={:?}", to.fmt_short(), remote.addrs.iter().map(|addr| format!("{addr:?}").chars().take(60).collect::<String>()).collect::<Vec<_>>());
        let connection = self.endpoint.connect(remote, SIGNALING_ALPN).await?;
        tracing::debug!("KD exchange connected {} paths={:?}", to.fmt_short(), connection.paths().iter().map(|path| format!("{:?}", path.remote_addr()).chars().take(40).collect::<String>()).collect::<Vec<_>>());
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(&encode_request(to, session, offer)).await?;
        send.finish()?;
        let response = recv.read_to_end(1 + MAX_SDP_BYTES).await?;
        connection.close(0u32.into(), b"done");
        decode_response(&response)
    }

    /// 受けた要求に答える。session を作ったら、期限までに開かなければ閉じる（それまで枠を持つ）。
    async fn answer_request(&self, remote: EndpointId, request: &[u8]) -> Response {
        let (to, session, offer) = match decode_request(request) {
            Ok(request) => request,
            Err(rejection) => return Err(rejection),
        };
        if to != self.endpoint.id() {
            return Err(Rejection::WrongPeer);
        }
        // 開いた session があれば、それを使ってもらう（行き違いの交渉で 2 本目を作らない）。
        let (glare, open) = {
            let state = self.state();
            (
                state.outgoing.contains(&remote),
                state.open_session_with(remote).is_some(),
            )
        };
        tracing::debug!("KD answer_request {} s={} glare={glare} open={open} lower={}", remote.fmt_short(), kd(&session), self.endpoint.id() < remote);
        if open || (glare && self.endpoint.id() < remote) {
            return Err(Rejection::Glare);
        }
        let Ok((slot, generation)) = self.reserve(None) else {
            return Err(Rejection::Busy);
        };
        let Ok(answer) = self
            .transport
            .answer(
                remote,
                session,
                offer,
                &stun_servers(self.endpoint.addr().relay_urls()),
            )
            .await
        else {
            return Err(Rejection::InvalidOffer);
        };
        if self.track(session, remote, generation).is_err() {
            return Err(Rejection::Closed);
        }
        let opened = self.changes.subscribe();
        let Some(this) = self.this.upgrade() else {
            return Err(Rejection::Closed);
        };
        std::mem::forget(slot);
        n0_future::task::spawn(async move {
            let open = n0_future::time::timeout(
                this.deadline,
                Self::opened(opened, remote, Some(session)),
            )
            .await;
            if !matches!(open, Ok(Ok(_))) && this.state().sessions.remove(&session).is_some() {
                tracing::debug!("KD answer not opened {} s={}", remote.fmt_short(), kd(&session));
                this.transport.close(session);
            }
            this.release(None);
        });
        Ok(answer)
    }
}

type Response = std::result::Result<String, Rejection>;

fn kd(session: &SessionId) -> String {
    format!("{:02x}{:02x}", session.0[0], session.0[1])
}

impl ProtocolHandler for Signaling {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let remote = connection.remote_id();
        tracing::debug!("KD accept {}", remote.fmt_short());
        let (mut send, mut recv) = connection.accept_bi().await?;
        let request = recv
            .read_to_end(REQUEST_HEADER_BYTES + MAX_SDP_BYTES)
            .await
            .map_err(AcceptError::from_err)?;
        let response = self.answer_request(remote, &request).await;
        send.write_all(&encode_response(&response))
            .await
            .map_err(AcceptError::from_err)?;
        send.finish().map_err(AcceptError::from_err)?;
        let _ = n0_future::time::timeout(self.deadline, connection.closed()).await;
        Ok(())
    }
}

pub(crate) fn encode_request(to: EndpointId, session: SessionId, offer: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(REQUEST_HEADER_BYTES + offer.len());
    bytes.push(VERSION);
    bytes.extend_from_slice(to.as_bytes());
    bytes.extend_from_slice(session.as_bytes());
    bytes.extend_from_slice(offer.as_bytes());
    bytes
}

fn decode_request(bytes: &[u8]) -> std::result::Result<(EndpointId, SessionId, &str), Rejection> {
    if bytes.first() != Some(&VERSION) {
        return Err(Rejection::Version);
    }
    let (Some(to), Some(session)) = (bytes.get(1..33), bytes.get(33..49)) else {
        return Err(Rejection::InvalidOffer);
    };
    let to = <[u8; 32]>::try_from(to)
        .ok()
        .and_then(|to| EndpointId::from_bytes(&to).ok())
        .ok_or(Rejection::InvalidOffer)?;
    let session = SessionId::from_bytes(session.try_into().map_err(|_| Rejection::InvalidOffer)?);
    let offer =
        std::str::from_utf8(&bytes[REQUEST_HEADER_BYTES..]).map_err(|_| Rejection::InvalidOffer)?;
    Ok((to, session, offer))
}

fn encode_response(response: &Response) -> Vec<u8> {
    match response {
        Ok(answer) => [&[ANSWER], answer.as_bytes()].concat(),
        Err(rejection) => vec![REJECT, rejection.code()],
    }
}

fn decode_response(bytes: &[u8]) -> Result<Response> {
    match bytes.split_first() {
        Some((&ANSWER, answer)) => Ok(Ok(std::str::from_utf8(answer)?.to_string())),
        Some((&REJECT, [code])) => Rejection::from_code(*code)
            .map(Err)
            .context("unknown rejection"),
        _ => bail!("invalid signaling response"),
    }
}

#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
mod tests;
