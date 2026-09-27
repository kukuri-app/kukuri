use super::*;

impl AppService {
    pub(crate) async fn build_author_social_view(
        &self,
        author_pubkey: &str,
    ) -> Result<AuthorSocialView> {
        let profile = self.services.store.get_profile(author_pubkey).await?;
        let relationship = self
            .services
            .projection_store
            .get_author_relationship(self.current_author_pubkey().as_str(), author_pubkey)
            .await?;
        let muted = self
            .services
            .projection_store
            .get_muted_author(author_pubkey)
            .await?
            .is_some();
        let local_author = self.current_author_pubkey();
        let blocking = self
            .services
            .store
            .list_block_edges_by_subject(local_author.as_str())
            .await?
            .into_iter()
            .any(|edge| {
                edge.target_pubkey.as_str() == author_pubkey
                    && edge.status == BlockEdgeStatus::Active
            });
        let blocked_by = self
            .services
            .store
            .list_block_edges_by_target(local_author.as_str())
            .await?
            .into_iter()
            .any(|edge| {
                edge.subject_pubkey.as_str() == author_pubkey
                    && edge.status == BlockEdgeStatus::Active
            });
        let mut view = author_social_view_from_parts(
            author_pubkey,
            profile.as_ref(),
            relationship.as_ref(),
            muted,
            blocking,
            blocked_by,
        );
        view.provenance = self
            .content_provenance_view("profile", author_pubkey, "author_docs")
            .await?;
        Ok(view)
    }

    pub(crate) async fn current_muted_author_pubkeys(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .services
            .projection_store
            .list_muted_authors()
            .await?
            .into_iter()
            .map(|row| row.author_pubkey)
            .collect())
    }

    /// #961: content surface から隠す author の集合。ミュート(端末内)に加え、どちらの向きでも
    /// Active な署名済み block edge を持つ相手を含める。取得・保存は禁止せず表示だけを隠す。
    pub(crate) async fn current_hidden_author_pubkeys(&self) -> Result<BTreeSet<String>> {
        let mut hidden = self.current_muted_author_pubkeys().await?;
        let local_author = self.current_author_pubkey();
        hidden.extend(
            self.services
                .store
                .list_block_edges_by_subject(local_author.as_str())
                .await?
                .into_iter()
                .filter(|edge| edge.status == BlockEdgeStatus::Active)
                .map(|edge| edge.target_pubkey.as_str().to_string()),
        );
        hidden.extend(
            self.services
                .store
                .list_block_edges_by_target(local_author.as_str())
                .await?
                .into_iter()
                .filter(|edge| edge.status == BlockEdgeStatus::Active)
                .map(|edge| edge.subject_pubkey.as_str().to_string()),
        );
        Ok(hidden)
    }

    pub(crate) async fn authors_blocked_either_direction(
        &self,
        left_pubkey: &str,
        right_pubkey: &str,
    ) -> Result<bool> {
        let left_blocks_right = self
            .services
            .store
            .list_block_edges_by_subject(left_pubkey)
            .await?
            .into_iter()
            .any(|edge| {
                edge.target_pubkey.as_str() == right_pubkey
                    && edge.status == BlockEdgeStatus::Active
            });
        if left_blocks_right {
            return Ok(true);
        }
        Ok(self
            .services
            .store
            .list_block_edges_by_subject(right_pubkey)
            .await?
            .into_iter()
            .any(|edge| {
                edge.target_pubkey.as_str() == left_pubkey && edge.status == BlockEdgeStatus::Active
            }))
    }

    pub(crate) async fn owner_blocks_visitor(
        &self,
        owner_pubkey: &str,
        visitor_pubkey: &str,
    ) -> Result<bool> {
        Ok(self
            .services
            .store
            .list_block_edges_by_subject(owner_pubkey)
            .await?
            .into_iter()
            .any(|edge| {
                edge.target_pubkey.as_str() == visitor_pubkey
                    && edge.status == BlockEdgeStatus::Active
            }))
    }
}
