//! 本人の書込みの保護と、remote の内容・record の cache（ADR 0058 §2）。
//!
//! native は `SqliteStore`、Web は IndexedDB が実装する。呼出元（iroh-node・docs-sync・blob-service・
//! desktop-runtime）は `Arc<dyn ContentCacheStore>` を持つ。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{AccountHistoryCursor, KukuriEnvelope};

/// 非保護分の容量の上限。
pub const REMOTE_CACHE_CAPACITY_BYTES: i64 = 3 * 1024 * 1024 * 1024;
/// 1 回の回収で消す件数の上限。
pub const REMOTE_CACHE_RECLAIM_STEP: usize = 128;
/// 非保護分がこれより長く使われなければ回収する。
pub const REMOTE_CACHE_UNUSED_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// 最後に使った時刻を書き直す間隔。
pub const REMOTE_CACHE_TOUCH_INTERVAL_MS: i64 = 60 * 60 * 1000;
/// これ以下の blob は行に、超えるものは file に置く（native）。
pub const OWNED_INLINE_BLOB_BYTES: u64 = 1024 * 1024;

/// 取得中の bytes の予約。drop で予約を返す。`counter` は実装の予約の合計（native は `SqliteStore`、Web は IndexedDB）。
pub struct RemoteCacheReservation {
    pub counter: Arc<AtomicU64>,
    pub bytes: u64,
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

/// 保持している record の key の一覧の 1 件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRecordKey {
    pub key: String,
    pub author: String,
    pub content_hash: String,
    pub content_len: u64,
}

/// 手元の key の一覧（先頭 `limit` 件）と保持分の一覧（先頭 `limit` 件）を合わせ、和集合の先頭 `limit` 件にする。
/// 読む量は `2 × limit` を超えない。戻り値の真偽は、切り詰めたか。`key` は (key, docs author) を返す。
pub fn merge_record_keys<T>(
    mut entries: Vec<T>,
    held: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> (&str, &str),
    descending: bool,
    limit: usize,
) -> (Vec<T>, bool) {
    entries.extend(held);
    entries.sort_by(|left, right| key(left).cmp(&key(right)));
    entries.dedup_by(|left, right| key(left) == key(right));
    if descending {
        entries.reverse();
    }
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    (entries, truncated)
}

#[async_trait]
pub trait ContentCacheStore: Send + Sync {
    /// 非保護分の容量の上限。これを超える内容は予約せず、cache へ置かない（Web は quota の半分まで。ADR 0058 §4）。
    fn remote_cache_capacity(&self) -> u64 {
        REMOTE_CACHE_CAPACITY_BYTES as u64
    }
    fn subscribe_adult_label_evictions(&self) -> tokio::sync::broadcast::Receiver<String>;
    fn empty_remote_cache_reservation(&self) -> RemoteCacheReservation;
    async fn reserve_remote_cache_bytes(
        &self,
        reservation: &mut RemoteCacheReservation,
        bytes: u64,
    ) -> Result<bool>;
    async fn put_remote_content(
        &self,
        kind: &str,
        key: &str,
        scope: &str,
        payload: &[u8],
    ) -> Result<bool>;
    async fn get_remote_content(&self, kind: &str, key: &str) -> Result<Option<Vec<u8>>>;
    async fn has_remote_content(&self, kind: &str, key: &str) -> Result<bool>;
    async fn remote_content_len(&self, kind: &str, key: &str) -> Result<Option<u64>>;
    async fn remote_content_chunk(
        &self,
        kind: &str,
        key: &str,
        offset: u64,
        limit: usize,
    ) -> Result<Option<Vec<u8>>>;
    async fn put_remote_record(
        &self,
        replica: &str,
        key: &str,
        author: &str,
        payload: &[u8],
    ) -> Result<bool>;
    /// `own_only` なら、本人が書いた record（保護参照 `own_docs` の付いた行）だけを返す（ADR 0058 §7）。
    async fn get_remote_records(
        &self,
        replica: &str,
        key: &str,
        author: Option<&str>,
        limit: usize,
        own_only: bool,
    ) -> Result<Vec<Vec<u8>>>;
    /// 表示用の行と envelope を保持する native の旧 profile。署名・作者の検証は提供側が行う。
    async fn stored_profile_envelope(
        &self,
        _pubkey: &str,
        _envelope_id: Option<&str>,
    ) -> Result<Option<KukuriEnvelope>> {
        Ok(None)
    }
    /// `own_only` の意味は `get_remote_records` と同じ。
    async fn remote_record_keys(
        &self,
        replica: &str,
        prefix: &str,
        descending: bool,
        author: Option<&str>,
        limit: usize,
        own_only: bool,
    ) -> Result<(Vec<RemoteRecordKey>, bool)>;
    async fn add_protected_ref(&self, reference: &str, kind: &str, key: &str) -> Result<()>;
    async fn put_owned_blob(&self, reference: &str, hash: &str, bytes: &[u8]) -> Result<()>;
    async fn put_owned_record(
        &self,
        replica: &str,
        key: &str,
        author: &str,
        payload: &[u8],
    ) -> Result<()>;
    /// 保護参照（`reference` で始まる参照。`own_docs` はそれ 1 つ）が守る record を、(参照, replica, key, docs author)
    /// の順に `after` の次から `limit` 件まで、位置と値（`DocReadRecord` の JSON）とともに返す（#1211 AC-3 の履歴）。
    /// 索引の範囲を `after` から読み、手前の行を読まない。
    async fn protected_records_after(
        &self,
        reference: &str,
        after: &AccountHistoryCursor,
        limit: usize,
    ) -> Result<Vec<(AccountHistoryCursor, Vec<u8>)>>;
    async fn reclaim_remote_cache_step(&self) -> Result<usize>;
    async fn mark_remote_blob_adult(&self, hash: &str) -> Result<()>;
    async fn forget_adult_remote_blobs_step(&self) -> Result<usize>;
    /// file path を受け取る操作は native だけ（Web は file を使わない。ADR 0056 §5）。
    #[cfg(not(target_family = "wasm"))]
    async fn put_remote_blob_file(&self, hash: &str, path: &std::path::Path) -> Result<()>;
    #[cfg(not(target_family = "wasm"))]
    async fn copy_remote_content_to_file(
        &self,
        kind: &str,
        key: &str,
        path: &std::path::Path,
    ) -> Result<Option<u64>>;
}
