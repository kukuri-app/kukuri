//! docs-sync / blob-service で共有するピア台帳とリモートフェッチのリトライ状態(WP-H2)。
//!
//! docs/blob の候補選択と取得観測を共有する。production の候補履歴はaccount DB、
//! memory nodeの候補はメモリ台帳に置き、どちらも選択時には有限cursorだけを読む。
//! docsとblobの健康観測はprotocolごとに分ける。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::PeerCandidateStore;
use anyhow::Result;
use chrono::Utc;
use iroh::address_lookup::MemoryLookup;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use tokio::sync::{Mutex, watch};
// n0_future の Instant（native は tokio の Instant。テストでの時間制御と互換）。
use n0_future::time::Instant;

mod health;
pub use health::{BlobPeerAttempt, BlobPeerHealth, MAX_BLOB_PEER_RECORDS};

use crate::config::SeedPeer;
use crate::tickets::relay_assisted_endpoint_addr;

pub const REMOTE_FETCH_RETRY_COOLDOWN: Duration = Duration::from_secs(3);
pub const REMOTE_FETCH_MAX_COOLDOWNS: usize = 1_024;
const REMOTE_FETCH_MAX_COOLDOWN_KEY_BYTES: usize = 256;
/// store を持たない経路（Web など）で、learned と imported の台帳が持つ件数の上限（ADR 0056 §5）。
/// 超えたら古いものから台帳と `MemoryLookup` から外す。
pub(crate) const STORELESS_PEER_LIMIT: usize = 256;
static REMOTE_FETCH_STATE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const PEER_FETCH_BACKOFF_BASE: Duration = Duration::from_secs(2);
const PEER_FETCH_BACKOFF_MAX: Duration = Duration::from_secs(60);
const PEER_CONNECTION_STATE_TTL: Duration = Duration::from_secs(300);
const PEER_FETCH_SUCCESS_TTL: Duration = Duration::from_secs(600);
const PEER_FETCH_REQUEST_LIMIT: u64 = 16;
const PEER_FETCH_REQUEST_WINDOW: Duration = Duration::from_secs(1);
const RECENT_PEER_FETCH_WINDOW: Duration = Duration::from_secs(30);
/// 本文・添付の取得元として覚える(hash, peer)の数。超えたら古いものから捨てる(#1395)。
const MAX_CONTENT_SOURCES: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PeerConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerFetchFailure {
    ConnectFailed,
    ConnectTimeout,
    TransferFailed,
    TransferTimeout,
    NotFound,
    Rejected,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStateSnapshot {
    pub connection_generation: u64,
    pub connection_status: PeerConnectionStatus,
    pub fetch_successes: u64,
    pub fetch_failures: u64,
    pub fetch_misses: u64,
    pub fetch_rejections: u64,
    pub consecutive_fetch_failures: u32,
    pub smoothed_fetch_latency_ms: Option<u64>,
}

#[derive(Debug, Default)]
struct PeerRuntimeRecord {
    connection_generation: u64,
    connection_status: PeerConnectionStatus,
    connection_observed_at: Option<Instant>,
    fetch_successes: u64,
    fetch_failures: u64,
    fetch_misses: u64,
    fetch_rejections: u64,
    consecutive_fetch_failures: u32,
    smoothed_fetch_latency_ms: Option<u64>,
    last_success_at: Option<Instant>,
    retry_after: Option<Instant>,
}

impl PeerRuntimeRecord {
    fn connection_status_at(&self, now: Instant) -> PeerConnectionStatus {
        if self
            .connection_observed_at
            .is_some_and(|observed| now.duration_since(observed) <= PEER_CONNECTION_STATE_TTL)
        {
            self.connection_status
        } else {
            PeerConnectionStatus::Unknown
        }
    }

    fn snapshot(&self, now: Instant) -> PeerStateSnapshot {
        PeerStateSnapshot {
            connection_generation: self.connection_generation,
            connection_status: self.connection_status_at(now),
            fetch_successes: self.fetch_successes,
            fetch_failures: self.fetch_failures,
            fetch_misses: self.fetch_misses,
            fetch_rejections: self.fetch_rejections,
            consecutive_fetch_failures: self.consecutive_fetch_failures,
            smoothed_fetch_latency_ms: self.smoothed_fetch_latency_ms,
        }
    }
}

/// Rate-limit subjects stay typed so an HTTP address is never treated as a verified P2P identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestRateSubject {
    HttpIp(String),
    PeerEndpoint(String),
    RelayClient(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestRateClass {
    HttpRequest,
    P2pRequest,
    RelayIngressBytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestRatePolicy {
    pub limit: u64,
    pub window: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestRateDecision {
    Allowed,
    Limited { retry_after: Duration },
}

type RequestRateWindows =
    BTreeMap<(RequestRateSubject, RequestRateClass), VecDeque<(Instant, u64)>>;

#[derive(Default)]
pub struct RequestRateLedger {
    windows: Mutex<RequestRateWindows>,
}

impl RequestRateLedger {
    pub async fn check_and_record(
        &self,
        subject: RequestRateSubject,
        class: RequestRateClass,
        amount: u64,
        policy: RequestRatePolicy,
        now: Instant,
    ) -> RequestRateDecision {
        let mut windows = self.windows.lock().await;
        let entries = windows.entry((subject, class)).or_default();
        while entries
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) >= policy.window)
        {
            entries.pop_front();
        }
        let used = entries.iter().map(|(_, amount)| *amount).sum::<u64>();
        if used.saturating_add(amount) > policy.limit {
            let retry_after = entries
                .front()
                .map(|(at, _)| policy.window.saturating_sub(now.duration_since(*at)))
                .unwrap_or(policy.window);
            return RequestRateDecision::Limited { retry_after };
        }
        entries.push_back((now, amount));
        RequestRateDecision::Allowed
    }

    pub async fn retain_recent(&self, oldest: Instant) {
        self.windows.lock().await.retain(|_, entries| {
            while entries.front().is_some_and(|(at, _)| *at < oldest) {
                entries.pop_front();
            }
            !entries.is_empty()
        });
    }
}

/// 合流した呼び出しへ配る remote 取得の結果(#1207)。
///
/// 走査は呼び出し側の future から切り離して実行するため、結果は共有できる形で持つ。
pub type SharedRemoteFetchResult = Result<Option<Arc<Vec<u8>>>, Arc<anyhow::Error>>;

type RemoteFetchResultSender = watch::Sender<Option<SharedRemoteFetchResult>>;
type RemoteFetchResultReceiver = watch::Receiver<Option<SharedRemoteFetchResult>>;

/// `RemoteFetchRetryState::begin` の結果。
pub enum RemoteFetchBegin {
    /// この呼び出しが走査を実行する。結果は sender へ 1 回だけ流す。
    Lead(RemoteFetchResultSender),
    /// 同じ対象の走査が実行中。結果を受け取るだけで、新しい走査は始めない。
    Join(RemoteFetchResultReceiver),
    CoolingDown,
}

/// serviceごとの失敗cooldown。明示的なremote取得の合流と実行枠は
/// iroh-nodeのNetworkWorkRuntimeがnode単位で所有する（#1221）。
/// `begin`の旧予約APIとwalk permitは互換用に残すが、通常取得taskは起動しない。
pub struct RemoteFetchRetryState {
    instance_id: u64,
    retry_after: BTreeMap<String, Instant>,
    retry_deadlines: BTreeSet<(Instant, String)>,
    in_flight: BTreeMap<String, RemoteFetchResultReceiver>,
}

impl Default for RemoteFetchRetryState {
    fn default() -> Self {
        Self {
            instance_id: REMOTE_FETCH_STATE_SEQUENCE
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("remote fetch service identity exhausted"),
            retry_after: BTreeMap::new(),
            retry_deadlines: BTreeSet::new(),
            in_flight: BTreeMap::new(),
        }
    }
}

impl RemoteFetchRetryState {
    /// Process-local service generation, never reused after this ledger drops.
    pub fn instance_id(&self) -> u64 {
        self.instance_id
    }

    pub fn is_cooling_down(&self, key: &str, now: Instant) -> bool {
        self.retry_after
            .get(key)
            .is_some_and(|deadline| *deadline > now)
    }

    /// `cooldown_key` は失敗クールダウンの単位、`flight_key` は合流の単位。
    /// 保存先が異なる取得(永続 / 一時)は `flight_key` を分けて合流させない。
    pub fn begin(
        &mut self,
        cooldown_key: &str,
        flight_key: &str,
        now: Instant,
    ) -> RemoteFetchBegin {
        if let Some(receiver) = self.in_flight.get(flight_key) {
            // 結果を流さずに sender が消えた予約(走査 task の異常終了)は引き継がない。
            if receiver.borrow().is_some() || receiver.has_changed().is_ok() {
                return RemoteFetchBegin::Join(receiver.clone());
            }
            self.in_flight.remove(flight_key);
        }
        if self
            .retry_after
            .get(cooldown_key)
            .is_some_and(|retry_after| *retry_after > now)
        {
            return RemoteFetchBegin::CoolingDown;
        }
        self.remove_cooldown(cooldown_key);
        let (sender, receiver) = watch::channel(None);
        self.in_flight.insert(flight_key.to_string(), receiver);
        RemoteFetchBegin::Lead(sender)
    }

    pub fn finish(&mut self, cooldown_key: &str, flight_key: &str, success: bool, now: Instant) {
        self.in_flight.remove(flight_key);
        while let Some((deadline, key)) = self.retry_deadlines.first() {
            if *deadline > now {
                break;
            }
            let key = key.clone();
            self.remove_cooldown(&key);
        }
        self.remove_cooldown(cooldown_key);
        if success || cooldown_key.len() > REMOTE_FETCH_MAX_COOLDOWN_KEY_BYTES {
            return;
        }
        if self.retry_after.len() >= REMOTE_FETCH_MAX_COOLDOWNS {
            let (_, key) = self
                .retry_deadlines
                .first()
                .expect("nonempty cooldown index")
                .clone();
            self.remove_cooldown(&key);
        }
        let deadline = now + REMOTE_FETCH_RETRY_COOLDOWN;
        self.retry_after.insert(cooldown_key.to_owned(), deadline);
        self.retry_deadlines
            .insert((deadline, cooldown_key.to_owned()));
    }

    fn remove_cooldown(&mut self, key: &str) {
        if let Some(deadline) = self.retry_after.remove(key) {
            self.retry_deadlines.remove(&(deadline, key.to_owned()));
        }
    }

    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    pub fn cooldown_len(&self) -> usize {
        self.retry_after.len()
    }
}

/// direct(IP アドレス)だけを残した EndpointAddr(relay を含まない候補)。
pub fn direct_endpoint_addr(endpoint_addr: &EndpointAddr) -> Option<EndpointAddr> {
    let mut direct = EndpointAddr::new(endpoint_addr.id);
    for addr in endpoint_addr.ip_addrs() {
        direct = direct.with_ip_addr(*addr);
    }
    (!direct.is_empty()).then_some(direct)
}

/// learned / seed / imported の 3 台帳を持つピア台帳。
///
/// 合成順序(learned → seed → imported、id 重複排除)と接続候補の優先順位
/// (direct → remote_info 由来 → relay 付き → 元の値)は外部挙動として固定
/// (characterization: docs-sync / blob-service 双方の
/// `connect_candidates_prefers_direct_remote_info_before_relay_hint`)。
pub struct PeerAddrBook {
    endpoint: Endpoint,
    discovery: Arc<MemoryLookup>,
    learned_peers: Mutex<BTreeMap<String, EndpointAddr>>,
    seed_peers: Mutex<BTreeMap<String, EndpointAddr>>,
    imported_peers: Mutex<BTreeMap<String, EndpointAddr>>,
    /// store を持たない経路の learned・imported の挿入順（古いものから外すため）。
    storeless_order: Mutex<[VecDeque<String>; 2]>,
    account_store: Option<(Arc<PeerCandidateStore>, &'static str)>,
    account_cursor: Mutex<[Option<(i64, String)>; 3]>,
    health: Arc<BlobPeerHealth>,
    fetch_cursor: Mutex<[Option<String>; 3]>,
    recent_peers: Mutex<VecDeque<RecentPeer>>,
    /// hash 別の取得元と失敗抑止。全体の上限は256組で、期限の切れた組は選択・更新時に外す。
    content_sources: Mutex<VecDeque<ContentSource>>,
    #[cfg(test)]
    sampled_peer_count: std::sync::atomic::AtomicUsize,
}

struct RecentPeer {
    id: String,
    expires_at: Instant,
    imported: bool,
}

struct ContentSource {
    hash: String,
    /// 取得元。発見で得た到達情報（relay URL 等）があれば持つ（#1632）。
    addr: EndpointAddr,
    expires_at: Instant,
    retry_after: Instant,
}

impl PeerAddrBook {
    pub fn new(endpoint: Endpoint, discovery: Arc<MemoryLookup>) -> Self {
        Self::with_fetch_health(endpoint, discovery, Arc::new(BlobPeerHealth::default()))
    }

    pub fn with_fetch_health(
        endpoint: Endpoint,
        discovery: Arc<MemoryLookup>,
        health: Arc<BlobPeerHealth>,
    ) -> Self {
        Self {
            endpoint,
            discovery,
            learned_peers: Mutex::new(BTreeMap::new()),
            seed_peers: Mutex::new(BTreeMap::new()),
            imported_peers: Mutex::new(BTreeMap::new()),
            storeless_order: Mutex::new([VecDeque::new(), VecDeque::new()]),
            account_store: None,
            account_cursor: Mutex::new([None, None, None]),
            health,
            fetch_cursor: Mutex::new([None, None, None]),
            recent_peers: Mutex::new(VecDeque::new()),
            content_sources: Mutex::new(VecDeque::new()),
            #[cfg(test)]
            sampled_peer_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn with_account_store(
        endpoint: Endpoint,
        discovery: Arc<MemoryLookup>,
        health: Arc<BlobPeerHealth>,
        store: Arc<PeerCandidateStore>,
        scope: &'static str,
    ) -> Self {
        let mut book = Self::with_fetch_health(endpoint, discovery, health);
        book.account_store = Some((store, scope));
        book
    }

    /// Sample a moving, bounded window before ranking. Source precedence for
    /// every sampled identity remains learned -> seed -> imported.
    pub async fn ranked_peers(&self) -> Vec<EndpointAddr> {
        if let Some((store, scope)) = &self.account_store {
            return match self.ranked_account_peers(store.as_ref(), scope).await {
                Ok(peers) => peers,
                Err(error) => {
                    tracing::warn!(%error, "failed to read peer candidates");
                    Vec::new()
                }
            };
        }
        let preferred = self.health.preferred().await;
        let (recent, newest_import) = {
            let mut recent = self.recent_peers.lock().await;
            let now = Instant::now();
            recent.retain(|peer| peer.expires_at > now);
            (
                recent
                    .iter()
                    .map(|peer| peer.id.clone())
                    .collect::<Vec<_>>(),
                recent
                    .iter()
                    .find(|peer| peer.imported)
                    .map(|peer| peer.id.clone()),
            )
        };
        let mut peers = {
            let mut cursors = self.fetch_cursor.lock().await;
            let learned = self.learned_peers.lock().await;
            let seeds = self.seed_peers.lock().await;
            let imported = self.imported_peers.lock().await;
            let mut sampled = Vec::new();
            for (index, source) in [&*learned, &*seeds, &*imported].into_iter().enumerate() {
                sampled.extend(fetch_source_window(source, &mut cursors[index], 4));
            }
            #[cfg(test)]
            self.sampled_peer_count
                .store(sampled.len(), Ordering::Relaxed);
            let ids = preferred
                .into_iter()
                .map(|peer| peer.to_string())
                .chain(recent)
                .chain(sampled);
            let mut seen = BTreeSet::new();
            ids.filter(|id| seen.insert(id.clone()))
                .filter_map(|id| {
                    learned
                        .get(&id)
                        .or_else(|| seeds.get(&id))
                        .or_else(|| imported.get(&id))
                        .cloned()
                })
                .take(12)
                .collect::<Vec<_>>()
        };
        self.health.rank(&mut peers).await;
        if let Some(imported) = newest_import
            && let Some(position) = peers
                .iter()
                .position(|peer| peer.id.to_string() == imported)
            && position >= 4
        {
            peers.swap(3, position);
        }
        peers.truncate(4);
        peers
    }

    /// `hash` の内容の取得元を覚える。台帳の外の peer でもよい(`ranked_peers_for` で候補に入る)。到達情報があれば、
    /// 次の選択でもそれで試す。
    pub async fn note_content_source(&self, hash: &str, peer: impl Into<EndpointAddr>) {
        self.update_content_source(hash, peer.into(), None).await;
    }

    pub async fn record_content_fetch_result(
        &self,
        hash: &str,
        peer: EndpointId,
        result: Result<(), PeerFetchFailure>,
    ) {
        if result == Err(PeerFetchFailure::Cancelled) {
            return;
        }
        self.update_content_source(hash, EndpointAddr::new(peer), Some(result))
            .await;
    }

    async fn update_content_source(
        &self,
        hash: &str,
        addr: EndpointAddr,
        result: Option<Result<(), PeerFetchFailure>>,
    ) {
        let mut sources = self.content_sources.lock().await;
        let now = Instant::now();
        sources.retain(|source| source.expires_at > now);
        let existing = sources
            .iter()
            .position(|source| source.hash == hash && source.addr.id == addr.id);
        // 同じtopic/見出しのhintを再登録しても、観測済みの欠損・一時失敗を取り消さない。
        if result.is_none() && existing.is_some_and(|index| sources[index].retry_after > now) {
            return;
        }
        let mut source = existing
            .and_then(|index| sources.remove(index))
            .unwrap_or_else(|| ContentSource {
                hash: hash.to_string(),
                addr: EndpointAddr::new(addr.id),
                expires_at: now + PEER_FETCH_SUCCESS_TTL,
                retry_after: now,
            });
        if !addr.is_empty() {
            source.addr = addr;
        }
        match result {
            Some(Err(PeerFetchFailure::NotFound)) => source.retry_after = source.expires_at,
            Some(Err(_)) => source.retry_after = now + REMOTE_FETCH_RETRY_COOLDOWN,
            _ => {
                source.expires_at = now + PEER_FETCH_SUCCESS_TTL;
                source.retry_after = now;
            }
        }
        sources.push_front(source);
        sources.truncate(MAX_CONTENT_SOURCES);
    }

    /// hashの有効な取得元を新しい順に優先し、抑止中のpeerは一般候補経由でも選ばない(最大4件)。
    pub async fn ranked_peers_for(&self, hash: &str) -> Vec<EndpointAddr> {
        let mut peers = self.ranked_peers().await;
        let mut sources = self.content_sources.lock().await;
        let now = Instant::now();
        sources.retain(|source| source.expires_at > now);
        peers.retain(|peer| {
            !sources.iter().any(|source| {
                source.hash == hash && source.addr.id == peer.id && source.retry_after > now
            })
        });
        let preferred = sources
            .iter()
            .filter(|source| source.hash == hash && source.retry_after <= now)
            .map(|source| source.addr.clone())
            .take(4)
            .collect::<Vec<_>>();
        for source in preferred.into_iter().rev() {
            let addr = match peers.iter().position(|peer| peer.id == source.id) {
                Some(position) => peers.remove(position),
                None => source,
            };
            peers.insert(0, addr);
        }
        peers.truncate(4);
        peers
    }

    async fn ranked_account_peers(
        &self,
        store: &PeerCandidateStore,
        scope: &str,
    ) -> Result<Vec<EndpointAddr>> {
        let now = Utc::now().timestamp_millis();
        let preferred = self.health.preferred().await;
        let (recent, newest_import) = {
            let mut recent = self.recent_peers.lock().await;
            recent.retain(|peer| peer.expires_at > Instant::now());
            (
                recent
                    .iter()
                    .map(|peer| peer.id.clone())
                    .collect::<Vec<_>>(),
                recent
                    .iter()
                    .find(|peer| peer.imported)
                    .map(|peer| peer.id.clone()),
            )
        };
        let mut cursors = self.account_cursor.lock().await;
        let mut sampled = Vec::new();
        for (index, source) in ["learned", "seed", "imported"].into_iter().enumerate() {
            let page = store
                .peer_candidate_window(scope, source, cursors[index].clone(), 4, now)
                .await?;
            if let Some((id, _, seen_ms)) = page.last() {
                cursors[index] = Some((*seen_ms, id.clone()));
            }
            for (_, bytes, _) in page {
                sampled.push(serde_json::from_slice::<EndpointAddr>(&bytes)?);
            }
        }
        #[cfg(test)]
        self.sampled_peer_count
            .store(sampled.len(), Ordering::Relaxed);
        let mut peers = Vec::new();
        let mut seen = BTreeSet::new();
        for id in preferred
            .into_iter()
            .map(|peer| peer.to_string())
            .chain(recent)
        {
            if !seen.insert(id.clone()) {
                continue;
            }
            for source in ["learned", "seed", "imported"] {
                if let Some(bytes) = store.peer_candidate_by_id(scope, source, &id, now).await? {
                    peers.push(serde_json::from_slice(&bytes)?);
                    break;
                }
            }
            if peers.len() == 12 {
                break;
            }
        }
        for peer in sampled {
            if seen.insert(peer.id.to_string()) {
                peers.push(peer);
                if peers.len() == 12 {
                    break;
                }
            }
        }
        self.health.rank(&mut peers).await;
        if let Some(imported) = newest_import
            && let Some(position) = peers
                .iter()
                .position(|peer| peer.id.to_string() == imported)
            && position >= 4
        {
            peers.swap(3, position);
        }
        peers.truncate(4);
        Ok(peers)
    }

    pub async fn begin_fetch_attempt(&self, peer: EndpointId) -> Option<BlobPeerAttempt> {
        self.health.begin(peer).await
    }

    pub async fn record_connection_state(
        &self,
        peer: EndpointId,
        generation: u64,
        status: PeerConnectionStatus,
    ) {
        self.health.connection(peer, generation, status).await;
    }

    pub async fn begin_connection_attempt(&self, peer: EndpointId) -> u64 {
        self.health
            .begin(peer)
            .await
            .map(|attempt| attempt.generation())
            .unwrap_or_default()
    }

    pub async fn record_peer_fetch_request(&self, peer: EndpointId) -> RequestRateDecision {
        self.health.record_request(peer).await
    }

    pub async fn record_fetch_success(&self, peer: EndpointId, latency: Duration) {
        self.health.success(peer, latency).await;
    }

    pub async fn record_fetch_failure(&self, peer: EndpointId, failure: PeerFetchFailure) {
        self.health.failure(peer, failure).await;
    }

    pub async fn peer_state_snapshot(&self, peer: EndpointId) -> Option<PeerStateSnapshot> {
        self.health.snapshot(peer).await
    }

    /// learned 台帳へ挿入し、台帳に変化があったかを返す(同値なら false)。
    pub async fn insert_learned_peer_addr(&self, endpoint_addr: EndpointAddr) -> Result<bool> {
        if self.account_store.is_none() && !endpoint_addr.is_empty() {
            self.discovery.add_endpoint_info(endpoint_addr.clone());
        }
        let key = endpoint_addr.id.to_string();
        if let Some((store, scope)) = &self.account_store {
            let changed = store
                .put_peer_candidate(
                    scope,
                    "learned",
                    &key,
                    &serde_json::to_vec(&endpoint_addr)?,
                    Utc::now().timestamp_millis(),
                )
                .await?;
            self.note_recent_peer(key, false).await;
            return Ok(changed);
        }
        let changed = {
            let mut learned_peers = self.learned_peers.lock().await;
            let changed = learned_peers.get(key.as_str()) != Some(&endpoint_addr);
            self.insert_storeless(&mut learned_peers, 0, key.clone(), endpoint_addr)
                .await;
            changed
        };
        self.note_recent_peer(key, false).await;
        Ok(changed)
    }

    pub async fn insert_imported_peer_addr(&self, endpoint_addr: EndpointAddr) -> Result<()> {
        if self.account_store.is_none() {
            self.discovery.add_endpoint_info(endpoint_addr.clone());
        }
        let key = endpoint_addr.id.to_string();
        if let Some((store, scope)) = &self.account_store {
            store
                .put_peer_candidate(
                    scope,
                    "imported",
                    &key,
                    &serde_json::to_vec(&endpoint_addr)?,
                    Utc::now().timestamp_millis(),
                )
                .await?;
            self.note_recent_peer(key, true).await;
            return Ok(());
        }
        {
            let mut imported_peers = self.imported_peers.lock().await;
            self.insert_storeless(&mut imported_peers, 1, key.clone(), endpoint_addr)
                .await;
        }
        self.note_recent_peer(key, true).await;
        Ok(())
    }

    /// store を持たない経路で台帳へ入れ、上限を超えた古いものを台帳と `MemoryLookup` から外す。
    async fn insert_storeless(
        &self,
        peers: &mut BTreeMap<String, EndpointAddr>,
        order: usize,
        key: String,
        endpoint_addr: EndpointAddr,
    ) {
        let mut orders = self.storeless_order.lock().await;
        if peers.insert(key.clone(), endpoint_addr).is_none() {
            orders[order].push_back(key);
        }
        while peers.len() > STORELESS_PEER_LIMIT {
            let Some(oldest) = orders[order].pop_front() else {
                break;
            };
            if peers.remove(&oldest).is_some()
                && let Ok(id) = EndpointId::from_str(&oldest)
            {
                self.discovery.remove_endpoint_info(id);
            }
        }
    }

    async fn note_recent_peer(&self, peer: String, imported: bool) {
        let mut recent = self.recent_peers.lock().await;
        recent.retain(|existing| existing.id != peer);
        recent.push_front(RecentPeer {
            id: peer,
            expires_at: Instant::now() + RECENT_PEER_FETCH_WINDOW,
            imported,
        });
        recent.truncate(4);
    }

    /// endpoint の remote_info と relay URL から learned ピアを記録し、
    /// 台帳に変化があったかを返す。
    pub async fn record_learned_peer(
        &self,
        endpoint_id: &str,
        relay_urls: &[RelayUrl],
    ) -> Result<bool> {
        let endpoint_id = EndpointId::from_str(endpoint_id.trim())?;
        let mut endpoint_addr = self
            .endpoint
            .remote_info(endpoint_id)
            .await
            .map(|remote_info| {
                EndpointAddr::from_parts(
                    remote_info.id(),
                    remote_info.into_addrs().map(|addr| addr.into_addr()),
                )
            })
            .unwrap_or_else(|| EndpointAddr::new(endpoint_id));
        for relay_url in relay_urls {
            endpoint_addr = endpoint_addr.with_relay_url(relay_url.clone());
        }
        self.insert_learned_peer_addr(endpoint_addr).await
    }

    /// seed 台帳を丸ごと差し替える(SeedPeer → EndpointAddr 変換 + discovery 登録)。
    pub async fn set_seed_peers(
        &self,
        peers: Vec<SeedPeer>,
        relay_urls: &[RelayUrl],
    ) -> Result<()> {
        let mut parsed = BTreeMap::new();
        for peer in peers {
            let endpoint_addr = peer.to_endpoint_addr_with_relays(relay_urls)?;
            if self.account_store.is_none() {
                self.discovery.add_endpoint_info(endpoint_addr.clone());
            }
            parsed.insert(endpoint_addr.id.to_string(), endpoint_addr);
        }
        if let Some((store, scope)) = &self.account_store {
            let seeds = parsed
                .into_iter()
                .map(|(id, addr)| Ok((id, serde_json::to_vec(&addr)?)))
                .collect::<Result<Vec<_>>>()?;
            return store
                .replace_seed_candidates(scope, seeds, Utc::now().timestamp_millis())
                .await;
        }
        *self.seed_peers.lock().await = parsed;
        Ok(())
    }

    /// 接続候補を優先順位つきで並べる: direct → remote_info 由来 → relay 付き → 元の値。
    pub async fn connect_candidates(&self, imported_peer: &EndpointAddr) -> Vec<EndpointAddr> {
        let mut candidates = Vec::new();
        if let Some(candidate) = direct_endpoint_addr(imported_peer) {
            candidates.push(candidate);
        }
        if let Some(remote_info) = self.endpoint.remote_info(imported_peer.id).await {
            let learned_peer = EndpointAddr::from_parts(
                remote_info.id(),
                remote_info.into_addrs().map(|addr| addr.into_addr()),
            );
            if !learned_peer.is_empty() {
                candidates.push(learned_peer);
            }
        }
        let relay_supported = relay_assisted_endpoint_addr(imported_peer);
        if relay_supported.relay_urls().next().is_some()
            && !candidates
                .iter()
                .any(|candidate| candidate == &relay_supported)
        {
            candidates.push(relay_supported);
        }
        if candidates.is_empty()
            || !candidates
                .iter()
                .any(|candidate| candidate == imported_peer)
        {
            candidates.push(imported_peer.clone());
        }
        candidates
    }

    /// 補助に使える peer: 直近に取得の成功を観測し、待機中でも切断中でもない peer(最大 2 件)。
    /// 状態の表示から読むので、候補の cursor・台帳・`remote_info` を触らない(ADR 0055 §3、#1221 R2-D)。
    pub async fn available_peer_ids(&self) -> Vec<String> {
        let mut available = self
            .health
            .preferred()
            .await
            .into_iter()
            .map(|peer| peer.to_string())
            .collect::<Vec<_>>();
        available.sort();
        available
    }
}

pub(crate) fn fetch_source_window(
    source: &BTreeMap<String, EndpointAddr>,
    cursor: &mut Option<String>,
    limit: usize,
) -> Vec<String> {
    use std::ops::Bound::{Excluded, Unbounded};
    let mut keys = Vec::with_capacity(limit);
    if let Some(after) = cursor.as_ref() {
        keys.extend(
            source
                .range((Excluded(after.clone()), Unbounded))
                .take(limit)
                .map(|(key, _)| key.clone()),
        );
        if keys.len() < limit {
            keys.extend(
                source
                    .range(..=after.clone())
                    .take(limit - keys.len())
                    .map(|(key, _)| key.clone()),
            );
        }
    } else {
        keys.extend(source.keys().take(limit).cloned());
    }
    if let Some(last) = keys.last() {
        *cursor = Some(last.clone());
    }
    keys
}

#[cfg(test)]
mod tests;
