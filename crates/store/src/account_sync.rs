//! 本人の端末間の account 同期で採用した item の状態（ADR 0061 §4）。item ごとに 1 行で、操作の log と
//! 重複排除の台帳は持たない。native は `SqliteStore`、Web は IndexedDB が実装する（W4 AC-2）。

use anyhow::Result;
use async_trait::async_trait;

/// 採用した 1 item。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccountSyncRow {
    /// docs の key（`AccountSyncItemKey::docs_key`）。
    pub key: String,
    pub op_id: String,
    /// 編集した端末での編集時刻（ミリ秒）。
    pub updated_at: i64,
    /// 採用した値（JSON）。無ければ tombstone。
    pub value: Option<String>,
}

/// 相手(端末)ごとの取得の位置（ADR 0061 §10）。自分の端末 ID の行は、DB を失ったときの作り直しの位置。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccountSyncCursor {
    pub device_id: String,
    /// この seq までの変更を merge した。
    pub seq: u64,
    /// 最後に読んだ相手の head。
    pub head: u64,
    /// 周回の途中なら、次に照会する prefix。
    pub cycle_prefix: Option<String>,
    /// 周回を始めたときの head（周回が終わったら `seq` にする）。
    pub cycle_head: u64,
    pub updated_at: i64,
}

/// 自分の行を除いて持つ相手の cursor の数（ADR 0061 §5）。
pub const ACCOUNT_SYNC_CURSOR_LIMIT: usize = 16;

#[async_trait]
pub trait AccountSyncStore: Send + Sync {
    async fn get_account_sync_row(&self, key: &str) -> Result<Option<AccountSyncRow>>;
    /// `(updated_at, op_id)` が今の行より大きいときだけ置き換える。置き換えたら true（同じ操作の再受信は false）。
    /// 置き換えた行は、replica へ未書込みになる。
    async fn adopt_account_sync_row(&self, row: &AccountSyncRow) -> Result<bool>;
    /// 行の版が `row` と同じなら、replica へ書いたとする。
    async fn mark_account_sync_written(&self, row: &AccountSyncRow) -> Result<()>;
    /// replica へ未書込みの行を key の順に `limit` 件（未書込みの索引で読む）。
    async fn list_unwritten_account_sync_rows(&self, limit: usize) -> Result<Vec<AccountSyncRow>>;
    async fn get_account_sync_cursor(&self, device_id: &str) -> Result<Option<AccountSyncCursor>>;
    /// cursor を置き、`own_device_id` 以外の行が上限を超えたら、最も古く更新した行を消す。
    async fn put_account_sync_cursor(
        &self,
        cursor: &AccountSyncCursor,
        own_device_id: &str,
    ) -> Result<()>;
    /// cursor の行（上限 + 自分の 1 行まで）。
    async fn list_account_sync_cursors(&self) -> Result<Vec<AccountSyncCursor>>;
    /// `prefix` で始まる key のうち、値のある行の key（key の順）。
    async fn list_account_sync_keys(&self, prefix: &str) -> Result<Vec<String>>;
}
