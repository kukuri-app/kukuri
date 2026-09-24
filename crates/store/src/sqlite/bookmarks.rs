use super::*;
use crate::BookmarkCursor;

#[async_trait]
impl BlobCacheStore for SqliteStore {
    async fn mark_blob_status(&self, hash: &BlobHash, status: BlobCacheStatus) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO blob_objects (blob_hash, status)
            VALUES (?1, ?2)
            ON CONFLICT(blob_hash) DO UPDATE SET status = excluded.status
            "#,
        )
        .bind(hash.as_str())
        .bind(match status {
            BlobCacheStatus::Missing => "missing",
            BlobCacheStatus::Available => "available",
            BlobCacheStatus::Pinned => "pinned",
        })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_blob_statuses(&self, rows: Vec<(BlobHash, BlobCacheStatus)>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for (hash, status) in rows {
            sqlx::query(
                r#"
                INSERT INTO blob_objects (blob_hash, status)
                VALUES (?1, ?2)
                ON CONFLICT(blob_hash) DO UPDATE SET status = excluded.status
                "#,
            )
            .bind(hash.as_str())
            .bind(match status {
                BlobCacheStatus::Missing => "missing",
                BlobCacheStatus::Available => "available",
                BlobCacheStatus::Pinned => "pinned",
            })
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[async_trait]
impl ReactionBookmarkStore for SqliteStore {
    async fn upsert_reaction_cache(&self, row: ReactionProjectionRow) -> Result<()> {
        let snapshot_json = row
            .custom_asset_snapshot
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        sqlx::query(
            r#"
            INSERT INTO reaction_cache (
              source_replica_id, target_object_id, reaction_id, author_pubkey, created_at,
              updated_at, reaction_key_kind, normalized_reaction_key, emoji, custom_asset_id,
              custom_asset_snapshot_json, status, source_key, source_envelope_id, derived_at,
              projection_version
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            ON CONFLICT(source_replica_id, target_object_id, reaction_id) DO UPDATE SET
              author_pubkey = excluded.author_pubkey,
              created_at = excluded.created_at,
              updated_at = excluded.updated_at,
              reaction_key_kind = excluded.reaction_key_kind,
              normalized_reaction_key = excluded.normalized_reaction_key,
              emoji = excluded.emoji,
              custom_asset_id = excluded.custom_asset_id,
              custom_asset_snapshot_json = excluded.custom_asset_snapshot_json,
              status = excluded.status,
              source_key = excluded.source_key,
              source_envelope_id = excluded.source_envelope_id,
              derived_at = excluded.derived_at,
              projection_version = excluded.projection_version
            "#,
        )
        .bind(row.source_replica_id.as_str())
        .bind(row.target_object_id.as_str())
        .bind(row.reaction_id.as_str())
        .bind(row.author_pubkey.as_str())
        .bind(row.created_at)
        .bind(row.updated_at)
        .bind(reaction_key_kind_name(&row.reaction_key_kind))
        .bind(row.normalized_reaction_key.as_str())
        .bind(row.emoji.as_deref())
        .bind(row.custom_asset_id.as_deref())
        .bind(snapshot_json.as_deref())
        .bind(object_status_name(&row.status))
        .bind(row.source_key.as_str())
        .bind(row.source_envelope_id.as_str())
        .bind(row.derived_at)
        .bind(row.projection_version)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_reaction_cache(
        &self,
        source_replica_id: &ReplicaId,
        target_object_id: &EnvelopeId,
        reaction_id: &EnvelopeId,
    ) -> Result<Option<ReactionProjectionRow>> {
        let row = sqlx::query(
            r#"
            SELECT source_replica_id, target_object_id, reaction_id, author_pubkey, created_at,
                   updated_at, reaction_key_kind, normalized_reaction_key, emoji,
                   custom_asset_id, custom_asset_snapshot_json, status, source_key,
                   source_envelope_id, derived_at, projection_version
            FROM reaction_cache
            WHERE source_replica_id = ?1 AND target_object_id = ?2 AND reaction_id = ?3
            "#,
        )
        .bind(source_replica_id.as_str())
        .bind(target_object_id.as_str())
        .bind(reaction_id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_reaction_projection).transpose()
    }

    async fn list_reaction_cache_for_target(
        &self,
        source_replica_id: &ReplicaId,
        target_object_id: &EnvelopeId,
    ) -> Result<Vec<ReactionProjectionRow>> {
        let rows = sqlx::query(
            r#"
            SELECT source_replica_id, target_object_id, reaction_id, author_pubkey, created_at,
                   updated_at, reaction_key_kind, normalized_reaction_key, emoji,
                   custom_asset_id, custom_asset_snapshot_json, status, source_key,
                   source_envelope_id, derived_at, projection_version
            FROM reaction_cache
            WHERE source_replica_id = ?1 AND target_object_id = ?2
            ORDER BY normalized_reaction_key ASC, reaction_id ASC
            "#,
        )
        .bind(source_replica_id.as_str())
        .bind(target_object_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_reaction_projection).collect()
    }

    async fn list_reaction_cache_for_targets(
        &self,
        source_replica_id: &ReplicaId,
        target_object_ids: &[EnvelopeId],
    ) -> Result<std::collections::HashMap<String, Vec<ReactionProjectionRow>>> {
        if target_object_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
            SELECT source_replica_id, target_object_id, reaction_id, author_pubkey, created_at,
                   updated_at, reaction_key_kind, normalized_reaction_key, emoji,
                   custom_asset_id, custom_asset_snapshot_json, status, source_key,
                   source_envelope_id, derived_at, projection_version
            FROM reaction_cache
            WHERE source_replica_id = "#,
        );
        builder.push_bind(source_replica_id.as_str());
        builder.push(" AND target_object_id IN (");
        let mut separated = builder.separated(", ");
        for target_object_id in target_object_ids {
            separated.push_bind(target_object_id.as_str());
        }
        separated.push_unseparated(")");
        builder
            .push(" ORDER BY target_object_id ASC, normalized_reaction_key ASC, reaction_id ASC");

        let rows = builder.build().fetch_all(&self.pool).await?;
        let mut reactions = std::collections::HashMap::<String, Vec<ReactionProjectionRow>>::new();
        for row in rows {
            let projection = row_to_reaction_projection(row)?;
            reactions
                .entry(projection.target_object_id.as_str().to_string())
                .or_default()
                .push(projection);
        }
        Ok(reactions)
    }

    async fn list_recent_reaction_cache_by_author(
        &self,
        author_pubkey: &str,
    ) -> Result<Vec<ReactionProjectionRow>> {
        let rows = sqlx::query(
            r#"
            SELECT source_replica_id, target_object_id, reaction_id, author_pubkey, created_at,
                   updated_at, reaction_key_kind, normalized_reaction_key, emoji,
                   custom_asset_id, custom_asset_snapshot_json, status, source_key,
                   source_envelope_id, derived_at, projection_version
            FROM reaction_cache
            WHERE author_pubkey = ?1
            ORDER BY updated_at DESC, reaction_id DESC
            "#,
        )
        .bind(author_pubkey)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_reaction_projection).collect()
    }

    async fn put_bookmarked_custom_reaction(&self, row: BookmarkedCustomReactionRow) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO bookmarked_custom_reactions (
              asset_id, owner_pubkey, blob_hash, search_key, mime, bytes, width, height, bookmarked_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(asset_id) DO UPDATE SET
              owner_pubkey = excluded.owner_pubkey,
              blob_hash = excluded.blob_hash,
              search_key = excluded.search_key,
              mime = excluded.mime,
              bytes = excluded.bytes,
              width = excluded.width,
              height = excluded.height,
              bookmarked_at = excluded.bookmarked_at
            "#,
        )
        .bind(row.asset_id.as_str())
        .bind(row.owner_pubkey.as_str())
        .bind(row.blob_hash.as_str())
        .bind(row.search_key.as_str())
        .bind(row.mime.as_str())
        .bind(row.bytes as i64)
        .bind(i64::from(row.width))
        .bind(i64::from(row.height))
        .bind(row.bookmarked_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_bookmarked_custom_reactions(&self) -> Result<Vec<BookmarkedCustomReactionRow>> {
        let rows = sqlx::query(
            r#"
            SELECT asset_id, owner_pubkey, blob_hash, search_key, mime, bytes, width, height, bookmarked_at
            FROM bookmarked_custom_reactions
            ORDER BY bookmarked_at DESC, asset_id DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(row_to_bookmarked_custom_reaction)
            .collect()
    }

    async fn remove_bookmarked_custom_reaction(&self, asset_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM bookmarked_custom_reactions
            WHERE asset_id = ?1
            "#,
        )
        .bind(asset_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn put_bookmarked_post(&self, row: BookmarkedPostRow) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO bookmarked_posts (
              source_object_id,
              source_envelope_id,
              source_replica_id,
              topic_id,
              channel_id,
              author_pubkey,
              created_at,
              object_kind,
              payload_ref_json,
              content,
              attachments_json,
              reply_to_object_id,
              root_object_id,
              repost_of_json,
              bookmarked_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(source_object_id) DO UPDATE SET
              source_envelope_id = excluded.source_envelope_id,
              source_replica_id = excluded.source_replica_id,
              topic_id = excluded.topic_id,
              channel_id = excluded.channel_id,
              author_pubkey = excluded.author_pubkey,
              created_at = excluded.created_at,
              object_kind = excluded.object_kind,
              payload_ref_json = excluded.payload_ref_json,
              content = excluded.content,
              attachments_json = excluded.attachments_json,
              reply_to_object_id = excluded.reply_to_object_id,
              root_object_id = excluded.root_object_id,
              repost_of_json = excluded.repost_of_json,
              bookmarked_at = excluded.bookmarked_at
            "#,
        )
        .bind(row.source_object_id.as_str())
        .bind(row.source_envelope_id.as_str())
        .bind(row.source_replica_id.as_str())
        .bind(row.topic_id.as_str())
        .bind(row.channel_id.as_str())
        .bind(row.author_pubkey.as_str())
        .bind(row.created_at)
        .bind(row.object_kind.as_str())
        .bind(serde_json::to_string(&row.payload_ref)?)
        .bind(row.content.as_deref())
        .bind(serde_json::to_string(&row.attachments)?)
        .bind(row.reply_to_object_id.as_ref().map(EnvelopeId::as_str))
        .bind(row.root_object_id.as_ref().map(EnvelopeId::as_str))
        .bind(
            row.repost_of
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(row.bookmarked_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_bookmarked_posts_page(
        &self,
        cursor: Option<&BookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedPostRow>> {
        anyhow::ensure!(
            !before || cursor.is_some(),
            "newer bookmark page requires cursor"
        );
        let comparison = if before { ">" } else { "<" };
        let order = if before { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT source_object_id, source_envelope_id, source_replica_id, topic_id, \
             channel_id, author_pubkey, created_at, object_kind, payload_ref_json, \
             content, attachments_json, reply_to_object_id, root_object_id, repost_of_json, \
             bookmarked_at FROM bookmarked_posts {} \
             ORDER BY bookmarked_at {order}, source_object_id {order} LIMIT 21",
            if cursor.is_some() {
                format!("WHERE (bookmarked_at, source_object_id) {comparison} (?1, ?2)")
            } else {
                String::new()
            }
        );
        let mut query = sqlx::query(&sql);
        if let Some(cursor) = cursor {
            query = query
                .bind(cursor.bookmarked_at)
                .bind(cursor.source_object_id.as_str());
        }
        query
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(row_to_bookmarked_post)
            .collect()
    }

    async fn bookmarked_post_ids(&self, ids: &[EnvelopeId]) -> Result<Vec<EnvelopeId>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        anyhow::ensure!(ids.len() <= 200, "bookmark status page exceeds 200 objects");
        let mut query = sqlx::QueryBuilder::new(
            "SELECT source_object_id FROM bookmarked_posts WHERE source_object_id IN (",
        );
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(id.as_str());
        }
        separated.push_unseparated(")");
        let found = query
            .build_query_scalar::<String>()
            .fetch_all(&self.pool)
            .await?;
        Ok(found.into_iter().map(EnvelopeId::from).collect())
    }

    async fn remove_bookmarked_post(&self, source_object_id: &EnvelopeId) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM bookmarked_posts
            WHERE source_object_id = ?1
            "#,
        )
        .bind(source_object_id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
