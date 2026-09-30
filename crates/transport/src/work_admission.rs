//! Bounded admission and cancellation policy for one account runtime (ADR 0055).
//!
//! This owns no I/O and spawns no tasks. The executor starts only returned work,
//! consumes cancellation commands, and acknowledges completion after I/O stops.
//! Scope authorization belongs to the caller; a token only proves that its
//! authorized scope is still registered with this owner. Production adapters
//! must check the completion disposition before publishing or storing a result.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use web_time::Instant;

static OWNER_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const LANES: [usize; 7] = [0, 0, 0, 0, 1, 1, 2];

#[derive(Clone, Copy, Debug)]
pub struct WorkLimits {
    pub scopes: usize,
    pub requests: usize,
    pub waiters_per_request: usize,
    pub metadata_bytes: usize,
    pub running: usize,
}

impl Default for WorkLimits {
    fn default() -> Self {
        Self {
            scopes: 64,
            requests: 256,
            waiters_per_request: 64,
            metadata_bytes: 4 * 1024 * 1024,
            running: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct WorkScope {
    owner: u64,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct WorkId {
    owner: u64,
    serial: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkWaiter {
    scope: WorkScope,
    work: WorkId,
    serial: u64,
}

impl WorkWaiter {
    pub fn work_id(self) -> WorkId {
        self.work
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum WorkProtocol {
    Gossip,
    Docs,
    Blob,
    ReceiveBinding,
    CommunityNode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum WorkMode {
    LocalOnly,
    Display,
    Fetch,
    Send,
    Sync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum WorkPersistence {
    Ephemeral,
    Store,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum WorkLane {
    Interactive,
    Send,
    Background,
}

impl WorkLane {
    fn index(self) -> usize {
        match self {
            Self::Interactive => 0,
            Self::Send => 1,
            Self::Background => 2,
        }
    }
}

/// A fixed-size identity. `object` is a digest of the complete target (including
/// replica or provider restrictions), never just an untrusted display label.
/// Different deadlines do not coalesce: joining cannot extend an attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct WorkKey {
    pub scope: WorkScope,
    pub protocol: WorkProtocol,
    pub object: [u8; 32],
    pub mode: WorkMode,
    pub persistence: WorkPersistence,
    pub byte_limit: u64,
    pub deadline: Instant,
    pub lane: WorkLane,
}

#[derive(Debug, Eq, PartialEq)]
pub enum AdmissionDenied {
    InactiveScope,
    Expired,
    MetadataTooLarge,
    ConflictingMetadata,
    LocalOnly,
    IdentifierExhausted,
}

#[derive(Debug, Eq, PartialEq)]
pub enum WorkAdmission {
    Admitted(WorkWaiter),
    Joined(WorkWaiter),
    Deferred { retry_at: Instant },
    Denied(AdmissionDenied),
}

#[derive(Debug)]
pub struct StartedWork {
    pub id: WorkId,
    pub key: WorkKey,
    /// Only bounded execution metadata, never a copy of the blob or post body.
    pub metadata: Box<[u8]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkCompletion {
    Publish,
    Discard,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkUsage {
    pub scopes: usize,
    pub requests: usize,
    pub running: usize,
    pub stopping: usize,
    pub metadata_bytes: usize,
    pub waiters: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Queued,
    Running,
    Stopping,
}

struct Entry {
    key: WorkKey,
    metadata: Option<Box<[u8]>>,
    metadata_hash: blake3::Hash,
    metadata_bytes: usize,
    waiters: BTreeSet<u64>,
    phase: Phase,
}

pub struct NetworkWorkOwner {
    owner: u64,
    serial: u64,
    limits: WorkLimits,
    scopes: BTreeMap<WorkScope, BTreeSet<WorkId>>,
    entries: BTreeMap<WorkId, Entry>,
    keys: BTreeMap<WorkKey, WorkId>,
    deadlines: BTreeSet<(Instant, WorkId)>,
    queues: [BTreeSet<WorkId>; 3],
    cancellations: BTreeSet<WorkId>,
    cursor: usize,
    running: usize,
    metadata_bytes: usize,
}

impl NetworkWorkOwner {
    pub fn new(limits: WorkLimits) -> Self {
        let owner = OWNER_SEQUENCE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("network work owner identity exhausted");
        Self {
            owner,
            serial: 0,
            limits,
            scopes: BTreeMap::new(),
            entries: BTreeMap::new(),
            keys: BTreeMap::new(),
            deadlines: BTreeSet::new(),
            queues: std::array::from_fn(|_| BTreeSet::new()),
            cancellations: BTreeSet::new(),
            cursor: 0,
            running: 0,
            metadata_bytes: 0,
        }
    }

    fn next_serial(&mut self) -> Option<u64> {
        self.serial = self.serial.checked_add(1)?;
        Some(self.serial)
    }

    /// Register one already-authorized scope generation. Reuse this token for
    /// unchanged demand; revoke and register afresh when its authority changes.
    pub fn register_scope(&mut self) -> Option<WorkScope> {
        if self.scopes.len() >= self.limits.scopes {
            return None;
        }
        let scope = WorkScope {
            owner: self.owner,
            generation: self.next_serial()?,
        };
        self.scopes.insert(scope, BTreeSet::new());
        Some(scope)
    }

    /// Only visits work using this scope. Stopping I/O continues to occupy both
    /// request and running capacity until the executor acknowledges completion.
    pub fn revoke_scope(&mut self, scope: WorkScope) {
        if let Some(work) = self.scopes.remove(&scope) {
            for id in work {
                self.cancel(id);
            }
        }
    }

    pub fn admit(&mut self, key: WorkKey, metadata: &[u8], now: Instant) -> WorkAdmission {
        self.expire(now);
        let denied = if !self.scopes.contains_key(&key.scope) {
            Some(AdmissionDenied::InactiveScope)
        } else if key.deadline <= now {
            Some(AdmissionDenied::Expired)
        } else if key.mode == WorkMode::LocalOnly {
            Some(AdmissionDenied::LocalOnly)
        } else if metadata.len() > self.limits.metadata_bytes {
            Some(AdmissionDenied::MetadataTooLarge)
        } else {
            None
        };
        if let Some(reason) = denied {
            return WorkAdmission::Denied(reason);
        }
        if let Some(id) = self.keys.get(&key).copied() {
            let entry = &self.entries[&id];
            if entry.metadata_hash != blake3::hash(metadata) {
                return WorkAdmission::Denied(AdmissionDenied::ConflictingMetadata);
            }
            if entry.phase == Phase::Stopping
                || entry.waiters.len() >= self.limits.waiters_per_request
            {
                return self.deferred(now);
            }
            let Some(serial) = self.next_serial() else {
                return WorkAdmission::Denied(AdmissionDenied::IdentifierExhausted);
            };
            self.entries
                .get_mut(&id)
                .expect("indexed work exists")
                .waiters
                .insert(serial);
            return WorkAdmission::Joined(WorkWaiter {
                scope: key.scope,
                work: id,
                serial,
            });
        }
        if self.entries.len() >= self.limits.requests
            || self.limits.waiters_per_request == 0
            || metadata.len() > self.limits.metadata_bytes - self.metadata_bytes
        {
            return self.deferred(now);
        }
        let Some(serial) = self.next_serial() else {
            return WorkAdmission::Denied(AdmissionDenied::IdentifierExhausted);
        };
        let id = WorkId {
            owner: self.owner,
            serial,
        };
        self.entries.insert(
            id,
            Entry {
                key,
                metadata: Some(metadata.into()),
                metadata_hash: blake3::hash(metadata),
                metadata_bytes: metadata.len(),
                waiters: BTreeSet::from([serial]),
                phase: Phase::Queued,
            },
        );
        self.keys.insert(key, id);
        self.scopes
            .get_mut(&key.scope)
            .expect("scope validated before admission")
            .insert(id);
        self.deadlines.insert((key.deadline, id));
        self.queues[key.lane.index()].insert(id);
        self.metadata_bytes += metadata.len();
        WorkAdmission::Admitted(WorkWaiter {
            scope: key.scope,
            work: id,
            serial,
        })
    }

    fn deferred(&self, now: Instant) -> WorkAdmission {
        // Retry is caller-owned and optional; completion can wake demand sooner.
        // Never return an already-expired deadline while stopping I/O holds slots.
        WorkAdmission::Deferred {
            retry_at: now + Duration::from_millis(250),
        }
    }

    pub fn release_waiter(&mut self, waiter: WorkWaiter) {
        let Some(entry) = self.entries.get_mut(&waiter.work) else {
            return;
        };
        if entry.key.scope != waiter.scope || !entry.waiters.remove(&waiter.serial) {
            return;
        }
        if entry.waiters.is_empty()
            && (entry.phase == Phase::Queued || entry.key.mode == WorkMode::Display)
        {
            self.cancel(waiter.work);
        }
    }

    /// Returns at most one job, and never waits on a permit or starts I/O itself.
    pub fn start_next(&mut self, now: Instant) -> Option<StartedWork> {
        self.expire(now);
        if self.running >= self.limits.running {
            return None;
        }
        for _ in 0..LANES.len() {
            let lane = LANES[self.cursor];
            self.cursor = (self.cursor + 1) % LANES.len();
            if let Some(id) = self.queues[lane].pop_first() {
                let entry = self.entries.get_mut(&id).expect("queued work exists");
                entry.phase = Phase::Running;
                self.running += 1;
                return Some(StartedWork {
                    id,
                    key: entry.key,
                    metadata: entry.metadata.take().expect("queued work owns metadata"),
                });
            }
        }
        None
    }

    pub fn expire(&mut self, now: Instant) {
        while let Some(&(deadline, id)) = self.deadlines.first() {
            if deadline > now {
                break;
            }
            self.deadlines.pop_first();
            self.cancel(id);
        }
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadlines.first().map(|(deadline, _)| *deadline)
    }

    fn cancel(&mut self, id: WorkId) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if entry.phase == Phase::Queued {
            self.remove(id);
        } else if entry.phase == Phase::Running {
            entry.phase = Phase::Stopping;
            self.deadlines.remove(&(entry.key.deadline, id));
            self.cancellations.insert(id);
        }
    }

    /// One cancellation per job. The executor must stop/await I/O, then complete.
    pub fn next_cancellation(&mut self) -> Option<WorkId> {
        self.cancellations.pop_first()
    }

    /// `Publish` is only a generation/deadline disposition, not authorization to
    /// skip the caller's content, audience or persistence guards.
    pub fn complete(&mut self, id: WorkId, now: Instant) -> WorkCompletion {
        let Some(entry) = self.entries.get(&id) else {
            return WorkCompletion::Unknown;
        };
        if entry.phase == Phase::Queued {
            return WorkCompletion::Unknown;
        }
        let publish = entry.phase == Phase::Running
            && entry.key.deadline > now
            && self.scopes.contains_key(&entry.key.scope);
        self.remove(id);
        if publish {
            WorkCompletion::Publish
        } else {
            WorkCompletion::Discard
        }
    }

    fn remove(&mut self, id: WorkId) {
        let Some(entry) = self.entries.remove(&id) else {
            return;
        };
        self.keys.remove(&entry.key);
        self.deadlines.remove(&(entry.key.deadline, id));
        self.queues[entry.key.lane.index()].remove(&id);
        self.cancellations.remove(&id);
        if let Some(work) = self.scopes.get_mut(&entry.key.scope) {
            work.remove(&id);
        }
        if entry.phase != Phase::Queued {
            self.running -= 1;
        }
        self.metadata_bytes -= entry.metadata_bytes;
    }

    /// Whether a reservation still occupies capacity (including stopping work).
    pub fn contains(&self, id: WorkId) -> bool {
        self.entries.contains_key(&id)
    }

    /// Diagnostic snapshot over the bounded live request set, never saved history.
    pub fn usage(&self) -> WorkUsage {
        WorkUsage {
            scopes: self.scopes.len(),
            requests: self.entries.len(),
            running: self.running,
            stopping: self
                .entries
                .values()
                .filter(|entry| entry.phase == Phase::Stopping)
                .count(),
            metadata_bytes: self.metadata_bytes,
            waiters: self.entries.values().map(|entry| entry.waiters.len()).sum(),
        }
    }
}

#[cfg(test)]
mod tests;
