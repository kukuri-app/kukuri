use anyhow::Result;
use sqlx::Row;

use super::SqliteStore;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateIndexGrant {
    pub base_url: String,
    pub topic_id: String,
    pub channel_id: String,
    pub applied_epoch_id: String,
}

impl SqliteStore {
    pub async fn save_private_index_grant(&self, grant: &PrivateIndexGrant) -> Result<()> {
        sqlx::query(
            "INSERT INTO cn_private_index_grants
             (base_url, topic_id, channel_id, applied_epoch_id, node_revision, channel_revision, active)
             VALUES (?, ?, ?, ?,
               COALESCE((SELECT revision FROM cn_private_index_stops WHERE kind='node' AND id=?), 0),
               COALESCE((SELECT revision FROM cn_private_index_stops
                         WHERE kind='channel' AND id=json_array(?, ?)), 0), 1)
             ON CONFLICT(base_url, topic_id, channel_id) DO UPDATE SET
               applied_epoch_id=excluded.applied_epoch_id,
               node_revision=excluded.node_revision,
               channel_revision=excluded.channel_revision, active=1",
        )
        .bind(&grant.base_url)
        .bind(&grant.topic_id)
        .bind(&grant.channel_id)
        .bind(&grant.applied_epoch_id)
        .bind(&grant.base_url)
        .bind(&grant.topic_id)
        .bind(&grant.channel_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One indexed row per CN tick. Advancing the timestamp keeps all grants
    /// eligible without materializing the account's complete grant set.
    pub async fn next_private_index_grant(
        &self,
        base_url: &str,
        now_ms: i64,
    ) -> Result<Option<PrivateIndexGrant>> {
        for _ in 0..8 {
            let mut tx = self.pool.begin().await?;
            let row = sqlx::query(
                "SELECT g.base_url, g.topic_id, g.channel_id, g.applied_epoch_id,
                    g.node_revision, g.channel_revision,
                    COALESCE(n.revision, 0) AS current_node_revision,
                    COALESCE(c.revision, 0) AS current_channel_revision
                 FROM cn_private_index_grants g
                 LEFT JOIN cn_private_index_stops n ON n.kind='node' AND n.id=g.base_url
                 LEFT JOIN cn_private_index_stops c ON c.kind='channel'
                   AND c.id=json_array(g.topic_id, g.channel_id)
                 WHERE g.base_url=? AND g.active=1
                 ORDER BY g.last_checked_at_ms, g.topic_id, g.channel_id LIMIT 1",
            )
            .bind(base_url)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = row else {
                return Ok(None);
            };
            let grant = PrivateIndexGrant {
                base_url: row.try_get("base_url")?,
                topic_id: row.try_get("topic_id")?,
                channel_id: row.try_get("channel_id")?,
                applied_epoch_id: row.try_get("applied_epoch_id")?,
            };
            let valid = row.try_get::<i64, _>("node_revision")?
                == row.try_get::<i64, _>("current_node_revision")?
                && row.try_get::<i64, _>("channel_revision")?
                    == row.try_get::<i64, _>("current_channel_revision")?;
            if !valid {
                sqlx::query(
                    "DELETE FROM cn_private_index_grants
                     WHERE base_url=? AND topic_id=? AND channel_id=?",
                )
                .bind(base_url)
                .bind(&grant.topic_id)
                .bind(&grant.channel_id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                continue;
            }
            sqlx::query(
                "UPDATE cn_private_index_grants SET last_checked_at_ms=?
                 WHERE base_url=? AND topic_id=? AND channel_id=? AND active=1",
            )
            .bind(now_ms)
            .bind(base_url)
            .bind(&grant.topic_id)
            .bind(&grant.channel_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(Some(grant));
        }
        Ok(None)
    }

    pub async fn mark_private_index_grant_applied(
        &self,
        grant: &PrivateIndexGrant,
        epoch_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE cn_private_index_grants SET applied_epoch_id=?
             WHERE base_url=? AND topic_id=? AND channel_id=?
               AND applied_epoch_id=? AND active=1",
        )
        .bind(epoch_id)
        .bind(&grant.base_url)
        .bind(&grant.topic_id)
        .bind(&grant.channel_id)
        .bind(&grant.applied_epoch_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn stop_private_index_grant(
        &self,
        base_url: &str,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM cn_private_index_grants
             WHERE base_url=? AND topic_id=? AND channel_id=?",
        )
        .bind(base_url)
        .bind(topic_id)
        .bind(channel_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn stop_private_index_grants_for_node(&self, base_url: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO cn_private_index_stops (kind, id) VALUES ('node', ?)
             ON CONFLICT(kind, id) DO UPDATE SET revision=revision+1",
        )
        .bind(base_url)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn stop_private_index_grants_for_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO cn_private_index_stops (kind, id) VALUES ('channel', json_array(?, ?))
             ON CONFLICT(kind, id) DO UPDATE SET revision=revision+1",
        )
        .bind(topic_id)
        .bind(channel_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
