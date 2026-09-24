//! Shared fetch admission for one node: caller-owned display work and bounded,
//! independently owned ordinary fetches. Only admitted running work gets a task.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use kukuri_transport::work_admission::{
    NetworkWorkOwner, WorkAdmission, WorkCompletion, WorkId, WorkKey, WorkLane, WorkLimits,
    WorkMode, WorkPersistence, WorkProtocol, WorkScope, WorkWaiter,
};
use tokio::sync::Notify;
use tokio::time::{Instant, timeout_at};

mod fetch;
pub(crate) use fetch::{FetchIdentity, FetchRequest};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkAdmissionError {
    Deferred,
    Expired,
    Closed,
    CoolingDown,
    ConflictingRequest,
}

impl std::fmt::Display for NetworkAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "network work admission: {self:?}")
    }
}

impl std::error::Error for NetworkAdmissionError {}

struct State {
    policy: NetworkWorkOwner,
    scopes: BTreeMap<WorkId, WorkScope>,
    ready: BTreeSet<WorkId>,
    cancelled: BTreeSet<WorkId>,
    fetches: BTreeMap<WorkId, fetch::FetchEntry>,
    identities: BTreeMap<FetchIdentity, WorkId>,
    closed: bool,
}

impl State {
    fn advance(&mut self) {
        let now = Instant::now().into_std();
        self.policy.expire(now);
        while let Some(id) = self.policy.next_cancellation() {
            self.cancelled.insert(id);
        }
        if !self.closed {
            while let Some(work) = self.policy.start_next(now) {
                self.ready.insert(work.id);
            }
        }
    }
}

pub(crate) struct NetworkWorkRuntime {
    state: Mutex<State>,
    changed: Arc<Notify>,
    driver: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Default for NetworkWorkRuntime {
    fn default() -> Self {
        Self::new(WorkLimits::default())
    }
}

impl NetworkWorkRuntime {
    fn new(limits: WorkLimits) -> Self {
        Self {
            state: Mutex::new(State {
                policy: NetworkWorkOwner::new(limits),
                scopes: BTreeMap::new(),
                ready: BTreeSet::new(),
                cancelled: BTreeSet::new(),
                fetches: BTreeMap::new(),
                identities: BTreeMap::new(),
                closed: false,
            }),
            changed: Arc::new(Notify::new()),
            driver: Mutex::new(None),
        }
    }

    pub(crate) async fn acquire(
        self: &Arc<Self>,
        hash: [u8; 32],
        deadline: Instant,
    ) -> Result<DisplayWorkLease, NetworkAdmissionError> {
        self.acquire_blob_work(
            WorkProtocol::Blob,
            hash,
            u64::MAX,
            WorkMode::Display,
            WorkLane::Interactive,
            deadline,
        )
        .await
    }

    /// A caller-owned bounded blob transfer shares the same running budget as
    /// ordinary and display fetches without joining either fetch identity.
    pub(crate) async fn acquire_bounded_blob(
        self: &Arc<Self>,
        hash: [u8; 32],
        max_bytes: u64,
        deadline: Instant,
    ) -> Result<DisplayWorkLease, NetworkAdmissionError> {
        self.acquire_blob_work(
            WorkProtocol::Blob,
            hash,
            max_bytes,
            WorkMode::Fetch,
            WorkLane::Background,
            deadline,
        )
        .await
    }

    async fn acquire_blob_work(
        self: &Arc<Self>,
        protocol: WorkProtocol,
        hash: [u8; 32],
        byte_limit: u64,
        mode: WorkMode,
        lane: WorkLane,
        deadline: Instant,
    ) -> Result<DisplayWorkLease, NetworkAdmissionError> {
        let lease = {
            let mut state = self.state.lock().expect("display admission poisoned");
            if state.closed {
                return Err(NetworkAdmissionError::Closed);
            }
            if deadline <= Instant::now() {
                return Err(NetworkAdmissionError::Expired);
            }
            let scope = state
                .policy
                .register_scope()
                .ok_or(NetworkAdmissionError::Deferred)?;
            // Each display consumer owns its cancellation boundary. Display
            // fetches never join a normal or another scope's stored fetch.
            let key = WorkKey {
                scope,
                protocol,
                object: hash,
                mode,
                persistence: WorkPersistence::Ephemeral,
                byte_limit,
                deadline: deadline.into_std(),
                lane,
            };
            let waiter = match state.policy.admit(key, &hash, Instant::now().into_std()) {
                WorkAdmission::Admitted(waiter) => waiter,
                _ => {
                    state.policy.revoke_scope(scope);
                    return Err(NetworkAdmissionError::Deferred);
                }
            };
            state.scopes.insert(waiter.work_id(), scope);
            state.advance();
            DisplayWorkLease {
                owner: self.clone(),
                scope,
                waiter,
                deadline,
                released: false,
            }
        };
        self.changed.notify_waiters();
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self.state.lock().expect("display admission poisoned");
                state.advance();
                if state.closed {
                    return Err(NetworkAdmissionError::Closed);
                }
                if deadline <= Instant::now() {
                    return Err(NetworkAdmissionError::Expired);
                }
                if state.ready.remove(&lease.waiter.work_id()) {
                    return Ok(lease);
                }
            }
            timeout_at(deadline, notified)
                .await
                .map_err(|_| NetworkAdmissionError::Expired)?;
        }
    }

    pub(crate) fn close(&self) {
        {
            let mut state = self.state.lock().expect("display admission poisoned");
            if state.closed {
                return;
            }
            state.closed = true;
            let scopes = state.scopes.values().copied().collect::<Vec<_>>();
            for scope in scopes {
                state.policy.revoke_scope(scope);
            }
            state.advance();
        }
        self.changed.notify_waiters();
    }

    fn release(&self, scope: WorkScope, waiter: WorkWaiter, publish: bool) -> bool {
        let publish = {
            let mut state = self.state.lock().expect("display admission poisoned");
            let id = waiter.work_id();
            if !publish {
                state.policy.release_waiter(waiter);
                state.policy.revoke_scope(scope);
            }
            let disposition = state.policy.complete(id, Instant::now().into_std());
            state.policy.revoke_scope(scope);
            state.scopes.remove(&id);
            state.ready.remove(&id);
            state.cancelled.remove(&id);
            state.advance();
            !state.closed && disposition == WorkCompletion::Publish
        };
        self.changed.notify_waiters();
        publish
    }
}

impl Drop for NetworkWorkRuntime {
    fn drop(&mut self) {
        if let Some(driver) = self
            .driver
            .get_mut()
            .expect("network driver poisoned")
            .take()
        {
            driver.abort();
        }
    }
}

pub(crate) struct DisplayWorkLease {
    owner: Arc<NetworkWorkRuntime>,
    scope: WorkScope,
    waiter: WorkWaiter,
    deadline: Instant,
    released: bool,
}

impl DisplayWorkLease {
    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.owner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self.owner.state.lock().expect("display admission poisoned");
                state.advance();
                if state.closed || state.cancelled.contains(&self.waiter.work_id()) {
                    return;
                }
            }
            if timeout_at(self.deadline, notified).await.is_err() {
                return;
            }
        }
    }

    pub(crate) fn finish(mut self) -> bool {
        self.released = true;
        self.owner.release(self.scope, self.waiter, true)
    }
}

impl Drop for DisplayWorkLease {
    fn drop(&mut self) {
        if !self.released {
            self.owner.release(self.scope, self.waiter, false);
        }
    }
}

#[cfg(test)]
mod tests;
