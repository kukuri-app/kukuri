//! account の保存（`AccountStore` の projection・台帳の trait）の IndexedDB 実装（ADR 0059 §2・§3）。
//!
//! 行は cache の database（`kukuri-cache-v1-<公開鍵の hex>`）の object store に置き、SQLite の実装と同じ意味・窓・順序を
//! 索引の範囲と cursor で作る（`crate::rows`）。全件を読んでから選ばない。端末だけのデータ（DM・outbox・通知・bookmark・
//! mute・取り下げ・参加者・account 同期・private channel の行）は回収しない。remote の投稿の行は内容の cache と同じ台帳で
//! 容量を数え、回収で消す（ADR 0058 §4、native の `charge_remote_projection`）。private channel の行は strict の
//! transaction で書く（ADR 0061 §9）。

use std::collections::HashSet;

use anyhow::{Result, ensure};
use async_trait::async_trait;
use kukuri_core::{BlobHash, EnvelopeId};
use kukuri_store::{
    ObjectProjectionRow, ObjectProjectionStore, Page, REMOTE_CACHE_TOUCH_INTERVAL_MS,
    REMOTE_CACHE_UNUSED_MS, TimelineCursor, adult_media_hashes_for_row,
};
use wasm_bindgen::JsValue;

use crate::IndexedDbCache;
use crate::content_cache::{ADULT_HASHES, ADULT_REFS, OBJECTS, OBSERVATIONS, Tx, place};
use crate::idb::Mode;
use crate::rows::{self, Txn, between, key, num, prefix, text, top};

mod direct_messages;
mod envelopes;
mod ledgers;
mod live_game;
mod notifications;
mod peer_candidates;
mod private_channels;
mod public_refs;
mod reactions;
mod social;

// desktop-runtime が account の保存として持てる（`Arc<dyn AccountStore>`）こと。
const _: fn() = || {
    fn account_store<T: kukuri_store::AccountStore>() {}
    account_store::<IndexedDbCache>();
};

pub(crate) fn now_ms() -> Result<i64> {
    Ok(i64::try_from(
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)?
            .as_millis(),
    )?)
}

/// 新しい順の 1 ページの続きの位置（SQLite の `*_page_from_rows` と同じ。`limit` 件ちょうどなら最後の行）。
pub(crate) fn next_cursor<T>(
    items: &[T],
    limit: usize,
    cursor: impl Fn(&T) -> TimelineCursor,
) -> Option<TimelineCursor> {
    (items.len() == limit)
        .then(|| items.last().map(cursor))
        .flatten()
}

fn object_cursor(row: &ObjectProjectionRow) -> TimelineCursor {
    TimelineCursor {
        created_at: row.created_at,
        object_id: row.object_id.clone(),
    }
}

/// 索引 `index` の `head`（固定の先頭の列）の範囲を、(時刻, id) の降順に cursor より前から読む範囲。
pub(crate) fn newest_first(
    head: &[JsValue],
    cursor: Option<&TimelineCursor>,
) -> Result<web_sys::IdbKeyRange> {
    match cursor {
        Some(cursor) => {
            let mut upper = head.to_vec();
            upper.extend([num(cursor.created_at), text(cursor.object_id.as_str())]);
            between(head, &upper, false, true)
        }
        None => prefix(head),
    }
}

/// 投稿の行を置く値（thread の root と、repost の索引）。
fn object_extra(row: &ObjectProjectionRow) -> [(&'static str, JsValue); 2] {
    let root = row.root_object_id.as_ref().unwrap_or(&row.object_id);
    let repost = match (&row.repost_of, row.object_kind.as_str()) {
        (Some(source), "repost") => key(&[
            text(&row.topic_id),
            text(&row.author_pubkey),
            text(source.source_object_id.as_str()),
            num(row.created_at),
        ]),
        _ => JsValue::UNDEFINED,
    };
    [("root", text(root.as_str())), ("repost", repost)]
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AdultHash {
    blob_hash: String,
    marked_at: i64,
    protected: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AdultRef {
    object_id: String,
    blob_hash: String,
}

/// 参照の無くなった非保護の成人向けの印を消す。
async fn release_adult_hash(tx: &Tx, hash: &str) -> Result<()> {
    let referenced = rows::count(tx, ADULT_REFS, Some("hash"), &prefix(&[text(hash)])?).await?;
    if referenced > 0 {
        return Ok(());
    }
    if let Some(row) = rows::get::<AdultHash>(tx, ADULT_HASHES, &text(hash)).await?
        && !row.protected
    {
        rows::delete(tx, ADULT_HASHES, &text(hash))?;
        tx.evicted_label(hash.to_owned());
    }
    Ok(())
}

/// remote の投稿の行（`projection` の内容）を回収するときに、その行と成人向けの印の参照を消す（native の
/// `delete_cache_item`）。
pub(crate) async fn forget_remote_projection(tx: &Tx, object_id: &str) -> Result<()> {
    let refs: Vec<AdultRef> = rows::scan(
        tx,
        ADULT_REFS,
        None,
        &prefix(&[text(object_id)])?,
        false,
        usize::MAX,
    )
    .await?;
    rows::delete(tx, ADULT_REFS, &prefix(&[text(object_id)])?.into())?;
    for entry in refs {
        release_adult_hash(tx, &entry.blob_hash).await?;
    }
    public_refs::forget_post(tx, object_id)?;
    rows::delete(tx, OBJECTS, &text(object_id))
}

impl IndexedDbCache {
    /// 投稿の行を置く（native の `put_object_projections_owned`）。remote の行は容量を数え、収まらなければ失敗にする。
    async fn put_objects(&self, rows: Vec<ObjectProjectionRow>, remote: bool) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let budget = self.budget();
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            for row in rows {
                let charged = tx
                    .row("projection", row.object_id.as_str())
                    .await?
                    .is_some();
                if remote || charged {
                    let charge = i64::try_from(serde_json::to_vec(&row)?.len())? * 2 + 512;
                    ensure!(
                        place(
                            &tx,
                            (
                                "projection",
                                row.object_id.as_str(),
                                row.source_replica_id.as_str()
                            ),
                            &[],
                            None,
                            budget,
                            now_ms()?,
                            Some(charge),
                        )
                        .await?,
                        "remote projection cache capacity exceeded"
                    );
                }
                rows::put(&tx, OBJECTS, &row, &object_extra(&row))?;
                // #1632: 公開投稿の本文・添付は、行と同じ transaction で公開参照に置く。
                public_refs::replace(
                    &tx,
                    "post",
                    row.object_id.as_str(),
                    kukuri_store::public_blob_hashes_for_row(&row),
                )?;
                let hashes = adult_media_hashes_for_row(&row)
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<HashSet<_>>();
                let refs = prefix(&[text(row.object_id.as_str())])?;
                if remote || charged {
                    let previous: Vec<AdultRef> =
                        rows::scan(&tx, ADULT_REFS, None, &refs, false, usize::MAX).await?;
                    rows::delete(&tx, ADULT_REFS, &refs.into())?;
                    for hash in &hashes {
                        if rows::get::<AdultHash>(&tx, ADULT_HASHES, &text(hash))
                            .await?
                            .is_none()
                        {
                            let marked = AdultHash {
                                blob_hash: hash.clone(),
                                marked_at: row.derived_at,
                                protected: false,
                            };
                            rows::put(&tx, ADULT_HASHES, &marked, &[])?;
                        }
                    }
                    for hash in &hashes {
                        let entry = AdultRef {
                            object_id: row.object_id.as_str().into(),
                            blob_hash: hash.clone(),
                        };
                        rows::put(&tx, ADULT_REFS, &entry, &[])?;
                    }
                    for entry in previous {
                        release_adult_hash(&tx, &entry.blob_hash).await?;
                    }
                } else {
                    // 本人・保護した行の印は、remote の cache の回収を越えて残す（行の参照も置く）。
                    for hash in hashes {
                        let marked =
                            match rows::get::<AdultHash>(&tx, ADULT_HASHES, &text(&hash)).await? {
                                Some(existing) => AdultHash {
                                    protected: true,
                                    ..existing
                                },
                                None => AdultHash {
                                    blob_hash: hash.clone(),
                                    marked_at: row.derived_at,
                                    protected: true,
                                },
                            };
                        rows::put(&tx, ADULT_HASHES, &marked, &[])?;
                        let entry = AdultRef {
                            object_id: row.object_id.as_str().into(),
                            blob_hash: hash,
                        };
                        rows::put(&tx, ADULT_REFS, &entry, &[])?;
                    }
                }
            }
            tx.commit().await
        })
        .await
    }

    /// 回収期限を過ぎた remote の行を除き、使った時刻が古い行は更新する（native の `available_remote_projections`）。
    async fn available(&self, rows: Vec<ObjectProjectionRow>) -> Result<Vec<ObjectProjectionRow>> {
        if rows.is_empty() {
            return Ok(rows);
        }
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            let now = now_ms()?;
            let mut kept = Vec::with_capacity(rows.len());
            for row in rows {
                match tx.row("projection", row.object_id.as_str()).await? {
                    Some(cached)
                        if !cached.protected && cached.used <= now - REMOTE_CACHE_UNUSED_MS => {}
                    Some(mut cached) => {
                        if cached.used <= now - REMOTE_CACHE_TOUCH_INTERVAL_MS {
                            cached.used = now;
                            tx.put_row(&cached)?;
                        }
                        kept.push(row);
                    }
                    None => kept.push(row),
                }
            }
            tx.commit().await?;
            Ok(kept)
        })
        .await
    }

    /// `index` の `head` の範囲の、新しい順の 1 ページ。
    async fn object_page(
        &self,
        index: &'static str,
        head: Vec<String>,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        if limit == 0 {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: cursor,
            });
        }
        let items = self
            .run(move |db| async move {
                let tx = Txn::begin(&db.idb, &[OBJECTS], Mode::Read)?;
                let head = head.iter().map(|part| text(part)).collect::<Vec<_>>();
                let range = newest_first(&head, cursor.as_ref())?;
                rows::scan::<ObjectProjectionRow>(&tx, OBJECTS, Some(index), &range, true, limit)
                    .await
            })
            .await?;
        let next_cursor = next_cursor(&items, limit, object_cursor);
        Ok(Page {
            items: self.available(items).await?,
            next_cursor,
        })
    }

    /// thread の 1 ページ（root が先頭、返信は古い順。native の `thread_page`）。
    async fn thread_page(
        &self,
        topic_id: &str,
        root_id: &EnvelopeId,
        channel_id: Option<&str>,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        if limit == 0 {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: cursor,
            });
        }
        let (topic_id, root_id) = (topic_id.to_owned(), root_id.as_str().to_owned());
        let channel_id = channel_id.map(str::to_owned);
        let items = self
            .run(move |db| async move {
                let tx = Txn::begin(&db.idb, &[OBJECTS], Mode::Read)?;
                let mut items = Vec::new();
                // root を返すのは cursor の無い最初のページだけ。root の位置の cursor は「返信の先頭から」を表す。
                if cursor.is_none()
                    && let Some(root) =
                        rows::get::<ObjectProjectionRow>(&tx, OBJECTS, &text(&root_id)).await?
                    && root.topic_id == topic_id
                    && root
                        .root_object_id
                        .as_ref()
                        .is_none_or(|id| id.as_str() == root_id)
                    && channel_id
                        .as_ref()
                        .is_none_or(|channel| *channel == root.channel_id)
                {
                    items.push(root);
                }
                let after = cursor.filter(|cursor| cursor.object_id.as_str() != root_id);
                let (index, mut head) = match &channel_id {
                    Some(channel) => ("thread_channel", vec![text(&topic_id), text(channel)]),
                    None => ("thread", vec![text(&topic_id)]),
                };
                head.push(text(&root_id));
                let range = match &after {
                    Some(after) => {
                        let mut lower = head.clone();
                        lower.extend([num(after.created_at), text(after.object_id.as_str())]);
                        between(&lower, &top(&head), true, false)?
                    }
                    None => prefix(&head)?,
                };
                let remaining = limit - items.len();
                let mut replies = Vec::new();
                rows::walk(&tx, OBJECTS, Some(index), &range, false, |value| {
                    let row: ObjectProjectionRow = rows::decode(value)?;
                    if row.object_id.as_str() != root_id {
                        replies.push(row);
                    }
                    Ok(replies.len() < remaining)
                })
                .await?;
                items.extend(replies.into_iter().take(remaining));
                Ok(items)
            })
            .await?;
        let next_cursor = next_cursor(&items, limit, object_cursor);
        Ok(Page {
            items: self.available(items).await?,
            next_cursor,
        })
    }
}

#[async_trait]
impl ObjectProjectionStore for IndexedDbCache {
    async fn put_object_projection(&self, row: ObjectProjectionRow) -> Result<()> {
        self.put_objects(vec![row], false).await
    }

    async fn put_object_projections(&self, rows: Vec<ObjectProjectionRow>) -> Result<()> {
        self.put_objects(rows, false).await
    }

    async fn put_remote_object_projection(&self, row: ObjectProjectionRow) -> Result<()> {
        self.put_objects(vec![row], true).await
    }

    async fn note_link_preview_image(&self, object_id: &str, hash: &str) -> Result<()> {
        let (object_id, hash) = (object_id.to_owned(), hash.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[crate::content_cache::PUBLIC_REFS], Mode::Write)?;
            public_refs::replace(&tx, "link_preview", &object_id, vec![hash])?;
            tx.commit().await
        })
        .await
    }

    async fn get_object_projection(
        &self,
        object_id: &EnvelopeId,
    ) -> Result<Option<ObjectProjectionRow>> {
        let id = object_id.as_str().to_owned();
        let row = self
            .run(move |db| async move {
                let tx = Txn::begin(&db.idb, &[OBJECTS], Mode::Read)?;
                rows::get::<ObjectProjectionRow>(&tx, OBJECTS, &text(&id)).await
            })
            .await?;
        Ok(self.available(row.into_iter().collect()).await?.pop())
    }

    async fn find_author_reposts_of(
        &self,
        topic_id: &str,
        author_pubkey: &str,
        source_object_id: &EnvelopeId,
        limit: usize,
    ) -> Result<Vec<ObjectProjectionRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let head = [topic_id, author_pubkey, source_object_id.as_str()].map(str::to_owned);
        let rows = self
            .run(move |db| async move {
                let tx = Txn::begin(&db.idb, &[OBJECTS], Mode::Read)?;
                let head = head.iter().map(|part| text(part)).collect::<Vec<_>>();
                rows::scan(&tx, OBJECTS, Some("repost"), &prefix(&head)?, true, limit).await
            })
            .await?;
        self.available(rows).await
    }

    async fn mark_adult_media_hashes(&self, hashes: &[BlobHash]) -> Result<()> {
        use kukuri_store::ContentCacheStore;
        for hash in hashes {
            if self.is_adult_media_hash(hash).await? {
                continue;
            }
            ensure!(
                self.put_remote_content("adult_marker", hash.as_str(), "display", &[])
                    .await?,
                "adult media label cache capacity exceeded"
            );
        }
        Ok(())
    }

    async fn is_adult_media_hash(&self, hash: &BlobHash) -> Result<bool> {
        use kukuri_store::ContentCacheStore;
        let id = hash.as_str().to_owned();
        let marked = self
            .run(move |db| async move {
                let tx = Txn::begin(&db.idb, &[ADULT_HASHES], Mode::Read)?;
                Ok(rows::get::<AdultHash>(&tx, ADULT_HASHES, &text(&id))
                    .await?
                    .is_some())
            })
            .await?;
        Ok(marked
            || self
                .get_remote_content("adult_marker", hash.as_str())
                .await?
                .is_some())
    }

    async fn list_topic_timeline(
        &self,
        topic_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        if limit == 0 {
            // SQLite の `LIMIT 0` と同じく、空のページで続きは無い。
            return Ok(Page {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        self.object_page("timeline", vec![topic_id.into()], cursor, limit)
            .await
    }

    async fn list_topic_timeline_in_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        self.object_page(
            "channel",
            vec![topic_id.into(), channel_id.into()],
            cursor,
            limit,
        )
        .await
    }

    async fn list_author_timeline_in_channel(
        &self,
        author_pubkey: &str,
        channel_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        self.object_page(
            "author",
            vec![author_pubkey.into(), channel_id.into()],
            cursor,
            limit,
        )
        .await
    }

    async fn list_thread(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        self.thread_page(topic_id, thread_root_object_id, None, cursor, limit)
            .await
    }

    async fn list_thread_filtered(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        allowed_channel: Option<&str>,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        self.thread_page(
            topic_id,
            thread_root_object_id,
            allowed_channel,
            cursor,
            limit,
        )
        .await
    }

    /// 投稿・live・game・presence・reaction の行と成人向けの印を作り直す。object store の `clear` は行を読まない。
    async fn rebuild_object_projections(&self, rows: Vec<ObjectProjectionRow>) -> Result<()> {
        use crate::content_cache::{GAME_ROOMS, LIVE_SESSIONS, PRESENCE, REACTIONS};
        self.run(|db| async move {
            let stores = [
                OBJECTS,
                LIVE_SESSIONS,
                GAME_ROOMS,
                PRESENCE,
                REACTIONS,
                ADULT_HASHES,
            ];
            let tx = Txn::begin(&db.idb, &stores, Mode::Write)?;
            for name in stores {
                rows::store(&tx, name)?
                    .clear()
                    .map_err(crate::idb::js_error)?;
            }
            tx.commit().await
        })
        .await?;
        let ids = rows
            .iter()
            .map(|row| row.object_id.as_str().to_owned())
            .collect::<HashSet<_>>();
        self.put_objects(rows, false).await?;
        // 投稿の観測は、作り直した投稿のものだけを残す（観測は上限 2048 件の表なので、その範囲を歩く）。
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[OBSERVATIONS], Mode::Write)?;
            let mut stale = Vec::new();
            rows::walk(
                &tx,
                OBSERVATIONS,
                None,
                &prefix(&[text("post")])?,
                false,
                |value| {
                    let row: kukuri_store::ContentObservationRow = rows::decode(value)?;
                    if !ids.contains(&row.subject_id) {
                        stale.push(row);
                    }
                    Ok(true)
                },
            )
            .await?;
            for row in stale {
                rows::delete(&tx, OBSERVATIONS, &ledgers::observation_key(&row))?;
            }
            tx.commit().await
        })
        .await
    }
}
