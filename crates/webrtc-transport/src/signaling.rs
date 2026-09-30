//! 専用 ALPN での接続交渉（ADR 0057 §7、#1422 W10 AC-1）。
//!
//! 既存の iroh の接続（relay 等）の上で、1 本の bi stream に要求（版・宛先・session id・offer の SDP）と
//! 応答（answer の SDP か拒否の理由）を 1 往復させる。DataChannel が開いたら、両端で相手への接続へ
//! custom path を足す（`Endpoint::add_remote_addrs`）。session は QUIC の TLS で認証された相手の
//! EndpointId・session id・世代へ束縛し、要求の中の送り手の値は使わない。

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, TransportAddr,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use iroh_base::CustomAddr;
use tokio::sync::broadcast;

use crate::{MAX_SDP_BYTES, SessionEvent, SessionId, WebRtcTransport};

/// 接続交渉の ALPN。版は要求の先頭の 1 byte でも確かめる。
pub const SIGNALING_ALPN: &[u8] = b"/kukuri/webrtc-signal/1";
/// 両方向あわせて同時に進める交渉の上限。
pub const MAX_NEGOTIATIONS: usize = 4;
/// 交渉の開始から DataChannel が開くまでの期限。
pub const NEGOTIATION_DEADLINE: Duration = Duration::from_secs(15);

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
    /// 同時に始めたので、相手が始めた交渉を使う。
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

#[derive(Default)]
struct State {
    generation: u64,
    /// 進行中の交渉の数（両方向）。
    active: usize,
    /// 自分が始めた交渉の相手（相手ごとに 1 つ）。
    outgoing: HashSet<EndpointId>,
    /// この交渉が作った session と、その相手・世代。閉じたら消す。
    sessions: HashMap<SessionId, (EndpointId, u64)>,
}

/// 接続交渉の owner。`Router` へ `SIGNALING_ALPN` で登録し、`connect` で交渉を始める。
pub struct Signaling {
    this: Weak<Signaling>,
    transport: Arc<WebRtcTransport>,
    endpoint: Endpoint,
    deadline: Duration,
    state: Mutex<State>,
    /// DataChannel が開いて custom path を足した session。
    opened: broadcast::Sender<(SessionId, EndpointId, CustomAddr)>,
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

impl Signaling {
    /// `transport` の event を受け取り、`endpoint`（`transport` を登録した Endpoint）へ custom path を足す。
    pub fn new(
        transport: Arc<WebRtcTransport>,
        endpoint: Endpoint,
        deadline: Duration,
    ) -> Result<Arc<Self>> {
        let mut events = transport
            .take_events()
            .context("the transport events are already taken")?;
        let (opened, _) = broadcast::channel(crate::EVENT_QUEUE);
        let signaling = Arc::new_cyclic(|this| Self {
            this: this.clone(),
            transport,
            endpoint,
            deadline,
            state: Mutex::new(State::default()),
            opened,
        });
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

    async fn on_event(&self, event: SessionEvent) {
        match event {
            SessionEvent::Opened {
                session,
                remote,
                addr,
            } => {
                let current = {
                    let state = self.state();
                    state.sessions.get(&session) == Some(&(remote, state.generation))
                };
                if current {
                    self.endpoint
                        .add_remote_addrs(
                            remote,
                            BTreeSet::from([TransportAddr::Custom(addr.clone())]),
                        )
                        .await;
                    let _ = self.opened.send((session, remote, addr));
                }
            }
            SessionEvent::Closed { session, .. } => {
                self.state().sessions.remove(&session);
            }
        }
    }

    /// 世代を終え、この交渉が作った session をすべて閉じる（account の切替・停止・freeze）。
    /// 古い世代の応答では session を作らない。
    pub fn reset(&self) {
        let sessions: Vec<_> = {
            let mut state = self.state();
            state.generation += 1;
            state.sessions.drain().map(|(session, _)| session).collect()
        };
        for session in sessions {
            self.transport.close(session);
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
        state.sessions.insert(session, (remote, generation));
        Ok(())
    }

    /// `session`（無ければ `remote` の任意の session）が開くまで待つ。期限は呼び出し側。
    async fn opened(
        mut opened: broadcast::Receiver<(SessionId, EndpointId, CustomAddr)>,
        remote: EndpointId,
        session: Option<SessionId>,
    ) -> Result<CustomAddr> {
        loop {
            match opened.recv().await {
                Ok((id, peer, addr)) if peer == remote && session.is_none_or(|s| s == id) => {
                    return Ok(addr);
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => bail!("signaling stopped"),
            }
        }
    }

    /// `remote` との交渉を始め、DataChannel が開いたら、その custom のアドレスを返す。
    /// `remote` は既存の経路（relay 等）で届く宛先。失敗・期限切れでは session を残さない。
    pub async fn connect(&self, remote: EndpointAddr) -> Result<CustomAddr> {
        let peer = remote.id;
        let (_slot, generation) = self.reserve(Some(peer))?;
        let opened = self.opened.subscribe();
        let mut session = None;
        let result = n0_future::time::timeout(self.deadline, async {
            let (id, offer) = self.transport.offer(peer).await?;
            session = Some(id);
            self.track(id, peer, generation)?;
            match self.exchange(remote, id, &offer).await? {
                Ok(answer) => {
                    self.transport.accept_answer(id, &answer).await?;
                    Self::opened(opened, peer, Some(id)).await
                }
                // 相手が始めた交渉の session が開くのを待つ。
                Err(Rejection::Glare) => {
                    self.transport.close(id);
                    session = None;
                    Self::opened(opened, peer, None).await
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
        let connection = self.endpoint.connect(remote, SIGNALING_ALPN).await?;
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
        let glare = self.state().outgoing.contains(&remote);
        if glare && self.endpoint.id() < remote {
            return Err(Rejection::Glare);
        }
        let Ok((slot, generation)) = self.reserve(None) else {
            return Err(Rejection::Busy);
        };
        let Ok(answer) = self.transport.answer(remote, session, offer).await else {
            return Err(Rejection::InvalidOffer);
        };
        if self.track(session, remote, generation).is_err() {
            return Err(Rejection::Closed);
        }
        let opened = self.opened.subscribe();
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
                this.transport.close(session);
            }
            this.release(None);
        });
        Ok(answer)
    }
}

type Response = std::result::Result<String, Rejection>;

impl ProtocolHandler for Signaling {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let remote = connection.remote_id();
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
