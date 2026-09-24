//! Node-shared observations for the blob ALPN. Source/address books remain
//! independent so their change notifications and scope choices do not collapse.
use super::*;
use std::sync::Mutex as StdMutex;

pub const MAX_BLOB_PEER_RECORDS: usize = 1_024;

struct Entry {
    touched: u64,
    record: Arc<StdMutex<PeerRuntimeRecord>>,
}

#[derive(Default)]
struct Records {
    entries: BTreeMap<EndpointId, Entry>,
    age: BTreeSet<(u64, EndpointId)>,
    sequence: u64,
    preferred: VecDeque<EndpointId>,
}

impl Records {
    fn next_age(&mut self) -> u64 {
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("blob peer cache age exhausted");
        self.sequence
    }
}

#[derive(Default)]
struct Rates {
    windows: BTreeMap<EndpointId, VecDeque<Instant>>,
    expiry: BTreeSet<(Instant, EndpointId)>,
}

#[derive(Default)]
pub struct BlobPeerHealth {
    records: Mutex<Records>,
    generation: AtomicU64,
    rates: Mutex<Rates>,
}

pub struct BlobPeerAttempt {
    owner: Arc<BlobPeerHealth>,
    peer: EndpointId,
    generation: u64,
    record: Arc<StdMutex<PeerRuntimeRecord>>,
}

impl BlobPeerHealth {
    async fn entry(&self, peer: EndpointId) -> Option<Arc<StdMutex<PeerRuntimeRecord>>> {
        let mut records = self.records.lock().await;
        let now = records.next_age();
        if let Some(entry) = records.entries.get(&peer) {
            let (old, record) = (entry.touched, entry.record.clone());
            records.age.remove(&(old, peer));
            records
                .entries
                .get_mut(&peer)
                .expect("existing record")
                .touched = now;
            records.age.insert((now, peer));
            return Some(record);
        }
        if records.entries.len() >= MAX_BLOB_PEER_RECORDS {
            // Active attempts pin their record, preserving generation fences.
            // The cache itself has fixed capacity, even if every slot is pinned.
            let expired = records.age.iter().find_map(|&(at, peer)| {
                (Arc::strong_count(&records.entries[&peer].record) == 1).then_some((at, peer))
            });
            let (at, retired) = expired?;
            records.age.remove(&(at, retired));
            records.entries.remove(&retired);
            records.preferred.retain(|peer| *peer != retired);
        }
        let record = Arc::new(StdMutex::new(PeerRuntimeRecord::default()));
        records.entries.insert(
            peer,
            Entry {
                touched: now,
                record: record.clone(),
            },
        );
        records.age.insert((now, peer));
        Some(record)
    }

    async fn touch(&self, peer: EndpointId, record: &Arc<StdMutex<PeerRuntimeRecord>>) {
        let mut records = self.records.lock().await;
        let Some(entry) = records.entries.get(&peer) else {
            return;
        };
        if !Arc::ptr_eq(&entry.record, record) {
            return;
        }
        let previous = entry.touched;
        let now = records.next_age();
        records.age.remove(&(previous, peer));
        records
            .entries
            .get_mut(&peer)
            .expect("existing record")
            .touched = now;
        records.age.insert((now, peer));
    }

    pub async fn begin(self: &Arc<Self>, peer: EndpointId) -> Option<BlobPeerAttempt> {
        let record = self.entry(peer).await?;
        let generation = self
            .generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("blob connection generation exhausted")
            + 1;
        let attempt = BlobPeerAttempt {
            owner: self.clone(),
            peer,
            generation,
            record,
        };
        attempt.connection(PeerConnectionStatus::Connecting).await;
        Some(attempt)
    }

    pub async fn snapshot(&self, peer: EndpointId) -> Option<PeerStateSnapshot> {
        let record = self.records.lock().await.entries.get(&peer)?.record.clone();
        let snapshot = record
            .lock()
            .expect("blob peer record poisoned")
            .snapshot(Instant::now());
        Some(snapshot)
    }

    pub(super) async fn preferred(&self) -> Vec<EndpointId> {
        let records = self.records.lock().await;
        let now = Instant::now();
        records
            .preferred
            .iter()
            .filter_map(|peer| {
                let entry = records.entries.get(peer)?;
                let state = entry.record.lock().expect("blob peer record poisoned");
                (state
                    .last_success_at
                    .is_some_and(|at| now.duration_since(at) <= PEER_FETCH_SUCCESS_TTL)
                    && state.retry_after.is_none_or(|at| at <= now)
                    && state.connection_status_at(now) != PeerConnectionStatus::Disconnected)
                    .then_some(*peer)
            })
            .collect()
    }

    async fn prefer(
        &self,
        peer: EndpointId,
        record: &Arc<StdMutex<PeerRuntimeRecord>>,
        generation: u64,
    ) {
        let mut records = self.records.lock().await;
        if !records
            .entries
            .get(&peer)
            .is_some_and(|entry| Arc::ptr_eq(&entry.record, record))
        {
            return;
        }
        if record
            .lock()
            .expect("blob peer record poisoned")
            .connection_generation
            != generation
        {
            return;
        }
        records.preferred.retain(|existing| *existing != peer);
        records.preferred.push_front(peer);
        records.preferred.truncate(2);
    }

    pub(crate) async fn rank(&self, peers: &mut [EndpointAddr]) {
        let records = self.records.lock().await;
        let now = Instant::now();
        peers.sort_by_key(|peer| {
            let state = records
                .entries
                .get(&peer.id)
                .map(|entry| entry.record.lock().expect("blob peer record poisoned"));
            let state = state.as_deref();
            let backing_off = state
                .and_then(|state| state.retry_after)
                .is_some_and(|at| at > now);
            let connection_rank = match state.map(|state| state.connection_status_at(now)) {
                Some(PeerConnectionStatus::Connected) => 0,
                Some(PeerConnectionStatus::Connecting) => 1,
                Some(PeerConnectionStatus::Unknown) | None => 2,
                Some(PeerConnectionStatus::Disconnected) => 3,
            };
            let recent_success = state
                .and_then(|state| state.last_success_at)
                .is_some_and(|at| now.duration_since(at) <= PEER_FETCH_SUCCESS_TTL);
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
                usize::from(!recent_success),
                failures,
                latency,
            )
        });
    }

    pub async fn record_request(&self, peer: EndpointId) -> RequestRateDecision {
        let now = Instant::now();
        let mut rates = self.rates.lock().await;
        while let Some(&(at, expired)) = rates.expiry.first() {
            if at > now {
                break;
            }
            rates.expiry.pop_first();
            rates.windows.remove(&expired);
        }
        if !rates.windows.contains_key(&peer) && rates.windows.len() >= MAX_BLOB_PEER_RECORDS {
            let at = rates.expiry.first().expect("full rate cache has expiry").0;
            return RequestRateDecision::Limited {
                retry_after: at.saturating_duration_since(now),
            };
        }
        let entries = rates.windows.entry(peer).or_default();
        while entries
            .front()
            .is_some_and(|at| now.duration_since(*at) >= PEER_FETCH_REQUEST_WINDOW)
        {
            entries.pop_front();
        }
        if entries.len() >= PEER_FETCH_REQUEST_LIMIT as usize {
            let at = *entries.front().expect("full request window");
            return RequestRateDecision::Limited {
                retry_after: PEER_FETCH_REQUEST_WINDOW.saturating_sub(now.duration_since(at)),
            };
        }
        let previous = entries.back().copied();
        entries.push_back(now);
        if let Some(previous) = previous {
            rates
                .expiry
                .remove(&(previous + PEER_FETCH_REQUEST_WINDOW, peer));
        }
        rates.expiry.insert((now + PEER_FETCH_REQUEST_WINDOW, peer));
        RequestRateDecision::Allowed
    }

    // Compatibility entry points. Real fetches use the pinned attempt below.
    pub(super) async fn connection(
        self: &Arc<Self>,
        peer: EndpointId,
        generation: u64,
        status: PeerConnectionStatus,
    ) {
        self.generation.fetch_max(generation, Ordering::Relaxed);
        if let Some(record) = self.entry(peer).await {
            BlobPeerAttempt {
                owner: self.clone(),
                peer,
                generation,
                record,
            }
            .connection(status)
            .await;
        }
    }

    pub(crate) async fn success(self: &Arc<Self>, peer: EndpointId, latency: Duration) {
        if let Some(record) = self.entry(peer).await {
            let generation = record
                .lock()
                .expect("blob peer record poisoned")
                .connection_generation;
            BlobPeerAttempt {
                owner: self.clone(),
                peer,
                generation,
                record,
            }
            .success(latency)
            .await;
        }
    }

    pub(crate) async fn failure(self: &Arc<Self>, peer: EndpointId, failure: PeerFetchFailure) {
        if let Some(record) = self.entry(peer).await {
            let generation = record
                .lock()
                .expect("blob peer record poisoned")
                .connection_generation;
            BlobPeerAttempt {
                owner: self.clone(),
                peer,
                generation,
                record,
            }
            .failure(failure)
            .await;
        }
    }
}

impl BlobPeerAttempt {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub async fn connection(&self, status: PeerConnectionStatus) {
        self.owner.touch(self.peer, &self.record).await;
        let mut state = self.record.lock().expect("blob peer record poisoned");
        if self.generation < state.connection_generation {
            return;
        }
        state.connection_generation = self.generation;
        state.connection_status = status;
        state.connection_observed_at = Some(Instant::now());
    }

    pub async fn success(&self, latency: Duration) {
        self.owner.touch(self.peer, &self.record).await;
        {
            let mut state = self.record.lock().expect("blob peer record poisoned");
            state.fetch_successes = state.fetch_successes.saturating_add(1);
            if self.generation < state.connection_generation {
                return;
            }
            state.consecutive_fetch_failures = 0;
            state.retry_after = None;
            state.last_success_at = Some(Instant::now());
            state.connection_status = PeerConnectionStatus::Connected;
            state.connection_observed_at = state.last_success_at;
            let latency = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX);
            state.smoothed_fetch_latency_ms = Some(match state.smoothed_fetch_latency_ms {
                Some(previous) => previous.saturating_mul(3).saturating_add(latency) / 4,
                None => latency,
            });
        }
        self.owner
            .prefer(self.peer, &self.record, self.generation)
            .await;
    }

    pub async fn failure(&self, failure: PeerFetchFailure) {
        self.owner.touch(self.peer, &self.record).await;
        let mut state = self.record.lock().expect("blob peer record poisoned");
        if failure == PeerFetchFailure::NotFound {
            state.fetch_misses = state.fetch_misses.saturating_add(1);
            return;
        }
        if failure == PeerFetchFailure::Rejected {
            state.fetch_rejections = state.fetch_rejections.saturating_add(1);
            return;
        }
        state.fetch_failures = state.fetch_failures.saturating_add(1);
        if self.generation < state.connection_generation {
            return;
        }
        state.consecutive_fetch_failures = state.consecutive_fetch_failures.saturating_add(1);
        if matches!(
            failure,
            PeerFetchFailure::ConnectFailed | PeerFetchFailure::ConnectTimeout
        ) {
            state.connection_status = PeerConnectionStatus::Disconnected;
        }
        if failure != PeerFetchFailure::Cancelled {
            let exponent = state.consecutive_fetch_failures.saturating_sub(1).min(5);
            state.retry_after = Some(
                Instant::now()
                    + PEER_FETCH_BACKOFF_BASE
                        .saturating_mul(2u32.saturating_pow(exponent))
                        .min(PEER_FETCH_BACKOFF_MAX),
            );
        }
    }
}

#[cfg(test)]
mod tests;
