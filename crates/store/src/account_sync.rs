//! 本人の端末間の account 同期で採用した item の状態（ADR 0061 §4）。item ごとに 1 行で、操作の log と
//! 重複排除の台帳は持たない。native は `SqliteStore`、Web は IndexedDB が実装する（W4 AC-2）。

use anyhow::Result;
use async_trait::async_trait;

/// 採用した 1 item。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSyncRow {
    /// docs の key（`AccountSyncItemKey::docs_key`）。
    pub key: String,
    pub op_id: String,
    /// 編集した端末での編集時刻（ミリ秒）。
    pub updated_at: i64,
    /// 採用した値（JSON）。無ければ tombstone。
    pub value: Option<String>,
}

#[async_trait]
pub trait AccountSyncStore: Send + Sync {
    async fn get_account_sync_row(&self, key: &str) -> Result<Option<AccountSyncRow>>;
    /// `(updated_at, op_id)` が今の行より大きいときだけ置き換える。置き換えたら true（同じ操作の再受信は false）。
    async fn adopt_account_sync_row(&self, row: &AccountSyncRow) -> Result<bool>;
    /// `prefix` で始まる key のうち、値のある行の key（key の順）。
    async fn list_account_sync_keys(&self, prefix: &str) -> Result<Vec<String>>;
}
