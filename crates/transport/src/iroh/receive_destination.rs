//! Bounded, authenticated destination lookup for account receive offers.
//! A peer address is only a candidate until its live signed binding is checked.

use super::*;
use std::ops::Bound::{Excluded, Unbounded};
use tokio::time::Instant;

use crate::receive_binding::fetch_receive_endpoint_binding;
use kukuri_core::receive_route_for_account;

const MAX_DESTINATION_ACCOUNTS: usize = 1_024;
const CANDIDATES_PER_LOOKUP: usize = 4;
const BINDING_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CACHED_BINDING_MS: i64 = 10_000;
const MAX_RENDEZVOUS_CANDIDATES: usize = 8;
const MAX_RENDEZVOUS_SOURCES: usize = 8;
const RENDEZVOUS_CANDIDATE_TTL: Duration = Duration::from_secs(45);

#[derive(Default)]
pub(super) struct DestinationWindow {
    entries: HashMap<Pubkey, DestinationEntry>,
    tick: u64,
    pub(super) clear_epoch: u64,
}

struct DestinationEntry {
    last_used: u64,
    revision: u64,
    source: usize,
    cursors: [Option<String>; 6],
    learned_cursors: [Option<(i64, String)>; 3],
    verified: Option<CachedDestination>,
    rendezvous_sources: BTreeMap<String, RendezvousSource>,
    rendezvous_cursor: usize,
    source_eviction_cursor: usize,
}

struct RendezvousSource {
    candidates: Vec<EndpointAddr>,
    expires_at: Instant,
}

struct CachedDestination {
    address: EndpointAddr,
    source: Option<String>,
    expires_at_ms: i64,
    expires_at: Instant,
}

impl DestinationWindow {
    fn touch(&mut self, recipient: &Pubkey) -> &mut DestinationEntry {
        self.tick = self.tick.wrapping_add(1);
        if !self.entries.contains_key(recipient)
            && self.entries.len() == MAX_DESTINATION_ACCOUNTS
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(account, _)| account.clone())
        {
            self.entries.remove(&oldest);
        }
        let tick = self.tick;
        let entry = self
            .entries
            .entry(recipient.clone())
            .or_insert_with(|| DestinationEntry {
                last_used: tick,
                revision: tick,
                source: 0,
                cursors: Default::default(),
                learned_cursors: Default::default(),
                verified: None,
                rendezvous_sources: BTreeMap::new(),
                rendezvous_cursor: 0,
                source_eviction_cursor: 0,
            });
        entry.last_used = tick;
        entry
    }

    fn cached(&mut self, recipient: &Pubkey, now_ms: i64) -> Option<EndpointAddr> {
        let entry = self.touch(recipient);
        if let Some(cached) = &entry.verified
            && cached.expires_at_ms > now_ms
            && cached.expires_at > Instant::now()
        {
            return Some(cached.address.clone());
        }
        entry.verified = None;
        None
    }

    fn select<const N: usize>(
        &mut self,
        recipient: &Pubkey,
        sources: [&BTreeMap<String, EndpointAddr>; N],
    ) -> (Vec<(EndpointAddr, Option<String>)>, u64) {
        let entry = self.touch(recipient);
        let mut selected = Vec::with_capacity(CANDIDATES_PER_LOOKUP);
        let mut seen = BTreeSet::new();
        entry
            .rendezvous_sources
            .retain(|_, source| source.expires_at > Instant::now());
        let rendezvous_candidates = entry
            .rendezvous_sources
            .iter()
            .flat_map(|(key, source)| {
                source
                    .candidates
                    .iter()
                    .cloned()
                    .map(|address| (address, Some(key.clone())))
            })
            .collect::<Vec<_>>();
        if entry.source.is_multiple_of(2) {
            for _ in 0..rendezvous_candidates.len().min(2) {
                let candidate = next_rendezvous_candidate(entry, &rendezvous_candidates);
                if seen.insert(candidate.0.id) {
                    selected.push(candidate);
                }
            }
        }
        let first_source = entry.source;
        entry.source = (entry.source + 1) % N;
        for offset in 0..N * CANDIDATES_PER_LOOKUP {
            if selected.len() == CANDIDATES_PER_LOOKUP {
                break;
            }
            let source = (first_source + offset) % N;
            if let Some(candidate) = next_peer(sources[source], &mut entry.cursors[source])
                && seen.insert(candidate.id)
            {
                selected.push((candidate, None));
            }
        }
        for _ in 0..rendezvous_candidates.len().min(CANDIDATES_PER_LOOKUP) {
            if selected.len() == CANDIDATES_PER_LOOKUP {
                break;
            }
            let candidate = next_rendezvous_candidate(entry, &rendezvous_candidates);
            if seen.insert(candidate.0.id) {
                selected.push(candidate);
            }
        }
        (selected, entry.revision)
    }

    fn attempted_learned(
        &mut self,
        recipient: &Pubkey,
        endpoint_id: EndpointId,
        pages: &[Option<(i64, String)>; 3],
    ) {
        let entry = self.touch(recipient);
        let id = endpoint_id.to_string();
        for (cursor, page) in entry.learned_cursors.iter_mut().zip(pages) {
            if page.as_ref().is_some_and(|(_, candidate)| candidate == &id) {
                *cursor = page.clone();
            }
        }
    }

    fn observe_rendezvous(
        &mut self,
        source: &str,
        recipient: &Pubkey,
        candidates: Vec<EndpointAddr>,
    ) {
        let entry = self.touch(recipient);
        entry
            .rendezvous_sources
            .retain(|_, value| value.expires_at > Instant::now());
        if !entry.rendezvous_sources.contains_key(source)
            && entry.rendezvous_sources.len() == MAX_RENDEZVOUS_SOURCES
        {
            let index = entry.source_eviction_cursor % entry.rendezvous_sources.len();
            entry.source_eviction_cursor = entry.source_eviction_cursor.wrapping_add(1);
            if let Some(oldest) = entry.rendezvous_sources.keys().nth(index).cloned() {
                entry.rendezvous_sources.remove(&oldest);
            }
        }
        entry.rendezvous_sources.insert(
            source.to_string(),
            RendezvousSource {
                candidates,
                expires_at: Instant::now() + RENDEZVOUS_CANDIDATE_TTL,
            },
        );
    }

    pub(super) fn clear_rendezvous(&mut self, source: Option<&str>) {
        self.clear_epoch = self.clear_epoch.wrapping_add(1);
        let mut tick = self.tick;
        for entry in self.entries.values_mut() {
            match source {
                Some(source) => {
                    entry.rendezvous_sources.remove(source);
                    let cached_endpoint = entry
                        .verified
                        .as_ref()
                        .filter(|cached| cached.source.as_deref() == Some(source))
                        .map(|cached| cached.address.id);
                    if let Some(endpoint_id) = cached_endpoint {
                        let replacement =
                            entry.rendezvous_sources.iter().find_map(|(name, other)| {
                                other
                                    .candidates
                                    .iter()
                                    .any(|candidate| candidate.id == endpoint_id)
                                    .then(|| name.clone())
                            });
                        if let Some(cached) = entry.verified.as_mut() {
                            cached.source = replacement.clone();
                        }
                        if replacement.is_none() {
                            entry.verified = None;
                        }
                    }
                }
                None => {
                    entry.rendezvous_sources.clear();
                    entry.verified = None;
                }
            }
            // The source may already have expired or been evicted while a
            // binding probe still holds its old candidate. Fence that probe
            // even when this account has no current entry for the source.
            tick = tick.wrapping_add(1);
            entry.revision = tick;
        }
        self.tick = tick;
    }

    fn store_verified(
        &mut self,
        recipient: &Pubkey,
        revision: u64,
        address: EndpointAddr,
        source: Option<String>,
        expires_at_ms: i64,
        expires_at: Instant,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(recipient) else {
            return false;
        };
        if entry.revision != revision {
            return false;
        }
        entry.verified = Some(CachedDestination {
            address,
            source,
            expires_at_ms,
            expires_at,
        });
        true
    }

    fn invalidate(&mut self, recipient: &Pubkey, endpoint_id: &str) {
        let tick = self.tick.wrapping_add(1);
        let Some(entry) = self.entries.get_mut(recipient) else {
            return;
        };
        self.tick = tick;
        entry.revision = tick;
        if entry
            .verified
            .as_ref()
            .is_some_and(|cached| cached.address.id.to_string() == endpoint_id)
        {
            entry.verified = None;
        }
    }
}

fn next_rendezvous_candidate(
    entry: &mut DestinationEntry,
    candidates: &[(EndpointAddr, Option<String>)],
) -> (EndpointAddr, Option<String>) {
    let index = entry.rendezvous_cursor % candidates.len();
    entry.rendezvous_cursor = entry.rendezvous_cursor.wrapping_add(1);
    candidates[index].clone()
}

fn next_peer(
    peers: &BTreeMap<String, EndpointAddr>,
    cursor: &mut Option<String>,
) -> Option<EndpointAddr> {
    let next = cursor
        .as_ref()
        .and_then(|after| peers.range((Excluded(after.clone()), Unbounded)).next())
        .or_else(|| peers.iter().next());
    let (key, address) = next?;
    *cursor = Some(key.clone());
    Some(address.clone())
}

impl IrohGossipTransport {
    pub(super) async fn offer_receive_candidates_impl(
        &self,
        source: &str,
        recipient: &Pubkey,
        candidates: Vec<EndpointAddr>,
        fence: ReceiveCandidateFence,
    ) -> Result<()> {
        receive_route_for_account(recipient)?;
        anyhow::ensure!(
            candidates.len() <= MAX_RENDEZVOUS_CANDIDATES,
            "too many account receive candidates"
        );
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        let mut unique = Vec::with_capacity(candidates.len());
        let mut seen = BTreeSet::new();
        for candidate in candidates {
            if candidate.id != self.endpoint.id() && seen.insert(candidate.id) {
                unique.push(candidate);
            }
        }
        let mut window = self.receive_destinations.lock().await;
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire)
                && fence.transport_instance == self.receive_offer_instance
                && fence.clear_epoch == window.clear_epoch,
            "account receive candidate fence is stale"
        );
        window.observe_rendezvous(source, recipient, unique);
        Ok(())
    }

    pub(super) async fn verify_receive_provider_impl(
        &self,
        sender: &Pubkey,
        provider: EndpointAddr,
    ) -> Result<()> {
        receive_route_for_account(sender)?;
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        let _permit = self
            .receive_destination_probes
            .try_acquire()
            .context("receive binding probe budget is full")?;
        let shutdown = self.offer_shutdown_notify.notified();
        tokio::pin!(shutdown);
        shutdown.as_mut().enable();
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        let binding = tokio::select! {
            _ = &mut shutdown => anyhow::bail!("account receive offer transport is closed"),
            result = fetch_receive_endpoint_binding(
                &self.endpoint,
                provider.clone(),
                sender,
                Instant::now() + BINDING_PROBE_TIMEOUT,
            ) => result?,
        };
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire)
                && binding.endpoint_id() == provider.id.to_string(),
            "receive provider changed or transport closed"
        );
        Ok(())
    }

    pub(super) async fn resolve_receive_destination_impl(
        &self,
        recipient: &Pubkey,
    ) -> Result<Option<EndpointAddr>> {
        receive_route_for_account(recipient)?;
        let shutdown = self.offer_shutdown_notify.notified();
        tokio::pin!(shutdown);
        shutdown.as_mut().enable();
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        let now_ms = Utc::now().timestamp_millis();
        if let Some(cached) = self
            .receive_destinations
            .lock()
            .await
            .cached(recipient, now_ms)
        {
            return Ok((!self.offer_closed.load(Ordering::Acquire)).then_some(cached));
        }
        // No queue of recipient lookups grows behind a busy transport.
        let Ok(_permit) = self.receive_destination_probes.try_acquire() else {
            return Ok(None);
        };
        let imported_page = if let Some(store) = &self.account_store {
            let after = self
                .receive_destinations
                .lock()
                .await
                .touch(recipient)
                .cursors[2]
                .clone();
            store
                .imported_peer_candidate_window("gossip", after.as_deref(), CANDIDATES_PER_LOOKUP)
                .await?
                .into_iter()
                .map(|(id, bytes)| Ok((id, serde_json::from_slice::<EndpointAddr>(&bytes)?)))
                .collect::<Result<BTreeMap<_, _>>>()?
        } else {
            BTreeMap::new()
        };
        let mut learned = [BTreeMap::new(), BTreeMap::new(), BTreeMap::new()];
        let mut learned_pages: [Option<(i64, String)>; 3] = Default::default();
        if let Some(store) = &self.account_store {
            let cursors = self
                .receive_destinations
                .lock()
                .await
                .touch(recipient)
                .learned_cursors
                .clone();
            for (index, scope) in ["gossip", "docs", "blob"].into_iter().enumerate() {
                let page = store
                    .peer_candidate_window(scope, "learned", cursors[index].clone(), 1, now_ms)
                    .await?;
                if let Some((id, bytes, seen_ms)) = page.into_iter().next() {
                    learned[index].insert(id.clone(), serde_json::from_slice(&bytes)?);
                    learned_pages[index] = Some((seen_ms, id));
                }
            }
        }
        let configured = self.configured_seed_peers.lock().await;
        let bootstrap = self.bootstrap_seed_peers.lock().await;
        let imported = self.imported_peers.lock().await;
        let imported_source = if self.account_store.is_some() {
            &imported_page
        } else {
            &*imported
        };
        let (candidates, revision) = self.receive_destinations.lock().await.select(
            recipient,
            [
                &configured,
                &bootstrap,
                imported_source,
                &learned[0],
                &learned[1],
                &learned[2],
            ],
        );
        drop((configured, bootstrap, imported));
        for (candidate, source) in candidates {
            if self.offer_closed.load(Ordering::Acquire) {
                return Ok(None);
            }
            self.receive_destinations.lock().await.attempted_learned(
                recipient,
                candidate.id,
                &learned_pages,
            );
            let deadline = Instant::now() + BINDING_PROBE_TIMEOUT;
            let result = tokio::select! {
                _ = &mut shutdown => return Ok(None),
                result = fetch_receive_endpoint_binding(
                    &self.endpoint,
                    candidate.clone(),
                    recipient,
                    deadline,
                ) => result,
            };
            let Ok(binding) = result else {
                continue;
            };
            if self.offer_closed.load(Ordering::Acquire) {
                return Ok(None);
            }
            let now_ms = Utc::now().timestamp_millis();
            let expires_at_ms = binding.expires_at_ms().min(now_ms + MAX_CACHED_BINDING_MS);
            if expires_at_ms <= now_ms {
                continue;
            }
            let stored = self.receive_destinations.lock().await.store_verified(
                recipient,
                revision,
                candidate.clone(),
                source,
                expires_at_ms,
                Instant::now() + Duration::from_millis((expires_at_ms - now_ms) as u64),
            );
            if self.offer_closed.load(Ordering::Acquire) {
                self.invalidate_receive_destination_impl(recipient, &candidate.id.to_string())
                    .await;
                return Ok(None);
            }
            return Ok(stored.then_some(candidate));
        }
        Ok(None)
    }

    pub(super) async fn invalidate_receive_destination_impl(
        &self,
        recipient: &Pubkey,
        endpoint_id: &str,
    ) {
        self.receive_destinations
            .lock()
            .await
            .invalidate(recipient, endpoint_id);
    }
}

#[cfg(test)]
#[path = "tests/receive_destination.rs"]
mod tests;
