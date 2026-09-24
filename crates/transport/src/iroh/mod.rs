use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
#[cfg(not(test))]
use std::net::SocketAddr;
#[cfg(test)]
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
#[cfg(test)]
use std::str::FromStr;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use futures_util::{StreamExt, stream};
#[cfg(test)]
use iroh::RelayMode;
use iroh::address_lookup::{AddrFilter, AddressLookup, Item as AddressLookupItem, MemoryLookup};
use iroh::endpoint::{
    Builder as EndpointBuilder, MtuDiscoveryConfig, QuicTransportConfig, TransportAddrUsage,
    presets,
};
use iroh::endpoint_info::EndpointInfo;
use iroh::protocol::Router;
#[cfg(test)]
use iroh::tls::CaTlsConfig;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayUrl, SecretKey};
use iroh_gossip::api::{Event as GossipEvent, GossipSender};
use iroh_gossip::{ALPN as GOSSIP_ALPN, Gossip, TopicId as GossipTopicId};
use iroh_mainline_address_lookup::DhtAddressLookup;
use kukuri_core::{GossipHint, Pubkey, SealedReceiveOfferV1, TopicId};
#[cfg(test)]
use kukuri_core::{HintObjectRef, KukuriEnvelope, build_post_envelope, generate_keys};
use tokio::sync::{Mutex, Notify, RwLock, Semaphore, broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, info, warn};

use crate::config::{
    ConnectMode, ConnectionPath, DhtDiscoveryOptions, DiscoveryMode, DiscoverySnapshot, SeedPeer,
    TransportNetworkConfig, TransportRelayConfig,
};
use crate::diagnostics::{peer_status_detail, topic_status_detail};
use crate::discovery::prepare_endpoint_for_discovery;
use crate::tickets::{
    encode_endpoint_ticket, endpoint_addr_with_relays, parse_endpoint_ticket, ticket_network_config,
};
use crate::traits::{
    HintEnvelope, HintStream, HintTransport, PeerSnapshot, ReceiveCandidateFence,
    ReceiveOfferEnvelope, ReceiveOfferLease, ReceiveOfferStop, ReceiveOfferStream,
    ReceiveOfferSubscription, TopicPeerSnapshot, Transport,
};

struct HintTopicState {
    sender: Arc<Mutex<GossipSender>>,
    broadcaster: broadcast::Sender<HintEnvelope>,
    bootstrap_peer_ids: BTreeSet<String>,
    neighbors: Arc<RwLock<BTreeSet<String>>>,
    last_received_at: Arc<Mutex<Option<i64>>>,
    last_error: Arc<Mutex<Option<String>>>,
    // parse 失敗した受信 hint の累計(wire 非互換の観測用。WP-C4)。プロセス生存中は単調増加。
    // 現状の読み手は cfg(test) のアクセサのみ(診断 UI への露出は契約変更のため別 WP)。
    #[cfg_attr(not(test), allow(dead_code))]
    invalid_hint_count: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    closed_notify: Arc<Notify>,
    update_warmup_task: Option<JoinHandle<()>>,
    _receiver_task: JoinHandle<()>,
}

struct ReceiveOfferTopicState {
    route: String,
    lease: ReceiveOfferLease,
    closing: bool,
    broadcaster: broadcast::Sender<ReceiveOfferEnvelope>,
    stop: watch::Sender<ReceiveOfferStop>,
    _sender: GossipSender,
    receiver_task: JoinHandle<()>,
}

static NEXT_RECEIVE_OFFER_TRANSPORT_INSTANCE: AtomicU64 = AtomicU64::new(1);

struct OutboundOfferHold {
    expires_at: tokio::time::Instant,
    task: JoinHandle<()>,
}

#[derive(Clone, Debug)]
struct TopicWarmupCoordinator {
    permits: Arc<Semaphore>,
    in_flight_peers: Arc<StdRwLock<BTreeSet<String>>>,
    warmup_cursor: Arc<AtomicU64>,
    #[cfg(test)]
    initial_warmup_tasks: Arc<AtomicUsize>,
}

impl Default for TopicWarmupCoordinator {
    fn default() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(2)),
            in_flight_peers: Arc::new(StdRwLock::new(BTreeSet::new())),
            warmup_cursor: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            initial_warmup_tasks: Arc::new(AtomicUsize::new(0)),
        }
    }
}

pub struct IrohGossipTransport {
    receive_offer_instance: u64,
    endpoint: Endpoint,
    gossip: Gossip,
    _router: Option<Router>,
    discovery: Arc<MemoryLookup>,
    network_config: TransportNetworkConfig,
    configured_seed_peers: Arc<Mutex<BTreeMap<String, EndpointAddr>>>,
    bootstrap_seed_peers: Arc<Mutex<BTreeMap<String, EndpointAddr>>>,
    imported_peers: Arc<Mutex<BTreeMap<String, EndpointAddr>>>,
    account_store: Option<Arc<kukuri_store::SqliteStore>>,
    imported_cursor: Mutex<Option<(i64, String)>>,
    hot_peer_ids: Mutex<VecDeque<EndpointId>>,
    gossip_health: Arc<crate::peers::BlobPeerHealth>,
    bootstrap_cursor: Mutex<(usize, [Option<String>; 3])>,
    receive_destinations: Mutex<receive_destination::DestinationWindow>,
    receive_destination_probes: Semaphore,
    subscribed_topics: Arc<Mutex<BTreeSet<String>>>,
    topic_states: Arc<Mutex<HashMap<String, HintTopicState>>>,
    receive_offer_topic: Mutex<Option<ReceiveOfferTopicState>>,
    outbound_offer_holds: Mutex<VecDeque<OutboundOfferHold>>,
    offer_closed: AtomicBool,
    hint_closed: AtomicBool,
    #[cfg(test)]
    hint_existing_snapshot_observed: Arc<Notify>,
    offer_shutdown_notify: Notify,
    #[cfg(test)]
    offer_receiver_tasks: Arc<AtomicUsize>,
    #[cfg(test)]
    offer_hold_tasks: Arc<AtomicUsize>,
    #[cfg(test)]
    offer_publish_joined: Arc<Notify>,
    #[cfg(test)]
    offer_publish_join_started: Arc<Notify>,
    topic_warmups: Arc<TopicWarmupCoordinator>,
    last_error: Arc<Mutex<Option<String>>>,
    discovery_mode: Arc<Mutex<DiscoveryMode>>,
    connect_mode: Arc<Mutex<ConnectMode>>,
    relay_urls: Arc<StdRwLock<Vec<RelayUrl>>>,
    env_locked: Arc<Mutex<bool>>,
}

mod discovery;
mod endpoint;
mod offer;
mod peer_state;
mod receive_destination;
mod relay;
#[cfg(test)]
mod tests;
mod topics;

#[cfg(test)]
pub(crate) use endpoint::bind_endpoint_with_options;
pub use relay::{build_endpoint_builder, sync_endpoint_relay_config};
#[cfg(test)]
pub(crate) use topics::{initial_topic_join_timeout, topic_to_gossip_id};

impl Drop for IrohGossipTransport {
    fn drop(&mut self) {
        self.offer_closed.store(true, Ordering::Release);
        self.hint_closed.store(true, Ordering::Release);
        self.offer_shutdown_notify.notify_waiters();
        if let Ok(mut topics) = self.topic_states.try_lock() {
            for (_, state) in topics.drain() {
                state.closed.store(true, Ordering::Release);
                state.closed_notify.notify_waiters();
                state._receiver_task.abort();
                if let Some(update) = state.update_warmup_task {
                    update.abort();
                }
            }
        }
        if let Ok(mut subscribed_topics) = self.subscribed_topics.try_lock() {
            subscribed_topics.clear();
        }
        if let Some(offer) = self.receive_offer_topic.get_mut().take() {
            let _ = offer.stop.send(ReceiveOfferStop::TransportClosed);
            offer.receiver_task.abort();
        }
        for hold in self.outbound_offer_holds.get_mut().drain(..) {
            hold.task.abort();
        }
    }
}

#[async_trait]
impl Transport for IrohGossipTransport {
    async fn peers(&self) -> Result<PeerSnapshot> {
        self.transport_peers_impl().await
    }
    async fn export_ticket(&self) -> Result<Option<String>> {
        self.transport_export_ticket_impl().await
    }
    async fn import_ticket(&self, ticket: &str) -> Result<()> {
        self.transport_import_ticket_impl(ticket).await
    }
    async fn configure_discovery(
        &self,
        mode: DiscoveryMode,
        env_locked: bool,
        configured_seed_peers: Vec<SeedPeer>,
        bootstrap_seed_peers: Vec<SeedPeer>,
    ) -> Result<()> {
        self.transport_configure_discovery_impl(
            mode,
            env_locked,
            configured_seed_peers,
            bootstrap_seed_peers,
        )
        .await
    }
    async fn discovery(&self) -> Result<DiscoverySnapshot> {
        self.transport_discovery_impl().await
    }
}

#[async_trait]
impl HintTransport for IrohGossipTransport {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<HintStream> {
        self.hint_subscribe_hints_impl(topic).await
    }
    async fn unsubscribe_hints(&self, topic: &TopicId) -> Result<()> {
        self.hint_unsubscribe_hints_impl(topic).await
    }
    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()> {
        self.hint_publish_hint_impl(topic, hint).await
    }

    async fn resolve_receive_destination(
        &self,
        recipient: &Pubkey,
    ) -> Result<Option<EndpointAddr>> {
        self.resolve_receive_destination_impl(recipient).await
    }

    async fn receive_candidate_fence(&self) -> Result<ReceiveCandidateFence> {
        Ok(ReceiveCandidateFence {
            transport_instance: self.receive_offer_instance,
            clear_epoch: self.receive_destinations.lock().await.clear_epoch,
        })
    }

    async fn offer_receive_candidates(
        &self,
        source: &str,
        recipient: &Pubkey,
        candidates: Vec<EndpointAddr>,
        fence: ReceiveCandidateFence,
    ) -> Result<()> {
        self.offer_receive_candidates_impl(source, recipient, candidates, fence)
            .await
    }

    async fn clear_receive_candidates(&self, source: Option<&str>) -> Result<()> {
        self.receive_destinations
            .lock()
            .await
            .clear_rendezvous(source);
        Ok(())
    }

    async fn invalidate_receive_destination(
        &self,
        recipient: &Pubkey,
        endpoint_id: &str,
    ) -> Result<()> {
        self.invalidate_receive_destination_impl(recipient, endpoint_id)
            .await;
        Ok(())
    }

    async fn verify_receive_provider(&self, sender: &Pubkey, provider: EndpointAddr) -> Result<()> {
        self.verify_receive_provider_impl(sender, provider).await
    }

    async fn subscribe_receive_offers(
        &self,
        recipient: &Pubkey,
    ) -> Result<ReceiveOfferSubscription> {
        self.subscribe_receive_offers_impl(recipient, None, false)
            .await?
            .ok_or_else(|| anyhow!("account receive route was superseded"))
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

    async fn receive_offer_transport_instance(&self) -> Result<u64> {
        Ok(self.receive_offer_instance)
    }

    async fn unsubscribe_receive_offers(
        &self,
        recipient: &Pubkey,
        lease: ReceiveOfferLease,
    ) -> Result<()> {
        self.unsubscribe_receive_offers_impl(recipient, lease).await
    }

    async fn publish_receive_offer(
        &self,
        recipient: &Pubkey,
        destination: EndpointAddr,
        offer: SealedReceiveOfferV1,
    ) -> Result<()> {
        self.publish_receive_offer_impl(recipient, destination, offer)
            .await
    }
}
