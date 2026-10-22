//! 保持する公開 topic と、取り込みの続きの位置（#1909）。どの読み書きも topic の主キーの範囲だけを読む。

use super::*;
use crate::{TopicRetentionCursor, TopicRetentionRow, TopicRetentionStore};

#[async_trait]
impl TopicRetentionStore for SqliteStore {
    async fn retain_topic(&self, topic_id: &str, retained_at: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO topic_retention (topic_id, retained_at) VALUES (?1, ?2) \
             ON CONFLICT(topic_id) DO NOTHING",
        )
        .bind(topic_id)
        .bind(retained_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn release_topic(&self, topic_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM topic_retention_cursors WHERE topic_id = ?1")
            .bind(topic_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM topic_retention WHERE topic_id = ?1")
            .bind(topic_id)
            .execute(&mut *tx)
            .await?;
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
}
