use super::*;
use crate::{ACCOUNT_SYNC_CURSOR_LIMIT, AccountSyncCursor, AccountSyncRow, AccountSyncStore};

#[async_trait]
impl AccountSyncStore for MemoryStore {
    async fn get_account_sync_row(&self, key: &str) -> Result<Option<AccountSyncRow>> {
        Ok(self.account_sync.read().await.rows.get(key).cloned())
    }

    async fn adopt_account_sync_row(&self, row: &AccountSyncRow) -> Result<bool> {
        let mut state = self.account_sync.write().await;
        let newer = state.rows.get(row.key.as_str()).is_none_or(|current| {
            (row.updated_at, row.op_id.as_str()) > (current.updated_at, current.op_id.as_str())
        });
        if newer {
            state.rows.insert(row.key.clone(), row.clone());
            state.unwritten.insert(row.key.clone());
        }
        Ok(newer)
    }

    async fn mark_account_sync_written(&self, row: &AccountSyncRow) -> Result<()> {
        let mut state = self.account_sync.write().await;
        if state.rows.get(row.key.as_str()).is_some_and(|current| {
            (current.updated_at, current.op_id.as_str()) == (row.updated_at, row.op_id.as_str())
        }) {
            state.unwritten.remove(row.key.as_str());
        }
        Ok(())
    }

    async fn list_unwritten_account_sync_rows(&self, limit: usize) -> Result<Vec<AccountSyncRow>> {
        let state = self.account_sync.read().await;
        Ok(state
            .unwritten
            .iter()
            .take(limit)
            .filter_map(|key| state.rows.get(key).cloned())
            .collect())
    }

    async fn get_account_sync_cursor(&self, device_id: &str) -> Result<Option<AccountSyncCursor>> {
        Ok(self
            .account_sync
            .read()
            .await
            .cursors
            .get(device_id)
            .cloned())
    }

    async fn put_account_sync_cursor(
        &self,
        cursor: &AccountSyncCursor,
        own_device_id: &str,
    ) -> Result<()> {
        let mut state = self.account_sync.write().await;
        state
            .cursors
            .insert(cursor.device_id.clone(), cursor.clone());
        let mut others = state
            .cursors
            .values()
            .filter(|cursor| cursor.device_id != own_device_id)
            .map(|cursor| {
                (
                    std::cmp::Reverse(cursor.updated_at),
                    cursor.device_id.clone(),
                )
            })
            .collect::<Vec<_>>();
        others.sort();
        for (_, device_id) in others.into_iter().skip(ACCOUNT_SYNC_CURSOR_LIMIT) {
            state.cursors.remove(&device_id);
        }
        Ok(())
    }

    async fn list_account_sync_cursors(&self) -> Result<Vec<AccountSyncCursor>> {
        Ok(self
            .account_sync
            .read()
            .await
            .cursors
            .values()
            .cloned()
            .collect())
    }

    async fn list_account_sync_keys(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .account_sync
            .read()
            .await
            .rows
            .range(prefix.to_string()..)
            .take_while(|(key, _)| key.starts_with(prefix))
            .filter(|(_, row)| row.value.is_some())
            .map(|(key, _)| key.clone())
            .collect())
    }
}
