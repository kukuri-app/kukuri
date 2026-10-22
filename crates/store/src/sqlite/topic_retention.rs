//! 保持する公開 topic と、取り込みの続きの位置（#1909）。どの読み書きも topic の主キーの範囲だけを読む。

use super::*;
use crate::{
    RETENTION_RETRY_DELAYS_MS, RETENTION_RETRY_LIMIT, TopicRetentionCursor, TopicRetentionLimits,
    TopicRetentionRetry, TopicRetentionRow, TopicRetentionStore,
};

#[async_trait]
impl TopicRetentionStore for SqliteStore {
    async fn retain_topic(&self, topic_id: &str, retained_at: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let added = sqlx::query(
            "INSERT INTO topic_retention (topic_id, retained_at) VALUES (?1, ?2) \
             ON CONFLICT(topic_id) DO NOTHING",
        )
        .bind(topic_id)
        .bind(retained_at)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // 保持をやめた後に保持し直すとき、前の保持の日が保護参照を外している途中で残っていれば、外すのをやめて読み直す
        // 日へ戻す。
        if added != 0 {
            sqlx::query(
                "UPDATE topic_retention_days SET read_done = 0, releasing = 0 WHERE topic_id = ?1",
            )
            .bind(topic_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn release_topic(&self, topic_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        // 日の行は、回収が保護参照を外し終えてから消す。
        for query in [
            "DELETE FROM topic_retention_cursors WHERE topic_id = ?1",
            "DELETE FROM topic_retention_retries WHERE topic_id = ?1",
            "DELETE FROM topic_retention_scopes WHERE topic_id = ?1",
            "UPDATE topic_retention_days SET releasing = 1 WHERE topic_id = ?1",
            "DELETE FROM topic_retention WHERE topic_id = ?1",
        ] {
            sqlx::query(query).bind(topic_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn retained_topics(&self, limit: usize) -> Result<Vec<TopicRetentionRow>> {
        let rows = sqlx::query(
            "SELECT topic_id, retained_at FROM topic_retention ORDER BY topic_id LIMIT ?1",
        )
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(TopicRetentionRow {
                    topic_id: row.try_get("topic_id")?,
                    retained_at: row.try_get("retained_at")?,
                })
            })
            .collect()
    }

    async fn topic_retention_cursors(
        &self,
        topic_id: &str,
        from_bucket: u64,
    ) -> Result<Vec<TopicRetentionCursor>> {
        let rows = sqlx::query(
            "SELECT index_prefix, bucket, created_at, object_id FROM topic_retention_cursors \
             WHERE topic_id = ?1 AND bucket >= ?2 ORDER BY index_prefix, bucket",
        )
        .bind(topic_id)
        .bind(i64::try_from(from_bucket)?)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(TopicRetentionCursor {
                    index_prefix: row.try_get("index_prefix")?,
                    bucket: u64::try_from(row.try_get::<i64, _>("bucket")?)?,
                    created_at: row.try_get("created_at")?,
                    object_id: row.try_get("object_id")?,
                })
            })
            .collect()
    }

    async fn put_topic_retention_cursor(
        &self,
        topic_id: &str,
        cursor: &TopicRetentionCursor,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO topic_retention_cursors (topic_id, index_prefix, bucket, created_at, object_id) \
             SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS(SELECT 1 FROM topic_retention WHERE topic_id = ?1) \
             ON CONFLICT(topic_id, index_prefix, bucket) DO UPDATE SET \
             created_at = excluded.created_at, object_id = excluded.object_id",
        )
        .bind(topic_id)
        .bind(cursor.index_prefix.as_str())
        .bind(i64::try_from(cursor.bucket)?)
        .bind(cursor.created_at)
        .bind(cursor.object_id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn prune_topic_retention_cursors(
        &self,
        topic_id: &str,
        before_bucket: u64,
    ) -> Result<()> {
        sqlx::query("DELETE FROM topic_retention_cursors WHERE topic_id = ?1 AND bucket < ?2")
            .bind(topic_id)
            .bind(i64::try_from(before_bucket)?)
            .execute(&self.pool)
            .await?;
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
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let retained = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM topic_retention WHERE topic_id = ?1",
        )
        .bind(topic_id)
        .fetch_one(&mut *tx)
        .await?;
        if retained == 0 {
            return Ok(());
        }
        let attempts = sqlx::query_scalar::<_, i64>(
            "SELECT attempts FROM topic_retention_retries \
             WHERE topic_id = ?1 AND object_kind = ?2 AND object_id = ?3",
        )
        .bind(topic_id)
        .bind(object_kind)
        .bind(object_id)
        .fetch_optional(&mut *tx)
        .await?;
        let next = match attempts {
            Some(attempts) => Some(usize::try_from(attempts)?),
            // 予定の合計は上限つきなので、数え上げの手間も上限つき。
            None if usize::try_from(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM topic_retention_retries")
                    .fetch_one(&mut *tx)
                    .await?,
            )? >= RETENTION_RETRY_LIMIT =>
            {
                None
            }
            None => Some(0),
        }
        .filter(|done| *done < RETENTION_RETRY_DELAYS_MS.len());
        match next {
            Some(done) => {
                sqlx::query(
                    "INSERT INTO topic_retention_retries \
                     (topic_id, object_kind, object_id, bucket, attempts, next_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                     ON CONFLICT(topic_id, object_kind, object_id) DO UPDATE SET \
                     bucket = excluded.bucket, attempts = excluded.attempts, next_at = excluded.next_at",
                )
                .bind(topic_id)
                .bind(object_kind)
                .bind(object_id)
                .bind(i64::try_from(bucket)?)
                .bind(i64::try_from(done + 1)?)
                .bind(now_ms.saturating_add(RETENTION_RETRY_DELAYS_MS[done]))
                .execute(&mut *tx)
                .await?;
            }
            None => {
                sqlx::query(
                    "DELETE FROM topic_retention_retries \
                     WHERE topic_id = ?1 AND object_kind = ?2 AND object_id = ?3",
                )
                .bind(topic_id)
                .bind(object_kind)
                .bind(object_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE topic_retention SET given_up = given_up + 1 WHERE topic_id = ?1",
                )
                .bind(topic_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn due_topic_retention_retries(
        &self,
        topic_id: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<TopicRetentionRetry>> {
        let rows = sqlx::query(
            "SELECT object_kind, object_id, bucket, attempts FROM topic_retention_retries \
             WHERE topic_id = ?1 AND next_at <= ?2 ORDER BY next_at LIMIT ?3",
        )
        .bind(topic_id)
        .bind(now_ms)
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(TopicRetentionRetry {
                    object_kind: row.try_get("object_kind")?,
                    object_id: row.try_get("object_id")?,
                    bucket: u64::try_from(row.try_get::<i64, _>("bucket")?)?,
                    attempts: u32::try_from(row.try_get::<i64, _>("attempts")?)?,
                })
            })
            .collect()
    }

    async fn finish_topic_retention_retry(
        &self,
        topic_id: &str,
        object_kind: &str,
        object_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM topic_retention_retries \
             WHERE topic_id = ?1 AND object_kind = ?2 AND object_id = ?3",
        )
        .bind(topic_id)
        .bind(object_kind)
        .bind(object_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn topic_retention_retry_counts(&self, topic_id: &str) -> Result<(u64, u64)> {
        let pending = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM topic_retention_retries WHERE topic_id = ?1",
        )
        .bind(topic_id)
        .fetch_one(&self.pool)
        .await?;
        let given_up = sqlx::query_scalar::<_, i64>(
            "SELECT given_up FROM topic_retention WHERE topic_id = ?1",
        )
        .bind(topic_id)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(0);
        Ok((u64::try_from(pending)?, u64::try_from(given_up)?))
    }

    async fn topic_retention_limits(&self) -> Result<TopicRetentionLimits> {
        let row = sqlx::query(
            "SELECT retention_days, capacity_bytes, used_bytes FROM topic_retention_limits WHERE id = 1",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(TopicRetentionLimits {
            retention_days: u32::try_from(row.get::<i64, _>("retention_days"))?,
            capacity_bytes: u64::try_from(row.get::<i64, _>("capacity_bytes"))?,
            used_bytes: u64::try_from(row.get::<i64, _>("used_bytes").max(0))?,
        })
    }

    async fn set_topic_retention_limits(
        &self,
        retention_days: u32,
        capacity_bytes: u64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE topic_retention_limits SET retention_days = ?1, capacity_bytes = ?2 WHERE id = 1",
        )
        .bind(i64::from(retention_days))
        .bind(i64::try_from(capacity_bytes)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn open_topic_retention_days(
        &self,
        topic_id: &str,
        from_bucket: u64,
        to_bucket: u64,
    ) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query(
            "SELECT (SELECT MIN(bucket) FROM topic_retention_days WHERE topic_id = ?1) AS oldest, \
             (SELECT MAX(bucket) FROM topic_retention_days WHERE topic_id = ?1) AS newest, \
             (SELECT released_through FROM topic_retention_limits WHERE id = 1) AS floor \
             FROM topic_retention WHERE topic_id = ?1",
        )
        .bind(topic_id)
        .fetch_optional(&mut *tx)
        .await?;
        // 保持していない topic には作らない。容量で保護を外した日とそれより前の日も作らない。
        let Some(row) = row else { return Ok(()) };
        let bucket = |name| -> Result<Option<u64>> {
            Ok(row
                .get::<Option<i64>, _>(name)
                .map(u64::try_from)
                .transpose()?)
        };
        let start = bucket("floor")?.map_or(from_bucket, |floor| from_bucket.max(floor + 1));
        // 日の行は古い側から消えるので、欠けは両端にだけある(期間を延ばしたときと保持し直したときの古い側と、新しい日)。
        let oldest = bucket("oldest")?.unwrap_or(to_bucket + 1);
        let newest = bucket("newest")?.unwrap_or(to_bucket);
        for bucket in (start..oldest.min(to_bucket + 1)).chain(start.max(newest + 1)..=to_bucket) {
            sqlx::query(
                "INSERT OR IGNORE INTO topic_retention_days (topic_id, bucket) VALUES (?1, ?2)",
            )
            .bind(topic_id)
            .bind(i64::try_from(bucket)?)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn protect_topic_retention_day(
        &self,
        topic_id: &str,
        bucket: u64,
        scope_key: &str,
    ) -> Result<bool> {
        // 登録済みの日は書かない(回収は、外し始める日の登録を同じ transaction で消す)。
        let registered = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM topic_retention_scopes WHERE scope_key = ?1)",
        )
        .bind(scope_key)
        .fetch_one(&self.pool)
        .await?;
        if registered != 0 {
            return Ok(true);
        }
        // 回収が日を外し始める transaction と直列にし、外し始めた日へ登録を残さない。
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let bucket = i64::try_from(bucket)?;
        let held = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM topic_retention_days \
             WHERE topic_id = ?1 AND bucket = ?2 AND releasing = 0)",
        )
        .bind(topic_id)
        .bind(bucket)
        .fetch_one(&mut *tx)
        .await?;
        if held == 0 {
            return Ok(false);
        }
        sqlx::query(
            "INSERT OR IGNORE INTO topic_retention_scopes \
             (scope_key, topic_id, bucket, protect_after) VALUES (?1, ?2, ?3, '')",
        )
        .bind(scope_key)
        .bind(topic_id)
        .bind(bucket)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn next_topic_retention_backfill(
        &self,
        topic_id: &str,
        from_bucket: u64,
        to_bucket: u64,
    ) -> Result<Option<u64>> {
        let bucket = sqlx::query_scalar::<_, i64>(
            "SELECT bucket FROM topic_retention_days WHERE topic_id = ?1 AND bucket >= ?2 \
             AND bucket <= ?3 AND read_done = 0 AND releasing = 0 AND bucket > \
             COALESCE((SELECT released_through FROM topic_retention_limits WHERE id = 1), -1) \
             ORDER BY bucket DESC LIMIT 1",
        )
        .bind(topic_id)
        .bind(i64::try_from(from_bucket)?)
        .bind(i64::try_from(to_bucket)?)
        .fetch_optional(&self.pool)
        .await?;
        Ok(bucket.map(u64::try_from).transpose()?)
    }

    async fn finish_topic_retention_day(&self, topic_id: &str, bucket: u64) -> Result<()> {
        let bucket = i64::try_from(bucket)?;
        let mut tx = self.pool.begin().await?;
        for query in [
            "UPDATE topic_retention_days SET read_done = 1 WHERE topic_id = ?1 AND bucket = ?2",
            "DELETE FROM topic_retention_cursors WHERE topic_id = ?1 AND bucket = ?2",
        ] {
            sqlx::query(query)
                .bind(topic_id)
                .bind(bucket)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn claim_topic_retention_blob(
        &self,
        topic_id: &str,
        bucket: u64,
        hash: &str,
    ) -> Result<()> {
        self.claim_retained(topic_id, bucket, &[("blob", hash.to_owned())])
            .await
    }

    async fn claim_topic_retention_records(
        &self,
        topic_id: &str,
        bucket: u64,
        scope_key: &str,
        record_key: &str,
    ) -> Result<()> {
        let keys = sqlx::query_scalar::<_, String>(
            "SELECT cache_key FROM remote_content_cache \
             WHERE kind = 'record' AND scope_key = ?1 AND record_key = ?2",
        )
        .bind(scope_key)
        .bind(record_key)
        .fetch_all(&self.pool)
        .await?;
        let items = keys
            .into_iter()
            .map(|key| ("record", key))
            .collect::<Vec<_>>();
        self.claim_retained(topic_id, bucket, &items).await
    }

    async fn protect_topic_retention_scopes_step(&self, limit: usize) -> Result<usize> {
        self.protect_retained_scopes_step(limit).await
    }

    async fn reclaim_topic_retention_step(
        &self,
        cutoff_bucket: u64,
        limit: usize,
    ) -> Result<usize> {
        // 外した参照と外し終えた日(参照が無くても 1 と数える)を合わせて `limit` まで進める。
        let mut spent = 0;
        while spent < limit {
            let Some((topic_id, bucket)) = self.next_release_target(cutoff_bucket).await? else {
                break;
            };
            spent += self
                .release_retained_refs(&topic_id, bucket, limit - spent)
                .await?
                .max(1);
        }
        Ok(spent)
    }

    async fn topic_retention_used(&self, topic_id: &str) -> Result<u64> {
        let used = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(used_bytes), 0) FROM topic_retention_days WHERE topic_id = ?1",
        )
        .bind(topic_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(u64::try_from(used.max(0))?)
    }
}
