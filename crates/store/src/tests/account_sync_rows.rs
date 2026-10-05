//! #1218 AC-3: 本人の端末間の account 同期で採用した item の行（ADR 0061 §4）を、SQLite と Memory の両方で同じ意味に保つ。

use super::*;
use crate::{AccountSyncRow, AccountSyncStore};

fn row(key: &str, updated_at: i64, op: char, value: Option<&str>) -> AccountSyncRow {
    AccountSyncRow {
        key: key.to_string(),
        op_id: op.to_string().repeat(32),
        updated_at,
        value: value.map(str::to_string),
    }
}

async fn assert_account_sync_rows(store: &dyn AccountSyncStore) {
    let key = "trust/always-visible/x";
    assert_eq!(store.get_account_sync_row(key).await.unwrap(), None);
    assert!(
        store
            .adopt_account_sync_row(&row(key, 100, 'a', Some("true")))
            .await
            .unwrap()
    );
    // 同じ操作の再受信・古い操作は置き換えない。
    assert!(
        !store
            .adopt_account_sync_row(&row(key, 100, 'a', Some("true")))
            .await
            .unwrap()
    );
    assert!(
        !store
            .adopt_account_sync_row(&row(key, 99, 'f', None))
            .await
            .unwrap()
    );
    // 同じ時刻なら op_id の大きいほうを採る。
    assert!(
        store
            .adopt_account_sync_row(&row(key, 100, 'b', None))
            .await
            .unwrap()
    );
    assert!(
        !store
            .adopt_account_sync_row(&row(key, 100, 'a', Some("true")))
            .await
            .unwrap()
    );
    assert_eq!(
        store.get_account_sync_row(key).await.unwrap(),
        Some(row(key, 100, 'b', None))
    );
    // 一覧は prefix の中の、値のある行だけ。
    store
        .adopt_account_sync_row(&row("trust/always-visible/y", 1, 'a', Some("true")))
        .await
        .unwrap();
    store
        .adopt_account_sync_row(&row("profile", 1, 'a', Some("{}")))
        .await
        .unwrap();
    assert_eq!(
        store
            .list_account_sync_keys("trust/always-visible/")
            .await
            .unwrap(),
        vec!["trust/always-visible/y".to_string()]
    );
}

#[tokio::test]
async fn account_sync_rows_keep_the_greatest_version_on_both_stores() {
    assert_account_sync_rows(&MemoryStore::default()).await;
    let tempdir = tempfile::tempdir().expect("tempdir");
    let sqlite = SqliteStore::connect_file(&tempdir.path().join("account-sync-rows.db"))
        .await
        .expect("sqlite store");
    assert_account_sync_rows(&sqlite).await;
}
