use super::*;
use crate::traits::{CONTENT_OBSERVATION_RETENTION_MS, MAX_CONTENT_OBSERVATIONS};

#[async_trait]
impl ContentObservationStore for SqliteStore {
    async fn put_content_observation(&self, row: ContentObservationRow) -> Result<bool> {
        let subject_exists = match row.subject_kind.as_str() {
            "post" => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT EXISTS(SELECT 1 FROM object_index_cache WHERE object_id = ?1)",
                )
                .bind(row.subject_id.as_str())
                .fetch_one(&self.pool)
                .await?
                    != 0
            }
            "profile" => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT EXISTS(SELECT 1 FROM profiles WHERE pubkey = ?1)",
                )
                .bind(row.subject_id.as_str())
                .fetch_one(&self.pool)
                .await?
                    != 0
            }
            _ => false,
        };
        if !subject_exists {
            return Ok(false);
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO content_observations (
              subject_kind, subject_id, node_base_url, capability, observed_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(subject_kind, subject_id, node_base_url, capability) DO UPDATE SET
              observed_at = MAX(content_observations.observed_at, excluded.observed_at)
            "#,
        )
        .bind(row.subject_kind.as_str())
        .bind(row.subject_id.as_str())
        .bind(row.node_base_url.as_str())
        .bind(row.capability.as_str())
        .bind(row.observed_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM content_observations WHERE observed_at < ?1")
            .bind(
                row.observed_at
                    .saturating_sub(CONTENT_OBSERVATION_RETENTION_MS),
            )
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"
            DELETE FROM content_observations
            WHERE rowid IN (
              SELECT rowid FROM content_observations
              ORDER BY observed_at DESC, rowid DESC
              LIMIT -1 OFFSET ?1
            )
            "#,
        )
        .bind(MAX_CONTENT_OBSERVATIONS as i64)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn list_content_observations_at(
        &self,
        subject_kind: &str,
        subject_id: &str,
        now_millis: i64,
    ) -> Result<Vec<ContentObservationRow>> {
        let cutoff = now_millis.saturating_sub(CONTENT_OBSERVATION_RETENTION_MS);
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM content_observations WHERE observed_at < ?1")
            .bind(cutoff)
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(
            r#"
            SELECT subject_kind, subject_id, node_base_url, capability, observed_at
            FROM content_observations
            WHERE subject_kind = ?1 AND subject_id = ?2
            ORDER BY observed_at DESC, node_base_url ASC, capability ASC
            "#,
        )
        .bind(subject_kind)
        .bind(subject_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|row| {
                Ok(ContentObservationRow {
                    subject_kind: row.try_get("subject_kind")?,
                    subject_id: row.try_get("subject_id")?,
                    node_base_url: row.try_get("node_base_url")?,
                    capability: row.try_get("capability")?,
                    observed_at: row.try_get("observed_at")?,
                })
            })
            .collect()
    }
}
