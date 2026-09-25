use crate::service::*;

impl AppService {
    pub async fn warm_social_graph(&self) -> Result<()> {
        let local_author = self.current_author_pubkey();
        self.ensure_author_subscription(local_author.as_str())
            .await?;
        for edge in self
            .services
            .store
            .list_follow_edges_by_subject(local_author.as_str())
            .await?
        {
            if edge.status == FollowEdgeStatus::Active {
                self.ensure_author_subscription(edge.target_pubkey.as_str())
                    .await?;
            }
        }
        for edge in self
            .services
            .store
            .list_block_edges_by_subject(local_author.as_str())
            .await?
        {
            if edge.status == BlockEdgeStatus::Active {
                self.ensure_author_subscription(edge.target_pubkey.as_str())
                    .await?;
                self.reconcile_blocked_dome_connections(&edge.target_pubkey)
                    .await?;
            }
        }
        for edge in self
            .services
            .store
            .list_block_edges_by_target(local_author.as_str())
            .await?
        {
            if edge.status == BlockEdgeStatus::Active {
                self.ensure_author_subscription(edge.subject_pubkey.as_str())
                    .await?;
                self.reconcile_blocked_dome_connections(&edge.subject_pubkey)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn get_my_profile(&self) -> Result<Profile> {
        let local_author = self.current_author_pubkey();
        self.ensure_author_subscription(local_author.as_str())
            .await?;
        Ok(self
            .services
            .store
            .get_profile(local_author.as_str())
            .await?
            .unwrap_or(Profile {
                pubkey: Pubkey::from(local_author),
                ..Profile::default()
            }))
    }

    pub async fn set_my_profile(&self, input: ProfileInput) -> Result<Profile> {
        let envelope = self.prepare_my_profile(input).await?;
        self.commit_my_profile(envelope).await
    }

    /// Prepare once so an account setup transaction can durably retain the
    /// signed operation before publishing it. Retrying commits the same ID.
    pub async fn prepare_my_profile(&self, input: ProfileInput) -> Result<KukuriEnvelope> {
        let author_pubkey = Pubkey::from(self.current_author_pubkey());
        // Normalize and length-check the text fields before any blob upload so invalid
        // input fails fast without persisting a side effect.
        let name = normalize_optional_text(input.name);
        let display_name = normalize_optional_text(input.display_name);
        let about = normalize_optional_text(input.about);
        ensure_optional_text_within_limit("profile name", name.as_deref(), MAX_PROFILE_NAME_CHARS)?;
        ensure_optional_text_within_limit(
            "profile display name",
            display_name.as_deref(),
            MAX_PROFILE_DISPLAY_NAME_CHARS,
        )?;
        ensure_optional_text_within_limit(
            "profile about",
            about.as_deref(),
            MAX_PROFILE_ABOUT_CHARS,
        )?;
        let current_profile = self.get_my_profile().await?;
        let picture_asset = if input.clear_picture {
            None
        } else if let Some(upload) = input.picture_upload {
            let stored = self
                .services
                .blob_service
                .put_blob(upload.bytes, upload.mime.as_str())
                .await?;
            Some(kukuri_core::AssetRef {
                hash: stored.hash,
                mime: stored.mime,
                bytes: stored.bytes,
                role: AssetRole::ProfileAvatar,
            })
        } else {
            current_profile.picture_asset.clone()
        };
        // #1239 / ADR 0053 §6: 読む側が docs author と key の組で読めるよう、docs author を申告する。
        let docs_author = self.services.docs_sync.local_docs_author().await?;
        build_profile_envelope_with_docs_author(
            self.services.keys.as_ref(),
            &KukuriProfileEnvelopeContentV1 {
                author_pubkey: author_pubkey.clone(),
                name,
                display_name,
                about,
                picture_asset,
            },
            docs_author.as_deref(),
        )
    }

    pub async fn commit_my_profile(&self, envelope: KukuriEnvelope) -> Result<Profile> {
        envelope.verify()?;
        if envelope.pubkey.as_str() != self.current_author_pubkey() {
            anyhow::bail!("profile operation belongs to another account");
        }
        let profile = parse_profile(&envelope)?
            .ok_or_else(|| anyhow::anyhow!("failed to parse profile envelope"))?;
        let current = self.get_my_profile().await?;
        if current.updated_at > profile.updated_at {
            return Ok(current);
        }
        self.services.store.put_envelope(envelope.clone()).await?;
        self.services
            .projection_store
            .upsert_profile_cache(profile.clone())
            .await?;
        persist_profile_doc(self.services.docs_sync.as_ref(), &profile, &envelope).await?;
        *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
        Ok(profile)
    }

    pub async fn follow_author(&self, pubkey: &str) -> Result<AuthorSocialView> {
        let target_pubkey = Pubkey::from(normalize_author_pubkey(pubkey)?);
        let docs_author = self.services.docs_sync.local_docs_author().await?;
        let envelope = build_follow_edge_envelope_with_docs_author(
            self.services.keys.as_ref(),
            &target_pubkey,
            FollowEdgeStatus::Active,
            docs_author.as_deref(),
        )?;
        let edge = parse_follow_edge(&envelope)?
            .ok_or_else(|| anyhow::anyhow!("failed to parse follow edge"))?;
        self.services.store.put_envelope(envelope.clone()).await?;
        persist_follow_edge_doc(self.services.docs_sync.as_ref(), &edge, &envelope).await?;
        self.ensure_author_subscription(target_pubkey.as_str())
            .await?;
        *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
        let view = self
            .build_author_social_view(target_pubkey.as_str())
            .await?;
        self.queue_public_notification_offer(
            PublicNotificationSource::Follow { envelope },
            BTreeSet::from([target_pubkey.as_str().to_string()]),
        )
        .await;
        Ok(view)
    }

    pub async fn unfollow_author(&self, pubkey: &str) -> Result<AuthorSocialView> {
        let target_pubkey = Pubkey::from(normalize_author_pubkey(pubkey)?);
        let docs_author = self.services.docs_sync.local_docs_author().await?;
        let envelope = build_follow_edge_envelope_with_docs_author(
            self.services.keys.as_ref(),
            &target_pubkey,
            FollowEdgeStatus::Revoked,
            docs_author.as_deref(),
        )?;
        let edge = parse_follow_edge(&envelope)?
            .ok_or_else(|| anyhow::anyhow!("failed to parse follow edge"))?;
        self.services.store.put_envelope(envelope.clone()).await?;
        persist_follow_edge_doc(self.services.docs_sync.as_ref(), &edge, &envelope).await?;
        self.ensure_author_subscription(target_pubkey.as_str())
            .await?;
        *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
        let view = self
            .build_author_social_view(target_pubkey.as_str())
            .await?;
        // #1221 R4-D: 相手の手元の edge を更新する(mutual の解除)。通知は作られない。
        self.queue_public_notification_offer(
            PublicNotificationSource::Follow { envelope },
            BTreeSet::from([target_pubkey.as_str().to_string()]),
        )
        .await;
        Ok(view)
    }

    pub async fn get_author_social_view(&self, pubkey: &str) -> Result<AuthorSocialView> {
        let author_pubkey = normalize_author_pubkey(pubkey)?;
        self.ensure_author_subscription(author_pubkey.as_str())
            .await?;
        if self
            .authors_blocked_either_direction(
                self.current_author_pubkey().as_str(),
                author_pubkey.as_str(),
            )
            .await?
        {
            self.reconcile_blocked_dome_connections(&Pubkey::from(author_pubkey.clone()))
                .await?;
        }
        self.build_author_social_view(author_pubkey.as_str()).await
    }

    pub async fn mute_author(&self, pubkey: &str) -> Result<AuthorSocialView> {
        let author_pubkey = normalize_author_pubkey(pubkey)?;
        self.ensure_author_subscription(author_pubkey.as_str())
            .await?;
        self.services
            .projection_store
            .put_muted_author(MutedAuthorRow {
                author_pubkey: author_pubkey.clone(),
                muted_at: Utc::now().timestamp_millis(),
            })
            .await?;
        self.build_author_social_view(author_pubkey.as_str()).await
    }

    pub async fn unmute_author(&self, pubkey: &str) -> Result<AuthorSocialView> {
        let author_pubkey = normalize_author_pubkey(pubkey)?;
        self.ensure_author_subscription(author_pubkey.as_str())
            .await?;
        self.services
            .projection_store
            .remove_muted_author(author_pubkey.as_str())
            .await?;
        self.build_author_social_view(author_pubkey.as_str()).await
    }

    pub async fn block_author(&self, pubkey: &str) -> Result<AuthorSocialView> {
        self.set_block_edge(pubkey, BlockEdgeStatus::Active).await
    }

    pub async fn unblock_author(&self, pubkey: &str) -> Result<AuthorSocialView> {
        self.set_block_edge(pubkey, BlockEdgeStatus::Revoked).await
    }

    async fn set_block_edge(
        &self,
        pubkey: &str,
        status: BlockEdgeStatus,
    ) -> Result<AuthorSocialView> {
        let target_pubkey = Pubkey::from(normalize_author_pubkey(pubkey)?);
        let docs_author = self.services.docs_sync.local_docs_author().await?;
        let envelope = build_block_edge_envelope_with_docs_author(
            self.services.keys.as_ref(),
            &target_pubkey,
            status,
            docs_author.as_deref(),
        )?;
        let edge = parse_block_edge(&envelope)?
            .ok_or_else(|| anyhow::anyhow!("failed to parse block edge"))?;
        self.services.store.put_envelope(envelope.clone()).await?;
        persist_block_edge_doc(self.services.docs_sync.as_ref(), &edge, &envelope).await?;
        self.ensure_author_subscription(target_pubkey.as_str())
            .await?;
        *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
        if edge.status == BlockEdgeStatus::Active {
            self.reconcile_blocked_dome_connections(&target_pubkey)
                .await?;
        }
        self.build_author_social_view(target_pubkey.as_str()).await
    }

    pub async fn list_social_connections(
        &self,
        kind: SocialConnectionKind,
    ) -> Result<Vec<AuthorSocialView>> {
        let local_author_pubkey = self.current_author_pubkey();
        let pubkeys = match kind {
            SocialConnectionKind::Following => self
                .services
                .store
                .list_follow_edges_by_subject(local_author_pubkey.as_str())
                .await?
                .into_iter()
                .filter(|edge| edge.status == FollowEdgeStatus::Active)
                .map(|edge| edge.target_pubkey.as_str().to_string())
                .collect::<BTreeSet<_>>(),
            SocialConnectionKind::Followed => self
                .services
                .store
                .list_follow_edges_by_target(local_author_pubkey.as_str())
                .await?
                .into_iter()
                .filter(|edge| edge.status == FollowEdgeStatus::Active)
                .map(|edge| edge.subject_pubkey.as_str().to_string())
                .collect::<BTreeSet<_>>(),
            SocialConnectionKind::Muted => self
                .services
                .projection_store
                .list_muted_authors()
                .await?
                .into_iter()
                .map(|row| row.author_pubkey)
                .collect::<BTreeSet<_>>(),
            SocialConnectionKind::Blocking => self
                .services
                .store
                .list_block_edges_by_subject(local_author_pubkey.as_str())
                .await?
                .into_iter()
                .filter(|edge| edge.status == BlockEdgeStatus::Active)
                .map(|edge| edge.target_pubkey.as_str().to_string())
                .collect::<BTreeSet<_>>(),
            SocialConnectionKind::BlockedBy => self
                .services
                .store
                .list_block_edges_by_target(local_author_pubkey.as_str())
                .await?
                .into_iter()
                .filter(|edge| edge.status == BlockEdgeStatus::Active)
                .map(|edge| edge.subject_pubkey.as_str().to_string())
                .collect::<BTreeSet<_>>(),
        };
        let mut items = Vec::with_capacity(pubkeys.len());
        for author_pubkey in pubkeys {
            items.push(
                self.build_author_social_view(author_pubkey.as_str())
                    .await?,
            );
        }
        items.sort_by(author_social_view_sort_key);
        Ok(items)
    }
}
