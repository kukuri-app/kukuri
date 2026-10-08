//! reaction の行・custom reaction と投稿の bookmark（`ReactionBookmarkStore`。native の `sqlite/bookmarks.rs`）。
//! bookmark の追加・解除は、保護参照の置き換え（ADR 0058 §2）と同じ transaction で行う。

use std::collections::HashMap;

use anyhow::{Result, ensure};
use async_trait::async_trait;
use kukuri_core::{EnvelopeId, ReplicaId};
use kukuri_store::{
    BOOKMARKED_CUSTOM_REACTION_LIMIT, BookmarkCursor, BookmarkedCustomReactionRow,
    BookmarkedPostRow, ReactionBookmarkStore, ReactionProjectionRow, bookmark_cache_refs,
};

use crate::IndexedDbCache;
use crate::content_cache::{
    OBJECTS, POST_BOOKMARKS, PUBLIC_REFS, REACTION_BOOKMARKS, REACTIONS, Tx,
};
use crate::idb::Mode;
use crate::rows::{self, Txn, between, key, num, prefix, text};

/// 1 回の bookmark の一覧の行数（native と同じく、次のページの有無を見るために 1 件多く読む）。
const BOOKMARK_PAGE_ROWS: usize = 21;

fn reaction_key(source: &str, target: &str, reaction: &str) -> wasm_bindgen::JsValue {
    key(&[text(source), text(target), text(reaction)])
}

async fn reactions_for_target(
    tx: &web_sys::IdbTransaction,
    source: &str,
    target: &str,
) -> Result<Vec<ReactionProjectionRow>> {
    let range = prefix(&[text(source), text(target)])?;
    rows::scan(tx, REACTIONS, Some("target"), &range, false, usize::MAX).await
}

impl IndexedDbCache {
    /// bookmark の行の変更と、その保護参照の置き換えを 1 つの transaction で行う。`stores` は行の store（と、custom
    /// reaction の bookmark では公開参照の store）。
    async fn bookmark_update(
        &self,
        stores: &'static [&'static str],
        reference: String,
        refs: Vec<(String, String)>,
        change: impl FnOnce(&Tx) -> Result<()> + Send + 'static,
    ) -> Result<()> {
        let budget = self.budget();
        self.run(move |db| async move {
            let tx = Tx::begin_with(&db, stores)?;
            change(&tx)?;
            tx.replace_refs(&reference, &refs, budget).await?;
            tx.commit().await
        })
        .await
    }
}

#[async_trait]
impl ReactionBookmarkStore for IndexedDbCache {
    async fn upsert_reaction_cache(&self, row: ReactionProjectionRow) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[REACTIONS, OBJECTS, PUBLIC_REFS], Mode::Write)?;
            rows::put(&tx, REACTIONS, &row, &[])?;
            // #1632: 公開 topic の有効な custom reaction の asset は公開参照。
            super::public_refs::replace_reaction(&tx, &row).await?;
            tx.commit().await
        })
        .await
    }

    async fn get_reaction_cache(
        &self,
        source_replica_id: &ReplicaId,
        target_object_id: &EnvelopeId,
        reaction_id: &EnvelopeId,
    ) -> Result<Option<ReactionProjectionRow>> {
        let id = [
            source_replica_id.as_str(),
            target_object_id.as_str(),
            reaction_id.as_str(),
        ]
        .map(str::to_owned);
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[REACTIONS], Mode::Read)?;
            rows::get(&tx, REACTIONS, &reaction_key(&id[0], &id[1], &id[2])).await
        })
        .await
    }

    async fn list_reaction_cache_for_target(
        &self,
        source_replica_id: &ReplicaId,
        target_object_id: &EnvelopeId,
    ) -> Result<Vec<ReactionProjectionRow>> {
        let (source, target) = (
            source_replica_id.as_str().to_owned(),
            target_object_id.as_str().to_owned(),
        );
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[REACTIONS], Mode::Read)?;
            reactions_for_target(&tx, &source, &target).await
        })
        .await
    }

    async fn list_reaction_cache_for_targets(
        &self,
        source_replica_id: &ReplicaId,
        target_object_ids: &[EnvelopeId],
    ) -> Result<HashMap<String, Vec<ReactionProjectionRow>>> {
        let source = source_replica_id.as_str().to_owned();
        let targets = target_object_ids
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[REACTIONS], Mode::Read)?;
            let mut reactions = HashMap::new();
            for target in targets {
                let rows = reactions_for_target(&tx, &source, &target).await?;
                if !rows.is_empty() {
                    reactions.insert(target, rows);
                }
            }
            Ok(reactions)
        })
        .await
    }

    async fn list_recent_reaction_cache_by_author(
        &self,
        author_pubkey: &str,
    ) -> Result<Vec<ReactionProjectionRow>> {
        let author = author_pubkey.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[REACTIONS], Mode::Read)?;
            let range = prefix(&[text(&author)])?;
            rows::scan(&tx, REACTIONS, Some("author"), &range, true, usize::MAX).await
        })
        .await
    }

    async fn put_bookmarked_custom_reaction(&self, row: BookmarkedCustomReactionRow) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[REACTION_BOOKMARKS, PUBLIC_REFS], Mode::Write)?;
            rows::put(&tx, REACTION_BOOKMARKS, &row, &[])?;
            // #1232 AC-3: 保存済みの custom reaction の画像は公開参照（ADR 0063 §1）。
            super::public_refs::replace(
                &tx,
                "reaction_bookmark",
                &row.asset_id,
                vec![row.blob_hash.as_str().to_string()],
            )?;
            tx.commit().await
        })
        .await
    }

    async fn list_bookmarked_custom_reactions(&self) -> Result<Vec<BookmarkedCustomReactionRow>> {
        self.run(|db| async move {
            let tx = Txn::begin(&db.idb, &[REACTION_BOOKMARKS], Mode::Read)?;
            let all = between(&[num(i64::MIN)], &[num(i64::MAX)], false, false)?;
            let rows: Vec<BookmarkedCustomReactionRow> = rows::scan(
                &tx,
                REACTION_BOOKMARKS,
                Some("order"),
                &all,
                true,
                BOOKMARKED_CUSTOM_REACTION_LIMIT,
            )
            .await?;
            Ok(rows
                .into_iter()
                .map(BookmarkedCustomReactionRow::with_search_key_fallback)
                .collect())
        })
        .await
    }

    async fn remove_bookmarked_custom_reaction(&self, asset_id: &str) -> Result<()> {
        let id = asset_id.to_owned();
        self.bookmark_update(
            &[REACTION_BOOKMARKS, PUBLIC_REFS],
            format!("reaction_bookmark:{asset_id}"),
            Vec::new(),
            move |tx| {
                rows::delete(tx, REACTION_BOOKMARKS, &text(&id))?;
                super::public_refs::replace(tx, "reaction_bookmark", &id, Vec::new())
            },
        )
        .await
    }

    async fn put_bookmarked_post(&self, row: BookmarkedPostRow) -> Result<()> {
        let reference = format!("bookmark:{}", row.source_object_id.as_str());
        let refs = bookmark_cache_refs(&row);
        self.bookmark_update(&[POST_BOOKMARKS], reference, refs, move |tx| {
            rows::put(tx, POST_BOOKMARKS, &row, &[])
        })
        .await
    }

    async fn list_bookmarked_posts_page(
        &self,
        cursor: Option<&BookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedPostRow>> {
        ensure!(
            !before || cursor.is_some(),
            "newer bookmark page requires cursor"
        );
        let cursor = cursor.cloned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[POST_BOOKMARKS], Mode::Read)?;
            let (low, high) = ([num(i64::MIN)], [num(i64::MAX)]);
            let range = match &cursor {
                Some(cursor) => {
                    let at = [
                        num(cursor.bookmarked_at),
                        text(cursor.source_object_id.as_str()),
                    ];
                    if before {
                        between(&at, &rows::top(&high), true, false)?
                    } else {
                        between(&low, &at, false, true)?
                    }
                }
                None => between(&low, &rows::top(&high), false, false)?,
            };
            rows::scan(
                &tx,
                POST_BOOKMARKS,
                Some("order"),
                &range,
                !before,
                BOOKMARK_PAGE_ROWS,
            )
            .await
        })
        .await
    }

    async fn bookmarked_post_ids(&self, ids: &[EnvelopeId]) -> Result<Vec<EnvelopeId>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        ensure!(ids.len() <= 200, "bookmark status page exceeds 200 objects");
        let mut ids = ids.to_vec();
        ids.sort();
        ids.dedup();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[POST_BOOKMARKS], Mode::Read)?;
            let mut found = Vec::new();
            for id in ids {
                if rows::get::<BookmarkedPostRow>(&tx, POST_BOOKMARKS, &text(id.as_str()))
                    .await?
                    .is_some()
                {
                    found.push(id);
                }
            }
            Ok(found)
        })
        .await
    }

    async fn remove_bookmarked_post(&self, source_object_id: &EnvelopeId) -> Result<()> {
        let id = source_object_id.as_str().to_owned();
        self.bookmark_update(
            &[POST_BOOKMARKS],
            format!("bookmark:{}", source_object_id.as_str()),
            Vec::new(),
            move |tx| rows::delete(tx, POST_BOOKMARKS, &text(&id)),
        )
        .await
    }
}
