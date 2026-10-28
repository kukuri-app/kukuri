use super::*;
use crate::{
    BookmarkedMediaRow, MEDIA_BOOKMARK_PAGE_SIZE, MediaBookmarkCursor, MediaBookmarkStore,
    MediaShelf,
};

/// キャッシュの一覧へ載せる条件(#1949)。載っておらず、ブックマークに無く、保護の無い部分(主 hash `?1` か poster `?2`)が
/// cache にある。
const CACHED_MEDIA_RECORDABLE: &str = "NOT EXISTS (SELECT 1 FROM cached_media WHERE hash = ?1) \
     AND NOT EXISTS (SELECT 1 FROM bookmarked_media WHERE hash = ?1) \
     AND EXISTS (SELECT 1 FROM remote_content_cache WHERE kind = 'blob' \
     AND cache_key IN (?1, ?2) AND is_protected = 0)";

impl SqliteStore {
    /// 条件を満たす画像・動画を、キャッシュの一覧へ載せる。
    async fn record_cached_media_in(
        tx: &mut sqlx::Transaction<'static, Sqlite>,
        row: &BookmarkedMediaRow,
    ) -> Result<()> {
        let sql = format!(
            "INSERT INTO cached_media (hash, poster_hash, cached_at, snapshot_json) \
             SELECT ?1, ?2, ?3, ?4 WHERE {CACHED_MEDIA_RECORDABLE}"
        );
        sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(&row.hash)
            .bind(row.poster_hash())
            .bind(row.bookmarked_at)
            .bind(serde_json::to_string(row)?)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
}

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
        sqlx::query("DELETE FROM cached_media WHERE hash = ?1")
            .bind(&row.hash)
            .execute(&mut *update.tx)
            .await?;
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
        shelf: MediaShelf,
        cursor: Option<&MediaBookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedMediaRow>> {
        anyhow::ensure!(
            !before || cursor.is_some(),
            "newer media bookmark page requires cursor"
        );
        let (table, at) = match shelf {
            MediaShelf::Bookmarks => ("bookmarked_media", "bookmarked_at"),
            MediaShelf::Cache => ("cached_media", "cached_at"),
        };
        let comparison = if before { ">" } else { "<" };
        let order = if before { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT snapshot_json FROM {table} {} ORDER BY {at} {order}, hash {order} LIMIT {}",
            if cursor.is_some() {
                format!("WHERE ({at}, hash) {comparison} (?1, ?2)")
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
        let saved = self.get_bookmarked_media(hash).await?;
        let mut update = self.begin_protected_ref_update().await?;
        sqlx::query("DELETE FROM bookmarked_media WHERE hash = ?1")
            .bind(hash)
            .execute(&mut *update.tx)
            .await?;
        self.set_refs_in(&mut update, &format!("media_bookmark:{hash}"), &[])
            .await?;
        if let Some(row) = saved {
            let row = BookmarkedMediaRow {
                bookmarked_at: super::remote_cache::now_ms()?,
                ..row
            };
            Self::record_cached_media_in(&mut update.tx, &row).await?;
        }
        self.commit_protected_ref_update(update).await
    }

    async fn record_cached_media(&self, row: BookmarkedMediaRow) -> Result<()> {
        // 表示のたびに呼ばれるので、載せないとき(載っている・自分の添付など)は書込みの transaction を始めない。
        let sql = format!("SELECT {CACHED_MEDIA_RECORDABLE}");
        let recordable: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(&row.hash)
            .bind(row.poster_hash())
            .fetch_one(&self.pool)
            .await?;
        if !recordable {
            return Ok(());
        }
        let mut update = self.begin_protected_ref_update().await?;
        Self::record_cached_media_in(&mut update.tx, &row).await?;
        self.commit_protected_ref_update(update).await
    }

    async fn remove_cached_media(&self, hash: &str) -> Result<()> {
        let poster: Option<Option<String>> =
            sqlx::query_scalar("SELECT poster_hash FROM cached_media WHERE hash = ?1")
                .bind(hash)
                .fetch_optional(&self.pool)
                .await?;
        // cache の行を消すと台帳の行も同じ transaction で消える。保護された部分は残し、台帳の行だけを外す。
        for key in std::iter::once(hash).chain(poster.flatten().as_deref()) {
            self.remove_remote_content("blob", key).await?;
        }
        sqlx::query("DELETE FROM cached_media WHERE hash = ?1")
            .bind(hash)
            .execute(&self.pool)
            .await?;
        Ok(())
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
        let first = store
            .list_bookmarked_media(MediaShelf::Bookmarks, None, false)
            .await
            .unwrap();
        assert_eq!(first.len(), 21);
        assert_eq!(first[0].hash, row(44).hash);
        let cursor = MediaBookmarkCursor {
            bookmarked_at: 10,
            hash: first[19].hash.clone(),
        };
        let second = store
            .list_bookmarked_media(MediaShelf::Bookmarks, Some(&cursor), false)
            .await
            .unwrap();
        assert_eq!(second[0].hash, row(24).hash);
        let back = MediaBookmarkCursor {
            bookmarked_at: 10,
            hash: second[0].hash.clone(),
        };
        let previous = store
            .list_bookmarked_media(MediaShelf::Bookmarks, Some(&back), true)
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

    async fn cached(store: &SqliteStore) -> Vec<String> {
        store
            .list_bookmarked_media(MediaShelf::Cache, None, false)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.hash)
            .collect()
    }

    #[tokio::test]
    async fn cached_media_follows_cache_and_bookmarks() {
        let store = SqliteStore::connect_memory().await.unwrap();
        let image = row(1);
        store.record_cached_media(image.clone()).await.unwrap();
        assert!(
            cached(&store).await.is_empty(),
            "cache に無い画像は載せない"
        );
        store
            .put_remote_content("blob", &image.hash, "blob", b"image")
            .await
            .unwrap();
        store.record_cached_media(image.clone()).await.unwrap();
        assert_eq!(cached(&store).await, vec![image.hash.clone()]);

        let own = row(2);
        store
            .put_owned_blob(&format!("own_blob:{}", own.hash), &own.hash, b"own")
            .await
            .unwrap();
        store.record_cached_media(own).await.unwrap();
        assert_eq!(
            cached(&store).await,
            vec![image.hash.clone()],
            "自分の添付は載せない"
        );

        store.put_bookmarked_media(image.clone()).await.unwrap();
        assert!(
            cached(&store).await.is_empty(),
            "ブックマークするとキャッシュから外す"
        );
        store.record_cached_media(image.clone()).await.unwrap();
        assert!(cached(&store).await.is_empty(), "ブックマーク中は載せない");
        store.remove_bookmarked_media(&image.hash).await.unwrap();
        assert_eq!(
            cached(&store).await,
            vec![image.hash.clone()],
            "外すとキャッシュへ戻す"
        );

        let mut video = row(3);
        let poster = format!("{:064x}", 4);
        video.attachments[0].role = AssetRole::VideoManifest;
        video.attachments.push(AssetRef {
            hash: BlobHash::new(poster.clone()),
            mime: "image/jpeg".into(),
            bytes: 1,
            role: AssetRole::VideoPoster,
            origin: None,
        });
        store
            .put_remote_content("blob", &poster, "blob", b"poster")
            .await
            .unwrap();
        store.record_cached_media(video.clone()).await.unwrap();
        assert_eq!(
            cached(&store).await.len(),
            2,
            "poster だけ置いた動画も載せる"
        );
        store.remove_remote_content("blob", &poster).await.unwrap();
        assert_eq!(
            cached(&store).await,
            vec![image.hash.clone()],
            "poster が cache から外れたら一覧から外す"
        );

        store.remove_cached_media(&image.hash).await.unwrap();
        assert!(cached(&store).await.is_empty());
        assert!(!store.has_remote_content("blob", &image.hash).await.unwrap());

        let plan: Vec<_> = sqlx::query("EXPLAIN QUERY PLAN SELECT snapshot_json FROM cached_media WHERE (cached_at, hash) < (?1, ?2) ORDER BY cached_at DESC, hash DESC LIMIT 21")
            .bind(10).bind("hash").fetch_all(&store.pool).await.unwrap().into_iter().map(|row| row.get::<String, _>("detail")).collect();
        assert!(
            plan.iter()
                .any(|detail| detail.contains("USING INDEX cached_media_order")),
            "{plan:?}"
        );
        let plan: Vec<_> = sqlx::query(
            "EXPLAIN QUERY PLAN DELETE FROM cached_media WHERE hash = ?1 OR poster_hash = ?1",
        )
        .bind("hash")
        .fetch_all(&store.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect();
        assert!(
            plan.iter()
                .any(|detail| detail.contains("cached_media_poster")),
            "{plan:?}"
        );
    }
}
