use super::*;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Weak;

use kukuri_transport::SharedRemoteFetchResult;
use tokio::sync::watch;
use tokio::task::{AbortHandle, Id, JoinSet};

pub(crate) type FetchFuture = Pin<Box<dyn Future<Output = anyhow::Result<Option<Vec<u8>>>> + Send>>;
pub(crate) type FetchFinished =
    Box<dyn FnOnce(bool) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// Preserve the existing service-specific coalescing boundary using the retry
/// ledger's non-reusable process-local generation. This is not a network identity
/// or scope proof, and does not depend on callback/Arc allocation lifetimes.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct FetchIdentity {
    pub service: u64,
    pub key: String,
}

pub(crate) struct FetchRequest {
    pub identity: FetchIdentity,
    pub protocol: WorkProtocol,
    pub object: [u8; 32],
    pub persistence: WorkPersistence,
    pub byte_limit: u64,
    pub cooling_down: bool,
}

pub(super) struct FetchEntry {
    identity: FetchIdentity,
    key: WorkKey,
    sender: watch::Sender<Option<SharedRemoteFetchResult>>,
    future: Option<FetchFuture>,
    finished: Option<FetchFinished>,
}

pub(crate) struct FetchWaiter {
    owner: Arc<NetworkWorkRuntime>,
    waiter: WorkWaiter,
    receiver: watch::Receiver<Option<SharedRemoteFetchResult>>,
}

impl FetchWaiter {
    pub(crate) async fn result(mut self) -> SharedRemoteFetchResult {
        match self.receiver.wait_for(Option::is_some).await {
            Ok(result) => result.clone().expect("finished result"),
            Err(_) => Ok(None),
        }
    }
}

impl Drop for FetchWaiter {
    fn drop(&mut self) {
        self.owner.release_fetch_waiter(self.waiter);
    }
}

impl NetworkWorkRuntime {
    pub(crate) fn submit_fetch(
        self: &Arc<Self>,
        request: FetchRequest,
        future: FetchFuture,
        finished: FetchFinished,
    ) -> Result<FetchWaiter, NetworkAdmissionError> {
        let FetchRequest {
            identity,
            protocol,
            object,
            persistence,
            byte_limit,
            cooling_down,
        } = request;
        let waiter = {
            let mut state = self.state.lock().expect("network admission poisoned");
            state.advance();
            if state.closed {
                return Err(NetworkAdmissionError::Closed);
            }
            // Flight keys contain a mode, numeric byte limit and content hash;
            // reject arbitrary retained labels before allocating a reservation.
            if identity.key.len() > 256 {
                return Err(NetworkAdmissionError::Deferred);
            }
            let now = Instant::now();
            if let Some(id) = state.identities.get(&identity).copied() {
                let entry = &state.fetches[&id];
                let key = entry.key;
                if key.protocol != protocol
                    || key.object != object
                    || key.persistence != persistence
                    || key.byte_limit != byte_limit
                {
                    return Err(NetworkAdmissionError::ConflictingRequest);
                }
                if key.deadline <= now.into_std() {
                    return Err(NetworkAdmissionError::Expired);
                }
                match state.policy.admit(key, &object, now.into_std()) {
                    WorkAdmission::Joined(waiter) => FetchWaiter {
                        owner: self.clone(),
                        waiter,
                        receiver: state.fetches[&id].sender.subscribe(),
                    },
                    _ => return Err(NetworkAdmissionError::Deferred),
                }
            } else {
                if cooling_down {
                    return Err(NetworkAdmissionError::CoolingDown);
                }
                let scope = state
                    .policy
                    .register_scope()
                    .ok_or(NetworkAdmissionError::Deferred)?;
                let key = WorkKey {
                    scope,
                    object,
                    protocol,
                    mode: WorkMode::Fetch,
                    persistence,
                    byte_limit,
                    lane: WorkLane::Interactive,
                    deadline: (now + crate::remote_fetch::REMOTE_FETCH_TOTAL_TIMEOUT).into_std(),
                };
                let waiter = match state.policy.admit(key, &object, now.into_std()) {
                    WorkAdmission::Admitted(waiter) => waiter,
                    _ => {
                        state.policy.revoke_scope(scope);
                        return Err(NetworkAdmissionError::Deferred);
                    }
                };
                let id = waiter.work_id();
                let (sender, receiver) = watch::channel(None);
                state.scopes.insert(id, scope);
                state.identities.insert(identity.clone(), id);
                state.fetches.insert(
                    id,
                    FetchEntry {
                        identity,
                        key,
                        sender,
                        future: Some(future),
                        finished: Some(finished),
                    },
                );
                state.advance();
                FetchWaiter {
                    owner: self.clone(),
                    waiter,
                    receiver,
                }
            }
        };
        self.ensure_fetch_driver();
        self.changed.notify_waiters();
        Ok(waiter)
    }

    fn ensure_fetch_driver(self: &Arc<Self>) {
        let mut driver = self.driver.lock().expect("network driver poisoned");
        if driver.is_none() {
            let weak = Arc::downgrade(self);
            let changed = self.changed.clone();
            *driver = Some(tokio::spawn(run(weak, changed)).abort_handle());
        }
    }

    fn release_fetch_waiter(&self, waiter: WorkWaiter) {
        let retired = {
            let mut state = self.state.lock().expect("network admission poisoned");
            state.policy.release_waiter(waiter);
            let retired = if !state.policy.contains(waiter.work_id()) {
                take_entry(&mut state, waiter.work_id())
            } else {
                None
            };
            state.advance();
            retired
        };
        // Queued futures can own the last Node Arc. Never drop them under the
        // admission lock: Node::drop closes this same runtime.
        if let Some((entry, _)) = retired {
            let _ = entry.sender.send(Some(Ok(None)));
        }
        self.changed.notify_waiters();
    }

    #[cfg(test)]
    pub(crate) fn fetch_count(&self) -> usize {
        self.state
            .lock()
            .expect("network admission poisoned")
            .fetches
            .len()
    }
}

fn take_entry(state: &mut State, id: WorkId) -> Option<(FetchEntry, bool)> {
    let entry = state.fetches.remove(&id)?;
    let publish = state.policy.complete(id, Instant::now().into_std()) == WorkCompletion::Publish
        && !state.closed;
    state.policy.revoke_scope(entry.key.scope);
    state.identities.remove(&entry.identity);
    state.scopes.remove(&id);
    state.ready.remove(&id);
    state.cancelled.remove(&id);
    Some((entry, publish))
}

async fn cancelled(owner: Weak<NetworkWorkRuntime>, id: WorkId, changed: Arc<Notify>) {
    loop {
        let notified = changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let Some(owner) = owner.upgrade() else { return };
        let stopped = {
            let state = owner.state.lock().expect("network admission poisoned");
            state.closed || state.cancelled.contains(&id)
        };
        if stopped {
            return;
        }
        drop(owner);
        notified.await;
    }
}

async fn complete(owner: &NetworkWorkRuntime, id: WorkId, result: SharedRemoteFetchResult) {
    let callback = {
        let mut state = owner.state.lock().expect("network admission poisoned");
        state
            .fetches
            .get_mut(&id)
            .and_then(|entry| entry.finished.take())
    };
    // Keep the in-flight identity and slot until the existing retry ledger has
    // recorded the outcome, even when every caller stopped waiting.
    if let Some(callback) = callback {
        callback(matches!(result, Ok(Some(_)))).await;
    }
    let retired = {
        let mut state = owner.state.lock().expect("network admission poisoned");
        let retired = take_entry(&mut state, id);
        state.advance();
        retired
    };
    if let Some((entry, publish)) = retired {
        let _ = entry
            .sender
            .send(Some(if publish { result } else { Ok(None) }));
    }
    owner.changed.notify_waiters();
}

async fn run(owner: Weak<NetworkWorkRuntime>, changed: Arc<Notify>) {
    let mut tasks = JoinSet::new();
    let mut running: HashMap<Id, (WorkId, AbortHandle)> = HashMap::new();
    loop {
        let notified = changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let Some(current) = owner.upgrade() else {
            break;
        };
        let (starts, retired, deadline, closed) = {
            let mut state = current.state.lock().expect("network admission poisoned");
            state.advance();
            let obsolete = state
                .fetches
                .iter()
                .filter_map(|(&id, entry)| {
                    (!state.policy.contains(id) || (state.closed && entry.future.is_some()))
                        .then_some(id)
                })
                .collect::<Vec<_>>();
            let retired = obsolete
                .into_iter()
                .filter_map(|id| take_entry(&mut state, id))
                .collect::<Vec<_>>();
            for (id, handle) in running.values() {
                if state.closed || state.cancelled.contains(id) {
                    handle.abort();
                }
            }
            let mut starts = Vec::new();
            if !state.closed {
                let ready = state.ready.iter().copied().collect::<Vec<_>>();
                for id in ready {
                    if let Some(entry) = state.fetches.get_mut(&id)
                        && let Some(future) = entry.future.take()
                    {
                        starts.push((id, Instant::from_std(entry.key.deadline), future));
                        state.ready.remove(&id);
                    }
                }
            }
            (
                starts,
                retired,
                state.policy.next_deadline().map(Instant::from_std),
                state.closed,
            )
        };
        drop(current);
        for (entry, _) in retired {
            let _ = entry.sender.send(Some(Ok(None)));
        }
        for (id, deadline, future) in starts {
            let weak = owner.clone();
            let signal = changed.clone();
            let handle = tasks.spawn(async move {
                let result = tokio::select! {
                    biased;
                    _ = cancelled(weak, id, signal) => Ok(None),
                    result = timeout_at(deadline, future) => result.unwrap_or(Ok(None)),
                };
                (
                    id,
                    result.map(|bytes| bytes.map(Arc::new)).map_err(Arc::new),
                )
            });
            running.insert(handle.id(), (id, handle));
        }
        if closed && tasks.is_empty() {
            break;
        }
        let timer = async {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            joined = tasks.join_next_with_id(), if !tasks.is_empty() => {
                let (id, result) = match joined.expect("nonempty task set") {
                    Ok((task, result)) => { running.remove(&task); result }
                    Err(error) => {
                        let (id, _) = running.remove(&error.id()).expect("tracked fetch task");
                        (id, Err(Arc::new(anyhow::anyhow!("fetch task terminated: {error}"))))
                    }
                };
                if let Some(current) = owner.upgrade() { complete(&current, id, result).await; }
            }
            _ = notified => {}
            _ = timer => {}
        }
    }
}
