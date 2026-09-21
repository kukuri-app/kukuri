//! #1239: 背景で小分けに進む docs の読み出しの進み具合を、SQLite と Memory の両方で同じ意味に保つ。

use super::*;

async fn assert_checkpoints(store: &dyn SocialProjectionStore) {
    assert_eq!(store.get_sync_checkpoint("a").await.expect("get"), None);
    store
        .put_sync_checkpoint("a", "after:1")
        .await
        .expect("put");
    store.put_sync_checkpoint("b", "done").await.expect("put b");
    store
        .put_sync_checkpoint("a", "after:2")
        .await
        .expect("overwrite");
    assert_eq!(
        store
            .get_sync_checkpoint("a")
            .await
            .expect("get a")
            .as_deref(),
        Some("after:2")
    );
    assert_eq!(
        store
            .get_sync_checkpoint("b")
            .await
            .expect("get b")
            .as_deref(),
        Some("done")
    );
}

#[tokio::test]
async fn sync_checkpoints_round_trip_on_both_stores() {
    assert_checkpoints(&MemoryStore::default()).await;
    let tempdir = tempfile::tempdir().expect("tempdir");
    let sqlite = SqliteStore::connect_file(&tempdir.path().join("checkpoints.db"))
        .await
        .expect("sqlite store");
    assert_checkpoints(&sqlite).await;
}
