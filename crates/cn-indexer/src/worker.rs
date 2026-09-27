//! 常駐取り込みワーカー（#613 T2、#1221 R5-H）。
//!
//! bucket reader だけで索引する。最近の検索・表示の需要がある scope は `demand_interval`（既定 30 秒）ごとに、
//! それ以外は `poll_interval` ごとの巡回で読む。どちらも同じ 64 物理 scope・同時 8 読取りの内側で動き、
//! 変更通知の queue や namespace の同期を持たない。`poll_interval` ごとの巡回では、解除した scope と
//! 受入下限未満の保存物も上限つきで回収する。
//!
//! 停止は [`WorkerHandle::shutdown`]。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::bucket_reader::BucketReader;
use crate::maintenance::IndexMaintenance;
use crate::state::IndexerRuntimeState;

/// ワーカーの動作設定。テストから各間隔を注入して短縮できる。
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// 全 scope の巡回と回収の間隔。
    pub poll_interval: Duration,
    /// 需要のある scope を読む間隔。
    pub demand_interval: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(300),
            demand_interval: Duration::from_secs(30),
        }
    }
}

/// 常駐取り込みワーカー。
pub struct IndexerWorker {
    reader: Arc<BucketReader>,
    maintenance: IndexMaintenance,
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

    /// ワーカーを止め、終了を待つ。
    pub async fn shutdown(mut self) {
        let _ = self.stop_tx.send(true);
        if let Err(error) = tokio::time::timeout(Duration::from_secs(10), &mut self.join).await {
            warn!(%error, "indexer worker did not stop within 10s");
        }
    }
}

impl IndexerWorker {
    pub fn new(
        reader: Arc<BucketReader>,
        maintenance: IndexMaintenance,
        state: Arc<IndexerRuntimeState>,
        config: WorkerConfig,
    ) -> Self {
        Self {
            reader,
            maintenance,
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
        let mut next_full = tokio::time::Instant::now();
        loop {
            let started = tokio::time::Instant::now();
            let full = started >= next_full;
            let now = chrono::Utc::now().timestamp();
            if full {
                next_full = started + self.config.poll_interval;
                self.maintenance.run_pass(now, &self.state).await;
            }
            match self.reader.poll_once(now, full).await {
                Ok((summary, physical_scopes)) => {
                    let at = chrono::Utc::now().timestamp();
                    self.state.record_ingest_success(at, &summary);
                    if full {
                        self.state.set_opened_scopes(physical_scopes as u64);
                        self.state.record_sync_success(at);
                    }
                }
                Err(error) => {
                    warn!(error = %format!("{error:#}"), "bucket reader failed");
                    self.state.record_error(None, &format!("{error:#}"));
                }
            }
            if full {
                self.state
                    .record_pass_duration(started.elapsed().as_millis() as u64);
            }
            tokio::select! {
                _ = tokio::time::sleep(self.config.demand_interval.min(self.config.poll_interval)) => {}
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                }
            }
        }
        self.state.set_worker_running(false);
        info!("indexer worker stopped");
    }
}
