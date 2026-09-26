use super::*;

pub(crate) fn gossip_disabled_channel_key(topic_id: &str, channel_id: &str) -> String {
    format!("{topic_id}::{channel_id}")
}

impl AppService {
    pub(crate) async fn is_topic_gossip_disabled(&self, topic_id: &str) -> bool {
        self.gossip_disabled_topics.lock().await.contains(topic_id)
    }

    pub(crate) async fn is_channel_gossip_disabled(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> bool {
        if self.is_topic_gossip_disabled(topic_id).await {
            return true;
        }
        self.gossip_disabled_channels
            .lock()
            .await
            .contains(&gossip_disabled_channel_key(topic_id, channel_id))
    }

    pub async fn list_gossip_disabled_topics(&self) -> Vec<String> {
        let mut topics = self
            .gossip_disabled_topics
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        topics.sort();
        topics
    }

    pub async fn list_gossip_disabled_channels(&self) -> Vec<String> {
        let mut channels = self
            .gossip_disabled_channels
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        channels.sort();
        channels
    }

    pub async fn restore_gossip_disabled_state(
        &self,
        disabled_topics: Vec<String>,
        disabled_channels: Vec<String>,
    ) {
        {
            let mut topics = self.gossip_disabled_topics.lock().await;
            topics.clear();
            topics.extend(disabled_topics);
        }
        {
            let mut channels = self.gossip_disabled_channels.lock().await;
            channels.clear();
            channels.extend(disabled_channels);
        }
    }
}
