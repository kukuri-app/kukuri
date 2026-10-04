//! 本人の書込みを保護参照つきで置く(#1221 R5-I)。

use super::*;
use kukuri_core::AccountHistoryCursor;

/// 本人が書いた docs record の保護参照。
pub(super) const OWN_DOCS_REF: &str = "own_docs";

/// 保護参照の record を位置の次から読む SQL(test が同じ SQL の query plan を確かめる)。`REST` は同じ参照の続き、`NEXT` は
/// その後ろの参照(範囲の上界まで)。どちらも索引 `(ref_id, kind, cache_key)` を位置から読む。
pub(super) const PROTECTED_RECORDS_REST: &str = "SELECT r.ref_id, c.scope_key, c.record_key,      c.record_author, c.payload FROM remote_content_cache_protected_ref r      JOIN remote_content_cache c ON c.kind = r.kind AND c.cache_key = r.cache_key      WHERE r.ref_id = ?1 AND r.kind = 'record' AND r.cache_key > ?2 ORDER BY r.cache_key LIMIT ?3";
pub(super) const PROTECTED_RECORDS_NEXT: &str = "SELECT r.ref_id, c.scope_key, c.record_key,      c.record_author, c.payload FROM remote_content_cache_protected_ref r      INDEXED BY remote_content_cache_protected_ref_owner      JOIN remote_content_cache c ON c.kind = r.kind AND c.cache_key = r.cache_key      WHERE r.ref_id > ?1 AND r.ref_id < ?2 AND r.kind = 'record'      ORDER BY r.ref_id, r.kind, r.cache_key LIMIT ?3";

impl SqliteStore {
    /// `reference` の保護参照を 1 件足す。内容がまだ無くても参照を先に置き、後から置く内容を容量の計数と回収の外にする。
    pub async fn add_protected_ref(&self, reference: &str, kind: &str, key: &str) -> Result<()> {
        let _gate = self.remote_cache_gate.lock().await;
        let mut tx = self.pool.begin().await?;
        Self::add_remote_protected_ref(&mut tx, kind, key, reference).await?;
        tx.commit().await?;
        Ok(())
    }

    /// 本人の書込みの blob を、保護参照 `reference` を付けて保護所有先へ置く(#1221 R5-I)。
    pub async fn put_owned_blob(&self, reference: &str, hash: &str, bytes: &[u8]) -> Result<()> {
        self.add_protected_ref(reference, "blob", hash).await?;
        if bytes.len() as u64 <= OWNED_INLINE_BLOB_BYTES {
            anyhow::ensure!(
                self.put_remote_content("blob", hash, "blob", bytes).await?,
                "owned blob was not stored"
            );
            return Ok(());
        }
        let root = self
            .remote_cache_files
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("file-backed remote cache is unavailable"))?;
        let staging = tempfile::NamedTempFile::new_in(root)?;
        tokio::fs::write(staging.path(), bytes).await?;
        self.put_remote_blob_file(hash, staging.path()).await
    }

    /// 保護参照(`reference` で始まる参照)が守る record を、(参照, cache key)の順に `after` の次から `limit` 件まで返す
    /// (#1211 AC-3)。cache key `<replica>\0<key>\0<author>` の順は (replica, key, author) の順と同じ。索引
    /// `(ref_id, kind, cache_key)` を `after` から読む。
    pub async fn protected_records_after(
        &self,
        reference: &str,
        after: &AccountHistoryCursor,
        limit: usize,
    ) -> Result<Vec<(AccountHistoryCursor, Vec<u8>)>> {
        // 範囲より前の位置は、範囲の先頭として読む。
        let (current, key) = match after.reference.as_str() < reference {
            true => (reference, String::new()),
            false => (
                after.reference.as_str(),
                Self::remote_record_cache_key(&after.replica, &after.key, &after.author),
            ),
        };
        let mut rows = sqlx::query(PROTECTED_RECORDS_REST)
            .bind(current)
            .bind(key)
            .bind(i64::try_from(limit)?)
            .fetch_all(&self.pool)
            .await?;
        if rows.len() < limit {
            rows.extend(
                sqlx::query(PROTECTED_RECORDS_NEXT)
                    .bind(current)
                    .bind(super::listing::prefix_upper_bound(reference).unwrap_or_default())
                    .bind(i64::try_from(limit - rows.len())?)
                    .fetch_all(&self.pool)
                    .await?,
            );
        }
        rows.into_iter()
            .map(|row| {
                Ok((
                    AccountHistoryCursor {
                        reference: row.try_get("ref_id")?,
                        replica: row.try_get("scope_key")?,
                        key: row.try_get("record_key")?,
                        author: row.try_get("record_author")?,
                    },
                    row.try_get("payload")?,
                ))
            })
            .collect()
    }

    /// 本人が書いた docs record を、保護参照を付けて保護所有先へ置く(#1221 R5-I)。同じ key の書き直しは同じ行を置き換える。
    pub async fn put_owned_record(
        &self,
        replica: &str,
        key: &str,
        author: &str,
        payload: &[u8],
    ) -> Result<()> {
        let cache_key = Self::remote_record_cache_key(replica, key, author);
        self.add_protected_ref(OWN_DOCS_REF, "record", &cache_key)
            .await?;
        anyhow::ensure!(
            self.put_remote_record(replica, key, author, payload)
                .await?,
            "owned record was not stored"
        );
        Ok(())
    }
}
