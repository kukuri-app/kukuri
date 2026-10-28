use super::*;
use crate::{
    BookmarkedMediaRow, MEDIA_BOOKMARK_PAGE_SIZE, MediaBookmarkCursor, MediaBookmarkStore,
    MediaShelf,
};

/// 一覧の種類ごとの行。memory には cache が無いので、キャッシュの行は保護の有無を見ずに載せる。
#[derive(Default)]
pub(super) struct MemoryMediaBookmarks {
    shelves: HashMap<MediaShelf, MemoryMediaShelf>,
}

#[derive(Default)]
struct MemoryMediaShelf {
    rows: HashMap<String, BookmarkedMediaRow>,
    order: BTreeSet<(i64, String)>,
}

impl MemoryMediaShelf {
    fn put(&mut self, row: BookmarkedMediaRow) {
        if let Some(old) = self.rows.insert(row.hash.clone(), row.clone()) {
            self.order.remove(&(old.bookmarked_at, old.hash));
        }
        self.order.insert((row.bookmarked_at, row.hash));
    }

    fn remove(&mut self, hash: &str) -> Option<BookmarkedMediaRow> {
        let old = self.rows.remove(hash)?;
        self.order.remove(&(old.bookmarked_at, old.hash.clone()));
        Some(old)
    }
}

impl MemoryMediaBookmarks {
    fn shelf(&mut self, shelf: MediaShelf) -> &mut MemoryMediaShelf {
        self.shelves.entry(shelf).or_default()
    }

    fn record_cached(&mut self, row: BookmarkedMediaRow) {
        let bookmarked = self
            .shelf(MediaShelf::Bookmarks)
            .rows
            .contains_key(&row.hash);
        let cache = self.shelf(MediaShelf::Cache);
        if !bookmarked && !cache.rows.contains_key(&row.hash) {
            cache.put(row);
        }
    }
}

#[async_trait]
impl MediaBookmarkStore for MemoryStore {
    async fn retain_bookmarked_media(&self, _row: &BookmarkedMediaRow) -> Result<()> {
        Ok(())
    }
    async fn put_bookmarked_media(&self, row: BookmarkedMediaRow) -> Result<()> {
        let mut saved = self.media_bookmarks.write().await;
        saved.shelf(MediaShelf::Cache).remove(&row.hash);
        saved.shelf(MediaShelf::Bookmarks).put(row);
        Ok(())
    }

    async fn get_bookmarked_media(&self, hash: &str) -> Result<Option<BookmarkedMediaRow>> {
        Ok(self
            .media_bookmarks
            .write()
            .await
            .shelf(MediaShelf::Bookmarks)
            .rows
            .get(hash)
            .cloned())
    }

    async fn list_bookmarked_media(
        &self,
        shelf: MediaShelf,
        cursor: Option<&MediaBookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedMediaRow>> {
        use std::ops::Bound::{Excluded, Unbounded};
        let mut saved = self.media_bookmarks.write().await;
        let saved = saved.shelf(shelf);
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
        if let Some(row) = saved.shelf(MediaShelf::Bookmarks).remove(hash) {
            saved.record_cached(row);
        }
        Ok(())
    }

    async fn record_cached_media(&self, row: BookmarkedMediaRow) -> Result<()> {
        self.media_bookmarks.write().await.record_cached(row);
        Ok(())
    }

    async fn remove_cached_media(&self, hash: &str) -> Result<()> {
        self.media_bookmarks
            .write()
            .await
            .shelf(MediaShelf::Cache)
            .remove(hash);
        Ok(())
    }
}
