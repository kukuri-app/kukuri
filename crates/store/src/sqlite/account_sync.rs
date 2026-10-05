use anyhow::Result;
use sqlx::Row;

use super::SqliteStore;
use crate::{ACCOUNT_SYNC_CURSOR_LIMIT, AccountSyncCursor, AccountSyncRow, AccountSyncStore};

fn account_sync_row(row: sqlx::sqlite::SqliteRow) -> AccountSyncRow {
    AccountSyncRow {
        key: row.get("item_key"),
        op_id: row.get("op_id"),
        updated_at: row.get("updated_at"),
        value: row.get("value"),
    }
}

fn cursor_row(row: sqlx::sqlite::SqliteRow) -> AccountSyncCursor {
    AccountSyncCursor {
        device_id: row.get("device_id"),
        seq: u64::try_from(row.get::<i64, _>("seq")).unwrap_or_default(),
        head: u64::try_from(row.get::<i64, _>("head")).unwrap_or_default(),
        cycle_prefix: row.get("cycle_prefix"),
        cycle_head: u64::try_from(row.get::<i64, _>("cycle_head")).unwrap_or_default(),
        updated_at: row.get("updated_at"),
    }
}

#[async_trait::async_trait]
impl AccountSyncStore for SqliteStore {
    async fn get_account_sync_row(&self, key: &str) -> Result<Option<AccountSyncRow>> {
        let row = sqlx::query(
            "SELECT item_key, op_id, updated_at, value FROM account_sync_items WHERE item_key = ?",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(account_sync_row))
    }

    async fn adopt_account_sync_row(&self, row: &AccountSyncRow) -> Result<bool> {
        let result = sqlx::query(
            "INSERT INTO account_sync_items (item_key, op_id, updated_at, value) VALUES (?, ?, ?, ?)
             ON CONFLICT(item_key) DO UPDATE SET
               op_id = excluded.op_id, updated_at = excluded.updated_at, value = excluded.value,
               written = 0
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

    async fn mark_account_sync_written(&self, row: &AccountSyncRow) -> Result<()> {
        sqlx::query(
            "UPDATE account_sync_items SET written = 1
             WHERE item_key = ? AND op_id = ? AND updated_at = ? AND written = 0",
        )
        .bind(&row.key)
        .bind(&row.op_id)
        .bind(row.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_unwritten_account_sync_rows(&self, limit: usize) -> Result<Vec<AccountSyncRow>> {
        Ok(sqlx::query(
            "SELECT item_key, op_id, updated_at, value FROM account_sync_items
             WHERE written = 0 ORDER BY item_key LIMIT ?",
        )
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(account_sync_row)
        .collect())
    }

    async fn get_account_sync_cursor(&self, device_id: &str) -> Result<Option<AccountSyncCursor>> {
        Ok(
            sqlx::query("SELECT * FROM account_sync_cursors WHERE device_id = ?")
                .bind(device_id)
                .fetch_optional(&self.pool)
                .await?
                .map(cursor_row),
        )
    }

    async fn put_account_sync_cursor(
        &self,
        cursor: &AccountSyncCursor,
        own_device_id: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT OR REPLACE INTO account_sync_cursors
               (device_id, seq, head, cycle_prefix, cycle_head, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&cursor.device_id)
        .bind(i64::try_from(cursor.seq)?)
        .bind(i64::try_from(cursor.head)?)
        .bind(&cursor.cycle_prefix)
        .bind(i64::try_from(cursor.cycle_head)?)
        .bind(cursor.updated_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM account_sync_cursors WHERE device_id IN (
               SELECT device_id FROM account_sync_cursors WHERE device_id != ?
               ORDER BY updated_at DESC, device_id LIMIT -1 OFFSET ?)",
        )
        .bind(own_device_id)
        .bind(i64::try_from(ACCOUNT_SYNC_CURSOR_LIMIT)?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn list_account_sync_cursors(&self) -> Result<Vec<AccountSyncCursor>> {
        Ok(
            sqlx::query("SELECT * FROM account_sync_cursors ORDER BY device_id")
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(cursor_row)
                .collect(),
        )
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
