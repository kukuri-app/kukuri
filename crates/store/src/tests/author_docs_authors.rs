//! #1239: author ごとの docs author の id(ADR 0053 §6)を、SQLite と Memory の両方で同じ意味に保つ。

use super::*;

async fn assert_author_docs_authors(store: &dyn SocialProjectionStore) {
    // author ごとの docs author の id(ADR 0053 §6)。
    assert_eq!(store.get_author_docs_author("x").await.expect("get"), None);
    store
        .put_author_docs_author("x", "d1")
        .await
        .expect("put docs author");
    store
        .put_author_docs_author("x", "d2")
        .await
        .expect("overwrite docs author");
    assert_eq!(
        store
            .get_author_docs_author("x")
            .await
            .expect("get docs author")
            .as_deref(),
        Some("d2")
    );
}

#[tokio::test]
async fn author_docs_authors_round_trip_on_both_stores() {
    assert_author_docs_authors(&MemoryStore::default()).await;
    let tempdir = tempfile::tempdir().expect("tempdir");
    let sqlite = SqliteStore::connect_file(&tempdir.path().join("author-docs-authors.db"))
        .await
        .expect("sqlite store");
    assert_author_docs_authors(&sqlite).await;
}
