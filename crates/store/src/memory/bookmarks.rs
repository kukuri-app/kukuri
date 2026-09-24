use super::*;

#[async_trait]
impl BlobCacheStore for MemoryStore {
    async fn mark_blob_status(&self, hash: &BlobHash, status: BlobCacheStatus) -> Result<()> {
        self.blob_statuses
            .write()
            .await
            .insert(hash.as_str().to_string(), status);
        Ok(())
    }

    async fn mark_blob_statuses(&self, rows: Vec<(BlobHash, BlobCacheStatus)>) -> Result<()> {
        let mut statuses = self.blob_statuses.write().await;
        for (hash, status) in rows {
            statuses.insert(hash.as_str().to_string(), status);
        }
        Ok(())
    }
}

#[async_trait]
impl ReactionBookmarkStore for MemoryStore {
    async fn upsert_reaction_cache(&self, row: ReactionProjectionRow) -> Result<()> {
        self.reaction_projection_rows.write().await.insert(
            (
                row.source_replica_id.as_str().to_string(),
                row.target_object_id.as_str().to_string(),
                row.reaction_id.as_str().to_string(),
            ),
            row,
        );
        Ok(())
    }

    async fn get_reaction_cache(
        &self,
        source_replica_id: &ReplicaId,
        target_object_id: &EnvelopeId,
        reaction_id: &EnvelopeId,
    ) -> Result<Option<ReactionProjectionRow>> {
        Ok(self
            .reaction_projection_rows
            .read()
            .await
            .get(&(
                source_replica_id.as_str().to_string(),
                target_object_id.as_str().to_string(),
                reaction_id.as_str().to_string(),
            ))
            .cloned())
    }

    async fn list_reaction_cache_for_target(
        &self,
        source_replica_id: &ReplicaId,
        target_object_id: &EnvelopeId,
    ) -> Result<Vec<ReactionProjectionRow>> {
        let mut items = self
            .reaction_projection_rows
            .read()
            .await
            .values()
            .filter(|row| {
                row.source_replica_id == *source_replica_id
                    && row.target_object_id == *target_object_id
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            left.normalized_reaction_key
                .cmp(&right.normalized_reaction_key)
                .then_with(|| left.reaction_id.cmp(&right.reaction_id))
        });
        Ok(items)
    }

    async fn list_reaction_cache_for_targets(
        &self,
        source_replica_id: &ReplicaId,
        target_object_ids: &[EnvelopeId],
    ) -> Result<HashMap<String, Vec<ReactionProjectionRow>>> {
        let target_ids = target_object_ids
            .iter()
            .map(|target_object_id| target_object_id.as_str().to_string())
            .collect::<HashSet<_>>();
        let mut grouped = HashMap::<String, Vec<ReactionProjectionRow>>::new();
        for row in self.reaction_projection_rows.read().await.values() {
            if row.source_replica_id == *source_replica_id
                && target_ids.contains(row.target_object_id.as_str())
            {
                grouped
                    .entry(row.target_object_id.as_str().to_string())
                    .or_default()
                    .push(row.clone());
            }
        }
        for rows in grouped.values_mut() {
            rows.sort_by(|left, right| {
                left.normalized_reaction_key
                    .cmp(&right.normalized_reaction_key)
                    .then_with(|| left.reaction_id.cmp(&right.reaction_id))
            });
        }
        Ok(grouped)
    }

    async fn list_recent_reaction_cache_by_author(
        &self,
        author_pubkey: &str,
    ) -> Result<Vec<ReactionProjectionRow>> {
        let mut items = self
            .reaction_projection_rows
            .read()
            .await
            .values()
            .filter(|row| row.author_pubkey == author_pubkey)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.reaction_id.cmp(&left.reaction_id))
        });
        Ok(items)
    }

    async fn put_bookmarked_custom_reaction(&self, row: BookmarkedCustomReactionRow) -> Result<()> {
        self.bookmarked_custom_reactions
            .write()
            .await
            .insert(row.asset_id.clone(), row);
        Ok(())
    }

    async fn list_bookmarked_custom_reactions(&self) -> Result<Vec<BookmarkedCustomReactionRow>> {
        let mut items = self
            .bookmarked_custom_reactions
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        // sqlite の読み出し(row_mapping.rs row_to_bookmarked_custom_reaction)と同義:
        // search_key が空文字/空白のみの行は読み出し時に asset_id へフォールバックする。
        // 格納値そのものは書き換えない(put 値は保持し、読み出しで適用する — WP-S6 T7)。
        for row in &mut items {
            if row.search_key.trim().is_empty() {
                row.search_key = row.asset_id.clone();
            }
        }
        items.sort_by(|left, right| {
            right
                .bookmarked_at
                .cmp(&left.bookmarked_at)
                .then_with(|| right.asset_id.cmp(&left.asset_id))
        });
        Ok(items)
    }

    async fn remove_bookmarked_custom_reaction(&self, asset_id: &str) -> Result<()> {
        self.bookmarked_custom_reactions
            .write()
            .await
            .remove(asset_id);
        Ok(())
    }

    async fn put_bookmarked_post(&self, row: BookmarkedPostRow) -> Result<()> {
        let mut bookmarks = self.bookmarked_posts.write().await;
        let id = row.source_object_id.as_str().to_string();
        if let Some(old) = bookmarks.rows.insert(id.clone(), row.clone()) {
            bookmarks
                .by_bookmarked_at
                .remove(&(old.bookmarked_at, id.clone()));
        }
        bookmarks.by_bookmarked_at.insert((row.bookmarked_at, id));
        Ok(())
    }

    async fn list_bookmarked_posts_page(
        &self,
        cursor: Option<&BookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedPostRow>> {
        use std::ops::Bound::{Excluded, Unbounded};
        let bookmarks = self.bookmarked_posts.read().await;
        let key = cursor.map(|cursor| {
            (
                cursor.bookmarked_at,
                cursor.source_object_id.as_str().to_string(),
            )
        });
        let ids: Vec<_> = if before {
            let key = key.ok_or_else(|| anyhow::anyhow!("newer bookmark page requires cursor"))?;
            bookmarks
                .by_bookmarked_at
                .range((Excluded(key), Unbounded))
                .take(21)
                .collect()
        } else {
            bookmarks
                .by_bookmarked_at
                .range((Unbounded, key.map(Excluded).unwrap_or(Unbounded)))
                .rev()
                .take(21)
                .collect()
        };
        Ok(ids
            .into_iter()
            .map(|(_, id)| bookmarks.rows[id].clone())
            .collect())
    }

    async fn bookmarked_post_ids(&self, ids: &[EnvelopeId]) -> Result<Vec<EnvelopeId>> {
        let bookmarks = self.bookmarked_posts.read().await;
        Ok(ids
            .iter()
            .filter(|id| bookmarks.rows.contains_key(id.as_str()))
            .cloned()
            .collect())
    }

    async fn remove_bookmarked_post(&self, source_object_id: &EnvelopeId) -> Result<()> {
        let mut bookmarks = self.bookmarked_posts.write().await;
        if let Some(old) = bookmarks.rows.remove(source_object_id.as_str()) {
            bookmarks
                .by_bookmarked_at
                .remove(&(old.bookmarked_at, source_object_id.as_str().to_string()));
        }
        Ok(())
    }
}
