use super::*;

/// snapshot 時に `Endpoint::remote_info` から観測した peer ごとの active transport 経路。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ObservedPeerPath {
    pub(crate) has_active_ip: bool,
    pub(crate) has_active_relay: bool,
    /// ブラウザとの直接経路(QUIC over WebRTC。接続交渉に relay を使う。ADR 0057 §9)。
    pub(crate) has_active_custom: bool,
}

impl ObservedPeerPath {
    /// 実データが relay 経由でのみ流れている(active な IP・custom 経路を持たない)peer。
    /// remote_info が観測できない peer は relay_carried ではない扱いにして旧判定へ倒す。
    fn relay_carried(&self) -> bool {
        self.has_active_relay && !self.has_active_ip && !self.has_active_custom
    }

    /// relay で接続交渉した直接経路(custom)で実データが流れる peer。
    fn relay_supported(&self) -> bool {
        self.has_active_custom && !self.has_active_ip
    }
}

/// topic 単位の active_path 判定。precedence 順:
/// 1. connected peer が 1 つ以上あり、全員が relay_carried → RelayFallback
/// 2. rendezvous peer あり、または relay で交渉した custom 経路の peer あり → RelaySupportedP2p
/// 3. それ以外 → DirectP2p
///
/// 返り値の fallback_peer_ids は active_path に関わらず relay_carried な peer を列挙する
/// (部分 fallback の可視化。判定 2/3 の結果には影響しない)。
fn topic_connection_path(
    connected_peers: &[String],
    rendezvous_peer_count: usize,
    observed_paths: &BTreeMap<String, ObservedPeerPath>,
) -> (ConnectionPath, Vec<String>) {
    let fallback_peer_ids = connected_peers
        .iter()
        .filter(|peer| {
            observed_paths
                .get(*peer)
                .is_some_and(|path| path.relay_carried())
        })
        .cloned()
        .collect::<Vec<_>>();
    let active_path =
        if !connected_peers.is_empty() && fallback_peer_ids.len() == connected_peers.len() {
            ConnectionPath::RelayFallback
        } else if rendezvous_peer_count > 0
            || connected_peers.iter().any(|peer| {
                observed_paths
                    .get(peer)
                    .is_some_and(ObservedPeerPath::relay_supported)
            })
        {
            ConnectionPath::RelaySupportedP2p
        } else {
            ConnectionPath::DirectP2p
        };
    (active_path, fallback_peer_ids)
}

/// app-api 側 `connection_path_rank` と同順(Direct=0 < RelaySupported=1 < RelayFallback=2)。
fn connection_path_rank(path: &ConnectionPath) -> u8 {
    match path {
        ConnectionPath::DirectP2p => 0,
        ConnectionPath::RelaySupportedP2p => 1,
        ConnectionPath::RelayFallback => 2,
    }
}

/// 集約 active_path は全 topic の max-rank(悪い経路優先)。topic なしは DirectP2p。
fn aggregate_connection_path(topic_diagnostics: &[TopicPeerSnapshot]) -> ConnectionPath {
    topic_diagnostics
        .iter()
        .map(|topic| topic.active_path.clone())
        .max_by_key(connection_path_rank)
        .unwrap_or_default()
}

/// 状態の読取りで使う topic の部品(topic の lock を持ち続けないため、共有の部品だけを clone する)。
struct TopicStateView {
    topic: String,
    configured: BTreeSet<String>,
    neighbors: Arc<RwLock<BTreeSet<String>>>,
    last_received_at: Arc<Mutex<Option<i64>>>,
    last_error: Arc<Mutex<Option<String>>>,
    rendezvous: Arc<Mutex<Vec<(String, EndpointAddr)>>>,
}

/// seed・ticket の候補を 4 件ずつ読む部品。neighbor の無い topic の再 join の task からも読むため、
/// 読む先を `Arc` で共有する(#1221 R2-B)。
#[derive(Clone)]
pub(crate) struct BootstrapCandidates {
    discovery: Arc<MemoryLookup>,
    configured_seed_peers: Arc<Mutex<BTreeMap<String, EndpointAddr>>>,
    bootstrap_seed_peers: Arc<Mutex<BTreeMap<String, EndpointAddr>>>,
    imported_peers: Arc<Mutex<BTreeMap<String, EndpointAddr>>>,
    account_store: Option<Arc<crate::PeerCandidateStore>>,
    imported_cursor: Arc<Mutex<Option<(i64, String)>>>,
    hot_peer_ids: Arc<Mutex<VecDeque<EndpointId>>>,
    gossip_health: Arc<crate::peers::BlobPeerHealth>,
    bootstrap_cursor: Arc<Mutex<(usize, [Option<String>; 3])>>,
}

impl BootstrapCandidates {
    pub(crate) async fn window(&self) -> Result<Vec<EndpointAddr>> {
        let mut cursor = self.bootstrap_cursor.lock().await;
        let mut peers = Vec::with_capacity(4);
        let mut seen = BTreeSet::new();
        // Rotate the starting source as well as each source's key. A large seed
        // list must not hide a later ticket, and old candidates get revisited.
        for offset in 0..12 {
            let source = (cursor.0 + offset) % 3;
            let next = if source == 2
                && let Some(store) = &self.account_store
            {
                let mut imported_cursor = self.imported_cursor.lock().await;
                let page = store
                    .peer_candidate_window(
                        "gossip",
                        "imported",
                        imported_cursor.clone(),
                        1,
                        Utc::now().timestamp_millis(),
                    )
                    .await?;
                page.into_iter()
                    .next()
                    .map(|(key, bytes, seen_ms)| {
                        *imported_cursor = Some((seen_ms, key.clone()));
                        serde_json::from_slice(&bytes).map(|peer| (key, peer))
                    })
                    .transpose()?
            } else {
                let entries = match source {
                    0 => self.configured_seed_peers.lock().await,
                    1 => self.bootstrap_seed_peers.lock().await,
                    _ => self.imported_peers.lock().await,
                };
                crate::peers::fetch_source_window(&entries, &mut cursor.1[source], 1)
                    .into_iter()
                    .next()
                    .and_then(|key| entries.get(&key).cloned().map(|peer| (key, peer)))
            };
            if let Some((key, peer)) = next {
                cursor.1[source] = Some(key.clone());
                if seen.insert(key) {
                    peers.push(peer);
                    if peers.len() == 4 {
                        break;
                    }
                }
            }
        }
        cursor.0 = (cursor.0 + 1) % 3;
        self.gossip_health.rank(&mut peers).await;
        for peer in &peers {
            self.remember_hot_endpoint(peer.clone()).await;
        }
        Ok(peers)
    }

    /// 最近使った endpoint を `MemoryLookup` に 16 件まで置き、古いものから外す（store の有無にかかわらない。ADR 0056 §5）。
    pub(crate) async fn remember_hot_endpoint(&self, peer: EndpointAddr) {
        let mut hot = self.hot_peer_ids.lock().await;
        hot.retain(|id| id != &peer.id);
        self.discovery.add_endpoint_info(peer.clone());
        hot.push_back(peer.id);
        while hot.len() > 16 {
            if let Some(id) = hot.pop_front() {
                self.discovery.remove_endpoint_info(id);
            }
        }
    }
}

impl IrohGossipTransport {
    pub(crate) fn bootstrap_candidates(&self) -> BootstrapCandidates {
        BootstrapCandidates {
            discovery: Arc::clone(&self.discovery),
            configured_seed_peers: Arc::clone(&self.configured_seed_peers),
            bootstrap_seed_peers: Arc::clone(&self.bootstrap_seed_peers),
            imported_peers: Arc::clone(&self.imported_peers),
            account_store: self.account_store.clone(),
            imported_cursor: Arc::clone(&self.imported_cursor),
            hot_peer_ids: Arc::clone(&self.hot_peer_ids),
            gossip_health: Arc::clone(&self.gossip_health),
            bootstrap_cursor: Arc::clone(&self.bootstrap_cursor),
        }
    }

    pub(crate) async fn bootstrap_peers(&self) -> Result<Vec<EndpointAddr>> {
        self.bootstrap_candidates().window().await
    }

    pub(crate) async fn remember_hot_endpoint(&self, peer: EndpointAddr) {
        self.bootstrap_candidates()
            .remember_hot_endpoint(peer)
            .await
    }

    /// 計測用: 状態の読取りで見た topic・peer と `remote_info` の数を足す(#1221 R2-D)。
    fn count_status_read_steps(&self, _steps: usize) {
        #[cfg(any(test, feature = "test-support"))]
        self.status_read_steps
            .fetch_add(_steps as u64, Ordering::Relaxed);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn status_read_steps(&self) -> u64 {
        self.status_read_steps.load(Ordering::Relaxed)
    }

    /// 稼働中の topic(lease と短期の送信先)の部品。seed・台帳・SQLite は読まない(#1221 R2-D)。
    async fn topic_state_views(&self) -> Vec<TopicStateView> {
        let states = self
            .topic_states
            .lock()
            .await
            .iter()
            .map(|(topic, state)| TopicStateView {
                topic: topic.clone(),
                configured: state.bootstrap_peer_ids.clone(),
                neighbors: Arc::clone(&state.neighbors),
                last_received_at: Arc::clone(&state.last_received_at),
                last_error: Arc::clone(&state.last_error),
                rendezvous: Arc::clone(&state.rendezvous),
            })
            .collect::<Vec<_>>();
        self.count_status_read_steps(states.len());
        states
    }

    async fn connected_peer_set(&self, states: &[TopicStateView]) -> BTreeSet<String> {
        let mut connected = BTreeSet::new();
        for state in states {
            connected.extend(state.neighbors.read().await.iter().cloned());
        }
        self.count_status_read_steps(connected.len());
        connected
    }

    pub(crate) async fn connected_peer_count(&self) -> usize {
        let states = self.topic_state_views().await;
        self.connected_peer_set(&states).await.len()
    }

    /// 設定済みの peer の数(設定した seed と CN の seed)。取り込んだ ticket は詳細のページで読む。
    pub(crate) async fn configured_peer_count(&self) -> usize {
        self.configured_seed_peers.lock().await.len() + self.bootstrap_seed_peers.lock().await.len()
    }

    pub(crate) async fn transport_peers_impl(&self) -> Result<PeerSnapshot> {
        let states = self.topic_state_views().await;
        let connected = self.connected_peer_set(&states).await;
        let observed_paths = self.observed_peer_paths(&connected).await;
        let mut topic_diagnostics = Vec::with_capacity(states.len());
        let mut fallback = BTreeSet::new();
        for state in states {
            let peers = state.neighbors.read().await.clone();
            let rendezvous = state
                .rendezvous
                .lock()
                .await
                .iter()
                .map(|(_, peer)| peer.id.to_string())
                .collect::<BTreeSet<_>>();
            let bootstrap = self.bootstrap_seed_peers.lock().await;
            let rendezvous_peer_count = peers
                .iter()
                .filter(|peer| bootstrap.contains_key(*peer) || rendezvous.contains(*peer))
                .count();
            drop(bootstrap);
            let connected_peers = peers.iter().cloned().collect::<Vec<_>>();
            let (active_path, fallback_peer_ids) =
                topic_connection_path(&connected_peers, rendezvous_peer_count, &observed_paths);
            topic_diagnostics.push(TopicPeerSnapshot {
                joined: !peers.is_empty(),
                peer_count: peers.len(),
                configured_peer_count: state.configured.len(),
                missing_peer_count: state.configured.difference(&peers).count(),
                active_path,
                rendezvous_peer_count,
                fallback_peer_count: fallback_peer_ids.len(),
                last_received_at: *state.last_received_at.lock().await,
                status_detail: topic_status_detail(state.configured.len(), peers.len()),
                last_error: state.last_error.lock().await.clone(),
                topic: state.topic,
            });
            fallback.extend(fallback_peer_ids);
        }
        topic_diagnostics.sort_by(|left, right| left.topic.cmp(&right.topic));
        let subscribed_topics = self
            .subscribed_topics
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let configured_peer_count = self.configured_peer_count().await;
        Ok(PeerSnapshot {
            connected: !connected.is_empty(),
            peer_count: connected.len(),
            configured_peer_count,
            active_path: aggregate_connection_path(&topic_diagnostics),
            fallback_peer_count: fallback.len(),
            pending_events: 0,
            status_detail: peer_status_detail(
                configured_peer_count,
                connected.len(),
                topic_diagnostics.len(),
            ),
            subscribed_topics,
            last_error: self.last_error.lock().await.clone(),
            topic_diagnostics,
        })
    }

    /// 詳細のページ。seed と topic の集合は id の順の範囲から読み、取り込んだ ticket は索引の順に SQLite から
    /// 1 ページだけ読む。取得の候補の cursor は進めない(#1221 R2-D)。
    pub(crate) async fn peer_page_impl(
        &self,
        kind: ConnectivityPeerKind,
        topic: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<PeerPage> {
        use std::ops::Bound::{Excluded, Unbounded};
        let after = cursor.map_or(Unbounded, |cursor| Excluded(cursor.to_string()));
        let page_of_map = |map: &BTreeMap<String, EndpointAddr>| {
            PeerPage::from_sorted(
                map.range((after.clone(), Unbounded)).map(|(id, _)| id),
                None,
                limit,
            )
        };
        match kind {
            ConnectivityPeerKind::ConfiguredSeed => {
                Ok(page_of_map(&*self.configured_seed_peers.lock().await))
            }
            ConnectivityPeerKind::BootstrapSeed => {
                Ok(page_of_map(&*self.bootstrap_seed_peers.lock().await))
            }
            ConnectivityPeerKind::ManualTicket => match &self.account_store {
                Some(store) => {
                    let ids = store
                        .imported_peer_candidate_ids("gossip", cursor, limit + 1)
                        .await?;
                    Ok(PeerPage::from_sorted(&ids, None, limit))
                }
                None => Ok(page_of_map(&*self.imported_peers.lock().await)),
            },
            ConnectivityPeerKind::Connected
            | ConnectivityPeerKind::Configured
            | ConnectivityPeerKind::Missing => {
                let mut ids = BTreeSet::new();
                for state in self.topic_state_views().await {
                    if topic.is_some_and(|topic| state.topic != topic) {
                        continue;
                    }
                    let neighbors = state.neighbors.read().await;
                    match kind {
                        ConnectivityPeerKind::Connected => ids.extend(neighbors.iter().cloned()),
                        ConnectivityPeerKind::Configured => ids.extend(state.configured),
                        _ => ids.extend(state.configured.difference(&neighbors).cloned()),
                    }
                }
                Ok(PeerPage::from_sorted(
                    ids.range((after, Unbounded)),
                    None,
                    limit,
                ))
            }
            ConnectivityPeerKind::DocsAssist | ConnectivityPeerKind::BlobAssist => {
                Ok(PeerPage::default())
            }
        }
    }

    /// connected peer ごとに remote_info を観測し、active な transport addr の種別を集計する。
    /// endpoint id が parse できない / remote_info が無い peer は観測なし(map に載せない)。
    /// 経路の種別(relay だけか)を見るためで、接続の生存の判定には使わない(ADR 0055 §3)。
    async fn observed_peer_paths(
        &self,
        peer_ids: &BTreeSet<String>,
    ) -> BTreeMap<String, ObservedPeerPath> {
        self.count_status_read_steps(peer_ids.len());
        let mut observed = BTreeMap::new();
        for peer_id in peer_ids {
            let Ok(endpoint_id) = peer_id.parse::<EndpointId>() else {
                continue;
            };
            let Some(info) = self.endpoint.remote_info(endpoint_id).await else {
                continue;
            };
            let mut path = ObservedPeerPath::default();
            for addr in info.addrs() {
                if !matches!(addr.usage(), TransportAddrUsage::Active) {
                    continue;
                }
                if addr.addr().is_relay() {
                    path.has_active_relay = true;
                } else if addr.addr().is_ip() {
                    path.has_active_ip = true;
                } else if addr.addr().is_custom() {
                    path.has_active_custom = true;
                }
            }
            observed.insert(peer_id.clone(), path);
        }
        observed
    }

    pub(crate) async fn transport_export_ticket_impl(&self) -> Result<Option<String>> {
        let endpoint_addr = self.endpoint.addr();
        #[cfg(not(target_family = "wasm"))]
        let bound_sockets = self.endpoint.bound_sockets();
        #[cfg(target_family = "wasm")]
        let bound_sockets = Vec::new();
        let ticket_config =
            ticket_network_config(&endpoint_addr, &bound_sockets, &self.network_config);
        match encode_endpoint_ticket(&endpoint_addr, &ticket_config) {
            Ok(ticket) => Ok(Some(ticket)),
            Err(error)
                if error
                    .to_string()
                    .contains("could not determine advertised host") =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peers(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn observed(entries: &[(&str, bool, bool)]) -> BTreeMap<String, ObservedPeerPath> {
        entries
            .iter()
            .map(|(id, has_active_ip, has_active_relay)| {
                (
                    id.to_string(),
                    ObservedPeerPath {
                        has_active_ip: *has_active_ip,
                        has_active_relay: *has_active_relay,
                        has_active_custom: false,
                    },
                )
            })
            .collect()
    }

    fn topic_with_path(path: ConnectionPath) -> TopicPeerSnapshot {
        TopicPeerSnapshot {
            active_path: path,
            ..TopicPeerSnapshot::default()
        }
    }

    // --- characterization: 既存 2 経路の判定は観測情報なしでは不変 ---

    #[test]
    fn topic_path_without_rendezvous_is_direct() {
        let (path, fallback) = topic_connection_path(&peers(&["peer-a"]), 0, &BTreeMap::new());
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert!(fallback.is_empty());
    }

    #[test]
    fn topic_path_with_rendezvous_is_relay_supported() {
        let (path, fallback) =
            topic_connection_path(&peers(&["seed-a", "peer-b"]), 1, &BTreeMap::new());
        assert_eq!(path, ConnectionPath::RelaySupportedP2p);
        assert!(fallback.is_empty());
    }

    #[test]
    fn topic_path_without_peers_is_direct_even_with_rendezvous_config() {
        let (path, fallback) = topic_connection_path(&[], 0, &BTreeMap::new());
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert!(fallback.is_empty());
    }

    #[test]
    fn aggregate_any_relay_supported_topic_is_relay_supported() {
        let topics = vec![
            topic_with_path(ConnectionPath::DirectP2p),
            topic_with_path(ConnectionPath::RelaySupportedP2p),
        ];
        assert_eq!(
            aggregate_connection_path(&topics),
            ConnectionPath::RelaySupportedP2p
        );
    }

    #[test]
    fn aggregate_without_topics_is_direct() {
        assert_eq!(aggregate_connection_path(&[]), ConnectionPath::DirectP2p);
    }

    // --- relay fallback 判定 ---

    /// #1422 AC-2 J6: custom 経路が開いた relay だけの peer は RelayFallback ではなく Relay Supported P2P。
    /// custom を閉じた後は relay だけの peer に戻る。
    #[test]
    fn topic_path_with_an_active_custom_path_is_relay_supported() {
        let web = |has_active_custom| {
            BTreeMap::from([(
                "web".to_string(),
                ObservedPeerPath {
                    has_active_ip: false,
                    has_active_relay: true,
                    has_active_custom,
                },
            )])
        };
        let (path, fallback) = topic_connection_path(&peers(&["web"]), 0, &web(true));
        assert_eq!(path, ConnectionPath::RelaySupportedP2p);
        assert!(fallback.is_empty());
        let (path, fallback) = topic_connection_path(&peers(&["web"]), 0, &web(false));
        assert_eq!(path, ConnectionPath::RelayFallback);
        assert_eq!(fallback, peers(&["web"]));
    }

    #[test]
    fn topic_path_all_peers_relay_carried_is_relay_fallback() {
        let (path, fallback) = topic_connection_path(
            &peers(&["peer-a", "peer-b"]),
            0,
            &observed(&[("peer-a", false, true), ("peer-b", false, true)]),
        );
        assert_eq!(path, ConnectionPath::RelayFallback);
        assert_eq!(fallback, peers(&["peer-a", "peer-b"]));
    }

    #[test]
    fn topic_path_with_one_active_ip_peer_keeps_previous_rule() {
        // peer-a は relay-only、peer-b は active IP を持つ → fallback 判定にはならず、
        // rendezvous なしなので旧判定どおり direct。fallback_peer_ids は relay-only 側のみ列挙。
        let (path, fallback) = topic_connection_path(
            &peers(&["peer-a", "peer-b"]),
            0,
            &observed(&[("peer-a", false, true), ("peer-b", true, false)]),
        );
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert_eq!(fallback, peers(&["peer-a"]));

        // 同じ構成で rendezvous peer がいれば旧判定どおり relay_supported。
        let (path, fallback) = topic_connection_path(
            &peers(&["peer-a", "peer-b"]),
            1,
            &observed(&[("peer-a", false, true), ("peer-b", true, false)]),
        );
        assert_eq!(path, ConnectionPath::RelaySupportedP2p);
        assert_eq!(fallback, peers(&["peer-a"]));
    }

    #[test]
    fn topic_path_with_holepunched_peer_is_not_relay_carried() {
        // relay と IP の両方が active(holepunch 済み)な peer は relay_carried ではない。
        let (path, fallback) =
            topic_connection_path(&peers(&["peer-a"]), 0, &observed(&[("peer-a", true, true)]));
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert!(fallback.is_empty());
    }

    #[test]
    fn topic_path_with_unobserved_peer_keeps_previous_rule() {
        // remote_info が観測できない peer は relay_carried 扱いにしない(旧判定へ倒す)。
        let (path, fallback) = topic_connection_path(
            &peers(&["peer-a", "peer-b"]),
            0,
            &observed(&[("peer-a", false, true)]),
        );
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert_eq!(fallback, peers(&["peer-a"]));
    }

    #[test]
    fn aggregate_any_relay_fallback_topic_is_relay_fallback() {
        let topics = vec![
            topic_with_path(ConnectionPath::RelaySupportedP2p),
            topic_with_path(ConnectionPath::RelayFallback),
            topic_with_path(ConnectionPath::RelayFallback),
        ];
        assert_eq!(
            aggregate_connection_path(&topics),
            ConnectionPath::RelayFallback
        );
    }
}
