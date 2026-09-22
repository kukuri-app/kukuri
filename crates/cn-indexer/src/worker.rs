//! 常駐取り込みワーカー（#613 T2）。
//!
//! 起動時に scope 管理 state から復元し、以後は「レプリカの変更通知（少し待ってまとめる）+
//! 一定間隔の全件見直し」で supported scope を取り込み続ける。全件見直しは何度実行しても同じ
//! 結果に収束する（冪等）ため、サポート対象・チャンネル秘密鍵の更新や通知の取りこぼしを
//! ここで回収する。
//!
//! - 索引解除の判定は「索引の真実源に実在する scope − いま対象であるべき scope」の差分で行う
//!   （`IndexerParticipant::indexed_scopes` / `desired_scopes`）。open の成否に依存しないため、
//!   一時的な失敗で誤って索引解除しない。ワーカー停止中に外れた scope も再起動後の見直しで
//!   索引解除される。
//! - scope 単位の連続失敗は再試行間隔を指数的に広げる（上限つき）。1 つの scope / entry の
//!   失敗でワーカー全体を止めない。
//! - 停止は [`WorkerHandle::shutdown`]。ループと購読タスクを止め、終了を待つ。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use kukuri_docs_sync::DocsSync;

use crate::participant::{IndexerParticipant, ScopeReplica};
use crate::state::IndexerRuntimeState;

/// ワーカーの動作設定。テストから各間隔を注入して短縮できる。
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// 全件見直しの間隔。
    pub poll_interval: Duration,
    /// 変更通知を受けてから取り込むまでの待ち（連続する通知をまとめる）。
    pub event_debounce: Duration,
    /// scope 単位の再試行間隔の初期値。
    pub backoff_base: Duration,
    /// scope 単位の再試行間隔の上限。
    pub backoff_max: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(300),
            event_debounce: Duration::from_secs(2),
            backoff_base: Duration::from_secs(5),
            backoff_max: Duration::from_secs(300),
        }
    }
}

/// 購読タスクから run loop へ流す変更通知（replica id, 変更された key）。
type ReplicaEvent = (String, String);

/// 取り込み対象（全件 or 変更 key に対応する object だけ）。
#[derive(Clone, Copy, Debug)]
enum IngestTarget<'a> {
    Scope,
    Keys(&'a [String]),
}

/// scope 単位の再試行状態。
#[derive(Debug)]
struct BackoffEntry {
    failures: u32,
    ready_at: tokio::time::Instant,
}

/// 常駐取り込みワーカー。
pub struct IndexerWorker {
    participant: Arc<IndexerParticipant>,
    docs_sync: Arc<dyn DocsSync>,
    state: Arc<IndexerRuntimeState>,
    config: WorkerConfig,
}

/// ワーカーの停止用の持ち手。
pub struct WorkerHandle {
    stop_tx: watch::Sender<bool>,
    join: JoinHandle<()>,
    state: Arc<IndexerRuntimeState>,
}

impl WorkerHandle {
    /// 観測状態への参照を返す。
    pub fn state(&self) -> Arc<IndexerRuntimeState> {
        Arc::clone(&self.state)
    }

    /// ワーカーを止め、終了を待つ。購読タスクも含めて止まる。
    pub async fn shutdown(self) {
        let _ = self.stop_tx.send(true);
        if let Err(error) = tokio::time::timeout(Duration::from_secs(10), self.join).await {
            warn!(%error, "indexer worker did not stop within 10s");
        }
    }
}

impl IndexerWorker {
    pub fn new(
        participant: Arc<IndexerParticipant>,
        docs_sync: Arc<dyn DocsSync>,
        state: Arc<IndexerRuntimeState>,
        config: WorkerConfig,
    ) -> Self {
        Self {
            participant,
            docs_sync,
            state,
            config,
        }
    }

    /// ワーカーを起動する。返った持ち手の `shutdown` で止める。
    pub fn spawn(self) -> WorkerHandle {
        let (stop_tx, stop_rx) = watch::channel(false);
        let state = Arc::clone(&self.state);
        let join = tokio::spawn(self.run(stop_rx));
        WorkerHandle {
            stop_tx,
            join,
            state,
        }
    }

    async fn run(self, mut stop_rx: watch::Receiver<bool>) {
        self.state.set_worker_running(true);
        info!("indexer worker started");

        // 変更通知の集約チャネル（値は replica id と変更された key）。購読タスクがここへ流す。
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<ReplicaEvent>();
        // replica id → 購読タスク。
        let mut subscriptions: HashMap<String, JoinHandle<()>> = HashMap::new();
        // replica id → 取り込み対象 scope（直近の全件見直し時点）。
        let mut active: HashMap<String, ScopeReplica> = HashMap::new();
        // replica id → 再試行状態。
        let mut backoff: HashMap<String, BackoffEntry> = HashMap::new();

        'main: loop {
            let pass_started = tokio::time::Instant::now();
            self.full_pass(&mut active, &mut subscriptions, &event_tx, &mut backoff)
                .await;
            self.state
                .record_pass_duration(pass_started.elapsed().as_millis() as u64);

            // 次の全件見直しまで、変更通知を処理しながら待つ。
            let next_pass = tokio::time::Instant::now() + self.config.poll_interval;
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => {
                        if *stop_rx.borrow() {
                            break 'main;
                        }
                    }
                    _ = tokio::time::sleep_until(next_pass) => break,
                    received = event_rx.recv() => {
                        let Some((replica_id, key)) = received else { break };
                        // まとめ待ち: 待機中に届いた通知を replica ごとに 1 回の取り込みにまとめる。
                        // 取り込み対象は変更された key に対応する object だけ（#1050 AC-4）。
                        let mut pending: HashMap<String, HashSet<String>> = HashMap::new();
                        pending.entry(replica_id).or_default().insert(key);
                        let debounce_end =
                            tokio::time::sleep(self.config.event_debounce);
                        tokio::pin!(debounce_end);
                        loop {
                            tokio::select! {
                                _ = stop_rx.changed() => {
                                    if *stop_rx.borrow() {
                                        break 'main;
                                    }
                                }
                                _ = &mut debounce_end => break,
                                more = event_rx.recv() => {
                                    match more {
                                        Some((id, key)) => {
                                            pending.entry(id).or_default().insert(key);
                                        }
                                        None => break,
                                    }
                                }
                            }
                        }
                        // heartbeat registration can change between full passes. Refresh once for
                        // the whole debounce batch so docs and blob fetches can reach the authoring
                        // peer before any changed object is ingested.
                        if let Err(error) = self.participant.refresh_seed_peers().await {
                            warn!(
                                error = %format!("{error:#}"),
                                "failed to refresh seed peers before event ingest; will retry"
                            );
                            self.state.record_error(None, &format!("{error:#}"));
                            continue;
                        }
                        for (replica_id, keys) in pending {
                            if let Some(scope) = active.get(&replica_id).cloned() {
                                let mut keys: Vec<String> = keys.into_iter().collect();
                                keys.sort();
                                let started = tokio::time::Instant::now();
                                self.ingest_with_backoff(
                                    &scope,
                                    IngestTarget::Keys(&keys),
                                    &mut backoff,
                                )
                                .await;
                                self.state.record_event_ingest_duration(
                                    started.elapsed().as_millis() as u64,
                                );
                            }
                        }
                    }
                }
            }
        }

        // 停止処理: 購読タスクを止める。
        for (_, handle) in subscriptions.drain() {
            handle.abort();
        }
        self.state.set_worker_running(false);
        info!("indexer worker stopped");
    }

    /// 全件見直し 1 巡（冪等）:
    /// 1. いま対象であるべき scope を求める。
    /// 2. 索引に実在するが対象でなくなった scope を索引解除する（秘密鍵失効を含む）。
    /// 3. 秘密鍵の登録とレプリカ open（`restore_scopes`）、購読の起動。
    /// 4. 各 scope を取り込む（再試行間隔中の scope は飛ばす）。
    async fn full_pass(
        &self,
        active: &mut HashMap<String, ScopeReplica>,
        subscriptions: &mut HashMap<String, JoinHandle<()>>,
        event_tx: &mpsc::UnboundedSender<ReplicaEvent>,
        backoff: &mut HashMap<String, BackoffEntry>,
    ) {
        // 1. 対象であるべき scope。
        // 日付境界をまたいでも、このpassの選択とopenは同じ時刻の窓を使う。
        let now = chrono::Utc::now().timestamp();
        let desired = match self.participant.desired_scopes_at(now).await {
            Ok(desired) => desired,
            Err(error) => {
                warn!(error = %format!("{error:#}"), "failed to list desired scopes; will retry");
                self.state.record_error(None, &format!("{error:#}"));
                return;
            }
        };
        let desired_keys: HashSet<&str> = desired
            .iter()
            .map(|scope| scope.replica_id.as_str())
            .collect();
        let desired_logical: HashSet<_> = desired
            .iter()
            .map(|scope| (scope.kind, scope.id.as_str()))
            .collect();

        // 2. 対象でなくなった scope の索引解除（索引の実在 scope との差分。再起動をまたいでも効く）。
        match self.participant.indexed_scopes().await {
            Ok(indexed) => {
                for (kind, id) in indexed {
                    let scope = ScopeReplica::from_scope(kind, id.as_str());
                    if desired_logical.contains(&(kind, id.as_str())) {
                        continue;
                    }
                    match self.participant.stop_and_deindex_scope(kind, &id).await {
                        Ok(()) => {
                            self.state.record_deindexed(1);
                            if let Some(handle) = subscriptions.remove(scope.replica_id.as_str()) {
                                handle.abort();
                            }
                            active.remove(scope.replica_id.as_str());
                            backoff.remove(scope.replica_id.as_str());
                        }
                        Err(error) => {
                            warn!(
                                kind = kind.as_str(),
                                scope_id = %id,
                                error = %format!("{error:#}"),
                                "failed to de-index removed scope; will retry"
                            );
                            self.state.record_error(
                                Some(scope.replica_id.as_str()),
                                &format!("{error:#}"),
                            );
                        }
                    }
                }
            }
            Err(error) => {
                warn!(error = %format!("{error:#}"), "failed to list indexed scopes; will retry");
                self.state.record_error(None, &format!("{error:#}"));
            }
        }

        // 購読が残っている「対象外」scope（索引が空で差分に出ないもの）も止める。
        let stale: Vec<String> = active
            .keys()
            .filter(|key| !desired_keys.contains(key.as_str()))
            .cloned()
            .collect();
        for key in stale {
            if let Some(handle) = subscriptions.remove(&key) {
                handle.abort();
            }
            if let Some(scope) = active.get(&key) {
                match self.participant.stop_replica(scope).await {
                    Ok(()) => {
                        active.remove(&key);
                        backoff.remove(&key);
                    }
                    Err(error) => {
                        // activeに残して次のpassで停止を再試行する。bucket終了では索引を消さない。
                        warn!(replica_id = %key, error = %error, "failed to stop stale replica; will retry");
                        self.state.record_error(Some(&key), &format!("{error:#}"));
                    }
                }
            }
        }

        // 3. 秘密鍵の登録とレプリカ open。open できたものだけを active / 購読対象にする。
        let opened = match self.participant.restore_scopes_at(now).await {
            Ok(opened) => opened,
            Err(error) => {
                warn!(error = %format!("{error:#}"), "failed to restore scopes; will retry");
                self.state.record_error(None, &format!("{error:#}"));
                return;
            }
        };
        for scope in &opened {
            let key = scope.replica_id.as_str().to_string();
            active.insert(key.clone(), scope.clone());
            let needs_spawn = match subscriptions.get(&key) {
                Some(handle) => handle.is_finished(),
                None => true,
            };
            if needs_spawn {
                subscriptions.insert(key, self.spawn_subscription(scope, event_tx.clone()));
            }
        }
        self.state.set_opened_scopes(opened.len() as u64);

        // 4. 各 scope の取り込み。
        let mut all_scopes_ingested = !opened.is_empty();
        for scope in &opened {
            all_scopes_ingested &= self
                .ingest_with_backoff(scope, IngestTarget::Scope, backoff)
                .await;
        }
        if all_scopes_ingested {
            self.state
                .record_sync_success(chrono::Utc::now().timestamp());
        }
    }

    /// scope を取り込む（全件、または変更された key の object だけ）。再試行間隔中なら何もしない。
    ///
    /// 変更通知駆動の取り込みも同じ backoff に従う。間隔中に届いた通知の取りこぼしは、次の
    /// 全件見直し（冪等）が回収する。
    async fn ingest_with_backoff(
        &self,
        scope: &ScopeReplica,
        target: IngestTarget<'_>,
        backoff: &mut HashMap<String, BackoffEntry>,
    ) -> bool {
        let key = scope.replica_id.as_str();
        if let Some(entry) = backoff.get(key)
            && tokio::time::Instant::now() < entry.ready_at
        {
            debug!(replica_id = %key, "scope is backing off; skipping this round");
            return false;
        }
        let result = match target {
            IngestTarget::Scope => self.participant.ingest_scope(scope).await,
            IngestTarget::Keys(keys) => self.participant.ingest_changed_keys(scope, keys).await,
        };
        match result {
            Ok(summary) => {
                backoff.remove(key);
                self.state
                    .record_ingest_success(chrono::Utc::now().timestamp(), &summary);
                debug!(
                    replica_id = %key,
                    scanned = summary.scanned,
                    indexed = summary.indexed,
                    scans_fresh = summary.scans_fresh,
                    scans_reused = summary.scans_reused,
                    "scope ingested"
                );
                true
            }
            Err(error) => {
                let failures = backoff.get(key).map(|entry| entry.failures).unwrap_or(0) + 1;
                // 失敗回数に応じて再試行間隔を指数的に広げる（上限つき）。
                let exponent = failures.saturating_sub(1).min(16);
                let delay = self
                    .config
                    .backoff_base
                    .saturating_mul(2u32.saturating_pow(exponent))
                    .min(self.config.backoff_max);
                backoff.insert(
                    key.to_string(),
                    BackoffEntry {
                        failures,
                        ready_at: tokio::time::Instant::now() + delay,
                    },
                );
                warn!(
                    replica_id = %key,
                    failures,
                    retry_in_secs = delay.as_secs(),
                    error = %format!("{error:#}"),
                    "failed to ingest scope; backing off"
                );
                self.state.record_error(Some(key), &format!("{error:#}"));
                false
            }
        }
    }

    /// scope の変更通知を購読し、届いた通知（replica id + 変更 key）を集約チャネルへ流す
    /// タスクを起動する。
    fn spawn_subscription(
        &self,
        scope: &ScopeReplica,
        event_tx: mpsc::UnboundedSender<ReplicaEvent>,
    ) -> JoinHandle<()> {
        let docs_sync = Arc::clone(&self.docs_sync);
        let replica_id = scope.replica_id.clone();
        tokio::spawn(async move {
            match docs_sync.subscribe_replica(&replica_id).await {
                Ok(mut stream) => {
                    while let Some(event) = stream.next().await {
                        let Ok(event) = event else { continue };
                        if event_tx
                            .send((replica_id.as_str().to_string(), event.key))
                            .is_err()
                        {
                            break;
                        }
                    }
                    debug!(replica_id = %replica_id.as_str(), "replica event stream ended");
                }
                Err(error) => {
                    // 購読に失敗しても定期の全件見直しが取り込みを続ける（次の見直しで再購読）。
                    warn!(
                        replica_id = %replica_id.as_str(),
                        error = %format!("{error:#}"),
                        "failed to subscribe to replica events; periodic passes still cover it"
                    );
                }
            }
        })
    }
}
