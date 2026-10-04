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
