use super::*;
use crate::content_cache::MEDIA_BOOKMARKS;
use kukuri_store::{
    BookmarkedMediaRow, MEDIA_BOOKMARK_PAGE_SIZE, MediaBookmarkCursor, MediaBookmarkStore,
};

#[async_trait]
impl MediaBookmarkStore for IndexedDbCache {
    async fn retain_bookmarked_media(&self, row: &BookmarkedMediaRow) -> Result<()> {
        for asset in &row.attachments {
            kukuri_store::ContentCacheStore::add_protected_ref(
                self,
                &format!("own_blob:{}", asset.hash.as_str()),
                "blob",
                asset.hash.as_str(),
            )
            .await?;
        }
        Ok(())
    }
    async fn put_bookmarked_media(&self, row: BookmarkedMediaRow) -> Result<()> {
        self.bookmark_update(
            &[MEDIA_BOOKMARKS],
            format!("media_bookmark:{}", row.hash),
            row.cache_refs(),
            move |tx| rows::put(tx, MEDIA_BOOKMARKS, &row, &[]),
        )
        .await
    }

    async fn get_bookmarked_media(&self, hash: &str) -> Result<Option<BookmarkedMediaRow>> {
        let hash = hash.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[MEDIA_BOOKMARKS], Mode::Read)?;
            rows::get(&tx, MEDIA_BOOKMARKS, &text(&hash)).await
        })
        .await
    }

    async fn list_bookmarked_media(
        &self,
        cursor: Option<&MediaBookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedMediaRow>> {
        ensure!(
            !before || cursor.is_some(),
            "newer media bookmark page requires cursor"
        );
        let cursor = cursor.cloned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[MEDIA_BOOKMARKS], Mode::Read)?;
            let (low, high) = ([num(i64::MIN)], [num(i64::MAX)]);
            let range = match cursor {
                Some(cursor) => {
                    let at = [num(cursor.bookmarked_at), text(&cursor.hash)];
                    if before {
                        between(&at, &top(&high), true, false)?
                    } else {
                        between(&low, &at, false, true)?
                    }
                }
                None => between(&low, &top(&high), false, false)?,
            };
            rows::scan(
                &tx,
                MEDIA_BOOKMARKS,
                Some("order"),
                &range,
                !before,
                MEDIA_BOOKMARK_PAGE_SIZE + 1,
            )
            .await
        })
        .await
    }

    async fn remove_bookmarked_media(&self, hash: &str) -> Result<()> {
        let hash = hash.to_owned();
        self.bookmark_update(
            &[MEDIA_BOOKMARKS],
            format!("media_bookmark:{hash}"),
            Vec::new(),
            move |tx| rows::delete(tx, MEDIA_BOOKMARKS, &text(&hash)),
        )
        .await
    }
}
