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
        let topics_to_unsubscribe = self
            .subscription_registry
            .subscriptions
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let private_channels_to_unsubscribe = self
            .subscription_registry
            .private_channel_subscriptions
            .lock()
            .await
            .keys()
            .filter_map(|key| key.split("::").nth(1).map(str::to_owned))
            .collect::<BTreeSet<_>>();
        let handles = {
            let mut subscriptions = self.subscription_registry.subscriptions.lock().await;
            subscriptions
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in handles {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
        let private_handles = {
            let mut subscriptions = self
                .subscription_registry
                .private_channel_subscriptions
                .lock()
                .await;
            subscriptions
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in private_handles {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
        for channel_id in private_channels_to_unsubscribe {
            let _ = self
                .services
                .hint_transport
                .unsubscribe_hints(&private_channel_hint_topic(channel_id.as_str()))
                .await;
        }
        for topic_id in topics_to_unsubscribe {
            let _ = self
                .services
                .hint_transport
                .unsubscribe_hints(&TopicId::new(topic_id))
                .await;
        }
        let author_handles = {
            let mut subscriptions = self.subscription_registry.author_subscriptions.lock().await;
            subscriptions
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in author_handles {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
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
