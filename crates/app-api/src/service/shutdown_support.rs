use super::*;

impl AppService {
    pub async fn shutdown(&self) {
        {
            let _save_access = self.services.content_save_access.lock().await;
            self.services.content_closed.send_replace(true);
        }
        self.shutdown_direct_message_outbox_retry().await;
        self.shutdown_account_receive_offers().await;
        self.services.session_projections.clear().await;
        {
            // 取り直しが止める途中の hint 購読の上に乗らないよう、lock を持ったまま止める。
            let mut leases = self.subscription_registry.scope_leases.lock().await;
            for task in leases.clear() {
                stop_scope_task(&self.services, task).await;
            }
        }
        let heartbeats =
            std::mem::take(&mut *self.subscription_registry.dome_heartbeats.lock().await);
        for (_, heartbeat) in heartbeats {
            heartbeat.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), heartbeat.wait()).await;
        }
        self.dome_host_sessions.lock().await.clear();
        let presence_handles = {
            let mut tasks = self.subscription_registry.live_presence_tasks.lock().await;
            tasks.drain().map(|(_, handle)| handle).collect::<Vec<_>>()
        };
        for handle in presence_handles {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
    }
}
