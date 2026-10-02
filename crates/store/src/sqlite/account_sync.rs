use anyhow::Result;
use sqlx::Row;

use super::SqliteStore;
use crate::{AccountSyncRow, AccountSyncStore};

#[async_trait::async_trait]
impl AccountSyncStore for SqliteStore {
    async fn get_account_sync_row(&self, key: &str) -> Result<Option<AccountSyncRow>> {
        let row = sqlx::query(
            "SELECT item_key, op_id, updated_at, value FROM account_sync_items WHERE item_key = ?",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| AccountSyncRow {
            key: row.get("item_key"),
            op_id: row.get("op_id"),
            updated_at: row.get("updated_at"),
            value: row.get("value"),
        }))
    }

    async fn adopt_account_sync_row(&self, row: &AccountSyncRow) -> Result<bool> {
        let result = sqlx::query(
            "INSERT INTO account_sync_items (item_key, op_id, updated_at, value) VALUES (?, ?, ?, ?)
             ON CONFLICT(item_key) DO UPDATE SET
               op_id = excluded.op_id, updated_at = excluded.updated_at, value = excluded.value
             WHERE (excluded.updated_at, excluded.op_id)
               > (account_sync_items.updated_at, account_sync_items.op_id)",
        )
        .bind(&row.key)
        .bind(&row.op_id)
        .bind(row.updated_at)
        .bind(&row.value)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn list_account_sync_keys(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT item_key FROM account_sync_items
             WHERE substr(item_key, 1, length(?1)) = ?1 AND value IS NOT NULL ORDER BY item_key",
        )
        .bind(prefix)
        .fetch_all(&self.pool)
        .await?)
    }
}
