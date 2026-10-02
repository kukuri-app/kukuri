use super::*;
use crate::{AccountSyncRow, AccountSyncStore};

#[async_trait]
impl AccountSyncStore for MemoryStore {
    async fn get_account_sync_row(&self, key: &str) -> Result<Option<AccountSyncRow>> {
        Ok(self.account_sync_rows.read().await.get(key).cloned())
    }

    async fn adopt_account_sync_row(&self, row: &AccountSyncRow) -> Result<bool> {
        let mut rows = self.account_sync_rows.write().await;
        let newer = rows.get(row.key.as_str()).is_none_or(|current| {
            (row.updated_at, row.op_id.as_str()) > (current.updated_at, current.op_id.as_str())
        });
        if newer {
            rows.insert(row.key.clone(), row.clone());
        }
        Ok(newer)
    }

    async fn list_account_sync_keys(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .account_sync_rows
            .read()
            .await
            .range(prefix.to_string()..)
            .take_while(|(key, _)| key.starts_with(prefix))
            .filter(|(_, row)| row.value.is_some())
            .map(|(key, _)| key.clone())
            .collect())
    }
}
