//! 本人の書込みを保護参照つきで置く(#1221 R5-I)。

use super::*;

/// 本人が書いた docs record の保護参照。
pub(super) const OWN_DOCS_REF: &str = "own_docs";

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
