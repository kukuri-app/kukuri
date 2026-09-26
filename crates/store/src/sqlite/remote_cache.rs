use super::*;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub const REMOTE_CACHE_CAPACITY_BYTES: i64 = 1024 * 1024 * 1024;
pub const REMOTE_CACHE_UNUSED_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const REMOTE_CACHE_RECLAIM_STEP: usize = 128;
const REMOTE_CACHE_TOUCH_INTERVAL_MS: i64 = 60 * 60 * 1000;

#[derive(Clone, Copy)]
enum CachePayload<'a> {
    Bytes(&'a [u8]),
    File { name: &'a str, bytes: u64 },
}

pub struct RemoteCacheReservation {
    counter: Arc<AtomicU64>,
    bytes: u64,
}

impl RemoteCacheReservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for RemoteCacheReservation {
    fn drop(&mut self) {
        self.counter.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// 保護参照の変更の途中。gate を持ったまま、行の変更と同じ transaction で参照を変える。
pub(super) struct ProtectedRefUpdate<'a> {
    _gate: tokio::sync::MutexGuard<'a, ()>,
    pub(super) tx: sqlx::Transaction<'static, Sqlite>,
    budget: i64,
    label_evictions: Vec<String>,
    removed_files: Vec<String>,
}

pub(super) fn now_ms() -> Result<i64> {
    Ok(i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?)
}

async fn delete_cache_item(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    kind: &str,
    key: &str,
    label_evictions: &mut Vec<String>,
    removed_files: &mut Vec<String>,
) -> Result<()> {
    let file_name = sqlx::query_scalar::<_, Option<String>>(
        "SELECT file_name FROM remote_content_cache WHERE kind = ?1 AND cache_key = ?2",
    )
    .bind(kind)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    if kind == "projection" {
        let hashes = sqlx::query_scalar::<_, String>(
            "SELECT blob_hash FROM remote_adult_media_hash_refs WHERE object_id = ?1",
        )
        .bind(key)
        .fetch_all(&mut **tx)
        .await?;
        sqlx::query("DELETE FROM remote_adult_media_hash_refs WHERE object_id = ?1")
            .bind(key)
            .execute(&mut **tx)
            .await?;
        for hash in hashes {
            let deleted = sqlx::query(
                "DELETE FROM adult_media_hashes WHERE blob_hash = ?1 AND is_protected = 0 \
                 AND NOT EXISTS (SELECT 1 FROM remote_adult_media_hash_refs WHERE blob_hash = ?1)",
            )
            .bind(&hash)
            .execute(&mut **tx)
            .await?;
            if deleted.rows_affected() != 0 {
                label_evictions.push(hash);
            }
        }
        sqlx::query("DELETE FROM object_thread_cache WHERE object_id = ?1")
            .bind(key)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM object_index_cache WHERE object_id = ?1")
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    let deleted =
        sqlx::query("DELETE FROM remote_content_cache WHERE kind = ?1 AND cache_key = ?2")
            .bind(kind)
            .bind(key)
            .execute(&mut **tx)
            .await?;
    if kind == "adult_marker" && deleted.rows_affected() != 0 {
        label_evictions.push(key.to_string());
    }
    if let Some(name) = file_name {
        removed_files.push(name);
    }
    Ok(())
}

impl SqliteStore {
    pub fn subscribe_adult_label_evictions(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.adult_label_evictions.subscribe()
    }

    pub(super) fn publish_adult_label_evictions(&self, hashes: Vec<String>) {
        for hash in hashes.into_iter().collect::<HashSet<_>>() {
            let _ = self.adult_label_evictions.send(hash);
        }
    }

    pub(super) async fn available_remote_projections(
        &self,
        mut rows: Vec<ObjectProjectionRow>,
    ) -> Result<Vec<ObjectProjectionRow>> {
        if rows.is_empty() {
            return Ok(rows);
        }
        let now = now_ms()?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT cache_key, last_used_at, is_protected FROM remote_content_cache \
             WHERE kind = 'projection' AND cache_key IN (",
        );
        let mut ids = query.separated(", ");
        for row in &rows {
            ids.push_bind(row.object_id.as_str());
        }
        ids.push_unseparated(")");
        let mut expired = HashSet::new();
        let mut touch_due = false;
        for row in query.build().fetch_all(&self.pool).await? {
            let last_used_at: i64 = row.get("last_used_at");
            if row.get::<i64, _>("is_protected") == 0
                && last_used_at <= now - REMOTE_CACHE_UNUSED_MS
            {
                expired.insert(row.get::<String, _>("cache_key"));
            } else if last_used_at <= now - REMOTE_CACHE_TOUCH_INTERVAL_MS {
                touch_due = true;
            }
        }
        rows.retain(|row| !expired.contains(row.object_id.as_str()));
        if rows.is_empty() || !touch_due {
            return Ok(rows);
        }
        let mut query =
            QueryBuilder::<Sqlite>::new("UPDATE remote_content_cache SET last_used_at = ");
        query.push_bind(now);
        query.push(" WHERE kind = 'projection' AND last_used_at <= ");
        query.push_bind(now - REMOTE_CACHE_TOUCH_INTERVAL_MS);
        query.push(" AND cache_key IN (");
        let mut ids = query.separated(", ");
        for row in &rows {
            ids.push_bind(row.object_id.as_str());
        }
        ids.push_unseparated(")");
        query.build().execute(&self.pool).await?;
        Ok(rows)
    }

    pub(super) async fn sync_remote_adult_hash_refs(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        row: &ObjectProjectionRow,
        label_evictions: &mut Vec<String>,
    ) -> Result<()> {
        let previous = sqlx::query_scalar::<_, String>(
            "SELECT blob_hash FROM remote_adult_media_hash_refs WHERE object_id = ?1",
        )
        .bind(row.object_id.as_str())
        .fetch_all(&mut **tx)
        .await?;
        sqlx::query("DELETE FROM remote_adult_media_hash_refs WHERE object_id = ?1")
            .bind(row.object_id.as_str())
            .execute(&mut **tx)
            .await?;
        for hash in adult_media_hashes_for_row(row)
            .into_iter()
            .collect::<HashSet<_>>()
        {
            sqlx::query(
                "INSERT INTO adult_media_hashes (blob_hash, marked_at, is_protected) \
                 VALUES (?1, ?2, 0) ON CONFLICT(blob_hash) DO NOTHING",
            )
            .bind(hash)
            .bind(row.derived_at)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                "INSERT INTO remote_adult_media_hash_refs (object_id, blob_hash) VALUES (?1, ?2)",
            )
            .bind(row.object_id.as_str())
            .bind(hash)
            .execute(&mut **tx)
            .await?;
        }
        for hash in previous {
            let deleted = sqlx::query(
                "DELETE FROM adult_media_hashes WHERE blob_hash = ?1 AND is_protected = 0 \
                 AND NOT EXISTS (SELECT 1 FROM remote_adult_media_hash_refs WHERE blob_hash = ?1)",
            )
            .bind(&hash)
            .execute(&mut **tx)
            .await?;
            if deleted.rows_affected() != 0 {
                label_evictions.push(hash);
            }
        }
        Ok(())
    }

    pub fn empty_remote_cache_reservation(&self) -> RemoteCacheReservation {
        RemoteCacheReservation {
            counter: self.remote_cache_reserved.clone(),
            bytes: 0,
        }
    }

    /// Reserve transfer bytes before appending the next bounded chunk in memory.
    /// The caller keeps the token until the fetch completes or is cancelled.
    pub async fn reserve_remote_cache_bytes(
        &self,
        reservation: &mut RemoteCacheReservation,
        bytes: u64,
    ) -> Result<bool> {
        anyhow::ensure!(
            Arc::ptr_eq(&reservation.counter, &self.remote_cache_reserved),
            "reservation belongs to another cache"
        );
        let _gate = self.remote_cache_gate.lock().await;
        let reserved = self.remote_cache_reserved.load(Ordering::Acquire);
        let target = reserved.saturating_add(bytes);
        if target > REMOTE_CACHE_CAPACITY_BYTES as u64 {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        let mut label_evictions = Vec::new();
        let mut removed_files = Vec::new();
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
            .execute(&mut *tx)
            .await?;
        let mut used = sqlx::query_scalar::<_, i64>(
            "SELECT used_bytes FROM remote_content_cache_usage WHERE id = 1",
        )
        .fetch_one(&mut *tx)
        .await?;
        let mut reclaimed = 0;
        while (used as u64).saturating_add(target) > REMOTE_CACHE_CAPACITY_BYTES as u64
            && reclaimed < REMOTE_CACHE_RECLAIM_STEP
        {
            let row = sqlx::query(
                "SELECT kind, cache_key, charged_bytes FROM remote_content_cache \
                 WHERE is_protected = 0 ORDER BY last_used_at, kind, cache_key LIMIT 1",
            )
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = row else { break };
            delete_cache_item(
                &mut tx,
                &row.get::<String, _>("kind"),
                &row.get::<String, _>("cache_key"),
                &mut label_evictions,
                &mut removed_files,
            )
            .await?;
            used -= row.get::<i64, _>("charged_bytes");
            reclaimed += 1;
        }
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = ?1 WHERE id = 1")
            .bind(used)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.publish_adult_label_evictions(label_evictions);
        self.remove_remote_blob_files(removed_files).await?;
        if (used as u64).saturating_add(target) > REMOTE_CACHE_CAPACITY_BYTES as u64 {
            return Ok(false);
        }
        self.remote_cache_reserved
            .fetch_add(bytes, Ordering::AcqRel);
        reservation.bytes += bytes;
        Ok(true)
    }

    /// One background pass only; callers reschedule while a full page remains.
    pub async fn reclaim_remote_cache_step(&self) -> Result<usize> {
        let _gate = self.remote_cache_gate.lock().await;
        let mut tx = self.pool.begin().await?;
        let mut label_evictions = Vec::new();
        let mut removed_files = Vec::new();
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(
            "SELECT kind, cache_key, charged_bytes FROM remote_content_cache \
             WHERE is_protected = 0 AND last_used_at <= ?1 \
             ORDER BY last_used_at, kind, cache_key LIMIT ?2",
        )
        .bind(now_ms()? - REMOTE_CACHE_UNUSED_MS)
        .bind(i64::try_from(REMOTE_CACHE_RECLAIM_STEP)?)
        .fetch_all(&mut *tx)
        .await?;
        let count = rows.len();
        let mut reclaimed_bytes = 0;
        for row in rows {
            delete_cache_item(
                &mut tx,
                &row.get::<String, _>("kind"),
                &row.get::<String, _>("cache_key"),
                &mut label_evictions,
                &mut removed_files,
            )
            .await?;
            reclaimed_bytes += row.get::<i64, _>("charged_bytes");
        }
        sqlx::query(
            "UPDATE remote_content_cache_usage SET used_bytes = used_bytes - ?1 WHERE id = 1",
        )
        .bind(reclaimed_bytes)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.publish_adult_label_evictions(label_evictions);
        self.remove_remote_blob_files(removed_files).await?;
        Ok(count)
    }

    pub(super) async fn charge_remote_projection(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        row: &ObjectProjectionRow,
        budget: i64,
        label_evictions: &mut Vec<String>,
        removed_files: &mut Vec<String>,
    ) -> Result<bool> {
        let charge = i64::try_from(serde_json::to_vec(row)?.len())? * 2 + 512;
        self.put_remote_content_in_tx(
            tx,
            "projection",
            row.object_id.as_str(),
            row.source_replica_id.as_str(),
            CachePayload::Bytes(&[]),
            None,
            None,
            budget,
            now_ms()?,
            Some(charge),
            label_evictions,
            removed_files,
        )
        .await
    }

    /// Store one remote record or blob. A false result means the bounded reclaim
    /// step could not admit it without exceeding the cache budget.
    pub async fn put_remote_content(
        &self,
        kind: &str,
        key: &str,
        scope: &str,
        payload: &[u8],
    ) -> Result<bool> {
        self.put_remote_content_with_budget(
            kind,
            key,
            scope,
            CachePayload::Bytes(payload),
            None,
            None,
            REMOTE_CACHE_CAPACITY_BYTES,
            now_ms()?,
        )
        .await
    }

    /// record を置く cache の key(保護参照もこの key を指す)。
    pub fn remote_record_cache_key(replica: &str, key: &str, author: &str) -> String {
        format!("{replica}\0{key}\0{author}")
    }

    pub async fn put_remote_record(
        &self,
        replica: &str,
        key: &str,
        author: &str,
        payload: &[u8],
    ) -> Result<bool> {
        let cache_key = Self::remote_record_cache_key(replica, key, author);
        self.put_remote_content_with_budget(
            "record",
            &cache_key,
            replica,
            CachePayload::Bytes(payload),
            Some(key),
            Some(author),
            REMOTE_CACHE_CAPACITY_BYTES,
            now_ms()?,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn put_remote_content_with_budget(
        &self,
        kind: &str,
        key: &str,
        scope: &str,
        payload: CachePayload<'_>,
        record_key: Option<&str>,
        record_author: Option<&str>,
        budget: i64,
        now: i64,
    ) -> Result<bool> {
        let _gate = self.remote_cache_gate.lock().await;
        let reserved = i64::try_from(self.remote_cache_reserved.load(Ordering::Acquire))?;
        let mut tx = self.pool.begin().await?;
        let mut label_evictions = Vec::new();
        let mut removed_files = Vec::new();
        let admitted = self
            .put_remote_content_in_tx(
                &mut tx,
                kind,
                key,
                scope,
                payload,
                record_key,
                record_author,
                budget.saturating_sub(reserved),
                now,
                None,
                &mut label_evictions,
                &mut removed_files,
            )
            .await?;
        tx.commit().await?;
        self.publish_adult_label_evictions(label_evictions);
        self.remove_remote_blob_files(removed_files).await?;
        Ok(admitted)
    }

    #[allow(clippy::too_many_arguments)]
    async fn put_remote_content_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        kind: &str,
        key: &str,
        scope: &str,
        payload: CachePayload<'_>,
        record_key: Option<&str>,
        record_author: Option<&str>,
        budget: i64,
        now: i64,
        charged_bytes: Option<i64>,
        label_evictions: &mut Vec<String>,
        removed_files: &mut Vec<String>,
    ) -> Result<bool> {
        anyhow::ensure!(
            matches!(kind, "blob" | "record" | "projection" | "adult_marker"),
            "unknown remote cache kind"
        );
        let payload_len = match payload {
            CachePayload::Bytes(bytes) => bytes.len() as u64,
            CachePayload::File { bytes, .. } => bytes,
        };
        let charge = charged_bytes.unwrap_or(
            i64::try_from(payload_len)? + i64::try_from(kind.len() + key.len() + scope.len())? + 64,
        );
        // The ledger row is the write lock shared by all cache writers.
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
            .execute(&mut **tx)
            .await?;
        let protected = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM remote_content_cache_protected_ref WHERE kind = ?1 AND cache_key = ?2)",
        )
        .bind(kind)
        .bind(key)
        .fetch_one(&mut **tx)
        .await?
            != 0;
        if !protected && charge > budget {
            return Ok(false);
        }
        let old = sqlx::query(
            "SELECT charged_bytes, is_protected, file_name FROM remote_content_cache WHERE kind = ?1 AND cache_key = ?2",
        )
        .bind(kind)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?;
        let old_unprotected = old
            .as_ref()
            .filter(|row| row.get::<i64, _>("is_protected") == 0)
            .map_or(0, |row| row.get::<i64, _>("charged_bytes"));
        let used = sqlx::query_scalar::<_, i64>(
            "SELECT used_bytes FROM remote_content_cache_usage WHERE id = 1",
        )
        .fetch_one(&mut **tx)
        .await?;
        let mut next_used = used - old_unprotected + if protected { 0 } else { charge };
        let mut reclaimed = 0;
        while reclaimed < REMOTE_CACHE_RECLAIM_STEP {
            let expired = sqlx::query(
                "SELECT kind, cache_key, charged_bytes FROM remote_content_cache \
                 WHERE is_protected = 0 AND last_used_at <= ?1 \
                 AND NOT (kind = ?2 AND cache_key = ?3) \
                 ORDER BY last_used_at, kind, cache_key LIMIT 1",
            )
            .bind(now - REMOTE_CACHE_UNUSED_MS)
            .bind(kind)
            .bind(key)
            .fetch_optional(&mut **tx)
            .await?;
            let victim = if let Some(row) = expired {
                Some(row)
            } else if next_used > budget {
                sqlx::query(
                    "SELECT kind, cache_key, charged_bytes FROM remote_content_cache \
                     WHERE is_protected = 0 AND NOT (kind = ?1 AND cache_key = ?2) \
                     ORDER BY last_used_at, kind, cache_key LIMIT 1",
                )
                .bind(kind)
                .bind(key)
                .fetch_optional(&mut **tx)
                .await?
            } else {
                None
            };
            let Some(victim) = victim else { break };
            let victim_kind: String = victim.get("kind");
            let victim_key: String = victim.get("cache_key");
            delete_cache_item(
                tx,
                &victim_kind,
                &victim_key,
                label_evictions,
                removed_files,
            )
            .await?;
            next_used -= victim.get::<i64, _>("charged_bytes");
            reclaimed += 1;
        }
        if next_used > budget {
            sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = ?1 WHERE id = 1")
                .bind(next_used - if protected { 0 } else { charge } + old_unprotected)
                .execute(&mut **tx)
                .await?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO remote_content_cache \
             (kind, cache_key, scope_key, record_key, record_author, payload, file_name, charged_bytes, is_protected, last_used_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(kind, cache_key) DO UPDATE SET \
             scope_key = excluded.scope_key, record_key = excluded.record_key, \
             record_author = excluded.record_author, payload = excluded.payload, file_name = excluded.file_name, \
             charged_bytes = excluded.charged_bytes, is_protected = excluded.is_protected, \
             last_used_at = excluded.last_used_at",
        )
        .bind(kind)
        .bind(key)
        .bind(scope)
        .bind(record_key)
        .bind(record_author)
        .bind(match payload {
            CachePayload::Bytes(bytes) => Some(bytes),
            CachePayload::File { .. } => None,
        })
        .bind(match payload {
            CachePayload::Bytes(_) => None,
            CachePayload::File { name, .. } => Some(name),
        })
        .bind(charge)
        .bind(if protected { 1 } else { 0 })
        .bind(now)
        .execute(&mut **tx)
        .await?;
        if let Some(name) = old
            .as_ref()
            .and_then(|row| row.get::<Option<String>, _>("file_name"))
            && !matches!(payload, CachePayload::File { name: current, .. } if current == name)
        {
            removed_files.push(name);
        }
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = ?1 WHERE id = 1")
            .bind(next_used)
            .execute(&mut **tx)
            .await?;
        Ok(true)
    }

    pub async fn get_remote_content(&self, kind: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let now = now_ms()?;
        let row = sqlx::query(
            "SELECT payload, file_name, last_used_at FROM remote_content_cache \
             WHERE kind = ?1 AND cache_key = ?2 \
             AND (is_protected = 1 OR last_used_at > ?3)",
        )
        .bind(kind)
        .bind(key)
        .bind(now - REMOTE_CACHE_UNUSED_MS)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        if row.get::<i64, _>("last_used_at") <= now - REMOTE_CACHE_TOUCH_INTERVAL_MS {
            self.touch_remote_content(kind, key, now).await?;
        }
        if let Some(name) = row.get::<Option<String>, _>("file_name") {
            return Ok(tokio::fs::read(self.remote_blob_path(&name)?).await.ok());
        }
        Ok(Some(row.get("payload")))
    }

    async fn touch_remote_content(&self, kind: &str, key: &str, now: i64) -> Result<()> {
        sqlx::query(
            "UPDATE remote_content_cache SET last_used_at = ?1 WHERE kind = ?2 AND cache_key = ?3 \
             AND last_used_at <= ?4",
        )
        .bind(now)
        .bind(kind)
        .bind(key)
        .bind(now - REMOTE_CACHE_TOUCH_INTERVAL_MS)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_remote_records(
        &self,
        replica: &str,
        key: &str,
        author: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Vec<u8>>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        anyhow::ensure!(limit <= 8, "remote record cache limit exceeded");
        let now = now_ms()?;
        let rows = if let Some(author) = author {
            sqlx::query(
                "SELECT cache_key, payload, is_protected, last_used_at FROM remote_content_cache \
                 WHERE kind = 'record' AND scope_key = ?1 AND record_key = ?2 \
                   AND record_author = ?3 LIMIT 1",
            )
            .bind(replica)
            .bind(key)
            .bind(author)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT cache_key, payload, is_protected, last_used_at FROM remote_content_cache \
                 WHERE kind = 'record' AND scope_key = ?1 AND record_key = ?2 \
                 ORDER BY record_author LIMIT ?3",
            )
            .bind(replica)
            .bind(key)
            .bind(i64::try_from(limit)?)
            .fetch_all(&self.pool)
            .await?
        };
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            if row.get::<i64, _>("is_protected") == 0
                && row.get::<i64, _>("last_used_at") <= now - REMOTE_CACHE_UNUSED_MS
            {
                continue;
            }
            if row.get::<i64, _>("last_used_at") <= now - REMOTE_CACHE_TOUCH_INTERVAL_MS {
                self.touch_remote_content("record", &row.get::<String, _>("cache_key"), now)
                    .await?;
            }
            result.push(row.get("payload"));
        }
        Ok(result)
    }

    pub async fn has_remote_content(&self, kind: &str, key: &str) -> Result<bool> {
        let now = now_ms()?;
        let row = sqlx::query(
            "SELECT last_used_at, is_protected, file_name FROM remote_content_cache \
             WHERE kind = ?1 AND cache_key = ?2",
        )
        .bind(kind)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(false) };
        if let Some(name) = row.get::<Option<String>, _>("file_name")
            && tokio::fs::metadata(self.remote_blob_path(&name)?)
                .await
                .is_err()
        {
            return Ok(false);
        }
        Ok(row.get::<i64, _>("is_protected") != 0
            || row.get::<i64, _>("last_used_at") > now - REMOTE_CACHE_UNUSED_MS)
    }

    pub async fn remote_content_len(&self, kind: &str, key: &str) -> Result<Option<u64>> {
        let now = now_ms()?;
        let row = sqlx::query(
            "SELECT length(payload) AS bytes, file_name, last_used_at FROM remote_content_cache \
             WHERE kind = ?1 AND cache_key = ?2 \
             AND (is_protected = 1 OR last_used_at > ?3)",
        )
        .bind(kind)
        .bind(key)
        .bind(now - REMOTE_CACHE_UNUSED_MS)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = row {
            if row.get::<i64, _>("last_used_at") <= now - REMOTE_CACHE_TOUCH_INTERVAL_MS {
                self.touch_remote_content(kind, key, now).await?;
            }
            if let Some(name) = row.get::<Option<String>, _>("file_name") {
                return Ok(tokio::fs::metadata(self.remote_blob_path(&name)?)
                    .await
                    .ok()
                    .map(|metadata| metadata.len()));
            }
            Ok(Some(u64::try_from(row.get::<i64, _>("bytes"))?))
        } else {
            Ok(None)
        }
    }

    pub async fn remote_content_chunk(
        &self,
        kind: &str,
        key: &str,
        offset: u64,
        limit: usize,
    ) -> Result<Option<Vec<u8>>> {
        anyhow::ensure!(limit <= 1024 * 1024, "remote cache chunk limit exceeded");
        let row = sqlx::query(
            "SELECT substr(payload, ?3, ?4) AS chunk, file_name FROM remote_content_cache \
             WHERE kind = ?1 AND cache_key = ?2",
        )
        .bind(kind)
        .bind(key)
        .bind(i64::try_from(offset)? + 1)
        .bind(i64::try_from(limit)?)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        if let Some(name) = row.get::<Option<String>, _>("file_name") {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};
            let Ok(mut file) = tokio::fs::File::open(self.remote_blob_path(&name)?).await else {
                return Ok(None);
            };
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            let mut chunk = vec![0; limit];
            let read = file.read(&mut chunk).await?;
            chunk.truncate(read);
            return Ok(Some(chunk));
        }
        Ok(Some(row.get("chunk")))
    }

    async fn add_remote_protected_ref(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        kind: &str,
        key: &str,
        reference: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
            .execute(&mut **tx)
            .await?;
        sqlx::query(
            "INSERT OR IGNORE INTO remote_content_cache_protected_ref (kind, cache_key, ref_id) \
             VALUES (?1, ?2, ?3)",
        )
        .bind(kind)
        .bind(key)
        .bind(reference)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "UPDATE remote_content_cache_usage SET used_bytes = used_bytes - \
             COALESCE((SELECT charged_bytes FROM remote_content_cache \
             WHERE kind = ?1 AND cache_key = ?2 AND is_protected = 0), 0) WHERE id = 1",
        )
        .bind(kind)
        .bind(key)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "UPDATE remote_content_cache SET is_protected = 1 WHERE kind = ?1 AND cache_key = ?2",
        )
        .bind(kind)
        .bind(key)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// 保護参照の変更を、呼出し元の行の変更と同じ transaction で行う。cache の書込みと直列にする。
    pub(super) async fn begin_protected_ref_update(&self) -> Result<ProtectedRefUpdate<'_>> {
        let gate = self.remote_cache_gate.lock().await;
        let budget = REMOTE_CACHE_CAPACITY_BYTES.saturating_sub(i64::try_from(
            self.remote_cache_reserved.load(Ordering::Acquire),
        )?);
        Ok(ProtectedRefUpdate {
            _gate: gate,
            tx: self.pool.begin().await?,
            budget,
            label_evictions: Vec::new(),
            removed_files: Vec::new(),
        })
    }

    /// `reference` の保護参照を `desired` に置き換える。外れた内容は容量の内なら非保護へ戻し、超えるなら消す。
    pub(super) async fn set_refs_in(
        &self,
        update: &mut ProtectedRefUpdate<'_>,
        reference: &str,
        desired: &[(String, String)],
    ) -> Result<()> {
        self.replace_remote_protected_refs(
            &mut update.tx,
            reference,
            desired,
            update.budget,
            &mut update.label_evictions,
            &mut update.removed_files,
        )
        .await
    }

    pub(super) async fn commit_protected_ref_update(
        &self,
        update: ProtectedRefUpdate<'_>,
    ) -> Result<()> {
        update.tx.commit().await?;
        self.publish_adult_label_evictions(update.label_evictions);
        self.remove_remote_blob_files(update.removed_files).await
    }

    pub(super) async fn replace_remote_protected_refs(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        reference: &str,
        desired: &[(String, String)],
        budget: i64,
        label_evictions: &mut Vec<String>,
        removed_files: &mut Vec<String>,
    ) -> Result<()> {
        let old = sqlx::query(
            "SELECT kind, cache_key FROM remote_content_cache_protected_ref WHERE ref_id = ?1",
        )
        .bind(reference)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("kind"),
                row.get::<String, _>("cache_key"),
            )
        })
        .collect::<HashSet<_>>();
        let desired = desired.iter().cloned().collect::<HashSet<_>>();
        for (kind, key) in old.difference(&desired) {
            sqlx::query(
                "DELETE FROM remote_content_cache_protected_ref \
                 WHERE kind = ?1 AND cache_key = ?2 AND ref_id = ?3",
            )
            .bind(kind)
            .bind(key)
            .bind(reference)
            .execute(&mut **tx)
            .await?;
            let remains = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM remote_content_cache_protected_ref \
                 WHERE kind = ?1 AND cache_key = ?2)",
            )
            .bind(kind)
            .bind(key)
            .fetch_one(&mut **tx)
            .await?;
            if remains != 0 {
                continue;
            }
            let row = sqlx::query(
                "SELECT charged_bytes FROM remote_content_cache \
                 WHERE kind = ?1 AND cache_key = ?2 AND is_protected = 1",
            )
            .bind(kind)
            .bind(key)
            .fetch_optional(&mut **tx)
            .await?;
            let Some(row) = row else { continue };
            let charge = row.get::<i64, _>("charged_bytes");
            let used = sqlx::query_scalar::<_, i64>(
                "SELECT used_bytes FROM remote_content_cache_usage WHERE id = 1",
            )
            .fetch_one(&mut **tx)
            .await?;
            if used.saturating_add(charge) <= budget {
                sqlx::query(
                    "UPDATE remote_content_cache SET is_protected = 0 WHERE kind = ?1 AND cache_key = ?2",
                )
                .bind(kind)
                .bind(key)
                .execute(&mut **tx)
                .await?;
                sqlx::query(
                    "UPDATE remote_content_cache_usage SET used_bytes = used_bytes + ?1 WHERE id = 1",
                )
                .bind(charge)
                .execute(&mut **tx)
                .await?;
            } else {
                delete_cache_item(tx, kind, key, label_evictions, removed_files).await?;
            }
        }
        for (kind, key) in desired.difference(&old) {
            Self::add_remote_protected_ref(tx, kind, key, reference).await?;
        }
        Ok(())
    }

    pub async fn remove_remote_content(&self, kind: &str, key: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let mut label_evictions = Vec::new();
        let mut removed_files = Vec::new();
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query(
            "SELECT charged_bytes, is_protected FROM remote_content_cache WHERE kind = ?1 AND cache_key = ?2",
        )
        .bind(kind)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = row
            && row.get::<i64, _>("is_protected") == 0
        {
            sqlx::query(
                "UPDATE remote_content_cache_usage SET used_bytes = used_bytes - ?1 WHERE id = 1",
            )
            .bind(row.get::<i64, _>("charged_bytes"))
            .execute(&mut *tx)
            .await?;
            delete_cache_item(&mut tx, kind, key, &mut label_evictions, &mut removed_files).await?;
        }
        tx.commit().await?;
        self.publish_adult_label_evictions(label_evictions);
        self.remove_remote_blob_files(removed_files).await?;
        Ok(())
    }
}

mod files;

#[cfg(test)]
mod tests;
