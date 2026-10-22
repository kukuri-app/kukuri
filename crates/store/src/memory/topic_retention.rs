use super::*;
use crate::{
    DEFAULT_RETENTION_CAPACITY_BYTES, DEFAULT_RETENTION_DAYS, RETENTION_RETRY_DELAYS_MS,
    RETENTION_RETRY_LIMIT, TopicRetentionCursor, TopicRetentionLimits, TopicRetentionRetry,
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
        retention.days.retain(|(topic, _), _| topic != topic_id);
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

    async fn topic_retention_limits(&self) -> Result<TopicRetentionLimits> {
        let (retention_days, capacity_bytes) = self
            .topic_retention
            .read()
            .await
            .limits
            .unwrap_or((DEFAULT_RETENTION_DAYS, DEFAULT_RETENTION_CAPACITY_BYTES));
        Ok(TopicRetentionLimits {
            retention_days,
            capacity_bytes,
            used_bytes: 0,
        })
    }

    async fn set_topic_retention_limits(
        &self,
        retention_days: u32,
        capacity_bytes: u64,
    ) -> Result<()> {
        self.topic_retention.write().await.limits = Some((retention_days, capacity_bytes));
        Ok(())
    }

    async fn open_topic_retention_days(
        &self,
        topic_id: &str,
        from_bucket: u64,
        to_bucket: u64,
    ) -> Result<()> {
        let mut retention = self.topic_retention.write().await;
        if !retention.topics.contains_key(topic_id) {
            return Ok(());
        }
        // 日の行は古い側から消えるので、欠けは両端にだけある。
        let buckets = retention
            .days
            .keys()
            .filter(|(topic, _)| topic == topic_id)
            .map(|(_, bucket)| *bucket);
        let oldest = buckets.clone().min().unwrap_or(to_bucket + 1);
        let newest = buckets.max().unwrap_or(to_bucket);
        let missing = (from_bucket..oldest.min(to_bucket + 1))
            .chain(from_bucket.max(newest + 1)..=to_bucket)
            .collect::<Vec<_>>();
        // 作った日は最初から読む。
        for bucket in missing {
            if retention
                .days
                .insert((topic_id.to_string(), bucket), false)
                .is_none()
            {
                retention
                    .cursors
                    .retain(|(topic, _, day), _| topic != topic_id || *day != bucket);
            }
        }
        Ok(())
    }

    async fn protect_topic_retention_day(
        &self,
        topic_id: &str,
        bucket: u64,
        _scope_key: &str,
    ) -> Result<bool> {
        Ok(self
            .topic_retention
            .read()
            .await
            .days
            .contains_key(&(topic_id.to_string(), bucket)))
    }

    async fn next_topic_retention_backfill(
        &self,
        topic_id: &str,
        from_bucket: u64,
        to_bucket: u64,
    ) -> Result<Option<u64>> {
        Ok(self
            .topic_retention
            .read()
            .await
            .days
            .iter()
            .filter(|((topic, bucket), done)| {
                topic == topic_id && (from_bucket..=to_bucket).contains(bucket) && !**done
            })
            .map(|((_, bucket), _)| *bucket)
            .max())
    }

    async fn finish_topic_retention_day(&self, topic_id: &str, bucket: u64) -> Result<()> {
        let mut retention = self.topic_retention.write().await;
        if let Some(day) = retention.days.get_mut(&(topic_id.to_string(), bucket)) {
            *day = true;
        }
        retention
            .cursors
            .retain(|(topic, _, day), _| topic != topic_id || *day != bucket);
        Ok(())
    }

    async fn claim_topic_retention_blob(&self, _: &str, _: u64, _: &str) -> Result<()> {
        Ok(())
    }

    async fn claim_topic_retention_records(&self, _: &str, _: u64, _: &str, _: &str) -> Result<()> {
        Ok(())
    }

    async fn protect_topic_retention_scopes_step(&self, _: usize) -> Result<usize> {
        Ok(0)
    }

    async fn reclaim_topic_retention_step(&self, cutoff_bucket: u64, _: usize) -> Result<usize> {
        self.topic_retention
            .write()
            .await
            .days
            .retain(|(_, bucket), _| *bucket >= cutoff_bucket);
        Ok(0)
    }

    async fn topic_retention_used(&self, _: &str) -> Result<u64> {
        Ok(0)
    }
}
