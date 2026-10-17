use super::*;

impl MemoryStore {
    /// sqlite の `live_session_list_query` と同じ並び。`live_now` は配信中の行だけ。
    async fn list_live_sessions(
        &self,
        topic_id: &str,
        channel_id: &str,
        live_now: bool,
        limit: usize,
    ) -> Result<Vec<LiveSessionProjectionRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let presence = self.live_presence.read().await;
        let index = self.live_session_index.read().await;
        let rows = self.live_session_rows.read().await;
        let scope = (topic_id.to_string(), channel_id.to_string());
        let mut items = index
            .get(&scope)
            .into_iter()
            .flat_map(|entries| entries.iter())
            .filter_map(|(_, Reverse(session_id))| rows.get(session_id).cloned())
            .filter(|row| !live_now || row.status == LiveSessionStatus::Live)
            .take(limit)
            .collect::<Vec<_>>();
        for row in &mut items {
            row.viewer_count = if row.status == LiveSessionStatus::Ended {
                0
            } else {
                // sqlite の viewer_count 相関サブクエリ(topic_id, channel_id, session_id
                // が行と一致する live_presence_cache の COUNT)と同義
                presence
                    .iter()
                    .filter(
                        |((presence_topic, presence_channel, presence_session, _), _)| {
                            presence_topic == &row.topic_id
                                && presence_channel == &row.channel_id
                                && presence_session == &row.session_id
                        },
                    )
                    .count()
            };
        }
        Ok(items)
    }
}

#[async_trait]
impl LiveGameProjectionStore for MemoryStore {
    async fn upsert_live_session_cache(&self, row: LiveSessionProjectionRow) -> Result<()> {
        let mut index = self.live_session_index.write().await;
        let mut rows = self.live_session_rows.write().await;
        if let Some(previous) = rows.get(row.session_id.as_str()) {
            if previous.revision >= row.revision {
                return Ok(());
            }
            remove_projection_index_entry(
                &mut index,
                previous.topic_id.as_str(),
                previous.channel_id.as_str(),
                previous.started_at,
                previous.session_id.as_str(),
            );
        }
        insert_projection_index_entry(
            &mut index,
            row.topic_id.as_str(),
            row.channel_id.as_str(),
            row.started_at,
            row.session_id.as_str(),
        );
        rows.insert(row.session_id.clone(), row);
        Ok(())
    }

    async fn list_channel_live_sessions(
        &self,
        topic_id: &str,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<LiveSessionProjectionRow>> {
        self.list_live_sessions(topic_id, channel_id, false, limit)
            .await
    }

    async fn list_channel_live_now_sessions(
        &self,
        topic_id: &str,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<LiveSessionProjectionRow>> {
        self.list_live_sessions(topic_id, channel_id, true, limit)
            .await
    }

    async fn get_live_session(
        &self,
        topic_id: &str,
        session_id: &str,
    ) -> Result<Option<LiveSessionProjectionRow>> {
        let presence = self.live_presence.read().await;
        let mut row = self
            .live_session_rows
            .read()
            .await
            .get(session_id)
            .filter(|row| row.topic_id == topic_id)
            .cloned();
        if let Some(row) = row.as_mut() {
            row.viewer_count = if row.status == LiveSessionStatus::Ended {
                0
            } else {
                presence
                    .iter()
                    .filter(
                        |((presence_topic, presence_channel, presence_session, _), _)| {
                            presence_topic == &row.topic_id
                                && presence_channel == &row.channel_id
                                && presence_session == &row.session_id
                        },
                    )
                    .count()
            };
        }
        Ok(row)
    }

    async fn upsert_game_room_cache(&self, row: GameRoomProjectionRow) -> Result<()> {
        let mut index = self.game_room_index.write().await;
        let mut rows = self.game_room_rows.write().await;
        if let Some(previous) = rows.get(row.room_id.as_str()) {
            if matches!(
                (previous.score_revision, row.score_revision),
                (Some(previous), Some(next)) if previous >= next
            ) {
                return Ok(());
            }
            remove_projection_index_entry(
                &mut index,
                previous.topic_id.as_str(),
                previous.channel_id.as_str(),
                previous.updated_at,
                previous.room_id.as_str(),
            );
        }
        insert_projection_index_entry(
            &mut index,
            row.topic_id.as_str(),
            row.channel_id.as_str(),
            row.updated_at,
            row.room_id.as_str(),
        );
        rows.insert(row.room_id.clone(), row);
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
        let index = self.game_room_index.read().await;
        let rows = self.game_room_rows.read().await;
        let scope = (topic_id.to_string(), channel_id.to_string());
        Ok(index
            .get(&scope)
            .into_iter()
            .flat_map(|entries| entries.iter().take(limit))
            .filter_map(|(_, Reverse(room_id))| rows.get(room_id).cloned())
            .collect())
    }

    async fn get_game_room(
        &self,
        topic_id: &str,
        room_id: &str,
    ) -> Result<Option<GameRoomProjectionRow>> {
        Ok(self
            .game_room_rows
            .read()
            .await
            .get(room_id)
            .filter(|row| row.topic_id == topic_id)
            .cloned())
    }

    async fn upsert_dome_connection_projection(
        &self,
        row: DomeConnectionProjectionRow,
    ) -> Result<()> {
        self.dome_connection_rows
            .write()
            .await
            .insert(row.context_id.clone(), row);
        Ok(())
    }

    async fn get_dome_connection_projection(
        &self,
        context_id: &str,
    ) -> Result<Option<DomeConnectionProjectionRow>> {
        Ok(self
            .dome_connection_rows
            .read()
            .await
            .get(context_id)
            .cloned())
    }

    async fn upsert_dome_hosting_projection(&self, row: DomeHostingProjectionRow) -> Result<()> {
        self.dome_hosting_rows
            .write()
            .await
            .insert(row.instance_id.clone(), row);
        Ok(())
    }

    async fn get_dome_hosting_projection(
        &self,
        instance_id: &str,
    ) -> Result<Option<DomeHostingProjectionRow>> {
        Ok(self
            .dome_hosting_rows
            .read()
            .await
            .get(instance_id)
            .cloned())
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
        // キーは sqlite の ON CONFLICT(topic_id, channel_id, session_id, author_pubkey)
        // と同義(topic_id を含めないと別 topic の presence を上書きしてしまう)
        self.live_presence.write().await.insert(
            (
                topic_id.to_string(),
                channel_id.to_string(),
                session_id.to_string(),
                author_pubkey.to_string(),
            ),
            (expires_at, updated_at),
        );
        Ok(())
    }

    async fn clear_expired_live_presence(&self, now_ms: i64) -> Result<()> {
        self.live_presence
            .write()
            .await
            .retain(|_, (expires_at, _)| *expires_at > now_ms);
        Ok(())
    }

    async fn clear_topic_live_presence(&self, topic_id: &str) -> Result<()> {
        self.live_presence
            .write()
            .await
            .retain(|(presence_topic, _, _, _), _| presence_topic != topic_id);
        Ok(())
    }
}

fn insert_projection_index_entry(
    index: &mut ProjectionIndex,
    topic_id: &str,
    channel_id: &str,
    timestamp: i64,
    id: &str,
) {
    index
        .entry((topic_id.to_string(), channel_id.to_string()))
        .or_default()
        .insert((Reverse(timestamp), Reverse(id.to_string())));
}

fn remove_projection_index_entry(
    index: &mut ProjectionIndex,
    topic_id: &str,
    channel_id: &str,
    timestamp: i64,
    id: &str,
) {
    let scope = (topic_id.to_string(), channel_id.to_string());
    let remove_scope = index.get_mut(&scope).is_some_and(|entries| {
        entries.remove(&(Reverse(timestamp), Reverse(id.to_string())));
        entries.is_empty()
    });
    if remove_scope {
        index.remove(&scope);
    }
}
