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

use anyhow::Result;
use chrono::Utc;
use iroh::address_lookup::MemoryLookup;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use kukuri_store::SqliteStore;
use tokio::sync::{Mutex, Semaphore, watch};
// 元実装(docs-sync / blob-service)と同じ tokio の Instant を使う(テストでの時間制御と互換)。
use tokio::time::Instant;

mod health;
pub use health::{BlobPeerAttempt, BlobPeerHealth, MAX_BLOB_PEER_RECORDS};

use crate::config::SeedPeer;
use crate::tickets::relay_assisted_endpoint_addr;

pub const REMOTE_FETCH_RETRY_COOLDOWN: Duration = Duration::from_secs(3);
pub const REMOTE_FETCH_MAX_COOLDOWNS: usize = 1_024;
const REMOTE_FETCH_MAX_COOLDOWN_KEY_BYTES: usize = 256;
static REMOTE_FETCH_STATE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
/// 1 つの retry state が同時に実行する remote 走査の上限(#1207)。超過分は順番を待つ。
pub const REMOTE_FETCH_MAX_CONCURRENT_WALKS: usize = 8;
const PEER_FETCH_BACKOFF_BASE: Duration = Duration::from_secs(2);
const PEER_FETCH_BACKOFF_MAX: Duration = Duration::from_secs(60);
const PEER_CONNECTION_STATE_TTL: Duration = Duration::from_secs(300);
const PEER_FETCH_SUCCESS_TTL: Duration = Duration::from_secs(600);
const PEER_FETCH_REQUEST_LIMIT: u64 = 16;
const PEER_FETCH_REQUEST_WINDOW: Duration = Duration::from_secs(1);
const RECENT_PEER_FETCH_WINDOW: Duration = Duration::from_secs(30);

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
    walk_permits: Arc<Semaphore>,
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
            walk_permits: Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT_WALKS)),
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

    pub fn walk_permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.walk_permits)
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
    account_store: Option<(Arc<SqliteStore>, &'static str)>,
    account_cursor: Mutex<[Option<(i64, String)>; 3]>,
    health: Arc<BlobPeerHealth>,
    fetch_cursor: Mutex<[Option<String>; 3]>,
    recent_peers: Mutex<VecDeque<RecentPeer>>,
    #[cfg(test)]
    sampled_peer_count: std::sync::atomic::AtomicUsize,
}

struct RecentPeer {
    id: String,
    expires_at: Instant,
    imported: bool,
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
            account_store: None,
            account_cursor: Mutex::new([None, None, None]),
            health,
            fetch_cursor: Mutex::new([None, None, None]),
            recent_peers: Mutex::new(VecDeque::new()),
            #[cfg(test)]
            sampled_peer_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn with_account_store(
        endpoint: Endpoint,
        discovery: Arc<MemoryLookup>,
        health: Arc<BlobPeerHealth>,
        store: Arc<SqliteStore>,
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
            return match self.ranked_account_peers(store, scope).await {
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

    async fn ranked_account_peers(
        &self,
        store: &SqliteStore,
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
            if learned_peers.get(key.as_str()) == Some(&endpoint_addr) {
                false
            } else {
                learned_peers.insert(key.clone(), endpoint_addr);
                true
            }
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
        self.imported_peers
            .lock()
            .await
            .insert(key.clone(), endpoint_addr);
        self.note_recent_peer(key, true).await;
        Ok(())
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

    /// 合成台帳のうち、endpoint がアクティブなアドレスを持つピア id を返す。
    pub async fn available_peer_ids(&self) -> Vec<String> {
        let peers = self.ranked_peers().await;
        let mut available = BTreeSet::new();
        for peer in peers {
            if self
                .endpoint
                .remote_info(peer.id)
                .await
                .is_some_and(|info| {
                    info.addrs().any(|addr| {
                        matches!(addr.usage(), iroh::endpoint::TransportAddrUsage::Active)
                    })
                })
            {
                available.insert(peer.id.to_string());
            }
        }
        available.into_iter().collect()
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
