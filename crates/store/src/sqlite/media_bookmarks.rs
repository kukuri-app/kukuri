use super::*;
use crate::{
    BookmarkedMediaRow, MEDIA_BOOKMARK_PAGE_SIZE, MediaBookmarkCursor, MediaBookmarkStore,
};

#[async_trait]
impl MediaBookmarkStore for SqliteStore {
    async fn retain_bookmarked_media(&self, row: &BookmarkedMediaRow) -> Result<()> {
        let mut update = self.begin_protected_ref_update().await?;
        for asset in &row.attachments {
            self.set_refs_in(
                &mut update,
                &format!("own_blob:{}", asset.hash.as_str()),
                &[("blob".into(), asset.hash.as_str().into())],
            )
            .await?;
        }
        self.commit_protected_ref_update(update).await
    }
    async fn put_bookmarked_media(&self, row: BookmarkedMediaRow) -> Result<()> {
        let mut update = self.begin_protected_ref_update().await?;
        sqlx::query("INSERT INTO bookmarked_media (hash, bookmarked_at, snapshot_json) VALUES (?1, ?2, ?3) ON CONFLICT(hash) DO UPDATE SET bookmarked_at = excluded.bookmarked_at, snapshot_json = excluded.snapshot_json")
            .bind(&row.hash).bind(row.bookmarked_at).bind(serde_json::to_string(&row)?)
            .execute(&mut *update.tx).await?;
        self.set_refs_in(
            &mut update,
            &format!("media_bookmark:{}", row.hash),
            &row.cache_refs(),
        )
        .await?;
        self.commit_protected_ref_update(update).await
    }

    async fn get_bookmarked_media(&self, hash: &str) -> Result<Option<BookmarkedMediaRow>> {
        let json: Option<String> =
            sqlx::query_scalar("SELECT snapshot_json FROM bookmarked_media WHERE hash = ?1")
                .bind(hash)
                .fetch_optional(&self.pool)
                .await?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    async fn list_bookmarked_media(
        &self,
        cursor: Option<&MediaBookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedMediaRow>> {
        anyhow::ensure!(
            !before || cursor.is_some(),
            "newer media bookmark page requires cursor"
        );
        let comparison = if before { ">" } else { "<" };
        let order = if before { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT snapshot_json FROM bookmarked_media {} ORDER BY bookmarked_at {order}, hash {order} LIMIT {}",
            if cursor.is_some() {
                format!("WHERE (bookmarked_at, hash) {comparison} (?1, ?2)")
            } else {
                String::new()
            },
            MEDIA_BOOKMARK_PAGE_SIZE + 1
        );
        let mut query = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql.as_str()));
        if let Some(cursor) = cursor {
            query = query.bind(cursor.bookmarked_at).bind(&cursor.hash);
        }
        query
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|json| serde_json::from_str(json).map_err(Into::into))
            .collect()
    }

    async fn remove_bookmarked_media(&self, hash: &str) -> Result<()> {
        let mut update = self.begin_protected_ref_update().await?;
        sqlx::query("DELETE FROM bookmarked_media WHERE hash = ?1")
            .bind(hash)
            .execute(&mut *update.tx)
            .await?;
        self.set_refs_in(&mut update, &format!("media_bookmark:{hash}"), &[])
            .await?;
        self.commit_protected_ref_update(update).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryStore, ReactionBookmarkStore};
    use kukuri_core::{AssetRef, AssetRole, MediaOrigin};

    fn row(index: i64) -> BookmarkedMediaRow {
        let hash = format!("{index:064x}");
        BookmarkedMediaRow {
            bookmarked_from_object_id: format!("post-{index}"),
            hash: hash.clone(),
            source: MediaOrigin {
                topic_id: "topic".into(),
                object_id: format!("post-{index}"),
                author_pubkey: "a".repeat(64),
                created_at: index,
            },
            attachments: vec![AssetRef {
                hash: BlobHash::new(hash),
                mime: "image/png".into(),
                bytes: 1,
                role: AssetRole::ImageOriginal,
                origin: None,
            }],
            content_labels: Vec::new(),
            bookmarked_at: 10,
        }
    }

    async fn pages(store: &dyn MediaBookmarkStore) {
        for index in 0..45 {
            store.put_bookmarked_media(row(index)).await.unwrap();
        }
        let first = store.list_bookmarked_media(None, false).await.unwrap();
        assert_eq!(first.len(), 21);
        assert_eq!(first[0].hash, row(44).hash);
        let cursor = MediaBookmarkCursor {
            bookmarked_at: 10,
            hash: first[19].hash.clone(),
        };
        let second = store
            .list_bookmarked_media(Some(&cursor), false)
            .await
            .unwrap();
        assert_eq!(second[0].hash, row(24).hash);
        let back = MediaBookmarkCursor {
            bookmarked_at: 10,
            hash: second[0].hash.clone(),
        };
        let previous = store
            .list_bookmarked_media(Some(&back), true)
            .await
            .unwrap();
        assert_eq!(previous.len(), 20);
        assert_eq!(previous.last().unwrap().hash, row(44).hash);
        store.remove_bookmarked_media(&row(44).hash).await.unwrap();
        assert!(
            store
                .get_bookmarked_media(&row(44).hash)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn media_bookmark_pages_match_memory_and_use_index() {
        pages(&MemoryStore::default()).await;
        let store = SqliteStore::connect_memory().await.unwrap();
        pages(&store).await;
        let plan: Vec<_> = sqlx::query("EXPLAIN QUERY PLAN SELECT snapshot_json FROM bookmarked_media WHERE (bookmarked_at, hash) < (?1, ?2) ORDER BY bookmarked_at DESC, hash DESC LIMIT 21")
            .bind(10).bind("hash").fetch_all(&store.pool).await.unwrap().into_iter().map(|row| row.get::<String, _>("detail")).collect();
        assert!(
            plan.iter().any(|detail| detail
                .contains("SEARCH bookmarked_media USING INDEX bookmarked_media_order")),
            "{plan:?}"
        );
        assert!(
            store
                .list_bookmarked_posts_page(None, false)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn media_bookmark_restart_and_shared_blob_protection() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("media.db");
        let store = SqliteStore::connect_file(&path).await.unwrap();
        let saved = row(1);
        store.put_bookmarked_media(saved.clone()).await.unwrap();
        store.close().await;
        let store = SqliteStore::connect_file(&path).await.unwrap();
        assert_eq!(
            store.get_bookmarked_media(&saved.hash).await.unwrap(),
            Some(saved.clone())
        );
        store.retain_bookmarked_media(&saved).await.unwrap();
        store.remove_bookmarked_media(&saved.hash).await.unwrap();
        let refs: Vec<String> = sqlx::query_scalar("SELECT ref_id FROM remote_content_cache_protected_ref WHERE kind = 'blob' AND cache_key = ?1").bind(&saved.hash).fetch_all(&store.pool).await.unwrap();
        assert_eq!(refs, vec![format!("own_blob:{}", saved.hash)]);
        store
            .add_protected_ref("other-bookmark", "blob", &saved.hash)
            .await
            .unwrap();
        store.put_bookmarked_media(saved.clone()).await.unwrap();
        store.remove_bookmarked_media(&saved.hash).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM remote_content_cache_protected_ref WHERE kind = 'blob' AND cache_key = ?1").bind(&saved.hash).fetch_one(&store.pool).await.unwrap();
        assert_eq!(count, 2);
    }
}
