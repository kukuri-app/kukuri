use super::*;

impl MemoryStore {
    pub(super) async fn store_upsert_profile_impl(&self, profile: Profile) -> Result<()> {
        let mut profiles = self.profiles.write().await;
        match profiles.get(profile.pubkey.as_str()) {
            Some(existing) if existing.updated_at > profile.updated_at => {}
            _ => {
                profiles.insert(profile.pubkey.0.clone(), profile);
            }
        }
        Ok(())
    }

    pub(super) async fn store_get_profile_impl(&self, pubkey: &str) -> Result<Option<Profile>> {
        Ok(self.profiles.read().await.get(pubkey).cloned())
    }

    pub(super) async fn store_get_profiles_impl(
        &self,
        pubkeys: &[String],
    ) -> Result<HashMap<String, Profile>> {
        let profiles = self.profiles.read().await;
        Ok(pubkeys
            .iter()
            .filter_map(|pubkey| {
                profiles
                    .get(pubkey.as_str())
                    .cloned()
                    .map(|profile| (pubkey.clone(), profile))
            })
            .collect())
    }

    pub(super) async fn store_upsert_follow_edge_impl(&self, edge: FollowEdge) -> Result<()> {
        let key = (
            edge.subject_pubkey.as_str().to_string(),
            edge.target_pubkey.as_str().to_string(),
        );
        let mut follow_edges = self.follow_edges.write().await;
        match follow_edges.get(&key) {
            Some(existing) if existing.updated_at > edge.updated_at => {}
            _ => {
                follow_edges.insert(key, edge);
            }
        }
        Ok(())
    }

    pub(super) async fn store_list_follow_edges_by_subject_impl(
        &self,
        subject_pubkey: &str,
    ) -> Result<Vec<FollowEdge>> {
        let mut items = self
            .follow_edges
            .read()
            .await
            .values()
            .filter(|edge| edge.subject_pubkey.as_str() == subject_pubkey)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.target_pubkey.cmp(&right.target_pubkey))
        });
        Ok(items)
    }

    pub(super) async fn store_list_follow_edges_by_target_impl(
        &self,
        target_pubkey: &str,
    ) -> Result<Vec<FollowEdge>> {
        let mut items = self
            .follow_edges
            .read()
            .await
            .values()
            .filter(|edge| edge.target_pubkey.as_str() == target_pubkey)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.subject_pubkey.cmp(&right.subject_pubkey))
        });
        Ok(items)
    }

    pub(super) async fn store_upsert_block_edge_impl(&self, edge: BlockEdge) -> Result<()> {
        let key = (
            edge.subject_pubkey.as_str().to_string(),
            edge.target_pubkey.as_str().to_string(),
        );
        let mut block_edges = self.block_edges.write().await;
        match block_edges.get(&key) {
            Some(existing) if existing.updated_at > edge.updated_at => {}
            _ => {
                block_edges.insert(key, edge);
            }
        }
        Ok(())
    }

    pub(super) async fn store_list_block_edges_by_subject_impl(
        &self,
        subject_pubkey: &str,
    ) -> Result<Vec<BlockEdge>> {
        let mut items = self
            .block_edges
            .read()
            .await
            .values()
            .filter(|edge| edge.subject_pubkey.as_str() == subject_pubkey)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.target_pubkey.cmp(&right.target_pubkey))
        });
        Ok(items)
    }

    pub(super) async fn store_get_block_edge_impl(
        &self,
        subject_pubkey: &str,
        target_pubkey: &str,
    ) -> Result<Option<BlockEdge>> {
        Ok(self
            .block_edges
            .read()
            .await
            .get(&(subject_pubkey.to_string(), target_pubkey.to_string()))
            .cloned())
    }

    pub(super) async fn store_list_block_edges_by_target_impl(
        &self,
        target_pubkey: &str,
    ) -> Result<Vec<BlockEdge>> {
        let mut items = self
            .block_edges
            .read()
            .await
            .values()
            .filter(|edge| edge.target_pubkey.as_str() == target_pubkey)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.subject_pubkey.cmp(&right.subject_pubkey))
        });
        Ok(items)
    }
}

#[async_trait]
impl SocialProjectionStore for MemoryStore {
    async fn upsert_profile_cache(&self, profile: Profile) -> Result<()> {
        self.upsert_profile(profile).await
    }

    async fn get_author_relationship(
        &self,
        local_author_pubkey: &str,
        author_pubkey: &str,
    ) -> Result<Option<AuthorRelationshipProjectionRow>> {
        let edges = self.follow_edges.read().await;
        let active = |subject: &str, target: &str| {
            edges
                .get(&(subject.to_string(), target.to_string()))
                .is_some_and(|edge| edge.status == kukuri_core::FollowEdgeStatus::Active)
        };
        let via = edges
            .values()
            .filter(|edge| {
                edge.subject_pubkey.as_str() == local_author_pubkey
                    && edge.status == kukuri_core::FollowEdgeStatus::Active
                    && active(edge.target_pubkey.as_str(), author_pubkey)
            })
            .map(|edge| edge.target_pubkey.as_str().to_string())
            .collect();
        Ok(AuthorRelationshipProjectionRow::derive(
            local_author_pubkey,
            author_pubkey,
            active(local_author_pubkey, author_pubkey),
            active(author_pubkey, local_author_pubkey),
            via,
        ))
    }

    async fn put_muted_author(&self, row: MutedAuthorRow) -> Result<()> {
        self.muted_authors
            .write()
            .await
            .insert(row.author_pubkey.clone(), row);
        Ok(())
    }

    async fn get_muted_author(&self, author_pubkey: &str) -> Result<Option<MutedAuthorRow>> {
        Ok(self.muted_authors.read().await.get(author_pubkey).cloned())
    }

    async fn list_muted_authors(&self) -> Result<Vec<MutedAuthorRow>> {
        let mut items = self
            .muted_authors
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .muted_at
                .cmp(&left.muted_at)
                .then_with(|| left.author_pubkey.cmp(&right.author_pubkey))
        });
        Ok(items)
    }

    async fn get_author_docs_author(&self, author_pubkey: &str) -> Result<Option<String>> {
        Ok(self
            .author_docs_authors
            .read()
            .await
            .get(author_pubkey)
            .cloned())
    }

    async fn put_author_docs_author(&self, author_pubkey: &str, docs_author: &str) -> Result<()> {
        self.author_docs_authors
            .write()
            .await
            .insert(author_pubkey.to_string(), docs_author.to_string());
        Ok(())
    }

    async fn remove_muted_author(&self, author_pubkey: &str) -> Result<()> {
        self.muted_authors.write().await.remove(author_pubkey);
        Ok(())
    }

    async fn put_private_channel_participant(
        &self,
        row: PrivateChannelParticipantRow,
    ) -> Result<bool> {
        let mut rows = self.private_channel_participants.write().await;
        let key = (
            row.channel_id.clone(),
            row.epoch_id.clone(),
            row.participant_pubkey.clone(),
        );
        if rows
            .get(&key)
            .is_some_and(|existing| existing.updated_at >= row.updated_at)
        {
            return Ok(false);
        }
        if let Some(left_at) = row.left_at {
            for existing in rows.values_mut().filter(|existing| {
                existing.channel_id == row.channel_id
                    && existing.participant_pubkey == row.participant_pubkey
                    && existing.left_at.is_none()
                    && existing.updated_at < left_at
            }) {
                existing.left_at = Some(left_at);
                existing.updated_at = left_at;
            }
        }
        rows.insert(key, row);
        Ok(self.touched(1, true))
    }

    async fn list_private_channel_participants(
        &self,
        channel_id: &str,
        after: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let rows = self.private_channel_participants.read().await;
        let pubkeys = rows
            .values()
            .filter(|row| {
                row.channel_id == channel_id
                    && row.left_at.is_none()
                    && row.participant_pubkey.as_str() > after
            })
            .map(|row| row.participant_pubkey.clone())
            .collect::<BTreeSet<_>>();
        let page = pubkeys.into_iter().take(limit).collect::<Vec<_>>();
        Ok(self.touched(page.len(), page))
    }

    async fn is_active_private_channel_participant(
        &self,
        channel_id: &str,
        participant_pubkey: &str,
    ) -> Result<bool> {
        Ok(self
            .private_channel_participants
            .read()
            .await
            .values()
            .any(|row| {
                row.channel_id == channel_id
                    && row.participant_pubkey == participant_pubkey
                    && row.left_at.is_none()
            }))
    }

    async fn has_private_channel_member(&self, participant_pubkey: &str) -> Result<bool> {
        Ok(self
            .private_channel_participants
            .read()
            .await
            .values()
            .any(|row| row.participant_pubkey == participant_pubkey))
    }

    async fn has_private_channel_participant(
        &self,
        channel_id: &str,
        participant_pubkey: &str,
    ) -> Result<bool> {
        Ok(self
            .private_channel_participants
            .read()
            .await
            .values()
            .any(|row| {
                row.channel_id == channel_id && row.participant_pubkey == participant_pubkey
            }))
    }

    /// 試験用の実装なので、数と資格喪失の印を読むときに求める(SQLite は書込みで保つ)。読んだ行は 1 行と数える。
    async fn private_channel_participant_counts(
        &self,
        channel_id: &str,
        epoch_id: &str,
    ) -> Result<(usize, usize)> {
        let owner = self
            .private_channel_keys
            .read()
            .await
            .channels
            .values()
            .find(|row| row.channel_id == channel_id)
            .map(|row| row.owner_pubkey.clone());
        let follow_edges = self.follow_edges.read().await;
        let active = |subject: &str, target: &str| {
            follow_edges
                .get(&(subject.to_string(), target.to_string()))
                .is_some_and(|edge| edge.status == kukuri_core::FollowEdgeStatus::Active)
        };
        let rows = self.private_channel_participants.read().await;
        let members = rows.values().filter(|row| {
            row.channel_id == channel_id && row.epoch_id == epoch_id && row.left_at.is_none()
        });
        let (mut count, mut stale) = (0, 0);
        for row in members {
            count += 1;
            let pubkey = row.participant_pubkey.as_str();
            if owner.as_deref().is_some_and(|owner| {
                owner != pubkey && !(active(owner, pubkey) && active(pubkey, owner))
            }) {
                stale += 1;
            }
        }
        Ok(self.touched(1, (count, stale)))
    }
}
