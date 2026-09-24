//! #1262: session候補と表示要求の有界な作業集合。timerは持たない。
use super::*;
use kukuri_core::BlobHash;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const SESSION_TARGET_LIMIT: usize = 64;
const FETCH_CONCURRENCY: usize = 2;
const FETCH_ATTEMPTS: usize = 3;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SessionCandidateView {
    pub replica_id: String,
    pub session_id: String,
    pub kind: String,
}

struct Budget {
    hash: BlobHash,
    attempts: usize,
}
struct Running {
    token: u64,
    hash: BlobHash,
    task: tokio::task::JoinHandle<()>,
}
struct Entry {
    topic: String,
    replica: ReplicaId,
    key: String,
    hashes: Vec<BlobHash>,
    observers: HashSet<String>,
    budgets: VecDeque<Budget>,
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
    fn budget(&self, hash: &BlobHash) -> usize {
        self.budgets
            .iter()
            .find(|b| b.hash == *hash)
            .map_or(0, |b| b.attempts)
    }
    fn spend(&mut self, hash: &BlobHash) {
        if let Some(budget) = self.budgets.iter_mut().find(|b| b.hash == *hash) {
            budget.attempts += 1;
        } else {
            // 現在候補の予算は捨てない。履歴も64以内、record候補はexact read上限以内。
            if self.budgets.len() == SESSION_TARGET_LIMIT
                && let Some(index) = self
                    .budgets
                    .iter()
                    .position(|b| !self.hashes.contains(&b.hash))
            {
                self.budgets.remove(index);
            }
            self.budgets.push_back(Budget {
                hash: hash.clone(),
                attempts: 1,
            });
        }
    }
}
#[derive(Default)]
struct State {
    stopped: bool,
    entries: VecDeque<Entry>,
    pending_docs: VecDeque<(String, ReplicaId, String, Option<String>)>,
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
            budgets: VecDeque::new(),
            requested: HashSet::new(),
            running: None,
        });
        self.entries.back_mut()
    }
}
pub(crate) struct SessionProjections {
    pub(crate) last_change: Arc<Mutex<Option<i64>>>,
    state: Mutex<State>,
    permits: Arc<tokio::sync::Semaphore>,
    tokens: AtomicU64,
}
impl Default for SessionProjections {
    fn default() -> Self {
        Self {
            last_change: Arc::default(),
            state: Mutex::default(),
            permits: Arc::new(tokio::sync::Semaphore::new(FETCH_CONCURRENCY)),
            tokens: AtomicU64::new(0),
        }
    }
}
impl SessionProjections {
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
    pub(crate) async fn take_ready_entries(
        &self,
        replica: &ReplicaId,
    ) -> Vec<(String, String, Option<String>)> {
        let mut state = self.state.lock().await;
        let mut ready = Vec::new();
        state.pending_docs.retain(|(topic, r, key, hash)| {
            if r == replica {
                ready.push((topic.clone(), key.clone(), hash.clone()));
                false
            } else {
                true
            }
        });
        ready
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
            if !entry.hashes.contains(hash) && entry.budget(hash) < FETCH_ATTEMPTS {
                entry.requested.insert(hash.as_str().to_owned());
            }
        }
        let changed = entry.hashes != hashes;
        entry.hashes = hashes;
        drop(state);
        if changed {
            let mut last = self.last_change.lock().await;
            *last = Some(
                Utc::now()
                    .timestamp_millis()
                    .max(last.unwrap_or_default().saturating_add(1)),
            );
        }
    }
    pub(crate) async fn candidates(
        &self,
        topic: &str,
        replicas: &[ReplicaId],
    ) -> Vec<SessionCandidateView> {
        self.state
            .lock()
            .await
            .entries
            .iter()
            .filter(|e| e.topic == topic && replicas.contains(&e.replica) && !e.hashes.is_empty())
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
        if entry.observers.insert(observer.to_owned()) || retry {
            entry
                .requested
                .extend(entry.hashes.iter().map(|h| h.as_str().to_owned()));
        }
        Ok(())
    }

    /// 待機taskは1 keyに1つ(全体64)、実取得は2つ。完了で次候補へ進むが失敗hashは再要求しない。
    pub(crate) fn schedule(
        self: &Arc<Self>,
        services: &ServiceHandles,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let registry = self.clone();
        let services = services.clone();
        Box::pin(async move {
            let mut state = registry.state.lock().await;
            for entry in &mut state.entries {
                if entry.observers.is_empty() || entry.running.is_some() {
                    continue;
                }
                let Some(hash) = entry
                    .hashes
                    .iter()
                    .find(|h| {
                        entry.requested.contains(h.as_str()) && entry.budget(h) < FETCH_ATTEMPTS
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
                let (topic, replica, key) = (
                    entry.topic.clone(),
                    entry.replica.clone(),
                    entry.key.clone(),
                );
                let registry = registry.clone();
                let services = services.clone();
                let task_hash = hash.clone();
                let deadline =
                    tokio::time::Instant::now() + kukuri_blob_service::DISPLAY_FETCH_TIMEOUT;
                let task = tokio::spawn(async move {
                    let _permit = permit;
                    let prepared = tokio::time::timeout_at(
                        deadline,
                        services.blob_service.prepare_display_fetch(&task_hash),
                    )
                    .await;
                    let fetched = if let Ok(Ok(fetch)) = prepared {
                        let allowed = {
                            let _access = services.session_display_access.lock().await;
                            let mut state = registry.state.lock().await;
                            if let Some(entry) = state.entries.iter_mut().find(|e| {
                                e.replica == replica
                                    && e.key == key
                                    && e.running.as_ref().is_some_and(|r| r.token == token)
                                    && !e.observers.is_empty()
                                    && tokio::time::Instant::now() < deadline
                            }) {
                                // 内側の共通walk枠も取得済み。待機取消には予算を使わない。
                                entry.spend(&task_hash);
                                true
                            } else {
                                false
                            }
                        };
                        if allowed {
                            tokio::time::timeout_at(deadline, fetch)
                                .await
                                .ok()
                                .and_then(Result::ok)
                                .flatten()
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let access = services.session_display_access.lock().await;
                    if let Some(bytes) = fetched
                        && let Err(error) = cache_and_project_displayed_manifest(
                            &services, &topic, &replica, &key, &task_hash, bytes, token,
                        )
                        .await
                    {
                        warn!(%error, "failed to project a displayed session");
                    }
                    {
                        let mut state = registry.state.lock().await;
                        if let Some(entry) = state.entries.iter_mut().find(|e| {
                            e.replica == replica
                                && e.key == key
                                && e.running.as_ref().is_some_and(|r| r.token == token)
                        }) {
                            entry.running = None;
                        }
                    }
                    drop(access);
                    drop(_permit);
                    registry.schedule(&services).await;
                });
                entry.running = Some(Running { token, hash, task });
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
    }
    pub(crate) async fn clear(&self) {
        let mut state = self.state.lock().await;
        state.stopped = true;
        for entry in &mut state.entries {
            entry.cancel().await;
        }
        state.entries.clear();
        state.pending_docs.clear();
    }
    #[cfg(test)]
    pub(crate) async fn wait_idle(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
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
                tokio::task::yield_now().await;
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
        .put_blob(
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
