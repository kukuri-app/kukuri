//! docs-sync / blob-service で共有するピア台帳とリモートフェッチのリトライ状態(WP-H2)。
//!
//! かつて両 crate にほぼ写しで存在したピア管理(learned / seed / imported の 3 台帳、
//! 合成順序、接続候補の優先順位、失敗クールダウン)の単一実装。
//!
//! **挙動差分は呼び出し側に残す**(REFACTORING.md / WP-H2 プランの決定):
//! - `insert_learned_peer_addr` は「台帳に変化があったか」の bool を返す。docs-sync は
//!   これを見て全レプリカへ同期先を配り直す(reapply)。blob-service は無視する。
//! - 状態復元(peer_state / restore)後の reapply も docs-sync 呼び出し側の責務。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use iroh::address_lookup::MemoryLookup;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use tokio::sync::{Mutex, watch};
// 元実装(docs-sync / blob-service)と同じ tokio の Instant を使う(テストでの時間制御と互換)。
use tokio::time::Instant;

use crate::config::SeedPeer;
use crate::tickets::relay_assisted_endpoint_addr;

pub const REMOTE_FETCH_RETRY_COOLDOWN: Duration = Duration::from_secs(3);
const PEER_FETCH_BACKOFF_BASE: Duration = Duration::from_secs(2);
const PEER_FETCH_BACKOFF_MAX: Duration = Duration::from_secs(60);
const PEER_CONNECTION_STATE_TTL: Duration = Duration::from_secs(300);
const PEER_FETCH_SUCCESS_TTL: Duration = Duration::from_secs(600);
const PEER_FETCH_REQUEST_LIMIT: u64 = 16;
const PEER_FETCH_REQUEST_WINDOW: Duration = Duration::from_secs(1);

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
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStateSnapshot {
    pub connection_generation: u64,
    pub connection_status: PeerConnectionStatus,
    pub fetch_successes: u64,
    pub fetch_failures: u64,
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

/// リモートフェッチ失敗のクールダウン(対象キー毎)と、実行中の走査の予約(#1207)。
///
/// 同じ flight key の走査は 1 本だけにする。実行中の予約は `finish` まで残るため、
/// 呼び出し側が途中で待つのをやめても、後続の呼び出しが並行して走査を始めることはない。
#[derive(Default)]
pub struct RemoteFetchRetryState {
    retry_after: BTreeMap<String, Instant>,
    in_flight: BTreeMap<String, RemoteFetchResultReceiver>,
}

impl RemoteFetchRetryState {
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
        self.retry_after.remove(cooldown_key);
        let (sender, receiver) = watch::channel(None);
        self.in_flight.insert(flight_key.to_string(), receiver);
        RemoteFetchBegin::Lead(sender)
    }

    pub fn finish(&mut self, cooldown_key: &str, flight_key: &str, success: bool, now: Instant) {
        self.in_flight.remove(flight_key);
        // 期限切れのクールダウンを残さない(台帳を有限に保つ)。
        self.retry_after.retain(|_, retry_after| *retry_after > now);
        if success {
            self.retry_after.remove(cooldown_key);
        } else {
            self.retry_after
                .insert(cooldown_key.to_string(), now + REMOTE_FETCH_RETRY_COOLDOWN);
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
    peer_states: Mutex<BTreeMap<String, PeerRuntimeRecord>>,
    connection_generation: AtomicU64,
    request_rates: RequestRateLedger,
}

impl PeerAddrBook {
    pub fn new(endpoint: Endpoint, discovery: Arc<MemoryLookup>) -> Self {
        Self {
            endpoint,
            discovery,
            learned_peers: Mutex::new(BTreeMap::new()),
            seed_peers: Mutex::new(BTreeMap::new()),
            imported_peers: Mutex::new(BTreeMap::new()),
            peer_states: Mutex::new(BTreeMap::new()),
            connection_generation: AtomicU64::new(0),
            request_rates: RequestRateLedger::default(),
        }
    }

    /// 3 台帳の合成(learned → seed → imported の順、id 重複排除)。
    pub async fn merged_peers(&self) -> Vec<EndpointAddr> {
        let mut peers = self
            .learned_peers
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for peer in self.seed_peers.lock().await.values() {
            if !peers.iter().any(|existing| existing.id == peer.id) {
                peers.push(peer.clone());
            }
        }
        for peer in self.imported_peers.lock().await.values() {
            if !peers.iter().any(|existing| existing.id == peer.id) {
                peers.push(peer.clone());
            }
        }
        peers
    }

    /// Prefer currently connected and recently successful peers. A peer in backoff remains in the
    /// list as a recovery probe, but is tried after healthy or unknown peers.
    pub async fn ranked_peers(&self) -> Vec<EndpointAddr> {
        let mut peers = self.merged_peers().await;
        let states = self.peer_states.lock().await;
        let now = Instant::now();
        peers.sort_by_key(|peer| {
            let state = states.get(&peer.id.to_string());
            let backing_off = state
                .and_then(|state| state.retry_after)
                .is_some_and(|retry_after| retry_after > now);
            let connection_rank = match state.map(|state| state.connection_status_at(now)) {
                Some(PeerConnectionStatus::Connected) => 0,
                Some(PeerConnectionStatus::Connecting) => 1,
                Some(PeerConnectionStatus::Unknown) | None => 2,
                Some(PeerConnectionStatus::Disconnected) => 3,
            };
            let recent_success = state
                .and_then(|state| state.last_success_at)
                .is_some_and(|at| now.duration_since(at) <= PEER_FETCH_SUCCESS_TTL);
            let success_rank = usize::from(!recent_success);
            let failures = state
                .filter(|_| backing_off)
                .map(|state| state.consecutive_fetch_failures)
                .unwrap_or_default();
            let latency = state
                .and_then(|state| state.smoothed_fetch_latency_ms)
                .unwrap_or(u64::MAX);
            (
                backing_off,
                connection_rank,
                success_rank,
                failures,
                latency,
            )
        });
        peers
    }

    pub async fn record_connection_state(
        &self,
        peer: EndpointId,
        generation: u64,
        status: PeerConnectionStatus,
    ) {
        let mut states = self.peer_states.lock().await;
        let state = states.entry(peer.to_string()).or_default();
        if generation < state.connection_generation {
            return;
        }
        state.connection_generation = generation;
        state.connection_status = status;
        state.connection_observed_at = Some(Instant::now());
    }

    pub async fn begin_connection_attempt(&self, peer: EndpointId) -> u64 {
        let generation = self.connection_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.record_connection_state(peer, generation, PeerConnectionStatus::Connecting)
            .await;
        generation
    }

    pub async fn record_peer_fetch_request(&self, peer: EndpointId) -> RequestRateDecision {
        self.request_rates
            .check_and_record(
                RequestRateSubject::PeerEndpoint(peer.to_string()),
                RequestRateClass::P2pRequest,
                1,
                RequestRatePolicy {
                    limit: PEER_FETCH_REQUEST_LIMIT,
                    window: PEER_FETCH_REQUEST_WINDOW,
                },
                Instant::now(),
            )
            .await
    }

    pub async fn record_fetch_success(&self, peer: EndpointId, latency: Duration) {
        let mut states = self.peer_states.lock().await;
        let state = states.entry(peer.to_string()).or_default();
        state.fetch_successes = state.fetch_successes.saturating_add(1);
        state.consecutive_fetch_failures = 0;
        state.retry_after = None;
        state.last_success_at = Some(Instant::now());
        state.connection_status = PeerConnectionStatus::Connected;
        state.connection_observed_at = state.last_success_at;
        let latency_ms = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX);
        state.smoothed_fetch_latency_ms = Some(match state.smoothed_fetch_latency_ms {
            Some(previous) => previous.saturating_mul(3).saturating_add(latency_ms) / 4,
            None => latency_ms,
        });
    }

    pub async fn record_fetch_failure(&self, peer: EndpointId, failure: PeerFetchFailure) {
        let mut states = self.peer_states.lock().await;
        let state = states.entry(peer.to_string()).or_default();
        state.fetch_failures = state.fetch_failures.saturating_add(1);
        state.consecutive_fetch_failures = state.consecutive_fetch_failures.saturating_add(1);
        if matches!(
            failure,
            PeerFetchFailure::ConnectFailed | PeerFetchFailure::ConnectTimeout
        ) {
            state.connection_status = PeerConnectionStatus::Disconnected;
        }
        if !matches!(failure, PeerFetchFailure::Cancelled) {
            let exponent = state.consecutive_fetch_failures.saturating_sub(1).min(5);
            let delay = PEER_FETCH_BACKOFF_BASE
                .saturating_mul(2u32.saturating_pow(exponent))
                .min(PEER_FETCH_BACKOFF_MAX);
            state.retry_after = Some(Instant::now() + delay);
        }
    }

    pub async fn peer_state_snapshot(&self, peer: EndpointId) -> Option<PeerStateSnapshot> {
        self.peer_states
            .lock()
            .await
            .get(&peer.to_string())
            .map(|state| state.snapshot(Instant::now()))
    }

    /// learned 台帳へ挿入し、台帳に変化があったかを返す(同値なら false)。
    pub async fn insert_learned_peer_addr(&self, endpoint_addr: EndpointAddr) -> bool {
        if !endpoint_addr.is_empty() {
            self.discovery.add_endpoint_info(endpoint_addr.clone());
        }
        let key = endpoint_addr.id.to_string();
        let mut learned_peers = self.learned_peers.lock().await;
        if learned_peers.get(key.as_str()) == Some(&endpoint_addr) {
            return false;
        }
        learned_peers.insert(key, endpoint_addr);
        true
    }

    pub async fn insert_imported_peer_addr(&self, endpoint_addr: EndpointAddr) {
        self.discovery.add_endpoint_info(endpoint_addr.clone());
        self.imported_peers
            .lock()
            .await
            .insert(endpoint_addr.id.to_string(), endpoint_addr);
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
        Ok(self.insert_learned_peer_addr(endpoint_addr).await)
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
            self.discovery.add_endpoint_info(endpoint_addr.clone());
            parsed.insert(endpoint_addr.id.to_string(), endpoint_addr);
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
        let peers = self.merged_peers().await;
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

    /// 状態保存用のスナップショット(learned / imported。seed は設定から再構築される)。
    pub async fn learned_peers_snapshot(&self) -> Vec<EndpointAddr> {
        self.learned_peers.lock().await.values().cloned().collect()
    }

    pub async fn imported_peers_snapshot(&self) -> Vec<EndpointAddr> {
        self.imported_peers.lock().await.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::presets;

    #[tokio::test]
    async fn successful_peer_is_ranked_before_recently_timed_out_peer() {
        let endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let discovery = Arc::new(MemoryLookup::new());
        let book = PeerAddrBook::new(endpoint, discovery);
        let slow_endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let fast_endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let slow = slow_endpoint.id();
        let fast = fast_endpoint.id();
        book.set_seed_peers(
            vec![
                SeedPeer {
                    endpoint_id: slow.to_string(),
                    addr_hint: Some("192.0.2.1:4433".into()),
                },
                SeedPeer {
                    endpoint_id: fast.to_string(),
                    addr_hint: Some("192.0.2.2:4433".into()),
                },
            ],
            &[],
        )
        .await
        .unwrap();

        book.record_fetch_failure(slow, PeerFetchFailure::TransferTimeout)
            .await;
        book.record_fetch_success(fast, Duration::from_millis(12))
            .await;

        let ranked = book.ranked_peers().await;
        assert_eq!(ranked.first().map(|peer| peer.id), Some(fast));
        assert_eq!(ranked.last().map(|peer| peer.id), Some(slow));
    }

    #[tokio::test]
    async fn stale_disconnect_does_not_replace_newer_connection_generation() {
        let endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let discovery = Arc::new(MemoryLookup::new());
        let book = PeerAddrBook::new(endpoint, discovery);
        let peer_endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let peer = peer_endpoint.id();

        book.record_connection_state(peer, 2, PeerConnectionStatus::Connected)
            .await;
        book.record_connection_state(peer, 1, PeerConnectionStatus::Disconnected)
            .await;

        let snapshot = book.peer_state_snapshot(peer).await.unwrap();
        assert_eq!(snapshot.connection_generation, 2);
        assert_eq!(snapshot.connection_status, PeerConnectionStatus::Connected);
    }

    #[test]
    fn stale_connection_observation_expires_to_unknown() {
        let now = Instant::now();
        let state = PeerRuntimeRecord {
            connection_generation: 7,
            connection_status: PeerConnectionStatus::Connected,
            connection_observed_at: Some(now),
            ..PeerRuntimeRecord::default()
        };
        assert_eq!(
            state
                .snapshot(now + PEER_CONNECTION_STATE_TTL + Duration::from_secs(1))
                .connection_status,
            PeerConnectionStatus::Unknown
        );
    }

    #[tokio::test]
    async fn request_frequency_keeps_http_peer_and_relay_subjects_separate() {
        let ledger = RequestRateLedger::default();
        let now = Instant::now();
        let policy = RequestRatePolicy {
            limit: 1,
            window: Duration::from_secs(10),
        };
        assert_eq!(
            ledger
                .check_and_record(
                    RequestRateSubject::HttpIp("192.0.2.1".into()),
                    RequestRateClass::HttpRequest,
                    1,
                    policy,
                    now,
                )
                .await,
            RequestRateDecision::Allowed
        );
        assert!(matches!(
            ledger
                .check_and_record(
                    RequestRateSubject::HttpIp("192.0.2.1".into()),
                    RequestRateClass::HttpRequest,
                    1,
                    policy,
                    now,
                )
                .await,
            RequestRateDecision::Limited { .. }
        ));
        assert_eq!(
            ledger
                .check_and_record(
                    RequestRateSubject::PeerEndpoint("192.0.2.1".into()),
                    RequestRateClass::P2pRequest,
                    1,
                    policy,
                    now,
                )
                .await,
            RequestRateDecision::Allowed
        );
        assert_eq!(
            ledger
                .check_and_record(
                    RequestRateSubject::RelayClient("192.0.2.1".into()),
                    RequestRateClass::RelayIngressBytes,
                    1,
                    policy,
                    now,
                )
                .await,
            RequestRateDecision::Allowed
        );
    }

    // かつて docs-sync / blob-service に同名で重複していたテストの単一版(WP-H2)。
    // #1207: 実行中の同じ対象は並行させず、先行する走査へ合流させる契約に変えた。
    #[test]
    fn remote_fetch_retry_state_joins_active_fetches_and_cools_down_failures() {
        let now = Instant::now();
        let mut state = RemoteFetchRetryState::default();

        let RemoteFetchBegin::Lead(_sender) = state.begin("hash-a", "store:hash-a", now) else {
            panic!("first caller must lead");
        };
        assert!(matches!(
            state.begin("hash-a", "store:hash-a", now),
            RemoteFetchBegin::Join(_)
        ));
        // 保存先が違う取得は合流しない。
        let RemoteFetchBegin::Lead(_ephemeral) = state.begin("hash-a", "ephemeral:hash-a", now)
        else {
            panic!("a different flight key must not join");
        };
        state.finish("hash-a", "ephemeral:hash-a", false, now);

        state.finish("hash-a", "store:hash-a", false, now);
        assert_eq!(state.in_flight_len(), 0);
        assert!(matches!(
            state.begin("hash-a", "store:hash-a", now + Duration::from_secs(1)),
            RemoteFetchBegin::CoolingDown
        ));
        let RemoteFetchBegin::Lead(_retry) =
            state.begin("hash-a", "store:hash-a", now + REMOTE_FETCH_RETRY_COOLDOWN)
        else {
            panic!("cooldown expiry must allow a new attempt");
        };

        state.finish(
            "hash-a",
            "store:hash-a",
            true,
            now + REMOTE_FETCH_RETRY_COOLDOWN,
        );
        assert_eq!(state.cooldown_len(), 0);
    }

    #[test]
    fn remote_fetch_retry_state_does_not_join_an_abandoned_reservation() {
        let now = Instant::now();
        let mut state = RemoteFetchRetryState::default();
        let RemoteFetchBegin::Lead(sender) = state.begin("hash-a", "store:hash-a", now) else {
            panic!("first caller must lead");
        };
        drop(sender);
        assert!(matches!(
            state.begin("hash-a", "store:hash-a", now),
            RemoteFetchBegin::Lead(_)
        ));
    }

    #[test]
    fn remote_fetch_retry_state_prunes_expired_cooldowns() {
        let now = Instant::now();
        let mut state = RemoteFetchRetryState::default();
        for index in 0..4 {
            let key = format!("hash-{index}");
            let _ = state.begin(&key, &key, now);
            state.finish(&key, &key, false, now);
        }
        assert_eq!(state.cooldown_len(), 4);
        let later = now + REMOTE_FETCH_RETRY_COOLDOWN + Duration::from_secs(1);
        let _ = state.begin("hash-z", "hash-z", later);
        state.finish("hash-z", "hash-z", false, later);
        assert_eq!(state.cooldown_len(), 1);
    }
}
