//! 信頼評価の観測の提供の、CN ごとの状態と送信待ち(#1061、#1510)。
//!
//! pass と操作は、node の 1 行と、key の行・上限つきの範囲だけを読み書きする。

use anyhow::Result;
use kukuri_core::KukuriEnvelope;
use sqlx::Row;

use super::SqliteStore;

/// CN ごとの観測の提供の状態。提供していない node は送信待ちを持たない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrustObservationNode {
    pub enabled: bool,
    pub needs_reconsent: bool,
    pub revocation_pending: bool,
}

impl SqliteStore {
    pub async fn trust_observation_node(
        &self,
        base_url: &str,
    ) -> Result<Option<TrustObservationNode>> {
        sqlx::query(
            "SELECT enabled, needs_reconsent, revocation_pending
             FROM cn_trust_observation_nodes WHERE base_url = ?",
        )
        .bind(base_url)
        .fetch_optional(&self.pool)
        .await?
        .map(|row| {
            Ok(TrustObservationNode {
                enabled: row.try_get("enabled")?,
                needs_reconsent: row.try_get("needs_reconsent")?,
                revocation_pending: row.try_get("revocation_pending")?,
            })
        })
        .transpose()
    }

    /// node の状態を書く(`None` は node を忘れる)。提供していない node の送信待ちは消す。
    pub async fn save_trust_observation_node(
        &self,
        base_url: &str,
        node: Option<TrustObservationNode>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        match node {
            Some(node) => {
                sqlx::query(
                    "INSERT INTO cn_trust_observation_nodes
                     (base_url, enabled, needs_reconsent, revocation_pending) VALUES (?, ?, ?, ?)
                     ON CONFLICT(base_url) DO UPDATE SET enabled = excluded.enabled,
                       needs_reconsent = excluded.needs_reconsent,
                       revocation_pending = excluded.revocation_pending",
                )
                .bind(base_url)
                .bind(node.enabled)
                .bind(node.needs_reconsent)
                .bind(node.revocation_pending)
                .execute(&mut *tx)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM cn_trust_observation_nodes WHERE base_url = ?")
                    .bind(base_url)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        if !node.is_some_and(|node| node.enabled) {
            sqlx::query("DELETE FROM cn_trust_observation_pending WHERE base_url = ?")
                .bind(base_url)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 提供中(有効で、削除要求が未完了でない)の node があるか。
    pub async fn trust_observation_sharing(&self) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM cn_trust_observation_nodes
             WHERE enabled = 1 AND revocation_pending = 0)",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    /// `key` の送信待ちのうち、最も新しい署名の時刻。次の署名はこれより後にする。
    pub async fn latest_queued_trust_observation_at(&self, key: &str) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT MAX(created_at) FROM cn_trust_observation_pending WHERE observation_key = ?",
        )
        .bind(key)
        .fetch_one(&self.pool)
        .await?)
    }

    /// 提供中の node の送信待ちへ置き、同じ key の古いものを置き換える。`base_url` を渡すと、その node だけへ置く。
    pub async fn queue_trust_observation(
        &self,
        base_url: Option<&str>,
        key: &str,
        envelope: &KukuriEnvelope,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO cn_trust_observation_pending
             (base_url, observation_key, created_at, envelope_id, envelope_json)
             SELECT base_url, ?, ?, ?, ? FROM cn_trust_observation_nodes
             WHERE enabled = 1 AND revocation_pending = 0 AND (? IS NULL OR base_url = ?)
             ON CONFLICT(base_url, observation_key) DO UPDATE SET
               created_at = excluded.created_at, envelope_id = excluded.envelope_id,
               envelope_json = excluded.envelope_json",
        )
        .bind(key)
        .bind(envelope.created_at)
        .bind(envelope.id.0.as_str())
        .bind(serde_json::to_string(envelope)?)
        .bind(base_url)
        .bind(base_url)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// node の送信待ちを、key の順に最大 `limit` 件。
    pub async fn queued_trust_observations(
        &self,
        base_url: &str,
        limit: usize,
    ) -> Result<Vec<(String, KukuriEnvelope)>> {
        sqlx::query(
            "SELECT observation_key, envelope_json FROM cn_trust_observation_pending
             WHERE base_url = ? ORDER BY observation_key LIMIT ?",
        )
        .bind(base_url)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            let envelope: String = row.try_get("envelope_json")?;
            Ok((
                row.try_get("observation_key")?,
                serde_json::from_str(&envelope)?,
            ))
        })
        .collect()
    }

    /// 送信待ちから外す。その間に新しい観測へ置き換わった key は残す。
    pub async fn dequeue_trust_observation(
        &self,
        base_url: &str,
        key: &str,
        envelope_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM cn_trust_observation_pending
             WHERE base_url = ? AND observation_key = ? AND envelope_id = ?",
        )
        .bind(base_url)
        .bind(key)
        .bind(envelope_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn count_queued_trust_observations(&self, base_url: &str) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM cn_trust_observation_pending WHERE base_url = ?",
        )
        .bind(base_url)
        .fetch_one(&self.pool)
        .await?)
    }
}
