//! 保持する公開 topic の日の保護参照(#1909 AC-3)。保護参照 `retain:<topic>:<bucket>` を付けた record と blob の
//! 大きさを、topic と日ごとの合計(`topic_retention_days`)と全体の合計(`topic_retention_limits`)に、付けたとき・内容が
//! 書き換わったとき・外したときに足し引きする(毎回の全件の合計は取らない)。

use super::*;
use crate::topic_retention::retain_reference;

impl SqliteStore {
    /// 保護参照を 1 件外す。最後の参照なら、内容を容量の内なら非保護へ戻し、超えるなら消す。
    pub(in crate::sqlite) async fn remove_remote_protected_ref(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        kind: &str,
        key: &str,
        reference: &str,
        budget: i64,
        label_evictions: &mut Vec<String>,
        removed_files: &mut Vec<String>,
    ) -> Result<()> {
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
            return Ok(());
        }
        let row = sqlx::query(
            "SELECT charged_bytes FROM remote_content_cache \
             WHERE kind = ?1 AND cache_key = ?2 AND is_protected = 1",
        )
        .bind(kind)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(row) = row else { return Ok(()) };
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
        Ok(())
    }

    /// `(kind, key)` に日の保護参照を付ける(既に付いていれば何もしない)。内容があれば、その大きさを合計に足す。内容の無い
    /// 参照は、内容が置かれたときに `sync_retained_bytes` が足す。
    pub(in crate::sqlite) async fn claim_retained_in_tx(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        kind: &str,
        key: &str,
        topic_id: &str,
        bucket: u64,
    ) -> Result<()> {
        let reference = retain_reference(topic_id, bucket);
        let held = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM remote_content_cache_protected_ref \
             WHERE kind = ?1 AND cache_key = ?2 AND ref_id = ?3)",
        )
        .bind(kind)
        .bind(key)
        .bind(&reference)
        .fetch_one(&mut **tx)
        .await?;
        if held != 0 {
            return Ok(());
        }
        Self::add_remote_protected_ref(tx, kind, key, &reference).await?;
        let charge = sqlx::query_scalar::<_, i64>(
            "SELECT charged_bytes FROM remote_content_cache WHERE kind = ?1 AND cache_key = ?2",
        )
        .bind(kind)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(0);
        Self::add_retained_bytes(tx, topic_id, bucket, charge).await
    }

    /// topic の日と全体の保護の合計に `delta` を足す。
    pub(in crate::sqlite) async fn add_retained_bytes(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        topic_id: &str,
        bucket: u64,
        delta: i64,
    ) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }
        sqlx::query(
            "UPDATE topic_retention_days SET used_bytes = MAX(used_bytes + ?3, 0) \
             WHERE topic_id = ?1 AND bucket = ?2",
        )
        .bind(topic_id)
        .bind(i64::try_from(bucket)?)
        .bind(delta)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "UPDATE topic_retention_limits SET used_bytes = MAX(used_bytes + ?1, 0) WHERE id = 1",
        )
        .bind(delta)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// 保護する bucket(`scope`)の record なら、その日の保護参照を付ける(書込みの前に呼び、内容を保護して置く)。
    pub(super) async fn claim_retained_scope(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        key: &str,
        scope: &str,
    ) -> Result<()> {
        let day =
            sqlx::query("SELECT topic_id, bucket FROM topic_retention_scopes WHERE scope_key = ?1")
                .bind(scope)
                .fetch_optional(&mut **tx)
                .await?;
        let Some(day) = day else { return Ok(()) };
        let bucket = u64::try_from(day.get::<i64, _>("bucket"))?;
        Self::claim_retained_in_tx(tx, "record", key, day.get("topic_id"), bucket).await
    }

    /// 内容の大きさが `delta` 変わった(置いた・書き換えた)とき、その内容の保持の保護参照の日の合計を合わせる。
    pub(super) async fn sync_retained_bytes(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        kind: &str,
        key: &str,
        delta: i64,
    ) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }
        let references = sqlx::query_scalar::<_, String>(
            "SELECT ref_id FROM remote_content_cache_protected_ref \
             WHERE kind = ?1 AND cache_key = ?2 AND ref_id >= 'retain:' AND ref_id < 'retain;'",
        )
        .bind(kind)
        .bind(key)
        .fetch_all(&mut **tx)
        .await?;
        for reference in references {
            if let Some((topic_id, bucket)) = reference
                .strip_prefix("retain:")
                .and_then(|rest| rest.rsplit_once(':'))
                && let Ok(bucket) = bucket.parse::<u64>()
            {
                Self::add_retained_bytes(tx, topic_id, bucket, delta).await?;
            }
        }
        Ok(())
    }

    /// 保持していて保護を外していない日なら、`items` に日の保護参照を付ける(内容が無くても先に付ける)。
    pub(in crate::sqlite) async fn claim_retained(
        &self,
        topic_id: &str,
        bucket: u64,
        items: &[(&str, String)],
    ) -> Result<()> {
        let mut update = self.begin_protected_ref_update().await?;
        let held = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM topic_retention_days d \
             JOIN topic_retention t ON t.topic_id = d.topic_id \
             WHERE d.topic_id = ?1 AND d.bucket = ?2 AND d.releasing = 0)",
        )
        .bind(topic_id)
        .bind(i64::try_from(bucket)?)
        .fetch_one(&mut *update.tx)
        .await?;
        if held != 0 {
            for (kind, key) in items {
                Self::claim_retained_in_tx(&mut update.tx, kind, key, topic_id, bucket).await?;
            }
        }
        self.commit_protected_ref_update(update).await
    }

    /// 日の保護参照を `limit` 件まで外し、外した内容の大きさを合計から引く。外し終えたら日の行を消す。外した件数を返す。
    pub(in crate::sqlite) async fn release_retained_refs(
        &self,
        topic_id: &str,
        bucket: u64,
        limit: usize,
    ) -> Result<usize> {
        let reference = retain_reference(topic_id, bucket);
        let mut update = self.begin_protected_ref_update().await?;
        let refs = sqlx::query(
            "SELECT kind, cache_key FROM remote_content_cache_protected_ref \
             WHERE ref_id = ?1 ORDER BY kind, cache_key LIMIT ?2",
        )
        .bind(&reference)
        .bind(i64::try_from(limit)?)
        .fetch_all(&mut *update.tx)
        .await?;
        for row in &refs {
            let (kind, key): (String, String) = (row.get("kind"), row.get("cache_key"));
            let charge = sqlx::query_scalar::<_, i64>(
                "SELECT charged_bytes FROM remote_content_cache WHERE kind = ?1 AND cache_key = ?2",
            )
            .bind(&kind)
            .bind(&key)
            .fetch_optional(&mut *update.tx)
            .await?
            .unwrap_or(0);
            Self::add_retained_bytes(&mut update.tx, topic_id, bucket, -charge).await?;
            Self::remove_remote_protected_ref(
                &mut update.tx,
                &kind,
                &key,
                &reference,
                update.budget,
                &mut update.label_evictions,
                &mut update.removed_files,
            )
            .await?;
        }
        if refs.len() < limit {
            sqlx::query(
                "DELETE FROM topic_retention_days WHERE topic_id = ?1 AND bucket = ?2 AND releasing = 1",
            )
            .bind(topic_id)
            .bind(i64::try_from(bucket)?)
            .execute(&mut *update.tx)
            .await?;
        }
        self.commit_protected_ref_update(update).await?;
        Ok(refs.len())
    }

    /// 登録より前に置かれた、保護する bucket の record に、日の保護参照を `limit` 件まで付ける。付けた件数を返す。
    /// record の cache key は `<replica>\0<key>\0<author>` なので、bucket の record は `<replica>\0` から `<replica>\x01`
    /// の前までに並ぶ。
    pub(in crate::sqlite) async fn protect_retained_scopes_step(
        &self,
        limit: usize,
    ) -> Result<usize> {
        let pending = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM topic_retention_scopes WHERE protect_after IS NOT NULL)",
        )
        .fetch_one(&self.pool)
        .await?;
        if pending == 0 {
            return Ok(0);
        }
        let mut update = self.begin_protected_ref_update().await?;
        let scope = sqlx::query(
            "SELECT scope_key, topic_id, bucket, protect_after FROM topic_retention_scopes \
             WHERE protect_after IS NOT NULL ORDER BY scope_key LIMIT 1",
        )
        .fetch_optional(&mut *update.tx)
        .await?;
        let Some(scope) = scope else {
            self.commit_protected_ref_update(update).await?;
            return Ok(0);
        };
        let scope_key: String = scope.get("scope_key");
        let topic_id: String = scope.get("topic_id");
        let bucket = u64::try_from(scope.get::<i64, _>("bucket"))?;
        let after: String = scope.get("protect_after");
        let lower = if after.is_empty() {
            format!("{scope_key}\0")
        } else {
            after
        };
        let keys = sqlx::query_scalar::<_, String>(
            "SELECT cache_key FROM remote_content_cache \
             WHERE kind = 'record' AND cache_key > ?1 AND cache_key < ?2 ORDER BY cache_key LIMIT ?3",
        )
        .bind(&lower)
        .bind(format!("{scope_key}\u{1}"))
        .bind(i64::try_from(limit)?)
        .fetch_all(&mut *update.tx)
        .await?;
        for key in &keys {
            Self::claim_retained_in_tx(&mut update.tx, "record", key, &topic_id, bucket).await?;
        }
        sqlx::query("UPDATE topic_retention_scopes SET protect_after = ?2 WHERE scope_key = ?1")
            .bind(&scope_key)
            .bind(keys.last().filter(|_| keys.len() == limit))
            .execute(&mut *update.tx)
            .await?;
        self.commit_protected_ref_update(update).await?;
        Ok(keys.len())
    }

    /// 保護参照を外す日を選ぶ。期間を過ぎた日を外す日にしてから、外している途中の日を古い順に返す。無ければ、合計が容量を
    /// 超えている間、最も古い保持の日を外し始め、その日より前の日を読まない印を置く。
    pub(in crate::sqlite) async fn next_release_target(
        &self,
        cutoff_bucket: u64,
    ) -> Result<Option<(String, u64)>> {
        let cutoff = i64::try_from(cutoff_bucket)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        // 登録も消し、以後の書込みに保護参照を付けない。
        for query in [
            "DELETE FROM topic_retention_scopes WHERE (topic_id, bucket) IN              (SELECT topic_id, bucket FROM topic_retention_days WHERE releasing = 0 AND bucket < ?1)",
            "UPDATE topic_retention_days SET releasing = 1 WHERE releasing = 0 AND bucket < ?1",
        ] {
            sqlx::query(query).bind(cutoff).execute(&mut *tx).await?;
        }
        let mut target = sqlx::query(
            "SELECT topic_id, bucket FROM topic_retention_days WHERE releasing = 1              ORDER BY bucket, topic_id LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if target.is_none() {
            target = sqlx::query(
                "SELECT d.topic_id, d.bucket FROM topic_retention_days d, topic_retention_limits l                  WHERE l.id = 1 AND l.used_bytes > l.capacity_bytes AND d.releasing = 0                  ORDER BY d.bucket, d.topic_id LIMIT 1",
            )
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(oldest) = &target {
                let (topic_id, bucket): (&str, i64) =
                    (oldest.get("topic_id"), oldest.get("bucket"));
                for query in [
                    "UPDATE topic_retention_days SET releasing = 1 WHERE topic_id = ?1 AND bucket = ?2",
                    "DELETE FROM topic_retention_scopes WHERE topic_id = ?1 AND bucket = ?2",
                ] {
                    sqlx::query(query)
                        .bind(topic_id)
                        .bind(bucket)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query(
                    "UPDATE topic_retention_limits                      SET released_through = MAX(COALESCE(released_through, ?1), ?1) WHERE id = 1",
                )
                .bind(bucket)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        target
            .map(|row| {
                Ok((
                    row.get("topic_id"),
                    u64::try_from(row.get::<i64, _>("bucket"))?,
                ))
            })
            .transpose()
    }
}
