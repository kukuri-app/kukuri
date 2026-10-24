use super::*;
use crate::{
    BookmarkedMediaRow, MEDIA_BOOKMARK_PAGE_SIZE, MediaBookmarkCursor, MediaBookmarkStore,
};

#[derive(Default)]
pub(super) struct MemoryMediaBookmarks {
    rows: HashMap<String, BookmarkedMediaRow>,
    order: BTreeSet<(i64, String)>,
}

#[async_trait]
impl MediaBookmarkStore for MemoryStore {
    async fn retain_bookmarked_media(&self, _row: &BookmarkedMediaRow) -> Result<()> {
        Ok(())
    }
    async fn put_bookmarked_media(&self, row: BookmarkedMediaRow) -> Result<()> {
        let mut saved = self.media_bookmarks.write().await;
        if let Some(old) = saved.rows.insert(row.hash.clone(), row.clone()) {
            saved.order.remove(&(old.bookmarked_at, old.hash));
        }
        saved.order.insert((row.bookmarked_at, row.hash));
        Ok(())
    }

    async fn get_bookmarked_media(&self, hash: &str) -> Result<Option<BookmarkedMediaRow>> {
        Ok(self.media_bookmarks.read().await.rows.get(hash).cloned())
    }

    async fn list_bookmarked_media(
        &self,
        cursor: Option<&MediaBookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedMediaRow>> {
        use std::ops::Bound::{Excluded, Unbounded};
        let saved = self.media_bookmarks.read().await;
        let key = cursor.map(|cursor| (cursor.bookmarked_at, cursor.hash.clone()));
        let keys: Vec<_> = if before {
            let key =
                key.ok_or_else(|| anyhow::anyhow!("newer media bookmark page requires cursor"))?;
            saved
                .order
                .range((Excluded(key), Unbounded))
                .take(MEDIA_BOOKMARK_PAGE_SIZE + 1)
                .collect()
        } else {
            saved
                .order
                .range((Unbounded, key.map(Excluded).unwrap_or(Unbounded)))
                .rev()
                .take(MEDIA_BOOKMARK_PAGE_SIZE + 1)
                .collect()
        };
        Ok(keys
            .into_iter()
            .map(|(_, hash)| saved.rows[hash].clone())
            .collect())
    }

    async fn remove_bookmarked_media(&self, hash: &str) -> Result<()> {
        let mut saved = self.media_bookmarks.write().await;
        if let Some(old) = saved.rows.remove(hash) {
            saved.order.remove(&(old.bookmarked_at, old.hash));
        }
        Ok(())
    }
}
