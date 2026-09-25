use std::sync::{Arc, Weak};
use std::time::Duration;

use super::maintenance_tasks::{MaintenanceJob, MaintenanceTasks};
use super::*;

impl DesktopRuntime {
    /// 設定変更とtest向けの1回実行。常駐schedulerと同じ独立laneを使用する。
    pub(crate) async fn run_community_node_session_maintenance_once(&self) {
        let mut tasks = MaintenanceTasks::default();
        for job in self.community_node_maintenance_jobs().await {
            tasks.insert(job.clone(), self.run_community_node_maintenance_job(job));
        }
        while tasks.next().await.is_some() {}
    }

    async fn community_node_maintenance_jobs(&self) -> Vec<MaintenanceJob> {
        let config = self.community_node_config.lock().await;
        if config.nodes.is_empty() {
            return Vec::new();
        }
        let mut jobs = config
            .nodes
            .iter()
            .map(|node| MaintenanceJob::Session(node.base_url.clone()))
            .collect::<Vec<_>>();
        jobs.extend([MaintenanceJob::Observations, MaintenanceJob::Connectivity]);
        jobs
    }

    async fn run_community_node_maintenance_job(&self, job: MaintenanceJob) {
        match job {
            MaintenanceJob::Session(base_url) => {
                if let Err(error) = self
                    .refresh_community_node_registration_if_due(&base_url)
                    .await
                {
                    warn!(base_url, %error, "failed to refresh community-node registration from session scheduler");
                }
                if let Err(error) = self.refresh_private_index_grant_once(&base_url).await {
                    warn!(base_url, %error, "failed to refresh private channel indexing grant");
                }
            }
            MaintenanceJob::Observations => {
                self.flush_community_node_trust_observations_once().await
            }
            MaintenanceJob::Connectivity => match self.app_service.get_sync_status().await {
                Ok(status) => {
                    self.maybe_self_heal_community_node_connectivity(&status)
                        .await;
                }
                Err(error) => {
                    warn!(%error, "failed to load sync status for community-node self-heal from session scheduler");
                }
            },
        }
    }

    /// セッション維持スケジューラを既定 tick(15 秒)で起動する。
    /// トレイ常駐中(フロントのポーリング停止時)も CN 側の bootstrap 登録(TTL 90 秒)と
    /// トークン失効前再認証を維持するための常駐タスク。起動は opt-in(コンストラクタでは
    /// 起動しない)で、production では src-tauri の状態構築時に 1 回だけ呼ぶ。
    /// 停止は `DesktopRuntime::shutdown` が行う。
    pub async fn start_community_node_session_scheduler(self: &Arc<Self>) {
        self.start_community_node_session_scheduler_with_interval(Duration::from_secs(
            COMMUNITY_NODE_SESSION_SCHEDULER_TICK_SECONDS,
        ))
        .await;
    }

    /// tick 間隔を注入できる起動口(テスト用)。既に起動済みなら no-op。
    pub(crate) async fn start_community_node_session_scheduler_with_interval(
        self: &Arc<Self>,
        tick: Duration,
    ) {
        let mut task = self.community_node_scheduler_task.lock().await;
        if task.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }
        // Weak 参照で runtime の所有権を持たない: 最後の Arc が drop されたら tick 側から
        // 自然終了する(shutdown 忘れでもリークしない)。upgrade した Arc は tick 実行中のみ保持。
        let weak: Weak<Self> = Arc::downgrade(self);
        *task = Some(tokio::spawn(async move {
            let mut timer = tokio::time::interval(tick);
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut tasks = MaintenanceTasks::default();
            info!(target: "kukuri_connectivity", "community-node maintenance scheduler started");
            loop {
                tokio::select! {
                    _ = timer.tick() => {
                        let Some(runtime) = weak.upgrade() else { return; };
                        for job in runtime.community_node_maintenance_jobs().await {
                            let runtime = runtime.clone();
                            tasks.insert(job.clone(), async move {
                                runtime.run_community_node_maintenance_job(job).await;
                            });
                        }
                    }
                    _ = tasks.next(), if !tasks.is_empty() => {}
                }
            }
        }));
    }
}
