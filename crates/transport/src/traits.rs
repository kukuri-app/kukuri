use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use futures_util::Stream;
pub use iroh::EndpointAddr;
use kukuri_core::{GossipHint, Pubkey, SealedReceiveOfferV1, TopicId};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::config::{ConnectionPath, DiscoveryMode, DiscoverySnapshot, SeedPeer};

pub type HintStream = Pin<Box<dyn Stream<Item = HintEnvelope> + Send>>;
pub type ReceiveOfferStream = Pin<Box<dyn Stream<Item = ReceiveOfferEnvelope> + Send>>;
pub type ReceiveOfferSubscription = (
    ReceiveOfferLease,
    ReceiveOfferStream,
    watch::Receiver<ReceiveOfferStop>,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveOfferStop {
    Active,
    Superseded,
    Closed,
    TransportClosed,
}

/// Process-unique lease so an old account owner cannot close a later receiver,
/// including after an endpoint/transport reload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiveOfferLease {
    id: u64,
    transport_instance: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiveCandidateFence {
    pub transport_instance: u64,
    pub clear_epoch: u64,
}

static NEXT_RECEIVE_OFFER_LEASE: AtomicU64 = AtomicU64::new(1);

impl ReceiveOfferLease {
    pub fn fresh() -> Self {
        Self::for_instance(0)
    }

    pub(crate) fn for_instance(transport_instance: u64) -> Self {
        Self {
            id: NEXT_RECEIVE_OFFER_LEASE.fetch_add(1, Ordering::Relaxed),
            transport_instance,
        }
    }

    pub fn transport_instance(self) -> u64 {
        self.transport_instance
    }
}

pub(crate) fn next_receive_offer_lease() -> ReceiveOfferLease {
    ReceiveOfferLease::fresh()
}

#[derive(Clone, Debug)]
pub struct ReceiveOfferEnvelope {
    pub offer: SealedReceiveOfferV1,
    pub received_at: i64,
    pub source_peer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HintEnvelope {
    pub hint: GossipHint,
    pub received_at: i64,
    pub source_peer: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerSnapshot {
    pub connected: bool,
    pub peer_count: usize,
    pub connected_peers: Vec<String>,
    pub configured_peers: Vec<String>,
    pub subscribed_topics: Vec<String>,
    pub active_path: ConnectionPath,
    pub fallback_peer_ids: Vec<String>,
    pub pending_events: usize,
    pub status_detail: String,
    pub last_error: Option<String>,
    pub topic_diagnostics: Vec<TopicPeerSnapshot>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicPeerSnapshot {
    pub topic: String,
    pub joined: bool,
    pub peer_count: usize,
    pub connected_peers: Vec<String>,
    pub configured_peer_ids: Vec<String>,
    pub missing_peer_ids: Vec<String>,
    pub active_path: ConnectionPath,
    pub rendezvous_peer_ids: Vec<String>,
    pub fallback_peer_ids: Vec<String>,
    pub last_received_at: Option<i64>,
    pub status_detail: String,
    pub last_error: Option<String>,
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn peers(&self) -> Result<PeerSnapshot>;
    async fn export_ticket(&self) -> Result<Option<String>>;
    async fn import_ticket(&self, ticket: &str) -> Result<()>;
    async fn configure_discovery(
        &self,
        _mode: DiscoveryMode,
        _env_locked: bool,
        _configured_seed_peers: Vec<SeedPeer>,
        _bootstrap_seed_peers: Vec<SeedPeer>,
    ) -> Result<()> {
        Ok(())
    }
    async fn discovery(&self) -> Result<DiscoverySnapshot> {
        Ok(DiscoverySnapshot::default())
    }
}

#[async_trait]
pub trait HintTransport: Send + Sync {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<HintStream>;
    async fn unsubscribe_hints(&self, topic: &TopicId) -> Result<()>;
    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()>;

    /// Return only a small rotating window of peers joined to this gossip
    /// scope. Private docs readers must not sample unrelated account peers.
    async fn topic_read_candidates(&self, _topic: &TopicId) -> Result<Vec<SeedPeer>> {
        Ok(Vec::new())
    }

    /// Resolve only an endpoint whose current binding proves `recipient`.
    /// None means deferred; callers must keep durable outbox rows pending.
    async fn resolve_receive_destination(
        &self,
        _recipient: &Pubkey,
    ) -> Result<Option<EndpointAddr>> {
        anyhow::bail!("authenticated receive destination resolution is not supported")
    }

    /// A token captured before CN I/O; a clear or transport rebuild makes it stale.
    async fn receive_candidate_fence(&self) -> Result<ReceiveCandidateFence> {
        anyhow::bail!("account receive candidate fence is not supported")
    }

    /// Feed bounded rendezvous addresses under their CN owner. Resolution
    /// still requires a live signed binding on QUIC.
    async fn offer_receive_candidates(
        &self,
        _source: &str,
        _recipient: &Pubkey,
        _candidates: Vec<EndpointAddr>,
        _fence: ReceiveCandidateFence,
    ) -> Result<()> {
        anyhow::bail!("account receive candidate feed is not supported")
    }

    /// Forget one CN owner's addresses (or all owners for a config reset).
    async fn clear_receive_candidates(&self, _source: Option<&str>) -> Result<()> {
        anyhow::bail!("account receive candidate clear is not supported")
    }

    async fn invalidate_receive_destination(
        &self,
        _recipient: &Pubkey,
        _endpoint_id: &str,
    ) -> Result<()> {
        anyhow::bail!("authenticated receive destination invalidation is not supported")
    }

    /// Verify that the live QUIC endpoint is currently bound to `sender`
    /// before an inline receive reference can initiate a blob fetch.
    async fn verify_receive_provider(
        &self,
        _sender: &Pubkey,
        _provider: EndpointAddr,
    ) -> Result<()> {
        anyhow::bail!("authenticated receive provider verification is not supported")
    }

    /// A new lease supersedes the previous consumer, including for the same
    /// account. The underlying route may be reused, but the old stream ends.
    async fn subscribe_receive_offers(
        &self,
        _recipient: &Pubkey,
    ) -> Result<ReceiveOfferSubscription> {
        anyhow::bail!("account receive offers are not supported by this transport")
    }

    /// Atomically restart only while `expected` still owns the route. A
    /// superseded owner receives None and must stop instead of reclaiming it.
    async fn resubscribe_receive_offers_if_current(
        &self,
        _recipient: &Pubkey,
        _expected: ReceiveOfferLease,
    ) -> Result<Option<ReceiveOfferSubscription>> {
        Ok(None)
    }

    /// Claim an empty route after the transport instance has changed. A newer
    /// owner already present on the instance wins; this call returns None.
    async fn subscribe_receive_offers_if_vacant(
        &self,
        _recipient: &Pubkey,
    ) -> Result<Option<ReceiveOfferSubscription>> {
        Ok(None)
    }

    async fn receive_offer_transport_instance(&self) -> Result<u64> {
        Ok(0)
    }

    /// Idempotent. Only the matching lease may close the current route.
    async fn unsubscribe_receive_offers(
        &self,
        _recipient: &Pubkey,
        _lease: ReceiveOfferLease,
    ) -> Result<()> {
        anyhow::bail!("account receive offers are not supported by this transport")
    }

    async fn publish_receive_offer(
        &self,
        _recipient: &Pubkey,
        _destination: EndpointAddr,
        _offer: SealedReceiveOfferV1,
    ) -> Result<()> {
        anyhow::bail!("account receive offers are not supported by this transport")
    }
}
