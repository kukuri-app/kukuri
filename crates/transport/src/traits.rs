use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
pub use iroh::EndpointAddr;
use kukuri_core::{GossipHint, Pubkey, SealedReceiveOfferV1, TopicId};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

use crate::config::{
    ConnectionPath, ConnectivityPeerKind, DiscoveryMode, DiscoverySnapshot, SeedPeer,
};

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
    /// この envelope の前に、受け手へ渡せずに捨てた hint の数(購読 stream の broadcast が溢れた `Lagged` と、gossip 層の
    /// `Lagged` を次の envelope に畳んだもの)。受け手は 1 以上なら取りこぼしがあったとみなす(#1567 AC-1)。
    #[serde(default)]
    pub dropped_before: u64,
}

/// broadcast の購読を hint の stream にする。受け手が遅れて捨てられた分(`Lagged(n)`)は黙って落とさず、次に届く envelope の
/// `dropped_before` に畳んで渡す(#1567 AC-1)。
pub fn hint_stream_from_sender(sender: &broadcast::Sender<HintEnvelope>) -> HintStream {
    let stream = BroadcastStream::new(sender.subscribe())
        .scan(0u64, |dropped, event| {
            std::future::ready(Some(match event {
                Ok(mut envelope) => {
                    envelope.dropped_before = envelope
                        .dropped_before
                        .saturating_add(std::mem::take(dropped));
                    Some(envelope)
                }
                Err(BroadcastStreamRecvError::Lagged(count)) => {
                    *dropped = dropped.saturating_add(count);
                    None
                }
            }))
        })
        .filter_map(std::future::ready);
    Box::pin(stream)
}

#[cfg(test)]
mod hint_stream_tests {
    use super::*;

    fn envelope(received_at: i64) -> HintEnvelope {
        HintEnvelope {
            hint: GossipHint::ProfileUpdated {
                author: Pubkey::from("a".repeat(64)),
            },
            received_at,
            source_peer: String::new(),
            dropped_before: 0,
        }
    }

    // #1567 AC-1: 受け手を読まずに容量 + k 件送ってから読むと、届く envelope は容量ぶんで、捨てた k 件は最初の envelope に載る。
    #[tokio::test]
    async fn dropped_hints_are_counted_on_the_next_envelope() {
        let capacity = 4;
        let dropped = 3;
        let (sender, receiver) = broadcast::channel(capacity);
        let mut stream = hint_stream_from_sender(&sender);
        drop(receiver);
        for index in 0..(capacity + dropped) {
            sender
                .send(envelope(index as i64))
                .expect("the stream subscribes");
        }
        let mut delivered = Vec::new();
        while let Some(envelope) = stream.next().await {
            delivered.push(envelope);
            if delivered.len() == capacity {
                break;
            }
        }
        assert_eq!(delivered[0].received_at, dropped as i64);
        assert_eq!(delivered[0].dropped_before, dropped as u64);
        assert!(
            delivered[1..]
                .iter()
                .all(|envelope| envelope.dropped_before == 0)
        );
        assert_eq!(
            delivered
                .iter()
                .map(|envelope| envelope.dropped_before)
                .sum::<u64>(),
            dropped as u64
        );
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// 通常の通信状態。件数と、稼働中の topic(lease と短期の送信先)の診断だけを持つ(#1221 R2-D)。
/// peer の一覧は [`Transport::peer_page`] でページとして読む。
pub struct PeerSnapshot {
    pub connected: bool,
    pub peer_count: usize,
    pub configured_peer_count: usize,
    pub subscribed_topics: Vec<String>,
    pub active_path: ConnectionPath,
    pub fallback_peer_count: usize,
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
    pub configured_peer_count: usize,
    pub missing_peer_count: usize,
    pub active_path: ConnectionPath,
    pub rendezvous_peer_count: usize,
    pub fallback_peer_count: usize,
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
    /// 購読している topic(lease の 64 件以内。#1221 R2-D)。
    async fn subscribed_topics(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// peer の一覧を id の順に `cursor` の後から最大 `limit` 件読む。取得の候補の cursor は進めない(#1221 R2-D)。
    async fn peer_page(
        &self,
        _kind: ConnectivityPeerKind,
        _topic: Option<&str>,
        _cursor: Option<&str>,
        _limit: usize,
    ) -> Result<PeerPage> {
        Ok(PeerPage::default())
    }
}

/// peer の一覧の 1 ページ。`next_cursor` は続きがあるときだけ、最後の id を返す。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct PeerPage {
    pub peer_ids: Vec<String>,
    pub next_cursor: Option<String>,
}

impl PeerPage {
    /// 昇順の `ids` から、`cursor` より後の最大 `limit` 件を取る。
    pub fn from_sorted<'a>(
        ids: impl IntoIterator<Item = &'a String>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Self {
        let mut peer_ids = ids
            .into_iter()
            .filter(|id| cursor.is_none_or(|cursor| id.as_str() > cursor))
            .take(limit + 1)
            .cloned()
            .collect::<Vec<_>>();
        let next_cursor = (peer_ids.len() > limit).then(|| {
            peer_ids.truncate(limit);
            peer_ids.last().cloned().unwrap_or_default()
        });
        Self {
            peer_ids,
            next_cursor,
        }
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

    /// CN の rendezvous が返した peer を、購読している topic(`hint/...`)だけへ join する(#1221 R2-B)。
    async fn join_topic_peers(
        &self,
        _source: &str,
        _topic: &TopicId,
        _peers: Vec<SeedPeer>,
    ) -> Result<()> {
        Ok(())
    }

    /// Forget one CN owner's addresses (or all owners for a config reset),
    /// including the topic rendezvous candidates learned from it.
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
