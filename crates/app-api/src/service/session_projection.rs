//! #1262: session候補と表示要求の有界な作業集合。timerは持たない。
use super::*;
use kukuri_core::BlobHash;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const SESSION_TARGET_LIMIT: usize = 64;
const FETCH_CONCURRENCY: usize = 2;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SessionCandidateView {
    pub replica_id: String,
    pub session_id: String,
    pub kind: String,
}

struct Running {
    token: u64,
    hash: BlobHash,
    task: n0_future::task::JoinHandle<()>,
}
struct Entry {
    topic: String,
    replica: ReplicaId,
    key: String,
    hashes: Vec<BlobHash>,
    observers: HashSet<String>,
    requested: HashSet<String>,
    running: Option<Running>,
}
impl Entry {
    fn candidate(&self) -> SessionCandidateView {
        let (kind, id) = self
            .key
            .strip_prefix("sessions/")
            .expect("validated session prefix")
            .split_once('/')
            .expect("validated session kind");
        SessionCandidateView {
            replica_id: self.replica.as_str().to_owned(),
            session_id: id
                .strip_suffix("/state")
                .expect("validated state suffix")
                .to_owned(),
            kind: kind.to_owned(),
        }
    }
    async fn cancel(&mut self) {
        if let Some(running) = self.running.take() {
            running.task.abort();
            // 取消済みtaskの終了まで保持し、再登録でexecutor上のtaskが積み上がらないようにする。
            let _ = running.task.await;
        }
    }
}

fn session_retry_key(entry: &Entry, hash: &BlobHash) -> String {
    super::hydration_limits::display_retry_key(
        "session",
        &format!("{}:{}", entry.topic, entry.replica.as_str()),
        &format!("{}:{}", entry.key, hash.as_str()),
    )
}
#[derive(Default)]
struct State {
    stopped: bool,
    entries: VecDeque<Entry>,
    pending_docs: VecDeque<(String, ReplicaId, String, Option<String>)>,
    retry_timer: Option<n0_future::task::JoinHandle<()>>,
    retry_due_ms: Option<i64>,
}
impl State {
    fn admit(&mut self, topic: &str, replica: &ReplicaId, key: &str) -> Option<&mut Entry> {
        if self.stopped {
            return None;
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|e| e.replica == *replica && e.key == key)
        {
            return self.entries.get_mut(index);
        }
        if self.entries.len() == SESSION_TARGET_LIMIT {
            let index = self
                .entries
                .iter()
                .position(|e| e.observers.is_empty() && e.running.is_none())?;
            self.entries.remove(index);
        }
        self.entries.push_back(Entry {
            topic: topic.to_owned(),
            replica: replica.clone(),
            key: key.to_owned(),
            hashes: Vec::new(),
            observers: HashSet::new(),
            requested: HashSet::new(),
            running: None,
        });
        self.entries.back_mut()
    }
}
/// 最後に同期で状態が変わった時刻(`SyncStatus::last_sync_ts`)。変えたら通信状態の差分の印を付ける(#1221 R2-D)。
#[derive(Default)]
pub(crate) struct SyncClock {
    at: Mutex<Option<i64>>,
    changes: std::sync::OnceLock<kukuri_transport::StatusChanges>,
}

impl SyncClock {
    pub(crate) async fn get(&self) -> Option<i64> {
        *self.at.lock().await
    }

    pub(crate) async fn set(&self, at: i64) {
        *self.at.lock().await = Some(at);
        self.mark(kukuri_transport::StatusKey::Summary);
    }

    /// 今の時刻にする。前の値以下にはしない。
    pub(crate) async fn advance(&self) {
        let mut at = self.at.lock().await;
        *at = Some(
            Utc::now()
                .timestamp_millis()
                .max(at.unwrap_or_default().saturating_add(1)),
        );
        drop(at);
        self.mark(kukuri_transport::StatusKey::Summary);
    }

    pub(crate) fn watch(&self, changes: kukuri_transport::StatusChanges) {
        let _ = self.changes.set(changes);
    }

    pub(crate) fn mark(&self, key: kukuri_transport::StatusKey) {
        if let Some(changes) = self.changes.get() {
            changes.mark(key);
        }
    }
}

pub(crate) struct SessionProjections {
    pub(crate) last_change: Arc<SyncClock>,
    state: Mutex<State>,
    permits: Arc<tokio::sync::Semaphore>,
    tokens: AtomicU64,
    retry_ledger: Arc<super::hydration_limits::MissingBodyLedger>,
}
impl Default for SessionProjections {
    fn default() -> Self {
        Self::with_retry_ledger(Arc::default())
    }
}
impl SessionProjections {
    pub(crate) fn with_retry_ledger(
        retry_ledger: Arc<super::hydration_limits::MissingBodyLedger>,
    ) -> Self {
        Self {
            last_change: Arc::default(),
            state: Mutex::default(),
            permits: Arc::new(tokio::sync::Semaphore::new(FETCH_CONCURRENCY)),
            tokens: AtomicU64::new(0),
            retry_ledger,
        }
    }
    pub(crate) async fn defer_entry(
        &self,
        topic: &str,
        replica: &ReplicaId,
        key: &str,
        expected_hash: Option<&str>,
    ) {
        let mut state = self.state.lock().await;
        if state.stopped {
            return;
        }
        if state
            .pending_docs
            .iter()
            .any(|(_, r, k, h)| r == replica && k == key && h.as_deref() == expected_hash)
        {
            return;
        }
        if state.pending_docs.len() == SESSION_TARGET_LIMIT {
            state.pending_docs.pop_front();
        }
        state.pending_docs.push_back((
            topic.into(),
            replica.clone(),
            key.into(),
            expected_hash.map(str::to_owned),
        ));
    }
    /// 1 keyの全recordを検証し終えた結果。同じ集合の再通知で予算を復活させない。
    pub(crate) async fn observe_key(
        &self,
        topic: &str,
        replica: &ReplicaId,
        key: &str,
        hashes: Vec<BlobHash>,
        worker: Option<u64>,
    ) {
        let mut state = self.state.lock().await;
        let Some(entry) = state.admit(topic, replica, key) else {
            return;
        };
        if entry
            .running
            .as_ref()
            .is_some_and(|r| !hashes.contains(&r.hash) && worker != Some(r.token))
        {
            entry.cancel().await;
        }
        entry
            .requested
            .retain(|h| hashes.iter().any(|hash| hash.as_str() == h));
        for hash in &hashes {
            if !entry.hashes.contains(hash) {
                entry.requested.insert(hash.as_str().to_owned());
            }
        }
        let changed = entry.hashes != hashes;
        entry.hashes = hashes;
        drop(state);
        if changed {
            self.last_change.advance().await;
        }
    }
    pub(crate) async fn candidates(
        &self,
        topic: &str,
        in_scope: impl Fn(&ReplicaId) -> bool,
    ) -> Vec<SessionCandidateView> {
        self.state
            .lock()
            .await
            .entries
            .iter()
            .filter(|e| e.topic == topic && in_scope(&e.replica) && !e.hashes.is_empty())
            .map(Entry::candidate)
            .collect()
    }
    pub(crate) async fn visibility(
        &self,
        topic: &str,
        replica: &ReplicaId,
        key: &str,
        observer: &str,
        visible: bool,
        retry: bool,
    ) -> Result<()> {
        anyhow::ensure!(
            observer.len() <= 128 && !observer.is_empty(),
            "invalid session observer"
        );
        let mut state = self.state.lock().await;
        if !visible {
            for entry in &mut state.entries {
                entry.observers.remove(observer);
                if entry.observers.is_empty() {
                    entry.cancel().await;
                }
            }
            return Ok(());
        }
        let entry = state
            .admit(topic, replica, key)
            .context("session display capacity reached")?;
        anyhow::ensure!(
            entry.observers.len() < SESSION_TARGET_LIMIT || entry.observers.contains(observer),
            "session observer capacity reached"
        );
        entry.observers.insert(observer.to_owned());
        if retry {
            for hash in &entry.hashes {
                self.retry_ledger
                    .request_manual_retry_key(&session_retry_key(entry, hash));
            }
        }
        entry
            .requested
            .extend(entry.hashes.iter().map(|h| h.as_str().to_owned()));
        Ok(())
    }

    /// 待機taskは1 keyに1つ(全体64)、実取得は2つ。失敗hashは表示中の期限通知でだけ再要求する。
    pub(crate) fn schedule(
        self: &Arc<Self>,
        services: &ServiceHandles,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let registry = self.clone();
        let services = services.clone();
        Box::pin(async move {
            let mut state = registry.state.lock().await;
            let now_ms = Utc::now().timestamp_millis();
            for entry in &mut state.entries {
                if entry.observers.is_empty() || entry.running.is_some() {
                    continue;
                }
                let Some(hash) = entry
                    .hashes
                    .iter()
                    .find(|h| {
                        entry.requested.contains(h.as_str())
                            && registry
                                .retry_ledger
                                .ready_key(&session_retry_key(entry, h), now_ms)
                    })
                    .cloned()
                else {
                    continue;
                };
                let Ok(permit) = registry.permits.clone().try_acquire_owned() else {
                    continue;
                };
                entry.requested.remove(hash.as_str());
                let token = registry.tokens.fetch_add(1, Ordering::Relaxed);
                let retry_key = session_retry_key(entry, &hash);
                let (topic, replica, key) = (
                    entry.topic.clone(),
                    entry.replica.clone(),
                    entry.key.clone(),
                );
                let registry = registry.clone();
                let services = services.clone();
                let task_hash = hash.clone();
                let deadline =
                    n0_future::time::Instant::now() + kukuri_blob_service::DISPLAY_FETCH_TIMEOUT;
                let task = n0_future::task::spawn(async move {
                    let _permit = permit;
                    let prepared = crate::timeout_at(
                        deadline,
                        services.blob_service.prepare_display_fetch(&task_hash),
                    )
                    .await;
                    let (fetched, mut attempt) = if let Ok(Ok(fetch)) = prepared {
                        let attempt = {
                            let _access = services.content_save_access.lock().await;
                            let mut state = registry.state.lock().await;
                            if state.entries.iter_mut().any(|e| {
                                e.replica == replica
                                    && e.key == key
                                    && e.running.as_ref().is_some_and(|r| r.token == token)
                                    && !e.observers.is_empty()
                                    && !*services.content_closed.borrow()
                                    && n0_future::time::Instant::now() < deadline
                            }) {
                                // 内側の共通walk枠も取得済み。待機取消には予算を使わない。
                                registry
                                    .retry_ledger
                                    .try_begin_key(&retry_key, Utc::now().timestamp_millis())
                            } else {
                                None
                            }
                        };
                        let fetched = if attempt.is_some() {
                            crate::timeout_at(deadline, fetch)
                                .await
                                .ok()
                                .and_then(Result::ok)
                                .flatten()
                        } else {
                            None
                        };
                        (fetched, attempt)
                    } else {
                        registry.retry_ledger.defer_key(
                            &retry_key,
                            Utc::now().timestamp_millis().saturating_add(5_000),
                        );
                        (None, None)
                    };
                    let access = services.content_save_access.lock().await;
                    let still_displayed = !*services.content_closed.borrow()
                        && registry.state.lock().await.entries.iter().any(|entry| {
                            entry.replica == replica
                                && entry.key == key
                                && entry.running.as_ref().is_some_and(|r| r.token == token)
                                && entry.hashes.contains(&task_hash)
                                && !entry.observers.is_empty()
                        });
                    if let Some(bytes) = fetched.filter(|_| still_displayed) {
                        match cache_and_project_displayed_manifest(
                            &services, &topic, &replica, &key, &task_hash, bytes, token,
                        )
                        .await
                        {
                            Ok(projected) if projected > 0 => {
                                if let Some(attempt) = attempt.take() {
                                    attempt.succeed();
                                }
                            }
                            Err(error) => warn!(%error, "failed to project a displayed session"),
                            _ => {}
                        }
                    } else if !still_displayed && let Some(attempt) = attempt.take() {
                        attempt.defer();
                    }
                    drop(attempt);
                    {
                        let mut state = registry.state.lock().await;
                        if let Some(entry) = state.entries.iter_mut().find(|e| {
                            e.replica == replica
                                && e.key == key
                                && e.running.as_ref().is_some_and(|r| r.token == token)
                        }) {
                            entry.running = None;
                            if !entry.observers.is_empty()
                                && entry.hashes.contains(&task_hash)
                                && registry
                                    .retry_ledger
                                    .next_attempt_at_key(&retry_key)
                                    .is_some()
                            {
                                entry.requested.insert(task_hash.as_str().to_owned());
                            }
                        }
                    }
                    drop(access);
                    drop(_permit);
                    registry.schedule(&services).await;
                });
                entry.running = Some(Running { token, hash, task });
            }
            let next_due = state
                .entries
                .iter()
                .filter(|entry| !entry.observers.is_empty() && entry.running.is_none())
                .flat_map(|entry| {
                    entry.hashes.iter().filter_map(|hash| {
                        entry
                            .requested
                            .contains(hash.as_str())
                            .then(|| {
                                registry
                                    .retry_ledger
                                    .next_attempt_at_key(&session_retry_key(entry, hash))
                            })
                            .flatten()
                    })
                })
                .min();
            if state.retry_due_ms != next_due {
                if let Some(timer) = state.retry_timer.take() {
                    timer.abort();
                }
                state.retry_due_ms = None;
                if let Some(due) = next_due
                    && (due > now_ms || registry.permits.available_permits() > 0)
                {
                    let registry_for_timer = registry.clone();
                    let services_for_timer = services.clone();
                    state.retry_timer = Some(n0_future::task::spawn(async move {
                        let delay = due.saturating_sub(Utc::now().timestamp_millis()).max(0) as u64;
                        n0_future::time::sleep(std::time::Duration::from_millis(delay)).await;
                        {
                            let mut state = registry_for_timer.state.lock().await;
                            state.retry_timer = None;
                            state.retry_due_ms = None;
                        }
                        registry_for_timer.schedule(&services_for_timer).await;
                    }));
                    state.retry_due_ms = Some(due);
                }
            }
        })
    }
    pub(crate) async fn remove_replicas(&self, replicas: &[ReplicaId]) {
        let mut state = self.state.lock().await;
        state
            .pending_docs
            .retain(|(_, r, _, _)| !replicas.contains(r));
        for entry in &mut state.entries {
            if replicas.contains(&entry.replica) {
                entry.cancel().await;
            }
        }
        state
            .entries
            .retain(|entry| !replicas.contains(&entry.replica));
        if state.entries.iter().all(|entry| entry.observers.is_empty()) {
            if let Some(timer) = state.retry_timer.take() {
                timer.abort();
            }
            state.retry_due_ms = None;
        }
    }
    pub(crate) async fn clear(&self) {
        let mut state = self.state.lock().await;
        state.stopped = true;
        if let Some(timer) = state.retry_timer.take() {
            timer.abort();
        }
        state.retry_due_ms = None;
        for entry in &mut state.entries {
            entry.cancel().await;
        }
        state.entries.clear();
        state.pending_docs.clear();
    }
    #[cfg(test)]
    pub(crate) async fn wait_idle(&self) {
        n0_future::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if self
                    .state
                    .lock()
                    .await
                    .entries
                    .iter()
                    .all(|e| e.running.is_none())
                {
                    break;
                }
                n0_future::future::yield_now().await;
            }
        })
        .await
        .expect("session acquisition tasks finish");
    }
}

/// 表示/退出との排他を持つ呼出元からだけ保存する。取得futureは保存も別taskも持たない。
async fn cache_and_project_displayed_manifest(
    services: &ServiceHandles,
    topic: &str,
    replica: &ReplicaId,
    key: &str,
    hash: &BlobHash,
    bytes: Vec<u8>,
    token: u64,
) -> Result<usize> {
    anyhow::ensure!(
        blake3::hash(&bytes).to_hex().as_str() == hash.as_str(),
        "manifest hash mismatch"
    );
    services
        .blob_service
        .put_remote_blob(
            bytes,
            if key.starts_with("sessions/live/") {
                LIVE_MANIFEST_MIME
            } else {
                GAME_MANIFEST_MIME
            },
        )
        .await?;
    super::hydration_support::hydrate_session_key_for_fetch(
        services,
        topic,
        replica,
        key,
        Some(token),
        None,
    )
    .await
}
