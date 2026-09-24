use super::*;

/// snapshot 時に `Endpoint::remote_info` から観測した peer ごとの active transport 経路。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ObservedPeerPath {
    pub(crate) has_active_ip: bool,
    pub(crate) has_active_relay: bool,
}

impl ObservedPeerPath {
    /// 実データが relay 経由でのみ流れている(active な IP 経路を持たない)peer。
    /// remote_info が観測できない peer は relay_carried ではない扱いにして旧判定へ倒す。
    fn relay_carried(&self) -> bool {
        self.has_active_relay && !self.has_active_ip
    }
}

/// topic 単位の active_path 判定。precedence 順:
/// 1. connected peer が 1 つ以上あり、全員が relay_carried → RelayFallback
/// 2. rendezvous peer あり → RelaySupportedP2p
/// 3. それ以外 → DirectP2p
///
/// 返り値の fallback_peer_ids は active_path に関わらず relay_carried な peer を列挙する
/// (部分 fallback の可視化。判定 2/3 の結果には影響しない)。
fn topic_connection_path(
    connected_peers: &[String],
    rendezvous_peer_ids: &[String],
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
        } else if !rendezvous_peer_ids.is_empty() {
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

/// 集約 fallback_peer_ids は全 topic の union(sorted dedup)。
fn aggregate_fallback_peer_ids(topic_diagnostics: &[TopicPeerSnapshot]) -> Vec<String> {
    topic_diagnostics
        .iter()
        .flat_map(|topic| topic.fallback_peer_ids.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

impl IrohGossipTransport {
    pub(crate) async fn bootstrap_peers(&self) -> Result<Vec<EndpointAddr>> {
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

    pub(crate) async fn remember_hot_endpoint(&self, peer: EndpointAddr) {
        if self.account_store.is_none() {
            self.discovery.add_endpoint_info(peer);
            return;
        }
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

    pub(crate) async fn configured_seed_peer_ids(&self) -> Vec<String> {
        self.configured_seed_peers
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    }

    pub(crate) async fn bootstrap_seed_peer_ids(&self) -> Vec<String> {
        self.bootstrap_seed_peers
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    }

    async fn configured_peer_ids(&self) -> Result<Vec<String>> {
        let mut ids = self
            .configured_seed_peers
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        ids.extend(self.bootstrap_seed_peers.lock().await.keys().cloned());
        if let Some(store) = &self.account_store {
            ids.extend(
                store
                    .peer_candidate_window(
                        "gossip",
                        "imported",
                        None,
                        4,
                        Utc::now().timestamp_millis(),
                    )
                    .await?
                    .into_iter()
                    .map(|(id, _, _)| id),
            );
        } else {
            ids.extend(self.imported_peers.lock().await.keys().cloned());
        }
        Ok(ids.into_iter().collect())
    }

    pub(crate) async fn connected_peer_ids(&self) -> Vec<String> {
        let mut connected = BTreeSet::new();
        for (_, state) in self.topic_states.lock().await.iter() {
            for peer in state.neighbors.read().await.iter() {
                connected.insert(peer.clone());
            }
        }
        connected.into_iter().collect::<Vec<_>>()
    }

    pub(crate) async fn transport_peers_impl(&self) -> Result<PeerSnapshot> {
        let topic_states = self
            .topic_states
            .lock()
            .await
            .iter()
            .map(|(topic, state)| {
                (
                    topic.clone(),
                    state.bootstrap_peer_ids.iter().cloned().collect::<Vec<_>>(),
                    Arc::clone(&state.neighbors),
                    Arc::clone(&state.last_received_at),
                    Arc::clone(&state.last_error),
                )
            })
            .collect::<Vec<_>>();
        let mut connected = BTreeSet::new();
        let configured_peers = self.configured_peer_ids().await?;
        let bootstrap_seed_peer_ids = self.bootstrap_seed_peer_ids().await;
        let mut topic_entries = Vec::with_capacity(topic_states.len());
        for (topic, configured_peer_ids, neighbors, last_received_at, last_error) in topic_states {
            let peers = neighbors.read().await.iter().cloned().collect::<Vec<_>>();
            let last_received_at = *last_received_at.lock().await;
            let last_error = last_error.lock().await.clone();
            for peer in &peers {
                connected.insert(peer.clone());
            }
            topic_entries.push((
                topic,
                configured_peer_ids,
                peers,
                last_received_at,
                last_error,
            ));
        }
        let observed_paths = self.observed_peer_paths(&connected).await;
        let mut topic_diagnostics = Vec::with_capacity(topic_entries.len());
        for (topic, configured_peer_ids, peers, last_received_at, last_error) in topic_entries {
            let configured_peer_count = configured_peer_ids.len();
            let connected_peer_count = peers.len();
            let missing_peer_ids = configured_peer_ids
                .iter()
                .filter(|peer| !peers.iter().any(|connected_peer| connected_peer == *peer))
                .cloned()
                .collect::<Vec<_>>();
            let rendezvous_peer_ids = peers
                .iter()
                .filter(|peer| bootstrap_seed_peer_ids.contains(peer))
                .cloned()
                .collect::<Vec<_>>();
            let (active_path, fallback_peer_ids) =
                topic_connection_path(&peers, &rendezvous_peer_ids, &observed_paths);
            topic_diagnostics.push(TopicPeerSnapshot {
                topic,
                joined: !peers.is_empty(),
                peer_count: connected_peer_count,
                connected_peers: peers,
                configured_peer_ids,
                missing_peer_ids,
                active_path,
                rendezvous_peer_ids,
                fallback_peer_ids,
                last_received_at,
                status_detail: topic_status_detail(configured_peer_count, connected_peer_count),
                last_error,
            });
        }
        topic_diagnostics.sort_by(|left, right| left.topic.cmp(&right.topic));
        let subscribed_topics = self
            .subscribed_topics
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let connected_peers = connected.into_iter().collect::<Vec<_>>();
        let configured_peer_count = configured_peers.len();
        let connected_peer_count = connected_peers.len();
        let subscribed_topic_count = topic_diagnostics.len();
        let active_path = aggregate_connection_path(&topic_diagnostics);
        let fallback_peer_ids = aggregate_fallback_peer_ids(&topic_diagnostics);

        Ok(PeerSnapshot {
            connected: !connected_peers.is_empty(),
            peer_count: connected_peer_count,
            connected_peers,
            configured_peers,
            subscribed_topics,
            active_path,
            fallback_peer_ids,
            pending_events: 0,
            status_detail: peer_status_detail(
                configured_peer_count,
                connected_peer_count,
                subscribed_topic_count,
            ),
            last_error: self.last_error.lock().await.clone(),
            topic_diagnostics,
        })
    }

    /// connected peer ごとに remote_info を観測し、active な transport addr の種別を集計する。
    /// endpoint id が parse できない / remote_info が無い peer は観測なし(map に載せない)。
    async fn observed_peer_paths(
        &self,
        peer_ids: &BTreeSet<String>,
    ) -> BTreeMap<String, ObservedPeerPath> {
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
                }
            }
            observed.insert(peer_id.clone(), path);
        }
        observed
    }

    pub(crate) async fn transport_export_ticket_impl(&self) -> Result<Option<String>> {
        let endpoint_addr = self.endpoint.addr();
        let ticket_config = ticket_network_config(
            &endpoint_addr,
            &self.endpoint.bound_sockets(),
            &self.network_config,
        );
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
                    },
                )
            })
            .collect()
    }

    fn topic_with_path(path: ConnectionPath, fallback_peer_ids: &[&str]) -> TopicPeerSnapshot {
        TopicPeerSnapshot {
            active_path: path,
            fallback_peer_ids: peers(fallback_peer_ids),
            ..TopicPeerSnapshot::default()
        }
    }

    // --- characterization: 既存 2 経路の判定は観測情報なしでは不変 ---

    #[test]
    fn topic_path_without_rendezvous_is_direct() {
        let (path, fallback) = topic_connection_path(&peers(&["peer-a"]), &[], &BTreeMap::new());
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert!(fallback.is_empty());
    }

    #[test]
    fn topic_path_with_rendezvous_is_relay_supported() {
        let (path, fallback) = topic_connection_path(
            &peers(&["seed-a", "peer-b"]),
            &peers(&["seed-a"]),
            &BTreeMap::new(),
        );
        assert_eq!(path, ConnectionPath::RelaySupportedP2p);
        assert!(fallback.is_empty());
    }

    #[test]
    fn topic_path_without_peers_is_direct_even_with_rendezvous_config() {
        let (path, fallback) = topic_connection_path(&[], &[], &BTreeMap::new());
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert!(fallback.is_empty());
    }

    #[test]
    fn aggregate_any_relay_supported_topic_is_relay_supported() {
        let topics = vec![
            topic_with_path(ConnectionPath::DirectP2p, &[]),
            topic_with_path(ConnectionPath::RelaySupportedP2p, &[]),
        ];
        assert_eq!(
            aggregate_connection_path(&topics),
            ConnectionPath::RelaySupportedP2p
        );
        assert!(aggregate_fallback_peer_ids(&topics).is_empty());
    }

    #[test]
    fn aggregate_without_topics_is_direct() {
        assert_eq!(aggregate_connection_path(&[]), ConnectionPath::DirectP2p);
        assert!(aggregate_fallback_peer_ids(&[]).is_empty());
    }

    // --- relay fallback 判定 ---

    #[test]
    fn topic_path_all_peers_relay_carried_is_relay_fallback() {
        let (path, fallback) = topic_connection_path(
            &peers(&["peer-a", "peer-b"]),
            &[],
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
            &[],
            &observed(&[("peer-a", false, true), ("peer-b", true, false)]),
        );
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert_eq!(fallback, peers(&["peer-a"]));

        // 同じ構成で rendezvous peer がいれば旧判定どおり relay_supported。
        let (path, fallback) = topic_connection_path(
            &peers(&["peer-a", "peer-b"]),
            &peers(&["peer-b"]),
            &observed(&[("peer-a", false, true), ("peer-b", true, false)]),
        );
        assert_eq!(path, ConnectionPath::RelaySupportedP2p);
        assert_eq!(fallback, peers(&["peer-a"]));
    }

    #[test]
    fn topic_path_with_holepunched_peer_is_not_relay_carried() {
        // relay と IP の両方が active(holepunch 済み)な peer は relay_carried ではない。
        let (path, fallback) = topic_connection_path(
            &peers(&["peer-a"]),
            &[],
            &observed(&[("peer-a", true, true)]),
        );
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert!(fallback.is_empty());
    }

    #[test]
    fn topic_path_with_unobserved_peer_keeps_previous_rule() {
        // remote_info が観測できない peer は relay_carried 扱いにしない(旧判定へ倒す)。
        let (path, fallback) = topic_connection_path(
            &peers(&["peer-a", "peer-b"]),
            &[],
            &observed(&[("peer-a", false, true)]),
        );
        assert_eq!(path, ConnectionPath::DirectP2p);
        assert_eq!(fallback, peers(&["peer-a"]));
    }

    #[test]
    fn aggregate_any_relay_fallback_topic_is_relay_fallback() {
        let topics = vec![
            topic_with_path(ConnectionPath::RelaySupportedP2p, &[]),
            topic_with_path(ConnectionPath::RelayFallback, &["peer-b", "peer-a"]),
            topic_with_path(ConnectionPath::RelayFallback, &["peer-a"]),
        ];
        assert_eq!(
            aggregate_connection_path(&topics),
            ConnectionPath::RelayFallback
        );
        assert_eq!(
            aggregate_fallback_peer_ids(&topics),
            peers(&["peer-a", "peer-b"])
        );
    }
}
