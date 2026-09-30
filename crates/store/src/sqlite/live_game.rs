use super::*;

const LIVE_SESSION_SELECT: &str = r#"
    SELECT lsc.session_id, lsc.revision, lsc.topic_id, lsc.host_pubkey, lsc.title, lsc.description,
           lsc.channel_id, lsc.status, lsc.started_at, lsc.ended_at, lsc.updated_at,
           lsc.source_replica_id, lsc.source_key, lsc.manifest_blob_hash, lsc.derived_at,
           lsc.projection_version,
           CASE
             WHEN lsc.status = 'ended' THEN 0
             ELSE COALESCE((
               SELECT COUNT(*)
               FROM live_presence_cache lpc
               WHERE lpc.topic_id = lsc.topic_id
                 AND lpc.channel_id = lsc.channel_id
                 AND lpc.session_id = lsc.session_id
             ), 0)
           END AS viewer_count
    FROM live_session_cache lsc
"#;

const GAME_ROOM_SELECT: &str = r#"
    SELECT room_id, score_revision, topic_id, host_pubkey, title, description, status, phase_label,
           channel_id, scores_json, room_kind, metaverse_json, updated_at,
           source_replica_id, source_key, manifest_blob_hash, derived_at, projection_version
    FROM game_room_cache
"#;

/// #1292: live 一覧を既存の (topic, channel, started_at, session_id) 索引の範囲から読む。
/// test の query plan も本番と同じ SQL builder を使う。
pub(crate) fn live_session_list_query<'a>(
    prefix: &str,
    topic_id: &'a str,
    channel_id: &'a str,
    limit: usize,
) -> QueryBuilder<Sqlite> {
    let mut builder = QueryBuilder::<Sqlite>::new(prefix);
    builder.push(LIVE_SESSION_SELECT);
    builder.push(" WHERE lsc.topic_id = ");
    builder.push_bind(topic_id);
    builder.push(" AND lsc.channel_id = ");
    builder.push_bind(channel_id);
    builder.push(" ORDER BY lsc.started_at DESC, lsc.session_id DESC LIMIT ");
    builder.push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
    builder
}

/// #1292: game 一覧を既存の (topic, channel, updated_at, room_id) 索引の範囲から読む。
pub(crate) fn game_room_list_query<'a>(
    prefix: &str,
    topic_id: &'a str,
    channel_id: &'a str,
    limit: usize,
) -> QueryBuilder<Sqlite> {
    let mut builder = QueryBuilder::<Sqlite>::new(prefix);
    builder.push(GAME_ROOM_SELECT);
    builder.push(" WHERE topic_id = ");
    builder.push_bind(topic_id);
    builder.push(" AND channel_id = ");
    builder.push_bind(channel_id);
    builder.push(" ORDER BY updated_at DESC, room_id DESC LIMIT ");
    builder.push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
    builder
}

#[async_trait]
impl LiveGameProjectionStore for SqliteStore {
    async fn upsert_live_session_cache(&self, row: LiveSessionProjectionRow) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO live_session_cache (
              session_id, revision, topic_id, channel_id, host_pubkey, title, description, status,
              started_at, ended_at, updated_at, source_replica_id, source_key, manifest_blob_hash,
              derived_at, projection_version
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            ON CONFLICT(session_id) DO UPDATE SET
              revision = excluded.revision,
              topic_id = excluded.topic_id,
              channel_id = excluded.channel_id,
              host_pubkey = excluded.host_pubkey,
              title = excluded.title,
              description = excluded.description,
              status = excluded.status,
              started_at = excluded.started_at,
              ended_at = excluded.ended_at,
              updated_at = excluded.updated_at,
              source_replica_id = excluded.source_replica_id,
              source_key = excluded.source_key,
              manifest_blob_hash = excluded.manifest_blob_hash,
              derived_at = excluded.derived_at,
              projection_version = excluded.projection_version
            WHERE excluded.revision > live_session_cache.revision
            "#,
        )
        .bind(row.session_id.as_str())
        .bind(row.revision)
        .bind(row.topic_id.as_str())
        .bind(row.channel_id.as_str())
        .bind(row.host_pubkey.as_str())
        .bind(row.title.as_str())
        .bind(row.description.as_str())
        .bind(live_status_name(&row.status))
        .bind(row.started_at)
        .bind(row.ended_at)
        .bind(row.updated_at)
        .bind(row.source_replica_id.as_str())
        .bind(row.source_key.as_str())
        .bind(row.manifest_blob_hash.as_str())
        .bind(row.derived_at)
        .bind(row.projection_version)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_channel_live_sessions(
        &self,
        topic_id: &str,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<LiveSessionProjectionRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = live_session_list_query("", topic_id, channel_id, limit)
            .build()
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(row_to_live_session_projection)
            .collect()
    }

    async fn get_live_session(
        &self,
        topic_id: &str,
        session_id: &str,
    ) -> Result<Option<LiveSessionProjectionRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "{LIVE_SESSION_SELECT} WHERE lsc.topic_id = ?1 AND lsc.session_id = ?2 LIMIT 1"
        )))
        .bind(topic_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_live_session_projection).transpose()
    }

    async fn upsert_game_room_cache(&self, row: GameRoomProjectionRow) -> Result<()> {
        let scores_json = serde_json::to_string(&row.scores)?;
        let metaverse_json = row
            .metaverse
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        sqlx::query(
            r#"
            INSERT INTO game_room_cache (
              room_id, score_revision, topic_id, channel_id, host_pubkey, title, description, status, phase_label,
              scores_json, room_kind, metaverse_json, updated_at, source_replica_id, source_key,
              manifest_blob_hash, derived_at, projection_version
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
            ON CONFLICT(room_id) DO UPDATE SET
              score_revision = excluded.score_revision,
              topic_id = excluded.topic_id,
              channel_id = excluded.channel_id,
              host_pubkey = excluded.host_pubkey,
              title = excluded.title,
              description = excluded.description,
              status = excluded.status,
              phase_label = excluded.phase_label,
              scores_json = excluded.scores_json,
              room_kind = excluded.room_kind,
              metaverse_json = excluded.metaverse_json,
              updated_at = excluded.updated_at,
              source_replica_id = excluded.source_replica_id,
              source_key = excluded.source_key,
              manifest_blob_hash = excluded.manifest_blob_hash,
              derived_at = excluded.derived_at,
              projection_version = excluded.projection_version
            WHERE excluded.score_revision IS NULL
               OR game_room_cache.score_revision IS NULL
               OR excluded.score_revision > game_room_cache.score_revision
            "#,
        )
        .bind(row.room_id.as_str())
        .bind(row.score_revision)
        .bind(row.topic_id.as_str())
        .bind(row.channel_id.as_str())
        .bind(row.host_pubkey.as_str())
        .bind(row.title.as_str())
        .bind(row.description.as_str())
        .bind(game_status_name(&row.status))
        .bind(row.phase_label.as_deref())
        .bind(scores_json)
        .bind(game_room_kind_name(&row.room_kind))
        .bind(metaverse_json.as_deref())
        .bind(row.updated_at)
        .bind(row.source_replica_id.as_str())
        .bind(row.source_key.as_str())
        .bind(row.manifest_blob_hash.as_str())
        .bind(row.derived_at)
        .bind(row.projection_version)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_channel_game_rooms(
        &self,
        topic_id: &str,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<GameRoomProjectionRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = game_room_list_query("", topic_id, channel_id, limit)
            .build()
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter().map(row_to_game_room_projection).collect()
    }

    async fn get_game_room(
        &self,
        topic_id: &str,
        room_id: &str,
    ) -> Result<Option<GameRoomProjectionRow>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "{GAME_ROOM_SELECT} WHERE topic_id = ?1 AND room_id = ?2 LIMIT 1"
        )))
        .bind(topic_id)
        .bind(room_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_game_room_projection).transpose()
    }

    async fn upsert_dome_connection_projection(
        &self,
        row: DomeConnectionProjectionRow,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dome_connection_projection_cache (
              context_id, topic_id, channel_id, snapshot_json, topology_digest,
              derived_at, projection_version
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(context_id) DO UPDATE SET
              topic_id = excluded.topic_id,
              channel_id = excluded.channel_id,
              snapshot_json = excluded.snapshot_json,
              topology_digest = excluded.topology_digest,
              derived_at = excluded.derived_at,
              projection_version = excluded.projection_version
            "#,
        )
        .bind(row.context_id)
        .bind(row.topic_id)
        .bind(row.channel_id)
        .bind(row.snapshot_json)
        .bind(row.topology_digest)
        .bind(row.derived_at)
        .bind(row.projection_version)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_dome_connection_projection(
        &self,
        context_id: &str,
    ) -> Result<Option<DomeConnectionProjectionRow>> {
        let row = sqlx::query(
            r#"
            SELECT context_id, topic_id, channel_id, snapshot_json, topology_digest,
                   derived_at, projection_version
            FROM dome_connection_projection_cache
            WHERE context_id = ?1
            "#,
        )
        .bind(context_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(DomeConnectionProjectionRow {
                context_id: row.try_get("context_id")?,
                topic_id: row.try_get("topic_id")?,
                channel_id: row.try_get("channel_id")?,
                snapshot_json: row.try_get("snapshot_json")?,
                topology_digest: row.try_get("topology_digest")?,
                derived_at: row.try_get("derived_at")?,
                projection_version: row.try_get("projection_version")?,
            })
        })
        .transpose()
    }

    async fn upsert_dome_hosting_projection(&self, row: DomeHostingProjectionRow) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dome_hosting_projection_cache (
              instance_id, context_id, topic_id, channel_id, state_json,
              lease_epoch, session_id, derived_at, projection_version
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(instance_id) DO UPDATE SET
              context_id = excluded.context_id,
              topic_id = excluded.topic_id,
              channel_id = excluded.channel_id,
              state_json = excluded.state_json,
              lease_epoch = excluded.lease_epoch,
              session_id = excluded.session_id,
              derived_at = excluded.derived_at,
              projection_version = excluded.projection_version
            "#,
        )
        .bind(row.instance_id)
        .bind(row.context_id)
        .bind(row.topic_id)
        .bind(row.channel_id)
        .bind(row.state_json)
        .bind(row.lease_epoch.map(|epoch| epoch as i64))
        .bind(row.session_id)
        .bind(row.derived_at)
        .bind(row.projection_version)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_dome_hosting_projection(
        &self,
        instance_id: &str,
    ) -> Result<Option<DomeHostingProjectionRow>> {
        let row = sqlx::query(
            r#"
            SELECT instance_id, context_id, topic_id, channel_id, state_json,
                   lease_epoch, session_id, derived_at, projection_version
            FROM dome_hosting_projection_cache
            WHERE instance_id = ?1
            "#,
        )
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let lease_epoch: Option<i64> = row.try_get("lease_epoch")?;
            Ok(DomeHostingProjectionRow {
                instance_id: row.try_get("instance_id")?,
                context_id: row.try_get("context_id")?,
                topic_id: row.try_get("topic_id")?,
                channel_id: row.try_get("channel_id")?,
                state_json: row.try_get("state_json")?,
                lease_epoch: lease_epoch.map(|epoch| epoch as u64),
                session_id: row.try_get("session_id")?,
                derived_at: row.try_get("derived_at")?,
                projection_version: row.try_get("projection_version")?,
            })
        })
        .transpose()
    }

    async fn upsert_live_presence(
        &self,
        topic_id: &str,
        channel_id: &str,
        session_id: &str,
        author_pubkey: &str,
        expires_at: i64,
        updated_at: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO live_presence_cache (
              topic_id, channel_id, session_id, author_pubkey, expires_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(topic_id, channel_id, session_id, author_pubkey) DO UPDATE SET
              expires_at = excluded.expires_at,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(topic_id)
        .bind(channel_id)
        .bind(session_id)
        .bind(author_pubkey)
        .bind(expires_at)
        .bind(updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn clear_expired_live_presence(&self, now_ms: i64) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM live_presence_cache
            WHERE expires_at <= ?1
            "#,
        )
        .bind(now_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn clear_topic_live_presence(&self, topic_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM live_presence_cache
            WHERE topic_id = ?1
            "#,
        )
        .bind(topic_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
