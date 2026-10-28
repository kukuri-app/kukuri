use super::*;
use crate::content_cache::{CACHED_MEDIA, MEDIA_BOOKMARKS};
use kukuri_store::{
    BookmarkedMediaRow, MEDIA_BOOKMARK_PAGE_SIZE, MediaBookmarkCursor, MediaBookmarkStore,
    MediaShelf,
};

/// キャッシュの一覧へ載せる条件（native の `CACHED_MEDIA_RECORDABLE`。#1949）。載っておらず、ブックマークに無く、
/// 保護の無い部分（主 hash か poster）が cache にある。`tx` は `CACHED_MEDIA`・`MEDIA_BOOKMARKS`・`contents` を読める。
async fn recordable(tx: &web_sys::IdbTransaction, row: &BookmarkedMediaRow) -> Result<bool> {
    for store in [CACHED_MEDIA, MEDIA_BOOKMARKS] {
        if rows::get::<BookmarkedMediaRow>(tx, store, &text(&row.hash))
            .await?
            .is_some()
        {
            return Ok(false);
        }
    }
    for part in std::iter::once(row.hash.as_str()).chain(row.poster_hash()) {
        let request = rows::store(tx, "contents")?
            .get(&key(&[text("blob"), text(part)]))
            .map_err(crate::idb::js_error)?;
        let content = crate::idb::done(&request).await?;
        if !content.is_undefined()
            && !js_sys::Reflect::get(&content, &"protected".into())
                .map_err(crate::idb::js_error)?
                .is_truthy()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 条件を満たす画像・動画を、キャッシュの一覧へ載せる。`poster` の索引で poster の回収から行を引く。
async fn record_cached(tx: &Tx, row: BookmarkedMediaRow) -> Result<()> {
    if !recordable(tx, &row).await? {
        return Ok(());
    }
    let poster = row.poster_hash().map_or(JsValue::UNDEFINED, text);
    rows::put(tx, CACHED_MEDIA, &row, &[("poster", poster)])
}

/// 主 hash か poster が cache から外れた画像・動画を、キャッシュの一覧から外す（内容を消す transaction の中で呼ぶ）。
pub(crate) async fn forget_cached_media(tx: &Tx, hash: &str) -> Result<()> {
    rows::delete(tx, CACHED_MEDIA, &text(hash))?;
    let range = rows::only(&text(hash))?;
    let videos: Vec<BookmarkedMediaRow> =
        rows::scan(tx, CACHED_MEDIA, Some("poster"), &range, false, usize::MAX).await?;
    for video in videos {
        rows::delete(tx, CACHED_MEDIA, &text(&video.hash))?;
    }
    Ok(())
}

fn now_ms() -> Result<i64> {
    Ok(i64::try_from(
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)?
            .as_millis(),
    )?)
}

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
            move |tx| {
                rows::delete(tx, CACHED_MEDIA, &text(&row.hash))?;
                rows::put(tx, MEDIA_BOOKMARKS, &row, &[])
            },
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
        shelf: MediaShelf,
        cursor: Option<&MediaBookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedMediaRow>> {
        ensure!(
            !before || cursor.is_some(),
            "newer media bookmark page requires cursor"
        );
        let store = match shelf {
            MediaShelf::Bookmarks => MEDIA_BOOKMARKS,
            MediaShelf::Cache => CACHED_MEDIA,
        };
        let cursor = cursor.cloned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[store], Mode::Read)?;
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
                store,
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
        let budget = self.budget();
        let now = now_ms()?;
        self.run(move |db| async move {
            let tx = Tx::begin_with(&db, &[MEDIA_BOOKMARKS])?;
            let saved: Option<BookmarkedMediaRow> =
                rows::get(&tx, MEDIA_BOOKMARKS, &text(&hash)).await?;
            rows::delete(&tx, MEDIA_BOOKMARKS, &text(&hash))?;
            tx.replace_refs(&format!("media_bookmark:{hash}"), &[], budget)
                .await?;
            if let Some(row) = saved {
                record_cached(
                    &tx,
                    BookmarkedMediaRow {
                        bookmarked_at: now,
                        ..row
                    },
                )
                .await?;
            }
            tx.commit().await
        })
        .await
    }

    async fn record_cached_media(&self, row: BookmarkedMediaRow) -> Result<()> {
        self.run(move |db| async move {
            // 表示のたびに呼ばれるので、載せないとき（載っている・自分の添付など）は書込みの transaction を始めない。
            let read = Txn::begin(
                &db.idb,
                &[CACHED_MEDIA, MEDIA_BOOKMARKS, "contents"],
                Mode::Read,
            )?;
            if !recordable(&read, &row).await? {
                return Ok(());
            }
            drop(read);
            let tx = Tx::begin_with(&db, &[MEDIA_BOOKMARKS])?;
            record_cached(&tx, row).await?;
            tx.commit().await
        })
        .await
    }

    async fn remove_cached_media(&self, hash: &str) -> Result<()> {
        let hash = hash.to_owned();
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            let saved: Option<BookmarkedMediaRow> =
                rows::get(&tx, CACHED_MEDIA, &text(&hash)).await?;
            let poster = saved
                .as_ref()
                .and_then(|row| row.poster_hash().map(str::to_owned));
            // 保護の無い部分を消す（台帳の行も `drop_item` で消える）。保護された部分は残し、台帳の行だけを外す。
            for part in std::iter::once(hash.as_str()).chain(poster.as_deref()) {
                if let Some(row) = tx.row("blob", part).await?.filter(|row| !row.protected) {
                    tx.forget(&row).await?;
                }
            }
            rows::delete(&tx, CACHED_MEDIA, &text(&hash))?;
            tx.commit().await
        })
        .await
    }
}
