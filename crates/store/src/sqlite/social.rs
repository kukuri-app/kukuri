use super::*;

impl SqliteStore {
    pub(super) async fn store_upsert_profile_impl(&self, profile: Profile) -> Result<()> {
        let existing = self.get_profile(profile.pubkey.as_str()).await?;
        if let Some(existing) = existing
            && existing.updated_at > profile.updated_at
        {
            return Ok(());
        }

        sqlx::query(
            r#"
            INSERT INTO profiles (
              pubkey, name, display_name, about,
              picture_blob_hash, picture_mime, picture_bytes, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(pubkey) DO UPDATE SET
              name = excluded.name,
              display_name = excluded.display_name,
              about = excluded.about,
              picture_blob_hash = excluded.picture_blob_hash,
              picture_mime = excluded.picture_mime,
              picture_bytes = excluded.picture_bytes,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(profile.pubkey.as_str())
        .bind(profile.name.clone())
        .bind(profile.display_name.clone())
        .bind(profile.about.clone())
        .bind(
            profile
                .picture_asset
                .as_ref()
                .map(|asset| asset.hash.as_str().to_string()),
        )
        .bind(
            profile
                .picture_asset
                .as_ref()
                .map(|asset| asset.mime.clone()),
        )
        .bind(
            profile
                .picture_asset
                .as_ref()
                .map(|asset| asset.bytes as i64),
        )
        .bind(profile.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub(super) async fn store_get_profile_impl(&self, pubkey: &str) -> Result<Option<Profile>> {
        let row = sqlx::query(
            r#"
            SELECT
              pubkey, name, display_name, about,
              picture_blob_hash, picture_mime, picture_bytes, updated_at
            FROM profiles
            WHERE pubkey = ?1
            "#,
        )
        .bind(pubkey)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| Profile {
            pubkey: row.get::<String, _>("pubkey").into(),
            name: row.try_get("name").ok(),
            display_name: row.try_get("display_name").ok(),
            about: row.try_get("about").ok(),
            picture_asset: row
                .try_get::<String, _>("picture_blob_hash")
                .ok()
                .map(|hash| kukuri_core::AssetRef {
                    hash: kukuri_core::BlobHash::new(hash),
                    mime: row
                        .try_get::<String, _>("picture_mime")
                        .ok()
                        .unwrap_or_else(|| "application/octet-stream".into()),
                    bytes: row
                        .try_get::<i64, _>("picture_bytes")
                        .ok()
                        .unwrap_or_default() as u64,
                    role: kukuri_core::AssetRole::ProfileAvatar,
                }),
            updated_at: row.get("updated_at"),
        }))
    }

    pub(super) async fn store_get_profiles_impl(
        &self,
        pubkeys: &[String],
    ) -> Result<std::collections::HashMap<String, Profile>> {
        if pubkeys.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
            SELECT
              pubkey, name, display_name, about,
              picture_blob_hash, picture_mime, picture_bytes, updated_at
            FROM profiles
            WHERE pubkey IN (
            "#,
        );
        let mut separated = builder.separated(", ");
        for pubkey in pubkeys {
            separated.push_bind(pubkey);
        }
        separated.push_unseparated(")");

        let rows = builder.build().fetch_all(&self.pool).await?;
        let mut profiles = std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            let profile = Profile {
                pubkey: row.get::<String, _>("pubkey").into(),
                name: row.try_get("name").ok(),
                display_name: row.try_get("display_name").ok(),
                about: row.try_get("about").ok(),
                picture_asset: row
                    .try_get::<String, _>("picture_blob_hash")
                    .ok()
                    .map(|hash| kukuri_core::AssetRef {
                        hash: kukuri_core::BlobHash::new(hash),
                        mime: row
                            .try_get::<String, _>("picture_mime")
                            .ok()
                            .unwrap_or_else(|| "application/octet-stream".into()),
                        bytes: row
                            .try_get::<i64, _>("picture_bytes")
                            .ok()
                            .unwrap_or_default() as u64,
                        role: kukuri_core::AssetRole::ProfileAvatar,
                    }),
                updated_at: row.get("updated_at"),
            };
            profiles.insert(profile.pubkey.as_str().to_string(), profile);
        }
        Ok(profiles)
    }

    pub(super) async fn store_upsert_follow_edge_impl(&self, edge: FollowEdge) -> Result<()> {
        let existing_updated_at = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT updated_at
            FROM follow_edges
            WHERE subject_pubkey = ?1 AND target_pubkey = ?2
            "#,
        )
        .bind(edge.subject_pubkey.as_str())
        .bind(edge.target_pubkey.as_str())
        .fetch_optional(&self.pool)
        .await?;

        if let Some(updated_at) = existing_updated_at
            && updated_at > edge.updated_at
        {
            return Ok(());
        }

        sqlx::query(
            r#"
            INSERT INTO follow_edges (
              subject_pubkey, target_pubkey, status, updated_at, source_envelope_id
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(subject_pubkey, target_pubkey) DO UPDATE SET
              status = excluded.status,
              updated_at = excluded.updated_at,
              source_envelope_id = excluded.source_envelope_id
            "#,
        )
        .bind(edge.subject_pubkey.as_str())
        .bind(edge.target_pubkey.as_str())
        .bind(follow_edge_status_name(&edge.status))
        .bind(edge.updated_at)
        .bind(edge.envelope_id.as_str())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub(super) async fn store_list_follow_edges_by_subject_impl(
        &self,
        subject_pubkey: &str,
    ) -> Result<Vec<FollowEdge>> {
        let rows = sqlx::query(
            r#"
            SELECT subject_pubkey, target_pubkey, status, updated_at, source_envelope_id
            FROM follow_edges
            WHERE subject_pubkey = ?1
            ORDER BY updated_at DESC, target_pubkey ASC
            "#,
        )
        .bind(subject_pubkey)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_follow_edge).collect()
    }

    pub(super) async fn store_list_follow_edges_by_target_impl(
        &self,
        target_pubkey: &str,
    ) -> Result<Vec<FollowEdge>> {
        let rows = sqlx::query(
            r#"
            SELECT subject_pubkey, target_pubkey, status, updated_at, source_envelope_id
            FROM follow_edges
            WHERE target_pubkey = ?1
            ORDER BY updated_at DESC, subject_pubkey ASC
            "#,
        )
        .bind(target_pubkey)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_follow_edge).collect()
    }

    pub(super) async fn store_upsert_block_edge_impl(&self, edge: BlockEdge) -> Result<()> {
        let existing_updated_at = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT updated_at
            FROM block_edges
            WHERE subject_pubkey = ?1 AND target_pubkey = ?2
            "#,
        )
        .bind(edge.subject_pubkey.as_str())
        .bind(edge.target_pubkey.as_str())
        .fetch_optional(&self.pool)
        .await?;

        if let Some(updated_at) = existing_updated_at
            && updated_at > edge.updated_at
        {
            return Ok(());
        }

        sqlx::query(
            r#"
            INSERT INTO block_edges (
              subject_pubkey, target_pubkey, status, updated_at, source_envelope_id
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(subject_pubkey, target_pubkey) DO UPDATE SET
              status = excluded.status,
              updated_at = excluded.updated_at,
              source_envelope_id = excluded.source_envelope_id
            "#,
        )
        .bind(edge.subject_pubkey.as_str())
        .bind(edge.target_pubkey.as_str())
        .bind(block_edge_status_name(&edge.status))
        .bind(edge.updated_at)
        .bind(edge.envelope_id.as_str())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub(super) async fn store_list_block_edges_by_subject_impl(
        &self,
        subject_pubkey: &str,
    ) -> Result<Vec<BlockEdge>> {
        let rows = sqlx::query(
            r#"
            SELECT subject_pubkey, target_pubkey, status, updated_at, source_envelope_id
            FROM block_edges
            WHERE subject_pubkey = ?1
            ORDER BY updated_at DESC, target_pubkey ASC
            "#,
        )
        .bind(subject_pubkey)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_block_edge).collect()
    }

    pub(super) async fn store_list_block_edges_by_target_impl(
        &self,
        target_pubkey: &str,
    ) -> Result<Vec<BlockEdge>> {
        let rows = sqlx::query(
            r#"
            SELECT subject_pubkey, target_pubkey, status, updated_at, source_envelope_id
            FROM block_edges
            WHERE target_pubkey = ?1
            ORDER BY updated_at DESC, subject_pubkey ASC
            "#,
        )
        .bind(target_pubkey)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_block_edge).collect()
    }
}

#[async_trait]
impl SocialProjectionStore for SqliteStore {
    async fn upsert_profile_cache(&self, profile: Profile) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO profile_cache (
              pubkey, name, display_name, about,
              picture_blob_hash, picture_mime, picture_bytes, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(pubkey) DO UPDATE SET
              name = excluded.name,
              display_name = excluded.display_name,
              about = excluded.about,
              picture_blob_hash = excluded.picture_blob_hash,
              picture_mime = excluded.picture_mime,
              picture_bytes = excluded.picture_bytes,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(profile.pubkey.as_str())
        .bind(profile.name)
        .bind(profile.display_name)
        .bind(profile.about)
        .bind(
            profile
                .picture_asset
                .as_ref()
                .map(|asset| asset.hash.as_str().to_string()),
        )
        .bind(
            profile
                .picture_asset
                .as_ref()
                .map(|asset| asset.mime.clone()),
        )
        .bind(
            profile
                .picture_asset
                .as_ref()
                .map(|asset| asset.bytes as i64),
        )
        .bind(profile.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_author_relationship(
        &self,
        local_author_pubkey: &str,
        author_pubkey: &str,
    ) -> Result<Option<AuthorRelationshipProjectionRow>> {
        let row = sqlx::query(
            r#"
            SELECT local_author_pubkey, author_pubkey, following, followed_by, mutual,
                   friend_of_friend, friend_of_friend_via_pubkeys_json, derived_at
            FROM author_relationship_cache
            WHERE local_author_pubkey = ?1 AND author_pubkey = ?2
            "#,
        )
        .bind(local_author_pubkey)
        .bind(author_pubkey)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_author_relationship_projection).transpose()
    }

    async fn list_author_relationships(
        &self,
        local_author_pubkey: &str,
        author_pubkeys: &[String],
    ) -> Result<std::collections::HashMap<String, AuthorRelationshipProjectionRow>> {
        if author_pubkeys.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
            SELECT local_author_pubkey, author_pubkey, following, followed_by, mutual,
                   friend_of_friend, friend_of_friend_via_pubkeys_json, derived_at
            FROM author_relationship_cache
            WHERE local_author_pubkey = "#,
        );
        builder.push_bind(local_author_pubkey);
        builder.push(" AND author_pubkey IN (");
        let mut separated = builder.separated(", ");
        for author_pubkey in author_pubkeys {
            separated.push_bind(author_pubkey);
        }
        separated.push_unseparated(")");

        let rows = builder.build().fetch_all(&self.pool).await?;
        let mut relationships = std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            let relationship = row_to_author_relationship_projection(row)?;
            relationships.insert(relationship.author_pubkey.clone(), relationship);
        }
        Ok(relationships)
    }

    async fn rebuild_author_relationships(
        &self,
        local_author_pubkey: &str,
        rows: Vec<AuthorRelationshipProjectionRow>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            DELETE FROM author_relationship_cache
            WHERE local_author_pubkey = ?1
            "#,
        )
        .bind(local_author_pubkey)
        .execute(&mut *tx)
        .await?;

        for row in rows {
            let via_json = serde_json::to_string(&row.friend_of_friend_via_pubkeys)?;
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO author_relationship_cache (
                  local_author_pubkey, author_pubkey, following, followed_by, mutual,
                  friend_of_friend, friend_of_friend_via_pubkeys_json, derived_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                "#,
            )
            .bind(row.local_author_pubkey.as_str())
            .bind(row.author_pubkey.as_str())
            .bind(row.following)
            .bind(row.followed_by)
            .bind(row.mutual)
            .bind(row.friend_of_friend)
            .bind(via_json)
            .bind(row.derived_at)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_sync_checkpoint(&self, checkpoint_key: &str) -> Result<Option<String>> {
        let value = sqlx::query_scalar::<_, String>(
            "SELECT checkpoint_value FROM sync_checkpoints WHERE checkpoint_key = ?1",
        )
        .bind(checkpoint_key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(value)
    }

    async fn put_sync_checkpoint(&self, checkpoint_key: &str, value: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO sync_checkpoints (checkpoint_key, checkpoint_value, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(checkpoint_key) DO UPDATE SET
              checkpoint_value = excluded.checkpoint_value,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(checkpoint_key)
        .bind(value)
        .bind(now_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_author_docs_author(&self, author_pubkey: &str) -> Result<Option<String>> {
        let value = sqlx::query_scalar::<_, String>(
            "SELECT docs_author FROM author_docs_authors WHERE author_pubkey = ?1",
        )
        .bind(author_pubkey)
        .fetch_optional(&self.pool)
        .await?;
        Ok(value)
    }

    async fn put_author_docs_author(&self, author_pubkey: &str, docs_author: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO author_docs_authors (author_pubkey, docs_author, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(author_pubkey) DO UPDATE SET
              docs_author = excluded.docs_author,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(author_pubkey)
        .bind(docs_author)
        .bind(now_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn put_muted_author(&self, row: MutedAuthorRow) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO muted_authors (author_pubkey, muted_at)
            VALUES (?1, ?2)
            ON CONFLICT(author_pubkey) DO UPDATE SET
              muted_at = excluded.muted_at
            "#,
        )
        .bind(row.author_pubkey.as_str())
        .bind(row.muted_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_muted_author(&self, author_pubkey: &str) -> Result<Option<MutedAuthorRow>> {
        let row = sqlx::query(
            r#"
            SELECT author_pubkey, muted_at
            FROM muted_authors
            WHERE author_pubkey = ?1
            "#,
        )
        .bind(author_pubkey)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_muted_author).transpose()
    }

    async fn list_muted_authors(&self) -> Result<Vec<MutedAuthorRow>> {
        let rows = sqlx::query(
            r#"
            SELECT author_pubkey, muted_at
            FROM muted_authors
            ORDER BY muted_at DESC, author_pubkey ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_muted_author).collect()
    }

    async fn remove_muted_author(&self, author_pubkey: &str) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM muted_authors
            WHERE author_pubkey = ?1
            "#,
        )
        .bind(author_pubkey)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
