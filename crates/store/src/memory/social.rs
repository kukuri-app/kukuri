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
        Ok(self
            .author_relationship_rows
            .read()
            .await
            .get(&(local_author_pubkey.to_string(), author_pubkey.to_string()))
            .cloned())
    }

    async fn list_author_relationships(
        &self,
        local_author_pubkey: &str,
        author_pubkeys: &[String],
    ) -> Result<HashMap<String, AuthorRelationshipProjectionRow>> {
        let relationships = self.author_relationship_rows.read().await;
        Ok(author_pubkeys
            .iter()
            .filter_map(|author_pubkey| {
                relationships
                    .get(&(local_author_pubkey.to_string(), author_pubkey.clone()))
                    .cloned()
                    .map(|relationship| (author_pubkey.clone(), relationship))
            })
            .collect())
    }

    async fn rebuild_author_relationships(
        &self,
        local_author_pubkey: &str,
        rows: Vec<AuthorRelationshipProjectionRow>,
    ) -> Result<()> {
        let mut guard = self.author_relationship_rows.write().await;
        guard.retain(|(local_author, _), _| local_author != local_author_pubkey);
        for row in rows {
            guard.insert(
                (row.local_author_pubkey.clone(), row.author_pubkey.clone()),
                row,
            );
        }
        Ok(())
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
}
