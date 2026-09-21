use super::*;

/// thread の 1 ページ(`SqliteStore` と同じ意味)。`items` は root が先頭、返信は古い順に並んでいる。
///
/// root を返すのは、cursor の無い最初のページだけ。root の位置の cursor は「返信の先頭から」を表す
/// (返信の時刻が root より前でも飛ばさない)。それ以外の cursor は、返信の中での位置。
fn thread_page(
    mut items: Vec<ObjectProjectionRow>,
    thread_root_object_id: &EnvelopeId,
    cursor: Option<TimelineCursor>,
    limit: usize,
) -> Page<ObjectProjectionRow> {
    let Some(cursor) = cursor else {
        return apply_asc_projection_cursor(items, None, limit);
    };
    items.retain(|row| row.object_id != *thread_root_object_id);
    let after = (cursor.object_id != *thread_root_object_id).then_some(cursor);
    apply_asc_projection_cursor(items, after, limit)
}

#[async_trait]
impl ObjectProjectionStore for MemoryStore {
    async fn put_object_projection(&self, row: ObjectProjectionRow) -> Result<()> {
        self.put_object_projections(vec![row]).await
    }

    async fn put_object_projections(&self, rows: Vec<ObjectProjectionRow>) -> Result<()> {
        let mut projections = self.object_projection_rows.write().await;
        let mut adult_hashes = self.adult_media_hashes.write().await;
        for row in rows {
            for hash in crate::models::adult_media_hashes_for_row(&row) {
                adult_hashes.insert(hash.to_string());
            }
            projections.insert(row.object_id.clone(), row);
        }
        Ok(())
    }

    async fn mark_adult_media_hashes(&self, hashes: &[BlobHash]) -> Result<()> {
        let mut adult_hashes = self.adult_media_hashes.write().await;
        for hash in hashes {
            adult_hashes.insert(hash.as_str().to_string());
        }
        Ok(())
    }

    async fn is_adult_media_hash(&self, hash: &BlobHash) -> Result<bool> {
        Ok(self.adult_media_hashes.read().await.contains(hash.as_str()))
    }

    async fn get_object_projection(
        &self,
        object_id: &EnvelopeId,
    ) -> Result<Option<ObjectProjectionRow>> {
        Ok(self
            .object_projection_rows
            .read()
            .await
            .get(object_id)
            .cloned())
    }

    async fn find_author_reposts_of(
        &self,
        topic_id: &str,
        author_pubkey: &str,
        source_object_id: &EnvelopeId,
        limit: usize,
    ) -> Result<Vec<ObjectProjectionRow>> {
        let mut rows =
            self.object_projection_rows
                .read()
                .await
                .values()
                .filter(|row| {
                    row.object_kind == "repost"
                        && row.topic_id == topic_id
                        && row.author_pubkey == author_pubkey
                        && row.repost_of.as_ref().is_some_and(|repost_of| {
                            &repost_of.source_object_id == source_object_id
                        })
                })
                .cloned()
                .collect::<Vec<_>>();
        rows.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        rows.truncate(limit);
        Ok(rows)
    }

    async fn list_topic_timeline(
        &self,
        topic_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        let mut items = self
            .object_projection_rows
            .read()
            .await
            .values()
            .filter(|row| row.topic_id == topic_id)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.object_id.cmp(&left.object_id))
        });
        Ok(apply_desc_projection_cursor(items, cursor, limit))
    }

    async fn list_topic_timeline_filtered(
        &self,
        topic_id: &str,
        allowed_channels: &BTreeSet<String>,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        let mut items = self
            .object_projection_rows
            .read()
            .await
            .values()
            .filter(|row| {
                row.topic_id == topic_id && allowed_channels.contains(row.channel_id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.object_id.cmp(&left.object_id))
        });
        Ok(apply_desc_projection_cursor(items, cursor, limit))
    }

    async fn list_thread(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        let mut items = self
            .object_projection_rows
            .read()
            .await
            .values()
            .filter(|row| {
                row.topic_id == topic_id
                    && (row.object_id == *thread_root_object_id
                        || row
                            .root_object_id
                            .as_ref()
                            .is_some_and(|root| root == thread_root_object_id))
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            let left_root = left.object_id == *thread_root_object_id;
            let right_root = right.object_id == *thread_root_object_id;
            left_root
                .cmp(&right_root)
                .reverse()
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left.object_id.cmp(&right.object_id))
        });
        Ok(thread_page(items, thread_root_object_id, cursor, limit))
    }

    async fn list_thread_filtered(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        allowed_channel: Option<&str>,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        let mut items = self
            .object_projection_rows
            .read()
            .await
            .values()
            .filter(|row| {
                row.topic_id == topic_id
                    && allowed_channel.is_none_or(|channel_id| row.channel_id == channel_id)
                    && (row.object_id == *thread_root_object_id
                        || row
                            .root_object_id
                            .as_ref()
                            .is_some_and(|root| root == thread_root_object_id))
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            let left_root = left.object_id == *thread_root_object_id;
            let right_root = right.object_id == *thread_root_object_id;
            left_root
                .cmp(&right_root)
                .reverse()
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left.object_id.cmp(&right.object_id))
        });
        Ok(thread_page(items, thread_root_object_id, cursor, limit))
    }

    async fn rebuild_object_projections(&self, rows: Vec<ObjectProjectionRow>) -> Result<()> {
        let retained_object_ids = rows
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<HashSet<_>>();
        let mut guard = self.object_projection_rows.write().await;
        guard.clear();
        self.adult_media_hashes.write().await.clear();
        {
            let mut adult_hashes = self.adult_media_hashes.write().await;
            for row in rows {
                for hash in crate::models::adult_media_hashes_for_row(&row) {
                    adult_hashes.insert(hash.to_string());
                }
                guard.insert(row.object_id.clone(), row);
            }
        }
        self.live_session_rows.write().await.clear();
        self.game_room_rows.write().await.clear();
        self.live_presence.write().await.clear();
        self.reaction_projection_rows.write().await.clear();
        self.content_observation_rows
            .write()
            .await
            .retain(|_, observation| {
                observation.subject_kind != "post"
                    || retained_object_ids.contains(observation.subject_id.as_str())
            });
        Ok(())
    }
}
