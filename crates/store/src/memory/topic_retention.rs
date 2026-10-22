use super::*;
use crate::{TopicRetentionCursor, TopicRetentionRow, TopicRetentionStore};

#[async_trait]
impl TopicRetentionStore for MemoryStore {
    async fn retain_topic(&self, topic_id: &str, retained_at: i64) -> Result<()> {
        self.topic_retention
            .write()
            .await
            .topics
            .entry(topic_id.to_string())
            .or_insert(retained_at);
        Ok(())
    }

    async fn release_topic(&self, topic_id: &str) -> Result<()> {
        let mut retention = self.topic_retention.write().await;
        retention.topics.remove(topic_id);
        retention
            .cursors
            .retain(|(topic, _, _), _| topic != topic_id);
        Ok(())
    }

    async fn retained_topics(&self, limit: usize) -> Result<Vec<TopicRetentionRow>> {
        Ok(self
            .topic_retention
            .read()
            .await
            .topics
            .iter()
            .take(limit)
            .map(|(topic_id, retained_at)| TopicRetentionRow {
                topic_id: topic_id.clone(),
                retained_at: *retained_at,
            })
            .collect())
    }

    async fn topic_retention_cursors(
        &self,
        topic_id: &str,
        from_bucket: u64,
    ) -> Result<Vec<TopicRetentionCursor>> {
        Ok(self
            .topic_retention
            .read()
            .await
            .cursors
            .iter()
            .filter(|((topic, _, bucket), _)| topic == topic_id && *bucket >= from_bucket)
            .map(
                |((_, index_prefix, bucket), (created_at, object_id))| TopicRetentionCursor {
                    index_prefix: index_prefix.clone(),
                    bucket: *bucket,
                    created_at: *created_at,
                    object_id: object_id.clone(),
                },
            )
            .collect())
    }

    async fn put_topic_retention_cursor(
        &self,
        topic_id: &str,
        cursor: &TopicRetentionCursor,
    ) -> Result<()> {
        let mut retention = self.topic_retention.write().await;
        if retention.topics.contains_key(topic_id) {
            retention.cursors.insert(
                (
                    topic_id.to_string(),
                    cursor.index_prefix.clone(),
                    cursor.bucket,
                ),
                (cursor.created_at, cursor.object_id.clone()),
            );
        }
        Ok(())
    }

    async fn prune_topic_retention_cursors(
        &self,
        topic_id: &str,
        before_bucket: u64,
    ) -> Result<()> {
        self.topic_retention
            .write()
            .await
            .cursors
            .retain(|(topic, _, bucket), _| topic != topic_id || *bucket >= before_bucket);
        Ok(())
    }
}
