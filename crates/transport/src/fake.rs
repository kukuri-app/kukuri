//! ネットワーク非依存の高速テストダブル(FakeNetwork / FakeTransport)。
//!
//! Decision(WP-B15, 2026-07-16): FakeTransport は実 iroh 実装との「等価ダブル」
//! ではなく、ドメインロジックを高速に検証するための意図的な単純化と定義する
//! (例: active_path は経路判定をせず機械的に返す)。trait 挙動の実物基準の
//! 検証は実 iroh 統合テスト(transport の iroh/tests 16 本 + app-api の
//! TestIrohStack 経由 50 箇所超)が担い、Fake と実物を同一テストベクタで回す
//! 共有 conformance suite は作らない(恒久 Out of Scope)。
//! 再検討のトリガ: Fake と実物の挙動乖離に起因するバグが実際に発生した場合。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures_util::stream;
use iroh::EndpointAddr;
use kukuri_core::{GossipHint, Pubkey, SealedReceiveOfferV1, TopicId, receive_route_for_account};
use tokio::sync::{Mutex, broadcast, watch};
#[cfg(test)]
use tokio::time::timeout;

use crate::config::{
    ConnectMode, ConnectionPath, ConnectivityPeerKind, DiscoveryMode, DiscoverySnapshot, SeedPeer,
};
use crate::diagnostics::{peer_status_detail, topic_status_detail};
use crate::traits::{
    HintEnvelope, HintStream, HintTransport, PeerPage, PeerSnapshot, ReceiveCandidateFence,
    ReceiveOfferEnvelope, ReceiveOfferLease, ReceiveOfferStop, ReceiveOfferSubscription,
    TopicPeerSnapshot, Transport, next_receive_offer_lease,
};

#[derive(Clone, Default)]
pub struct FakeNetwork {
    hints: Arc<Mutex<HashMap<String, broadcast::Sender<HintEnvelope>>>>,
    offers: Arc<Mutex<HashMap<String, broadcast::Sender<ReceiveOfferEnvelope>>>>,
    topic_subscribers: Arc<Mutex<HashMap<String, BTreeSet<String>>>>,
    known_peers: Arc<Mutex<BTreeSet<String>>>,
    verified_receive_providers: Arc<Mutex<HashMap<String, BTreeSet<String>>>>,
    receive_candidates: Arc<Mutex<FakeReceiveCandidates>>,
}

#[derive(Default)]
struct FakeReceiveCandidates {
    clear_epoch: u64,
    by_account: HashMap<String, BTreeMap<String, Vec<EndpointAddr>>>,
}

impl FakeNetwork {
    /// Test-only proof registry. Unregistered account/endpoint pairs fail closed.
    pub async fn trust_receive_provider(&self, account: &Pubkey, endpoint_id: &str) {
        self.verified_receive_providers
            .lock()
            .await
            .entry(account.as_str().to_string())
            .or_default()
            .insert(endpoint_id.to_string());
    }
}

struct FakeOfferRoute {
    route: String,
    lease: ReceiveOfferLease,
    stop: watch::Sender<ReceiveOfferStop>,
}

#[derive(Clone)]
pub struct FakeTransport {
    local_id: String,
    network: FakeNetwork,
    configured_seed_peers: Arc<Mutex<BTreeSet<String>>>,
    bootstrap_seed_peers: Arc<Mutex<BTreeSet<String>>>,
    imported_peers: Arc<Mutex<BTreeSet<String>>>,
    subscribed_topics: Arc<Mutex<BTreeSet<String>>>,
    active_offer_route: Arc<Mutex<Option<FakeOfferRoute>>>,
    discovery_mode: Arc<Mutex<DiscoveryMode>>,
    env_locked: Arc<Mutex<bool>>,
}

impl FakeTransport {
    pub fn new(local_id: impl Into<String>, network: FakeNetwork) -> Self {
        Self {
            local_id: local_id.into(),
            network,
            configured_seed_peers: Arc::new(Mutex::new(BTreeSet::new())),
            bootstrap_seed_peers: Arc::new(Mutex::new(BTreeSet::new())),
            imported_peers: Arc::new(Mutex::new(BTreeSet::new())),
            subscribed_topics: Arc::new(Mutex::new(BTreeSet::new())),
            active_offer_route: Arc::new(Mutex::new(None)),
            discovery_mode: Arc::new(Mutex::new(DiscoveryMode::StaticPeer)),
            env_locked: Arc::new(Mutex::new(false)),
        }
    }

    async fn hint_sender(&self, topic: &TopicId) -> broadcast::Sender<HintEnvelope> {
        let mut topics = self.network.hints.lock().await;
        topics
            .entry(topic.0.clone())
            .or_insert_with(|| broadcast::channel(128).0)
            .clone()
    }

    async fn offer_sender(
        &self,
        recipient: &Pubkey,
    ) -> Result<broadcast::Sender<ReceiveOfferEnvelope>> {
        let route = receive_route_for_account(recipient)?;
        let mut topics = self.network.offers.lock().await;
        Ok(topics
            .entry(route.0)
            .or_insert_with(|| broadcast::channel(64).0)
            .clone())
    }

    async fn subscribe_receive_offers_impl(
        &self,
        recipient: &Pubkey,
        expected: Option<ReceiveOfferLease>,
        if_vacant: bool,
    ) -> Result<Option<ReceiveOfferSubscription>> {
        let route = receive_route_for_account(recipient)?;
        let sender = self.offer_sender(recipient).await?;
        let mut active = self.active_offer_route.lock().await;
        if if_vacant && active.is_some() {
            return Ok(None);
        }
        if let Some(expected) = expected
            && !active
                .as_ref()
                .is_some_and(|current| current.route == route.as_str() && current.lease == expected)
        {
            return Ok(None);
        }
        if let Some(old) = active.take() {
            let _ = old.stop.send(ReceiveOfferStop::Superseded);
        }
        let (stop, _) = watch::channel(ReceiveOfferStop::Active);
        let lease = next_receive_offer_lease();
        *active = Some(FakeOfferRoute {
            route: route.as_str().to_string(),
            lease,
            stop: stop.clone(),
        });
        let stream = stream::unfold(
            (sender.subscribe(), stop.subscribe()),
            |(mut receiver, mut stop)| async move {
                loop {
                    if *stop.borrow() != ReceiveOfferStop::Active {
                        return None;
                    }
                    tokio::select! {
                        biased;
                        changed = stop.changed() => {
                            let _ = changed;
                            return None;
                        }
                        event = receiver.recv() => match event {
                            Ok(envelope) => return Some((envelope, (receiver, stop))),
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => return None,
                        },
                    }
                }
            },
        );
        Ok(Some((lease, Box::pin(stream), stop.subscribe())))
    }
}

impl FakeTransport {
    /// 設定済みの peer(取り込んだ ticket と seed)と、topic ごとの接続中の peer。
    async fn fake_peer_view(&self) -> (BTreeSet<String>, Vec<(String, BTreeSet<String>)>) {
        let mut configured = self.imported_peers.lock().await.clone();
        configured.extend(self.configured_seed_peers.lock().await.iter().cloned());
        configured.extend(self.bootstrap_seed_peers.lock().await.iter().cloned());
        let topic_subscribers = self.network.topic_subscribers.lock().await.clone();
        let topics = self
            .subscribed_topics
            .lock()
            .await
            .iter()
            .map(|topic| {
                let subscribers = topic_subscribers.get(topic).cloned().unwrap_or_default();
                let connected = configured.intersection(&subscribers).cloned().collect();
                (topic.clone(), connected)
            })
            .collect();
        (configured, topics)
    }
}

#[async_trait]
impl Transport for FakeTransport {
    async fn peers(&self) -> Result<PeerSnapshot> {
        let (configured, topics) = self.fake_peer_view().await;
        let topic_diagnostics = topics
            .iter()
            .map(|(topic, connected)| TopicPeerSnapshot {
                topic: topic.clone(),
                joined: !connected.is_empty(),
                peer_count: connected.len(),
                configured_peer_count: configured.len(),
                missing_peer_count: configured.len() - connected.len(),
                active_path: if connected.is_empty() {
                    ConnectionPath::DirectP2p
                } else {
                    ConnectionPath::RelaySupportedP2p
                },
                rendezvous_peer_count: connected.len(),
                fallback_peer_count: 0,
                last_received_at: None,
                status_detail: topic_status_detail(configured.len(), connected.len()),
                last_error: None,
            })
            .collect::<Vec<_>>();
        let connected_max = topics.iter().map(|(_, connected)| connected.len()).max();
        Ok(PeerSnapshot {
            connected: !configured.is_empty(),
            peer_count: configured.len(),
            configured_peer_count: configured.len(),
            subscribed_topics: topics.into_iter().map(|(topic, _)| topic).collect(),
            active_path: ConnectionPath::DirectP2p,
            fallback_peer_count: 0,
            pending_events: 0,
            status_detail: peer_status_detail(
                if topic_diagnostics.is_empty() {
                    0
                } else {
                    configured.len()
                },
                connected_max.unwrap_or(0),
                topic_diagnostics.len(),
            ),
            last_error: None,
            topic_diagnostics,
        })
    }

    async fn subscribed_topics(&self) -> Result<Vec<String>> {
        Ok(self
            .subscribed_topics
            .lock()
            .await
            .iter()
            .cloned()
            .collect())
    }

    async fn peer_page(
        &self,
        kind: ConnectivityPeerKind,
        topic: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<PeerPage> {
        let (configured, topics) = self.fake_peer_view().await;
        let mut ids = BTreeSet::new();
        for (_, connected) in topics
            .iter()
            .filter(|(name, _)| topic.is_none_or(|topic| topic == name))
        {
            match kind {
                ConnectivityPeerKind::Connected => ids.extend(connected.iter().cloned()),
                ConnectivityPeerKind::Configured => ids.extend(configured.iter().cloned()),
                ConnectivityPeerKind::Missing => {
                    ids.extend(configured.difference(connected).cloned())
                }
                _ => {}
            }
        }
        match kind {
            ConnectivityPeerKind::ManualTicket => ids = self.imported_peers.lock().await.clone(),
            ConnectivityPeerKind::BootstrapSeed => {
                ids = self.bootstrap_seed_peers.lock().await.clone()
            }
            ConnectivityPeerKind::ConfiguredSeed => {
                ids = self.configured_seed_peers.lock().await.clone()
            }
            _ => {}
        }
        Ok(PeerPage::from_sorted(&ids, cursor, limit))
    }

    async fn export_ticket(&self) -> Result<Option<String>> {
        self.network
            .known_peers
            .lock()
            .await
            .insert(self.local_id.clone());
        Ok(Some(self.local_id.clone()))
    }

    async fn import_ticket(&self, ticket: &str) -> Result<()> {
        self.imported_peers.lock().await.insert(ticket.to_string());
        self.network
            .known_peers
            .lock()
            .await
            .insert(ticket.to_string());
        Ok(())
    }

    async fn configure_discovery(
        &self,
        mode: DiscoveryMode,
        env_locked: bool,
        configured_seed_peers: Vec<SeedPeer>,
        bootstrap_seed_peers: Vec<SeedPeer>,
    ) -> Result<()> {
        *self.discovery_mode.lock().await = mode;
        *self.env_locked.lock().await = env_locked;
        let configured = configured_seed_peers
            .into_iter()
            .map(|peer| peer.endpoint_id)
            .collect::<BTreeSet<_>>();
        let bootstrap = bootstrap_seed_peers
            .into_iter()
            .map(|peer| peer.endpoint_id)
            .collect::<BTreeSet<_>>();
        *self.configured_seed_peers.lock().await = configured;
        *self.bootstrap_seed_peers.lock().await = bootstrap;
        Ok(())
    }

    async fn discovery(&self) -> Result<DiscoverySnapshot> {
        let (connected, _) = self.fake_peer_view().await;
        Ok(DiscoverySnapshot {
            mode: self.discovery_mode.lock().await.clone(),
            connect_mode: ConnectMode::DirectOnly,
            active_path: ConnectionPath::DirectP2p,
            env_locked: *self.env_locked.lock().await,
            configured_seed_peer_count: self.configured_seed_peers.lock().await.len(),
            bootstrap_seed_peer_count: self.bootstrap_seed_peers.lock().await.len(),
            connected_peer_count: connected.len(),
            local_endpoint_id: self.local_id.clone(),
            last_discovery_error: None,
        })
    }
}

#[async_trait]
impl HintTransport for FakeTransport {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<HintStream> {
        let hint_topic = kukuri_core::wire::hint_topic_id(topic);
        self.subscribed_topics
            .lock()
            .await
            .insert(hint_topic.as_str().to_string());
        self.network
            .topic_subscribers
            .lock()
            .await
            .entry(hint_topic.as_str().to_string())
            .or_default()
            .insert(self.local_id.clone());
        let sender = self.hint_sender(topic).await;
        Ok(crate::hint_stream_from_sender(&sender))
    }

    async fn unsubscribe_hints(&self, topic: &TopicId) -> Result<()> {
        let hint_topic = kukuri_core::wire::hint_topic_id(topic);
        self.subscribed_topics
            .lock()
            .await
            .remove(hint_topic.as_str());
        let mut subscribers = self.network.topic_subscribers.lock().await;
        if let Some(topic_subscribers) = subscribers.get_mut(hint_topic.as_str()) {
            topic_subscribers.remove(self.local_id.as_str());
            if topic_subscribers.is_empty() {
                subscribers.remove(hint_topic.as_str());
            }
        }
        Ok(())
    }

    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()> {
        let sender = self.hint_sender(topic).await;
        let _ = sender.send(HintEnvelope {
            hint,
            received_at: Utc::now().timestamp_millis(),
            source_peer: self.local_id.clone(),
            dropped_before: 0,
        });
        Ok(())
    }

    async fn resolve_receive_destination(
        &self,
        recipient: &Pubkey,
    ) -> Result<Option<EndpointAddr>> {
        receive_route_for_account(recipient)?;
        let candidates = self
            .network
            .receive_candidates
            .lock()
            .await
            .by_account
            .get(recipient.as_str())
            .map(|sources| sources.values().flatten().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let verified = self.network.verified_receive_providers.lock().await;
        Ok(candidates.into_iter().find(|candidate| {
            verified
                .get(recipient.as_str())
                .is_some_and(|ids| ids.contains(&candidate.id.to_string()))
        }))
    }

    async fn receive_candidate_fence(&self) -> Result<ReceiveCandidateFence> {
        Ok(ReceiveCandidateFence {
            transport_instance: 0,
            clear_epoch: self.network.receive_candidates.lock().await.clear_epoch,
        })
    }

    async fn offer_receive_candidates(
        &self,
        source: &str,
        recipient: &Pubkey,
        candidates: Vec<EndpointAddr>,
        fence: ReceiveCandidateFence,
    ) -> Result<()> {
        receive_route_for_account(recipient)?;
        anyhow::ensure!(candidates.len() <= 8, "too many fake receive candidates");
        let mut state = self.network.receive_candidates.lock().await;
        anyhow::ensure!(
            fence.transport_instance == 0 && fence.clear_epoch == state.clear_epoch,
            "stale fake receive candidate fence"
        );
        state
            .by_account
            .entry(recipient.as_str().to_string())
            .or_default()
            .insert(source.to_string(), candidates);
        Ok(())
    }

    async fn clear_receive_candidates(&self, source: Option<&str>) -> Result<()> {
        let mut state = self.network.receive_candidates.lock().await;
        state.clear_epoch = state.clear_epoch.wrapping_add(1);
        match source {
            Some(source) => {
                for sources in state.by_account.values_mut() {
                    sources.remove(source);
                }
            }
            None => state.by_account.clear(),
        }
        Ok(())
    }

    async fn invalidate_receive_destination(
        &self,
        _recipient: &Pubkey,
        _endpoint_id: &str,
    ) -> Result<()> {
        Ok(())
    }

    async fn verify_receive_provider(&self, sender: &Pubkey, provider: EndpointAddr) -> Result<()> {
        receive_route_for_account(sender)?;
        anyhow::ensure!(
            self.network
                .verified_receive_providers
                .lock()
                .await
                .get(sender.as_str())
                .is_some_and(|ids| ids.contains(&provider.id.to_string())),
            "fake receive provider is not bound to sender"
        );
        Ok(())
    }

    async fn subscribe_receive_offers(
        &self,
        recipient: &Pubkey,
    ) -> Result<ReceiveOfferSubscription> {
        self.subscribe_receive_offers_impl(recipient, None, false)
            .await?
            .ok_or_else(|| anyhow::anyhow!("account receive route was superseded"))
    }

    async fn resubscribe_receive_offers_if_current(
        &self,
        recipient: &Pubkey,
        expected: ReceiveOfferLease,
    ) -> Result<Option<ReceiveOfferSubscription>> {
        self.subscribe_receive_offers_impl(recipient, Some(expected), false)
            .await
    }

    async fn subscribe_receive_offers_if_vacant(
        &self,
        recipient: &Pubkey,
    ) -> Result<Option<ReceiveOfferSubscription>> {
        self.subscribe_receive_offers_impl(recipient, None, true)
            .await
    }

    async fn unsubscribe_receive_offers(
        &self,
        recipient: &Pubkey,
        lease: ReceiveOfferLease,
    ) -> Result<()> {
        let route = receive_route_for_account(recipient)?;
        let mut active = self.active_offer_route.lock().await;
        if active
            .as_ref()
            .is_some_and(|current| current.route == route.as_str() && current.lease == lease)
        {
            let old = active.take().expect("matching offer route");
            let _ = old.stop.send(ReceiveOfferStop::Closed);
        }
        Ok(())
    }

    async fn publish_receive_offer(
        &self,
        recipient: &Pubkey,
        _destination: EndpointAddr,
        offer: SealedReceiveOfferV1,
    ) -> Result<()> {
        offer.encode()?;
        let sender = self.offer_sender(recipient).await?;
        let _ = sender.send(ReceiveOfferEnvelope {
            offer,
            received_at: Utc::now().timestamp_millis(),
            source_peer: self.local_id.clone(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use kukuri_core::{
        BlobHash, KukuriKeys, ReceiveOfferReferenceV1, ReceiveOfferScopeV1, seal_receive_offer,
    };

    use crate::test_support::{
        HintRoundtripParticipant, format_peer_snapshot, wait_for_hint_roundtrip,
    };

    #[tokio::test]
    async fn fake_account_switch_stops_delivery_to_the_old_offer_stream() {
        let transport = FakeTransport::new("sender", FakeNetwork::default());
        let sender = KukuriKeys::generate();
        let old = KukuriKeys::generate();
        let current = KukuriKeys::generate();
        let (_, mut old_stream, _) = transport
            .subscribe_receive_offers(&old.public_key())
            .await
            .unwrap();
        let (current_lease, mut current_stream, _) = transport
            .subscribe_receive_offers(&current.public_key())
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        let offer = seal_receive_offer(
            &sender,
            &old.public_key(),
            ReceiveOfferReferenceV1 {
                provider_endpoint_id: "11".repeat(32),
                payload_hash: BlobHash("22".repeat(32)),
                payload_bytes: 1,
                scope: ReceiveOfferScopeV1::PublicSource,
            },
            now,
            now + 60_000,
        )
        .unwrap();
        let endpoint = iroh::SecretKey::from_bytes(&[3; 32]).public();
        transport
            .publish_receive_offer(&old.public_key(), EndpointAddr::new(endpoint), offer)
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(100), old_stream.next())
                .await
                .unwrap()
                .is_none(),
            "switch must close the old account stream before further delivery"
        );
        transport
            .unsubscribe_receive_offers(&current.public_key(), current_lease)
            .await
            .unwrap();
        let current_offer = seal_receive_offer(
            &sender,
            &current.public_key(),
            ReceiveOfferReferenceV1 {
                provider_endpoint_id: "11".repeat(32),
                payload_hash: BlobHash("22".repeat(32)),
                payload_bytes: 1,
                scope: ReceiveOfferScopeV1::PublicSource,
            },
            now,
            now + 60_000,
        )
        .unwrap();
        transport
            .publish_receive_offer(
                &current.public_key(),
                EndpointAddr::new(endpoint),
                current_offer,
            )
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(100), current_stream.next())
                .await
                .unwrap()
                .is_none(),
            "unsubscribe must close the current account stream"
        );
    }

    fn initial_topic_join_timeout() -> Duration {
        if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
            Duration::from_secs(180)
        } else {
            Duration::from_secs(15)
        }
    }

    #[tokio::test]
    async fn fake_transport_discovery_reports_seed_sources_separately() {
        let transport = FakeTransport::new("local-peer", FakeNetwork::default());

        transport
            .configure_discovery(
                DiscoveryMode::StaticPeer,
                false,
                vec![SeedPeer {
                    endpoint_id: "configured-peer".into(),
                    addr_hint: None,
                }],
                vec![SeedPeer {
                    endpoint_id: "bootstrap-peer".into(),
                    addr_hint: None,
                }],
            )
            .await
            .expect("configure discovery");
        transport
            .import_ticket("manual-ticket-peer")
            .await
            .expect("import ticket");

        let discovery = transport.discovery().await.expect("discovery");
        assert_eq!(discovery.configured_seed_peer_count, 1);
        assert_eq!(discovery.bootstrap_seed_peer_count, 1);
        assert_eq!(discovery.connected_peer_count, 3);
        for (kind, expected) in [
            (ConnectivityPeerKind::ConfiguredSeed, "configured-peer"),
            (ConnectivityPeerKind::BootstrapSeed, "bootstrap-peer"),
            (ConnectivityPeerKind::ManualTicket, "manual-ticket-peer"),
        ] {
            assert_eq!(
                transport
                    .peer_page(kind, None, None, 64)
                    .await
                    .expect("page")
                    .peer_ids,
                vec![expected.to_string()]
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn topic_hint_peer_count_tracks_real_subscribers() {
        let network = FakeNetwork::default();
        let transport_a = FakeTransport::new("transport-a", network.clone());
        let transport_b = FakeTransport::new("transport-b", network);
        let ticket_a = transport_a
            .export_ticket()
            .await
            .expect("ticket a")
            .expect("ticket a value");
        let ticket_b = transport_b
            .export_ticket()
            .await
            .expect("ticket b")
            .expect("ticket b value");
        transport_a
            .import_ticket(&ticket_b)
            .await
            .expect("import b");
        transport_b
            .import_ticket(&ticket_a)
            .await
            .expect("import a");

        let demo = TopicId::new("kukuri:topic:demo");
        let test7 = TopicId::new("kukuri:topic:test7");
        let join_timeout = initial_topic_join_timeout();
        let (mut demo_stream_a, mut demo_stream_b) = tokio::try_join!(
            transport_a.subscribe_hints(&demo),
            transport_b.subscribe_hints(&demo)
        )
        .expect("subscribe demo hints");
        wait_for_hint_roundtrip(
            HintRoundtripParticipant {
                transport: &transport_a,
                stream: &mut demo_stream_a,
                expected_source_peer: None,
            },
            HintRoundtripParticipant {
                transport: &transport_b,
                stream: &mut demo_stream_b,
                expected_source_peer: None,
            },
            &demo,
            join_timeout,
            "demo",
        )
        .await;

        match timeout(join_timeout, async {
            loop {
                let peers_a = transport_a.peers().await.expect("peers a");
                let peers_b = transport_b.peers().await.expect("peers b");
                if peers_a.peer_count >= 1 && peers_b.peer_count >= 1 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        {
            Ok(()) => {}
            Err(_) => {
                let peers_a = transport_a.peers().await.expect("peers a");
                let peers_b = transport_b.peers().await.expect("peers b");
                panic!(
                    "peer readiness timeout: a={} b={}",
                    format_peer_snapshot(&peers_a),
                    format_peer_snapshot(&peers_b)
                );
            }
        }

        match timeout(join_timeout, async {
            loop {
                let peers_a = transport_a.peers().await.expect("peers a");
                let demo_diag = peers_a
                    .topic_diagnostics
                    .iter()
                    .find(|topic| topic.topic == "hint/kukuri:topic:demo")
                    .expect("demo diag");
                if demo_diag.peer_count == 1 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        {
            Ok(()) => {}
            Err(_) => {
                let peers_a = transport_a.peers().await.expect("peers a");
                let peers_b = transport_b.peers().await.expect("peers b");
                panic!(
                    "demo peer count timeout: a={} b={}",
                    format_peer_snapshot(&peers_a),
                    format_peer_snapshot(&peers_b)
                );
            }
        }

        let mut test7_stream_a = transport_a
            .subscribe_hints(&test7)
            .await
            .expect("subscribe test7 a");

        match timeout(join_timeout, async {
            loop {
                let peers_a = transport_a.peers().await.expect("peers a");
                let demo_diag = peers_a
                    .topic_diagnostics
                    .iter()
                    .find(|topic| topic.topic == "hint/kukuri:topic:demo")
                    .expect("demo diag");
                let test7_diag = peers_a
                    .topic_diagnostics
                    .iter()
                    .find(|topic| topic.topic == "hint/kukuri:topic:test7")
                    .expect("test7 diag");
                if demo_diag.peer_count == 1 && test7_diag.peer_count == 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        {
            Ok(()) => {}
            Err(_) => {
                let peers_a = transport_a.peers().await.expect("peers a");
                let peers_b = transport_b.peers().await.expect("peers b");
                panic!(
                    "initial peer counts timeout: a={} b={}",
                    format_peer_snapshot(&peers_a),
                    format_peer_snapshot(&peers_b)
                );
            }
        }

        let mut test7_stream_b = transport_b
            .subscribe_hints(&test7)
            .await
            .expect("subscribe test7 b");
        wait_for_hint_roundtrip(
            HintRoundtripParticipant {
                transport: &transport_a,
                stream: &mut test7_stream_a,
                expected_source_peer: None,
            },
            HintRoundtripParticipant {
                transport: &transport_b,
                stream: &mut test7_stream_b,
                expected_source_peer: None,
            },
            &test7,
            join_timeout,
            "test7",
        )
        .await;
        match timeout(join_timeout, async {
            loop {
                let peers_a = transport_a.peers().await.expect("peers a");
                let test7_diag = peers_a
                    .topic_diagnostics
                    .iter()
                    .find(|topic| topic.topic == "hint/kukuri:topic:test7")
                    .expect("test7 diag");
                if test7_diag.peer_count == 1 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        {
            Ok(()) => {}
            Err(_) => {
                let peers_a = transport_a.peers().await.expect("peers a");
                let peers_b = transport_b.peers().await.expect("peers b");
                panic!(
                    "join peer count timeout: a={} b={}",
                    format_peer_snapshot(&peers_a),
                    format_peer_snapshot(&peers_b)
                );
            }
        }

        transport_b
            .unsubscribe_hints(&test7)
            .await
            .expect("unsubscribe test7 b");
        match timeout(join_timeout, async {
            loop {
                let peers_a = transport_a.peers().await.expect("peers a");
                let test7_diag = peers_a
                    .topic_diagnostics
                    .iter()
                    .find(|topic| topic.topic == "hint/kukuri:topic:test7")
                    .expect("test7 diag");
                if test7_diag.peer_count == 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        {
            Ok(()) => {}
            Err(_) => {
                let peers_a = transport_a.peers().await.expect("peers a");
                let peers_b = transport_b.peers().await.expect("peers b");
                panic!(
                    "leave peer count timeout: a={} b={}",
                    format_peer_snapshot(&peers_a),
                    format_peer_snapshot(&peers_b)
                );
            }
        }
    }

    #[tokio::test]
    async fn fake_transport_hint_roundtrip() {
        let network = FakeNetwork::default();
        let left = FakeTransport::new("left", network.clone());
        let right = FakeTransport::new("right", network);
        let topic = TopicId::new("kukuri:topic:fake");
        let _left_stream = left
            .subscribe_hints(&topic)
            .await
            .expect("left subscribe hints");
        let mut right_stream = right
            .subscribe_hints(&topic)
            .await
            .expect("right subscribe hints");

        left.import_ticket("right").await.expect("import");
        let hint = GossipHint::Presence {
            topic_id: topic.clone(),
            author: "author-1".into(),
            ttl_ms: 30_000,
        };
        left.publish_hint(&topic, hint.clone())
            .await
            .expect("publish hint");

        let received = timeout(Duration::from_secs(1), right_stream.next())
            .await
            .expect("receive timeout")
            .expect("event");
        assert_eq!(received.hint, hint);
    }
}
