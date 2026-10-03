//! One account-owned retry loop for protected DM outbox rows. Peer receive
//! subscriptions have no retry timers and cannot block on offer publication.

use super::*;
use std::sync::atomic::Ordering;

impl AppService {
    pub(crate) async fn start_direct_message_outbox_retry(&self) -> Result<()> {
        let closed = &self.subscription_registry.dm_outbox_retry_closed;
        anyhow::ensure!(
            !closed.load(Ordering::Acquire),
            "DM outbox retry owner is closed"
        );
        let mut owner = self.subscription_registry.dm_outbox_retry_task.lock().await;
        if owner.as_ref().is_some_and(|task| !task.is_finished()) {
            return Ok(());
        }
        if let Some(old) = owner.take() {
            old.abort();
            old.wait().await;
        }
        anyhow::ensure!(
            !closed.load(Ordering::Acquire),
            "DM outbox retry owner is closed"
        );
        let services = self.services.clone();
        let closed = Arc::clone(closed);
        let wake = Arc::clone(&self.subscription_registry.dm_outbox_retry_wake);
        #[cfg(test)]
        self.subscription_registry
            .dm_outbox_retry_starts
            .fetch_add(1, Ordering::SeqCst);
        *owner = Some(AbortOnDropTask::new(n0_future::task::spawn(async move {
            let mut interval = n0_future::time::interval(std::time::Duration::from_millis(
                DIRECT_MESSAGE_RETRY_INTERVAL_MS,
            ));
            interval.set_missed_tick_behavior(n0_future::time::MissedTickBehavior::Skip);
            // #1219 AC-2: private channel の鍵更新の配布の続きも、tick ごとに 1 操作 1 page ずつ進める。
            let mut rotation_cursor = Default::default();
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = wake.notified() => {}
                }
                if closed.load(Ordering::Acquire) {
                    return;
                }
                if let Err(error) = AppService::flush_due_direct_message_outbox(
                    &services,
                    Utc::now().timestamp_millis(),
                )
                .await
                {
                    warn!(%error, "account DM outbox retry deferred");
                }
                if let Err(error) =
                    AppService::step_private_channel_rotations(&services, &mut rotation_cursor)
                        .await
                {
                    warn!(%error, "private channel rotation step deferred");
                }
            }
        })));
        Ok(())
    }

    /// 復帰の契機（ADR 0059 §5）。DM・epoch 制御の送信待ちの再送を、再送の owner に次の間隔を待たずに 1 回行わせ、
    /// 取り下げの書込みを再開する。どちらも既存の実行枠で、同じ ID で送る。
    pub async fn resume_pending_writes(&self) -> Result<()> {
        self.subscription_registry.dm_outbox_retry_wake.notify_one();
        self.resume_withdrawal_writes().await
    }

    pub(crate) async fn shutdown_direct_message_outbox_retry(&self) {
        self.subscription_registry
            .dm_outbox_retry_closed
            .store(true, Ordering::Release);
        let task = self
            .subscription_registry
            .dm_outbox_retry_task
            .lock()
            .await
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = n0_future::time::timeout(std::time::Duration::from_secs(2), task.wait()).await;
        }
    }
}
