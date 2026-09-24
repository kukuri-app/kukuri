use anyhow::{Result, ensure};
use sqlx::Row;

use super::SqliteStore;

const LEARNED_SOURCE: &str = "learned";
const LEARNED_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
const LEARNED_BUDGET_BYTES: i64 = 64 * 1024 * 1024;
const MAX_ADDR_BYTES: usize = 4 * 1024;

impl SqliteStore {
    /// The account database owns the candidate history. Only learned rows are
    /// time/size evicted; explicit tickets and configured seeds are user input.
    pub async fn put_peer_candidate(
        &self,
        scope: &str,
        source: &str,
        endpoint_id: &str,
        addr: &[u8],
        now_ms: i64,
    ) -> Result<bool> {
        self.put_peer_candidate_bounded(
            scope,
            source,
            endpoint_id,
            addr,
            now_ms,
            LEARNED_BUDGET_BYTES,
        )
        .await
    }

    async fn put_peer_candidate_bounded(
        &self,
        scope: &str,
        source: &str,
        endpoint_id: &str,
        addr: &[u8],
        now_ms: i64,
        budget_bytes: i64,
    ) -> Result<bool> {
        ensure!(
            addr.len() <= MAX_ADDR_BYTES,
            "peer address exceeds candidate budget"
        );
        let bytes = (addr.len() + scope.len() + source.len() + endpoint_id.len() + 64) as i64;
        let mut tx = self.pool.begin().await?;
        let previous = sqlx::query(
            "SELECT endpoint_addr, accounted_bytes FROM peer_candidates \
             WHERE scope = ? AND source = ? AND endpoint_id = ?",
        )
        .bind(scope)
        .bind(source)
        .bind(endpoint_id)
        .fetch_optional(&mut *tx)
        .await?;
        let old_bytes = previous
            .as_ref()
            .map(|row| row.get::<i64, _>("accounted_bytes"))
            .unwrap_or(0);
        let changed = previous
            .as_ref()
            .is_none_or(|row| row.get::<Vec<u8>, _>("endpoint_addr") != addr);
        sqlx::query(
            "INSERT INTO peer_candidates \
             (scope, source, endpoint_id, endpoint_addr, accounted_bytes, seen_ms) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(scope, source, endpoint_id) DO UPDATE SET \
             endpoint_addr = excluded.endpoint_addr, \
             accounted_bytes = excluded.accounted_bytes, seen_ms = excluded.seen_ms",
        )
        .bind(scope)
        .bind(source)
        .bind(endpoint_id)
        .bind(addr)
        .bind(bytes)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
        if source == LEARNED_SOURCE {
            sqlx::query(
                "UPDATE peer_candidate_budget SET learned_bytes = learned_bytes + ? WHERE id = 1",
            )
            .bind(bytes - old_bytes)
            .execute(&mut *tx)
            .await?;
            prune_learned(&mut tx, now_ms).await?;
            loop {
                let total: i64 = sqlx::query_scalar(
                    "SELECT learned_bytes FROM peer_candidate_budget WHERE id = 1",
                )
                .fetch_one(&mut *tx)
                .await?;
                if total <= budget_bytes {
                    break;
                }
                let row = sqlx::query(
                    "DELETE FROM peer_candidates WHERE rowid IN \
                     (SELECT rowid FROM peer_candidates WHERE source = 'learned' \
                      ORDER BY seen_ms, endpoint_id LIMIT 1) RETURNING accounted_bytes",
                )
                .fetch_one(&mut *tx)
                .await?;
                sqlx::query("UPDATE peer_candidate_budget SET learned_bytes = learned_bytes - ? WHERE id = 1")
                    .bind(row.get::<i64, _>("accounted_bytes"))
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn peer_candidate_window(
        &self,
        scope: &str,
        source: &str,
        after: Option<(i64, String)>,
        limit: usize,
        now_ms: i64,
    ) -> Result<Vec<(String, Vec<u8>, i64)>> {
        let limit = limit.min(64);
        if source == LEARNED_SOURCE {
            let oldest: Option<i64> = sqlx::query_scalar(
                "SELECT seen_ms FROM peer_candidates WHERE source = 'learned' ORDER BY seen_ms LIMIT 1",
            )
            .fetch_optional(&self.pool)
            .await?;
            if oldest.is_some_and(|seen| seen < now_ms.saturating_sub(LEARNED_RETENTION_MS)) {
                let mut tx = self.pool.begin().await?;
                prune_learned(&mut tx, now_ms).await?;
                tx.commit().await?;
            }
        }
        let cutoff = if source == LEARNED_SOURCE {
            now_ms.saturating_sub(LEARNED_RETENTION_MS)
        } else {
            i64::MIN
        };
        let (after_ms, after_id) = after.clone().unwrap_or((i64::MIN, String::new()));
        let mut rows = sqlx::query(
            "SELECT endpoint_id, endpoint_addr, seen_ms FROM peer_candidates \
             WHERE scope = ? AND source = ? AND seen_ms >= ? \
             AND (seen_ms > ? OR (seen_ms = ? AND endpoint_id > ?)) \
             ORDER BY seen_ms, endpoint_id LIMIT ?",
        )
        .bind(scope)
        .bind(source)
        .bind(cutoff)
        .bind(after_ms)
        .bind(after_ms)
        .bind(after_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        if rows.len() < limit
            && let Some((after_ms, after_id)) = after
        {
            rows.extend(
                sqlx::query(
                    "SELECT endpoint_id, endpoint_addr, seen_ms FROM peer_candidates \
                     WHERE scope = ? AND source = ? AND seen_ms >= ? \
                     AND (seen_ms < ? OR (seen_ms = ? AND endpoint_id <= ?)) \
                     ORDER BY seen_ms, endpoint_id LIMIT ?",
                )
                .bind(scope)
                .bind(source)
                .bind(cutoff)
                .bind(after_ms)
                .bind(after_ms)
                .bind(after_id)
                .bind((limit - rows.len()) as i64)
                .fetch_all(&self.pool)
                .await?,
            );
        }
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.get("endpoint_id"),
                    row.get("endpoint_addr"),
                    row.get("seen_ms"),
                )
            })
            .collect())
    }

    pub async fn peer_candidate_by_id(
        &self,
        scope: &str,
        source: &str,
        endpoint_id: &str,
        now_ms: i64,
    ) -> Result<Option<Vec<u8>>> {
        let cutoff = if source == LEARNED_SOURCE {
            now_ms.saturating_sub(LEARNED_RETENTION_MS)
        } else {
            i64::MIN
        };
        Ok(sqlx::query_scalar(
            "SELECT endpoint_addr FROM peer_candidates \
             WHERE scope = ? AND source = ? AND endpoint_id = ? AND seen_ms >= ?",
        )
        .bind(scope)
        .bind(source)
        .bind(endpoint_id)
        .bind(cutoff)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn replace_seed_candidates(
        &self,
        scope: &str,
        seeds: Vec<(String, Vec<u8>)>,
        now_ms: i64,
    ) -> Result<()> {
        let digest = blake3::hash(&serde_json::to_vec(&seeds)?);
        let mut tx = self.pool.begin().await?;
        let previous: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT digest FROM peer_seed_state WHERE scope = ?")
                .bind(scope)
                .fetch_optional(&mut *tx)
                .await?;
        if previous.as_deref() == Some(digest.as_bytes()) {
            return Ok(());
        }
        sqlx::query("DELETE FROM peer_candidates WHERE scope = ? AND source = 'seed'")
            .bind(scope)
            .execute(&mut *tx)
            .await?;
        for (id, addr) in seeds {
            ensure!(
                addr.len() <= MAX_ADDR_BYTES,
                "peer address exceeds candidate budget"
            );
            sqlx::query(
                "INSERT INTO peer_candidates \
                 (scope, source, endpoint_id, endpoint_addr, accounted_bytes, seen_ms) \
                 VALUES (?, 'seed', ?, ?, 0, ?)",
            )
            .bind(scope)
            .bind(id)
            .bind(addr)
            .bind(now_ms)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO peer_seed_state(scope, digest) VALUES (?, ?) \
             ON CONFLICT(scope) DO UPDATE SET digest = excluded.digest",
        )
        .bind(scope)
        .bind(digest.as_bytes().as_slice())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn prune_learned(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, now_ms: i64) -> Result<()> {
    let rows = sqlx::query(
        "DELETE FROM peer_candidates WHERE rowid IN \
         (SELECT rowid FROM peer_candidates WHERE source = 'learned' AND seen_ms < ? \
          ORDER BY seen_ms LIMIT 64) RETURNING accounted_bytes",
    )
    .bind(now_ms.saturating_sub(LEARNED_RETENTION_MS))
    .fetch_all(&mut **tx)
    .await?;
    let freed: i64 = rows
        .iter()
        .map(|row| row.get::<i64, _>("accounted_bytes"))
        .sum();
    if freed > 0 {
        sqlx::query(
            "UPDATE peer_candidate_budget SET learned_bytes = learned_bytes - ? WHERE id = 1",
        )
        .bind(freed)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn learned_candidates_expire_but_explicit_tickets_remain() -> Result<()> {
        let store = SqliteStore::connect_memory().await?;
        store
            .put_peer_candidate("docs", "learned", "old", b"old", 0)
            .await?;
        store
            .put_peer_candidate("docs", "imported", "ticket", b"ticket", 0)
            .await?;
        let now = LEARNED_RETENTION_MS + 1;
        assert!(
            store
                .peer_candidate_window("docs", "learned", None, 4, now)
                .await?
                .is_empty()
        );
        assert_eq!(
            store
                .peer_candidate_window("docs", "imported", None, 4, now)
                .await?
                .len(),
            1
        );
        let retained: i64 =
            sqlx::query_scalar("SELECT learned_bytes FROM peer_candidate_budget WHERE id = 1")
                .fetch_one(&store.pool)
                .await?;
        assert_eq!(retained, 0);
        Ok(())
    }

    #[tokio::test]
    async fn candidate_window_wraps_without_materializing_history() -> Result<()> {
        let store = SqliteStore::connect_memory().await?;
        for index in 0..100 {
            store
                .put_peer_candidate(
                    "docs",
                    "learned",
                    &format!("peer-{index:03}"),
                    b"address",
                    index,
                )
                .await?;
        }
        let mut cursor = None;
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..25 {
            let page = store
                .peer_candidate_window("docs", "learned", cursor.clone(), 4, 100)
                .await?;
            assert_eq!(page.len(), 4);
            cursor = page.last().map(|(id, _, seen)| (*seen, id.clone()));
            seen.extend(page.into_iter().map(|(id, _, _)| id));
        }
        assert_eq!(seen.len(), 100);
        assert_eq!(
            store
                .peer_candidate_window("docs", "learned", cursor, 4, 100)
                .await?[0]
                .0,
            "peer-000"
        );
        Ok(())
    }

    #[tokio::test]
    async fn unchanged_seed_config_does_not_rewrite_the_ledger() -> Result<()> {
        let store = SqliteStore::connect_memory().await?;
        let seeds = vec![("seed".to_string(), b"addr".to_vec())];
        store
            .replace_seed_candidates("docs", seeds.clone(), 1)
            .await?;
        store.replace_seed_candidates("docs", seeds, 2).await?;
        let seen: i64 = sqlx::query_scalar(
            "SELECT seen_ms FROM peer_candidates WHERE scope = 'docs' AND source = 'seed'",
        )
        .fetch_one(&store.pool)
        .await?;
        assert_eq!(seen, 1);
        Ok(())
    }

    #[tokio::test]
    async fn learned_budget_evicts_oldest_rows_before_returning() -> Result<()> {
        let store = SqliteStore::connect_memory().await?;
        for index in 0..100 {
            store
                .put_peer_candidate_bounded(
                    "docs",
                    "learned",
                    &format!("peer-{index:03}"),
                    b"address",
                    index,
                    1_000,
                )
                .await?;
        }
        let retained: i64 =
            sqlx::query_scalar("SELECT learned_bytes FROM peer_candidate_budget WHERE id = 1")
                .fetch_one(&store.pool)
                .await?;
        assert!(retained <= 1_000);
        assert!(
            store
                .peer_candidate_by_id("docs", "learned", "peer-000", 100)
                .await?
                .is_none()
        );
        assert!(
            store
                .peer_candidate_by_id("docs", "learned", "peer-099", 100)
                .await?
                .is_some()
        );
        Ok(())
    }
}
