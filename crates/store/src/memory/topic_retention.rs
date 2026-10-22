use super::*;
use crate::{
    RETENTION_RETRY_DELAYS_MS, RETENTION_RETRY_LIMIT, TopicRetentionCursor, TopicRetentionRetry,
    TopicRetentionRow, TopicRetentionStore,
};

#[async_trait]
impl TopicRetentionStore for MemoryStore {
    async fn retain_topic(&self, topic_id: &str, retained_at: i64) -> Result<()> {
        self.topic_retention
            .write()
            .await
            .topics
            .entry(topic_id.to_string())
            .or_insert((retained_at, 0));
        Ok(())
    }

    async fn release_topic(&self, topic_id: &str) -> Result<()> {
        let mut retention = self.topic_retention.write().await;
        retention.topics.remove(topic_id);
        retention
            .cursors
            .retain(|(topic, _, _), _| topic != topic_id);
        retention
            .retries
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
            .map(|(topic_id, (retained_at, _))| TopicRetentionRow {
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

    async fn schedule_topic_retention_retry(
        &self,
        topic_id: &str,
        object_kind: &str,
        object_id: &str,
        bucket: u64,
        now_ms: i64,
    ) -> Result<()> {
        let mut retention = self.topic_retention.write().await;
        if !retention.topics.contains_key(topic_id) {
            return Ok(());
        }
        let key = (
            topic_id.to_string(),
            object_kind.to_string(),
            object_id.to_string(),
        );
        let done = match retention.retries.get(&key) {
            Some((_, attempts, _)) => Some(*attempts as usize),
            None if retention.retries.len() >= RETENTION_RETRY_LIMIT => None,
            None => Some(0),
        }
        .filter(|done| *done < RETENTION_RETRY_DELAYS_MS.len());
        match done {
            Some(done) => {
                retention.retries.insert(
                    key,
                    (
                        bucket,
                        u32::try_from(done + 1)?,
                        now_ms.saturating_add(RETENTION_RETRY_DELAYS_MS[done]),
                    ),
                );
            }
            None => {
                retention.retries.remove(&key);
                if let Some((_, given_up)) = retention.topics.get_mut(topic_id) {
                    *given_up += 1;
                }
            }
        }
        Ok(())
    }

    async fn due_topic_retention_retries(
        &self,
        topic_id: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<TopicRetentionRetry>> {
        let retention = self.topic_retention.read().await;
        let mut due = retention
            .retries
            .iter()
            .filter(|((topic, _, _), (_, _, next_at))| topic == topic_id && *next_at <= now_ms)
            .collect::<Vec<_>>();
        due.sort_by_key(|(_, (_, _, next_at))| *next_at);
        Ok(due
            .into_iter()
            .take(limit)
            .map(
                |((_, object_kind, object_id), (bucket, attempts, _))| TopicRetentionRetry {
                    object_kind: object_kind.clone(),
                    object_id: object_id.clone(),
                    bucket: *bucket,
                    attempts: *attempts,
                },
            )
            .collect())
    }

    async fn finish_topic_retention_retry(
        &self,
        topic_id: &str,
        object_kind: &str,
        object_id: &str,
    ) -> Result<()> {
        self.topic_retention.write().await.retries.remove(&(
            topic_id.to_string(),
            object_kind.to_string(),
            object_id.to_string(),
        ));
        Ok(())
    }

    async fn topic_retention_retry_counts(&self, topic_id: &str) -> Result<(u64, u64)> {
        let retention = self.topic_retention.read().await;
        let pending = retention
            .retries
            .keys()
            .filter(|(topic, _, _)| topic == topic_id)
            .count();
        let given_up = retention
            .topics
            .get(topic_id)
            .map_or(0, |(_, given_up)| *given_up);
        Ok((u64::try_from(pending)?, given_up))
    }
}
