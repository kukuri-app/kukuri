use super::*;

/// ADR 0058 §2 の保存 trait。今の inherent method へ委譲する。
#[async_trait::async_trait]
impl crate::ContentCacheStore for SqliteStore {
    fn subscribe_adult_label_evictions(&self) -> tokio::sync::broadcast::Receiver<String> {
        SqliteStore::subscribe_adult_label_evictions(self)
    }

    fn empty_remote_cache_reservation(&self) -> RemoteCacheReservation {
        SqliteStore::empty_remote_cache_reservation(self)
    }

    async fn reserve_remote_cache_bytes(
        &self,
        reservation: &mut RemoteCacheReservation,
        bytes: u64,
    ) -> Result<bool> {
        SqliteStore::reserve_remote_cache_bytes(self, reservation, bytes).await
    }

    async fn put_remote_content(
        &self,
        kind: &str,
        key: &str,
        scope: &str,
        payload: &[u8],
    ) -> Result<bool> {
        SqliteStore::put_remote_content(self, kind, key, scope, payload).await
    }

    async fn get_remote_content(&self, kind: &str, key: &str) -> Result<Option<Vec<u8>>> {
        SqliteStore::get_remote_content(self, kind, key).await
    }

    async fn has_remote_content(&self, kind: &str, key: &str) -> Result<bool> {
        SqliteStore::has_remote_content(self, kind, key).await
    }

    async fn remote_content_len(&self, kind: &str, key: &str) -> Result<Option<u64>> {
        SqliteStore::remote_content_len(self, kind, key).await
    }

    async fn remote_content_chunk(
        &self,
        kind: &str,
        key: &str,
        offset: u64,
        limit: usize,
    ) -> Result<Option<Vec<u8>>> {
        SqliteStore::remote_content_chunk(self, kind, key, offset, limit).await
    }

    async fn put_remote_record(
        &self,
        replica: &str,
        key: &str,
        author: &str,
        payload: &[u8],
    ) -> Result<bool> {
        SqliteStore::put_remote_record(self, replica, key, author, payload).await
    }

    async fn get_remote_records(
        &self,
        replica: &str,
        key: &str,
        author: Option<&str>,
        limit: usize,
        own_only: bool,
    ) -> Result<Vec<Vec<u8>>> {
        SqliteStore::get_remote_records(self, replica, key, author, limit, own_only).await
    }

    async fn remote_record_keys(
        &self,
        replica: &str,
        prefix: &str,
        descending: bool,
        author: Option<&str>,
        limit: usize,
        own_only: bool,
    ) -> Result<(Vec<RemoteRecordKey>, bool)> {
        SqliteStore::remote_record_keys(self, replica, prefix, descending, author, limit, own_only)
            .await
    }

    async fn stored_profile_envelope(
        &self,
        pubkey: &str,
        envelope_id: Option<&str>,
    ) -> Result<Option<KukuriEnvelope>> {
        if let Some(id) = envelope_id {
            return self.get_envelope(&EnvelopeId::from(id)).await;
        }
        let Some(profile) = self.store_get_profile_impl(pubkey, true).await? else {
            return Ok(None);
        };
        let docs_author = self.get_author_docs_author(pubkey).await?;
        for author in docs_author.as_deref().map(Some).into_iter().chain([None]) {
            let id = profile.envelope_id_hint(author)?;
            if let Some(envelope) = self.get_envelope(&id).await? {
                return Ok(Some(envelope));
            }
        }
        Ok(None)
    }

    async fn add_protected_ref(&self, reference: &str, kind: &str, key: &str) -> Result<()> {
        SqliteStore::add_protected_ref(self, reference, kind, key).await
    }

    async fn put_owned_blob(&self, reference: &str, hash: &str, bytes: &[u8]) -> Result<()> {
        SqliteStore::put_owned_blob(self, reference, hash, bytes).await
    }

    async fn put_owned_record(
        &self,
        replica: &str,
        key: &str,
        author: &str,
        payload: &[u8],
    ) -> Result<()> {
        SqliteStore::put_owned_record(self, replica, key, author, payload).await
    }

    async fn protected_records_after(
        &self,
        reference: &str,
        after: &kukuri_core::AccountHistoryCursor,
        limit: usize,
    ) -> Result<Vec<(kukuri_core::AccountHistoryCursor, Vec<u8>)>> {
        SqliteStore::protected_records_after(self, reference, after, limit).await
    }

    async fn reclaim_remote_cache_step(&self) -> Result<usize> {
        SqliteStore::reclaim_remote_cache_step(self).await
    }

    async fn mark_remote_blob_adult(&self, hash: &str) -> Result<()> {
        SqliteStore::mark_remote_blob_adult(self, hash).await
    }

    async fn forget_adult_remote_blobs_step(&self) -> Result<usize> {
        SqliteStore::forget_adult_remote_blobs_step(self).await
    }

    async fn is_public_blob(&self, hash: &str) -> Result<bool> {
        SqliteStore::is_public_blob(self, hash).await
    }

    async fn due_public_blob_announcements(
        &self,
        now: i64,
        limit: usize,
    ) -> Result<(Vec<String>, Option<i64>)> {
        SqliteStore::due_public_blob_announcements(self, now, limit).await
    }

    async fn reschedule_public_blob_announcement(
        &self,
        hash: &str,
        next_at: Option<i64>,
    ) -> Result<()> {
        SqliteStore::reschedule_public_blob_announcement(self, hash, next_at).await
    }

    async fn restart_public_blob_announcements(&self, now: i64, spread: i64) -> Result<()> {
        SqliteStore::restart_public_blob_announcements(self, now, spread).await
    }

    async fn backfill_public_blob_refs_step(&self, limit: usize) -> Result<bool> {
        SqliteStore::backfill_public_blob_refs_step(self, limit).await
    }

    async fn put_remote_blob_file(&self, hash: &str, path: &std::path::Path) -> Result<()> {
        SqliteStore::put_remote_blob_file(self, hash, path).await
    }

    async fn copy_remote_content_to_file(
        &self,
        kind: &str,
        key: &str,
        path: &std::path::Path,
    ) -> Result<Option<u64>> {
        SqliteStore::copy_remote_content_to_file(self, kind, key, path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn stored_profile_lookup_uses_a_fixed_number_of_primary_key_reads() -> Result<()> {
        let steps = Arc::new(AtomicU64::new(0));
        let store = SqliteStore::connect_memory_counting_vm_steps(steps.clone()).await?;
        let keys = kukuri_core::generate_keys();
        let envelope = kukuri_core::build_profile_envelope(
            &keys,
            &kukuri_core::KukuriProfileEnvelopeContentV1 {
                author_pubkey: keys.public_key(),
                display_name: Some("Stored Author".into()),
                ..Default::default()
            },
        )?;
        store.put_envelope(envelope.clone()).await?;
        assert_eq!(
            store
                .store_get_profile_impl(envelope.pubkey.as_str(), true)
                .await?
                .expect("display row"),
            kukuri_core::parse_profile(&envelope)?.expect("signed profile"),
            "display rows must preserve nullable signed profile fields"
        );
        // 形式を移行した後の author を知っていても、tag の無い旧 envelope の候補を調べる。
        store
            .put_author_docs_author(envelope.pubkey.as_str(), &"a".repeat(64))
            .await?;
        let mut counts = Vec::new();
        for size in [0, 2048] {
            if size > 0 {
                sqlx::query("WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < ?1) \
                    INSERT INTO envelopes (envelope_id,pubkey,created_at,kind,content,tags_json,sig) \
                    SELECT 'noise-' || n,'other',n,'other','','[]','' FROM seq")
                    .bind(size).execute(store.pool()).await?;
            }
            steps.store(0, Ordering::Relaxed);
            let loaded = crate::ContentCacheStore::stored_profile_envelope(
                &store,
                envelope.pubkey.as_str(),
                None,
            )
            .await?
            .expect("stored profile");
            assert_eq!(loaded, envelope);
            counts.push(steps.load(Ordering::Relaxed));
        }
        assert!(counts[0] > 0);
        assert_eq!(
            counts[0], counts[1],
            "lookup must not scan or sort envelope history"
        );
        Ok(())
    }
}
