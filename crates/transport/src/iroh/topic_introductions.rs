//! #1910: 読み先の近隣に、その topic でのその peer の近隣を紹介してもらう。直接の近隣が持たない公開の record を、
//! まだ知らない通常の peer（近隣の近隣）から既存の docs の読取りで読めるようにする。紹介は保持の証明ではない。
//!
//! 要求するのは公開 topic と公開スレッドの hint topic だけで、private channel・DM・account 同期の hint topic では
//! 求めず、求められても答えない。応答で渡すのは、購読している公開の hint topic の近隣の endpoint ID と到達情報
//! （gossip が参加者の間で既に交換する範囲）だけ。要求元がその topic の参加者かは確かめない（公開 topic は誰でも
//! 合流でき、合流すれば gossip で同じ情報を得る）。
use super::peer_state::BootstrapCandidates;
use super::*;
use iroh::TransportAddr;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use n0_future::task::AbortOnDropHandle;
use n0_future::time::Instant;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::ops::Bound::{Excluded, Unbounded};
use std::str::FromStr;

pub const TOPIC_PEERS_ALPN: &[u8] = b"/kukuri/topic-peers/1";
/// topic ごとに覚える、紹介で得た候補の上限。超えたら最も古い紹介を外す。
pub(crate) const MAX_INTRODUCED_PEERS: usize = 8;
/// 紹介で得た候補を覚える期間。再紹介で更新する。
pub(crate) const INTRODUCTION_TTL: Duration = Duration::from_secs(10 * 60);
/// 1 回の紹介で返す（受け取る）候補の上限。
pub(crate) const INTRODUCTION_PEERS: usize = 4;
/// topic ごとの紹介の要求の間隔。
pub(crate) const INTRODUCTION_INTERVAL: Duration = Duration::from_secs(60);
const INTRODUCTION_TIMEOUT: Duration = Duration::from_secs(5);
/// 1 endpoint が同時に答える紹介の数。超えた要求は待たせずに接続を閉じる。
const INTRODUCTION_CONCURRENT_REQUESTS: usize = 4;
const MAX_REQUEST_BYTES: usize = 256;
const MAX_RESPONSE_BYTES: usize = 4 * 1024;
const MAX_RELAY_URLS: usize = 4;
const MAX_DIRECT_ADDRS: usize = 8;
const WIRE_VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
struct IntroductionRequest {
    version: u8,
    topic: String,
}

#[derive(Serialize, Deserialize)]
struct IntroducedPeerWire {
    endpoint_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    relay_urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    direct_addrs: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct IntroductionResponse {
    peers: Vec<IntroducedPeerWire>,
}

/// 紹介を求め、答える hint topic（公開 topic と公開スレッド）。
pub(crate) fn introduces_topic_peers(hint_topic: &str) -> bool {
    use kukuri_core::wire::{
        ACCOUNT_SYNC_TOPIC_PREFIX, DM_TOPIC_PREFIX, HINT_TOPIC_PREFIX, PRIVATE_CHANNEL_TOPIC_PREFIX,
    };
    hint_topic
        .strip_prefix(HINT_TOPIC_PREFIX)
        .is_some_and(|topic| {
            ![
                PRIVATE_CHANNEL_TOPIC_PREFIX,
                DM_TOPIC_PREFIX,
                ACCOUNT_SYNC_TOPIC_PREFIX,
            ]
            .iter()
            .any(|prefix| topic.starts_with(prefix))
        })
}

/// 要求元を除いた近隣から、要求元の ID の次から順に最大 [`INTRODUCTION_PEERS`] 件（要求元ごとに違う窓）。
pub(crate) fn introduction_window(neighbors: &BTreeSet<String>, requester: &str) -> Vec<String> {
    neighbors
        .range::<str, _>((Excluded(requester), Unbounded))
        .chain(neighbors.range::<str, _>((Unbounded, Excluded(requester))))
        .take(INTRODUCTION_PEERS)
        .cloned()
        .collect()
}

/// 紹介で得た候補。
#[derive(Clone, Debug)]
pub(crate) struct IntroducedPeer {
    pub(crate) endpoint_id: String,
    pub(crate) addr_hint: Option<String>,
    expires_at: Instant,
}

/// topic ごとの、紹介で得た候補の台帳（古い順）。
#[derive(Default)]
pub(crate) struct IntroducedPeers(Vec<IntroducedPeer>);

impl IntroducedPeers {
    /// 失効した候補と近隣になった候補を外し、残りを返す。
    pub(crate) fn live(
        &mut self,
        neighbors: &BTreeSet<String>,
        now: Instant,
    ) -> Vec<IntroducedPeer> {
        self.0
            .retain(|peer| peer.expires_at > now && !neighbors.contains(&peer.endpoint_id));
        self.0.clone()
    }

    fn insert(&mut self, peer: IntroducedPeer) {
        self.0.retain(|known| known.endpoint_id != peer.endpoint_id);
        if self.0.len() >= MAX_INTRODUCED_PEERS {
            self.0.remove(0);
        }
        self.0.push(peer);
    }
}

#[derive(Default)]
struct IntroductionRun {
    last_at: Option<Instant>,
    /// 求めた回数(試験の観測用)。
    requests: u64,
    /// 進行中の要求。topic の購読の解除で落ちる。
    _in_flight: Option<AbortOnDropHandle<()>>,
}

/// 1 回の紹介の要求が使う部品（要求の task へ渡す）。
#[derive(Clone)]
struct IntroductionParts {
    endpoint: Endpoint,
    candidates: BootstrapCandidates,
    topic: String,
    neighbors: Arc<RwLock<BTreeSet<String>>>,
    introduced: Arc<Mutex<IntroducedPeers>>,
}

impl IntroductionParts {
    async fn request_from(&self, peer: EndpointId) {
        let request = request_introductions(&self.endpoint, EndpointAddr::new(peer), &self.topic);
        let peers = match timeout(INTRODUCTION_TIMEOUT, request).await {
            Ok(Ok(peers)) => peers,
            Ok(Err(error)) => {
                debug!(topic = %self.topic, %peer, %error, "topic peer introduction failed");
                return;
            }
            Err(_) => {
                debug!(topic = %self.topic, %peer, "topic peer introduction timed out");
                return;
            }
        };
        let own = self.endpoint.id();
        let neighbors = self.neighbors.read().await.clone();
        let now = Instant::now();
        let mut introduced = self.introduced.lock().await;
        for (address, addr_hint) in peers {
            let endpoint_id = address.id.to_string();
            if address.id == own || address.id == peer || neighbors.contains(&endpoint_id) {
                continue;
            }
            self.candidates.remember_hot_endpoint(address).await;
            introduced.insert(IntroducedPeer {
                endpoint_id,
                addr_hint,
                expires_at: now + INTRODUCTION_TTL,
            });
        }
    }
}

/// topic ごとの紹介。lease の task（近隣の成立）と試験の hook が同じ入口で紹介を求める。
#[derive(Clone)]
pub(super) struct TopicIntroductions {
    parts: IntroductionParts,
    run: Arc<Mutex<IntroductionRun>>,
}

impl TopicIntroductions {
    pub(super) fn new(
        endpoint: Endpoint,
        candidates: BootstrapCandidates,
        hint_topic: String,
        neighbors: Arc<RwLock<BTreeSet<String>>>,
    ) -> Self {
        Self {
            parts: IntroductionParts {
                endpoint,
                candidates,
                topic: hint_topic,
                neighbors,
                introduced: Arc::default(),
            },
            run: Arc::default(),
        }
    }

    pub(super) fn introduced(&self) -> Arc<Mutex<IntroducedPeers>> {
        Arc::clone(&self.parts.introduced)
    }

    /// `peer`（この topic の近隣）に紹介を求める。topic ごとに同時 1 件で、間隔は [`INTRODUCTION_INTERVAL`]。
    pub(super) async fn ask(&self, peer: EndpointId) {
        if !introduces_topic_peers(&self.parts.topic) {
            return;
        }
        let mut run = self.run.lock().await;
        let now = Instant::now();
        if run
            .last_at
            .is_some_and(|at| now.saturating_duration_since(at) < INTRODUCTION_INTERVAL)
        {
            return;
        }
        run.last_at = Some(now);
        run.requests += 1;
        let parts = self.parts.clone();
        run._in_flight = Some(AbortOnDropHandle::new(n0_future::task::spawn(async move {
            parts.request_from(peer).await
        })));
    }
}

struct CloseOnDrop(Connection);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0
            .close(0u32.into(), b"topic peer introduction complete");
    }
}

/// `peer` に `topic` の近隣を紹介してもらう。応答は上限の内で読み、到達情報を組み立てて返す。
pub(crate) async fn request_introductions(
    endpoint: &Endpoint,
    peer: EndpointAddr,
    topic: &str,
) -> Result<Vec<(EndpointAddr, Option<String>)>> {
    let connection = CloseOnDrop(endpoint.connect(peer, TOPIC_PEERS_ALPN).await?);
    let (mut send, mut recv) = connection.0.open_bi().await?;
    let request = IntroductionRequest {
        version: WIRE_VERSION,
        topic: topic.to_string(),
    };
    send.write_all(&serde_json::to_vec(&request)?).await?;
    send.finish()?;
    let bytes = recv.read_to_end(MAX_RESPONSE_BYTES).await?;
    let response: IntroductionResponse = serde_json::from_slice(&bytes)?;
    let mut peers = Vec::new();
    for wire in response.peers.into_iter().take(INTRODUCTION_PEERS) {
        let Ok(id) = EndpointId::from_str(wire.endpoint_id.trim()) else {
            continue;
        };
        let mut address = EndpointAddr::new(id);
        let mut addr_hint = None;
        for socket in wire
            .direct_addrs
            .iter()
            .take(MAX_DIRECT_ADDRS)
            .filter_map(|addr| addr.parse::<SocketAddr>().ok())
        {
            addr_hint.get_or_insert_with(|| socket.to_string());
            address = address.with_ip_addr(socket);
        }
        for url in wire
            .relay_urls
            .iter()
            .take(MAX_RELAY_URLS)
            .filter_map(|url| url.parse::<RelayUrl>().ok())
        {
            address = address.with_relay_url(url);
        }
        peers.push((address, addr_hint));
    }
    Ok(peers)
}

#[derive(Clone)]
struct Installed {
    endpoint: Endpoint,
    topic_states: Arc<Mutex<HashMap<String, HintTopicState>>>,
}

/// 紹介に答える protocol handler。Router の起動時には購読の状態が無いので、transport が後から入れる
/// （`ReceiveBindingSlot` と同じ形）。
#[derive(Clone)]
pub struct TopicPeerIntroductions {
    installed: Arc<StdRwLock<Option<Installed>>>,
    permits: Arc<Semaphore>,
}

impl Default for TopicPeerIntroductions {
    fn default() -> Self {
        Self {
            installed: Arc::default(),
            permits: Arc::new(Semaphore::new(INTRODUCTION_CONCURRENT_REQUESTS)),
        }
    }
}

impl std::fmt::Debug for TopicPeerIntroductions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TopicPeerIntroductions")
            .finish_non_exhaustive()
    }
}

impl TopicPeerIntroductions {
    pub(super) fn install(
        &self,
        endpoint: Endpoint,
        topic_states: Arc<Mutex<HashMap<String, HintTopicState>>>,
    ) {
        *self
            .installed
            .write()
            .expect("topic peer introductions poisoned") = Some(Installed {
            endpoint,
            topic_states,
        });
    }

    /// node の停止で外す。以後の要求には答えない。
    pub fn clear(&self) {
        *self
            .installed
            .write()
            .expect("topic peer introductions poisoned") = None;
    }

    async fn serve(&self, connection: &Connection) -> Result<()> {
        let installed = self
            .installed
            .read()
            .expect("topic peer introductions poisoned")
            .clone();
        let Some(installed) = installed else {
            return Ok(());
        };
        let requester = connection.remote_id().to_string();
        let (mut send, mut recv) = connection.accept_bi().await?;
        let request: IntroductionRequest =
            serde_json::from_slice(&recv.read_to_end(MAX_REQUEST_BYTES).await?)?;
        anyhow::ensure!(
            request.version == WIRE_VERSION,
            "unsupported topic peer introduction request"
        );
        let mut peers = Vec::new();
        let neighbors = if introduces_topic_peers(&request.topic) {
            installed
                .topic_states
                .lock()
                .await
                .get(&request.topic)
                .map(|state| Arc::clone(&state.neighbors))
        } else {
            None
        };
        if let Some(neighbors) = neighbors {
            let window = introduction_window(&*neighbors.read().await, &requester);
            for endpoint_id in window {
                let (mut relay_urls, mut direct_addrs) = (Vec::new(), Vec::new());
                if let Ok(id) = endpoint_id.parse::<EndpointId>()
                    && let Some(info) = installed.endpoint.remote_info(id).await
                {
                    // 使っている経路の address を先に(要求元が最初に試す到達の手掛かりになる)。
                    let mut addrs = info.addrs().collect::<Vec<_>>();
                    addrs.sort_by_key(|addr| !matches!(addr.usage(), TransportAddrUsage::Active));
                    for addr in addrs {
                        match addr.addr() {
                            TransportAddr::Relay(url) if relay_urls.len() < MAX_RELAY_URLS => {
                                relay_urls.push(url.to_string());
                            }
                            TransportAddr::Ip(socket) if direct_addrs.len() < MAX_DIRECT_ADDRS => {
                                direct_addrs.push(socket.to_string());
                            }
                            _ => {}
                        }
                    }
                }
                peers.push(IntroducedPeerWire {
                    endpoint_id,
                    relay_urls,
                    direct_addrs,
                });
            }
        }
        let bytes = serde_json::to_vec(&IntroductionResponse { peers })?;
        anyhow::ensure!(
            bytes.len() <= MAX_RESPONSE_BYTES,
            "topic peer introduction response is too large"
        );
        send.write_all(&bytes).await?;
        send.finish()?;
        send.stopped().await?;
        Ok(())
    }
}

impl ProtocolHandler for TopicPeerIntroductions {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let connection = CloseOnDrop(connection);
        let Ok(_permit) = self.permits.try_acquire() else {
            return Ok(());
        };
        timeout(INTRODUCTION_TIMEOUT, self.serve(&connection.0))
            .await
            .context("topic peer introduction timed out")
            .and_then(|result| result)
            .map_err(|error| AcceptError::from_boxed(error.into_boxed_dyn_error()))
    }
}

impl IrohGossipTransport {
    /// 紹介に答える handler（iroh-node の Router に登録済み）へ、この transport の購読の状態を入れる。
    pub fn with_topic_peer_introductions(self, introductions: TopicPeerIntroductions) -> Self {
        introductions.install(self.endpoint.clone(), Arc::clone(&self.topic_states));
        self
    }

    /// 試験用: gossip で合流せずに `peer` をこの topic の近隣に置き、近隣の成立と同じく紹介を求める。
    #[cfg(any(test, feature = "test-support"))]
    pub async fn place_topic_neighbor_for_tests(
        &self,
        topic: &TopicId,
        peer: EndpointAddr,
    ) -> Result<()> {
        let hint_topic = kukuri_core::wire::hint_topic_id(topic);
        let (neighbors, introductions) = self
            .topic_states
            .lock()
            .await
            .get(hint_topic.as_str())
            .map(|state| (Arc::clone(&state.neighbors), state.introductions.clone()))
            .context("the topic is not subscribed")?;
        self.remember_hot_endpoint(peer.clone()).await;
        neighbors.write().await.insert(peer.id.to_string());
        introductions.ask(peer.id).await;
        Ok(())
    }

    /// 試験用: この topic で紹介を求めた回数。
    #[cfg(any(test, feature = "test-support"))]
    pub async fn introduction_request_count_for_tests(&self, topic: &TopicId) -> u64 {
        let hint_topic = kukuri_core::wire::hint_topic_id(topic);
        let Some(introductions) = self
            .topic_states
            .lock()
            .await
            .get(hint_topic.as_str())
            .map(|state| state.introductions.clone())
        else {
            return 0;
        };
        introductions.run.lock().await.requests
    }

    /// 試験用: 紹介で覚えている、この topic の候補の endpoint ID。
    #[cfg(any(test, feature = "test-support"))]
    pub async fn introduced_topic_peers_for_tests(&self, topic: &TopicId) -> Vec<String> {
        let hint_topic = kukuri_core::wire::hint_topic_id(topic);
        let Some((neighbors, introduced)) = self
            .topic_states
            .lock()
            .await
            .get(hint_topic.as_str())
            .map(|state| {
                (
                    Arc::clone(&state.neighbors),
                    state.introductions.introduced(),
                )
            })
        else {
            return Vec::new();
        };
        let neighbors = neighbors.read().await.clone();
        introduced
            .lock()
            .await
            .live(&neighbors, Instant::now())
            .into_iter()
            .map(|peer| peer.endpoint_id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn introductions_cover_public_topics_and_public_threads_only() {
        assert!(introduces_topic_peers("hint/kukuri:topic:general"));
        assert!(introduces_topic_peers("hint/public-channel/pc-1-abcd"));
        assert!(!introduces_topic_peers("hint/private/room"));
        assert!(!introduces_topic_peers("hint/kukuri:dm:00"));
        assert!(!introduces_topic_peers("hint/kukuri:account:00"));
        assert!(!introduces_topic_peers("kukuri:topic:general"));
    }

    #[test]
    fn the_window_starts_after_the_requester_and_excludes_it() {
        let neighbors = ids(&["a", "b", "c", "d", "e", "f"]);
        assert_eq!(introduction_window(&neighbors, "c"), ["d", "e", "f", "a"]);
        assert_eq!(introduction_window(&neighbors, "f"), ["a", "b", "c", "d"]);
        assert_eq!(introduction_window(&neighbors, "zz"), ["a", "b", "c", "d"]);
        assert_eq!(introduction_window(&ids(&["a"]), "a"), Vec::<String>::new());
    }

    #[test]
    fn the_ledger_keeps_eight_live_candidates_and_drops_neighbors() {
        let now = Instant::now();
        let mut ledger = IntroducedPeers::default();
        for index in 0..10 {
            ledger.insert(IntroducedPeer {
                endpoint_id: format!("peer-{index}"),
                addr_hint: None,
                expires_at: now + INTRODUCTION_TTL,
            });
        }
        ledger.insert(IntroducedPeer {
            endpoint_id: "stale".into(),
            addr_hint: None,
            expires_at: now,
        });
        let live = ledger.live(&ids(&["peer-5"]), now);
        let live_ids = live
            .iter()
            .map(|peer| peer.endpoint_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            live_ids,
            ["peer-3", "peer-4", "peer-6", "peer-7", "peer-8", "peer-9"]
        );
    }
}
