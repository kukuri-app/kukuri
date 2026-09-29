//! 非利用の回収と、表示設定 OFF での成人向けの blob の削除(#1419)。どちらも 1 回 `REMOTE_CACHE_RECLAIM_STEP` 件まで。

use super::*;

/// 表示設定 ON の間に置いた成人向けの blob の `scope_key`(#1419)。
const REMOTE_ADULT_BLOB_SCOPE: &str = "adult";

impl SqliteStore {
    /// One background pass only; callers reschedule while a full page remains.
    pub async fn reclaim_remote_cache_step(&self) -> Result<usize> {
        self.delete_remote_cache_step(false).await
    }

    /// 成人向けとして置いた blob に印を付ける(#1419)。表示設定を OFF に戻したときに
    /// `forget_adult_remote_blobs_step` が消す。
    pub async fn mark_remote_blob_adult(&self, hash: &str) -> Result<()> {
        sqlx::query(
            "UPDATE remote_content_cache SET scope_key = ?1 WHERE kind = 'blob' AND cache_key = ?2",
        )
        .bind(REMOTE_ADULT_BLOB_SCOPE)
        .bind(hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 印の付いた成人向けの blob を 1 回 `REMOTE_CACHE_RECLAIM_STEP` 件まで消す(#1419)。索引で選ぶので
    /// cache の総件数によらない。消した件数を返し、呼出元は 0 になるまで繰り返す。
    pub async fn forget_adult_remote_blobs_step(&self) -> Result<usize> {
        self.delete_remote_cache_step(true).await
    }

    async fn delete_remote_cache_step(&self, adult: bool) -> Result<usize> {
        let _gate = self.remote_cache_gate.lock().await;
        let mut tx = self.pool.begin().await?;
        let mut label_evictions = Vec::new();
        let mut removed_files = Vec::new();
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
            .execute(&mut *tx)
            .await?;
        let step = i64::try_from(REMOTE_CACHE_RECLAIM_STEP)?;
        let rows = if adult {
            sqlx::query(
                "SELECT kind, cache_key, charged_bytes FROM remote_content_cache \
                 WHERE kind = 'blob' AND scope_key = ?1 AND is_protected = 0 LIMIT ?2",
            )
            .bind(REMOTE_ADULT_BLOB_SCOPE)
            .bind(step)
            .fetch_all(&mut *tx)
            .await?
        } else {
            sqlx::query(
                "SELECT kind, cache_key, charged_bytes FROM remote_content_cache \
                 WHERE is_protected = 0 AND last_used_at <= ?1 \
                 ORDER BY last_used_at, kind, cache_key LIMIT ?2",
            )
            .bind(now_ms()? - REMOTE_CACHE_UNUSED_MS)
            .bind(step)
            .fetch_all(&mut *tx)
            .await?
        };
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
}
