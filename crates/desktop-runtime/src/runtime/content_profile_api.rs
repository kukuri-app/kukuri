use super::*;

use kukuri_core::TrustObservationKind;
use tracing::warn;

impl DesktopRuntime {
    pub(crate) async fn ensure_desired_subscription(
        &self,
        subscription: &DesiredSubscription,
    ) -> Result<()> {
        let scope = match &subscription.scope {
            DesiredSubscriptionScope::Public => TimelineScope::Public,
            DesiredSubscriptionScope::Channel { channel_id } => TimelineScope::Channel {
                channel_id: kukuri_core::ChannelId::new(channel_id),
            },
        };
        self.app_service
            .ensure_scope_subscriptions(subscription.topic.as_str(), &scope)
            .await
    }

    pub(crate) async fn remove_desired_subscription(
        &self,
        subscription: &DesiredSubscription,
        topic_still_desired: bool,
    ) -> Result<()> {
        match (&subscription.scope, topic_still_desired) {
            (_, false) => {
                self.app_service
                    .unsubscribe_topic(subscription.topic.as_str())
                    .await
            }
            (DesiredSubscriptionScope::Channel { channel_id }, true) => {
                self.app_service
                    .unsubscribe_private_channel(subscription.topic.as_str(), channel_id.as_str())
                    .await
            }
            (DesiredSubscriptionScope::Public, true) => Ok(()),
        }
    }

    pub async fn create_post(&self, request: CreatePostRequest) -> Result<String> {
        let attachments = request
            .attachments
            .into_iter()
            .map(pending_attachment_from_request)
            .collect::<Result<Vec<_>>>()?;
        self.app_service
            .create_post_with_attachments_in_channel(
                request.topic.as_str(),
                request.channel_ref,
                request.content.as_str(),
                request.reply_to.as_deref(),
                attachments,
                request.content_labels,
            )
            .await
    }

    /// #858: 成人向け表現の表示設定(既定 OFF)。canonical source はローカル JSON。
    pub fn get_content_display_settings(&self) -> kukuri_app_api::ContentDisplaySettings {
        kukuri_app_api::ContentDisplaySettings {
            adult_content_enabled: self.app_service.adult_content_display_enabled(),
        }
    }

    pub fn set_adult_content_display_enabled(
        &self,
        enabled: bool,
    ) -> Result<kukuri_app_api::ContentDisplaySettings> {
        save_content_display_settings(
            &self.db_path,
            &ContentDisplaySettingsState {
                adult_content_enabled: enabled,
            },
        )?;
        self.app_service.set_adult_content_display_enabled(enabled);
        Ok(kukuri_app_api::ContentDisplaySettings {
            adult_content_enabled: enabled,
        })
    }

    pub async fn withdraw_post(&self, request: WithdrawPostRequest) -> Result<String> {
        let reason_visibility = match request.reason_visibility {
            WithdrawalReasonVisibilityRequest::Public => {
                kukuri_core::WithdrawalReasonVisibility::Public
            }
            WithdrawalReasonVisibilityRequest::Private => {
                kukuri_core::WithdrawalReasonVisibility::Private
            }
        };
        let reason = request.reason.map(|reason| match reason {
            PostWithdrawalReasonRequest::AuthorRequest => {
                kukuri_core::PostWithdrawalReason::AuthorRequest
            }
            PostWithdrawalReasonRequest::Correction => {
                kukuri_core::PostWithdrawalReason::Correction
            }
            PostWithdrawalReasonRequest::Privacy => kukuri_core::PostWithdrawalReason::Privacy,
            PostWithdrawalReasonRequest::Other => kukuri_core::PostWithdrawalReason::Other,
        });
        self.app_service
            .withdraw_post(
                request.topic.as_str(),
                request.object_id.as_str(),
                request.channel_ref,
                request.replacement_object_id.as_deref(),
                reason_visibility,
                reason,
            )
            .await
    }

    pub async fn create_repost(&self, request: CreateRepostRequest) -> Result<String> {
        self.app_service
            .create_repost(
                request.topic.as_str(),
                request.source_topic.as_str(),
                request.source_object_id.as_str(),
                request.commentary.as_deref(),
            )
            .await
    }

    pub async fn toggle_reaction(
        &self,
        request: ToggleReactionRequest,
    ) -> Result<ReactionStateView> {
        self.app_service
            .toggle_reaction(
                request.target_topic_id.as_str(),
                request.target_object_id.as_str(),
                reaction_key_from_request(request.reaction_key)?,
                request.channel_ref,
            )
            .await
    }

    pub async fn list_my_custom_reaction_assets(&self) -> Result<Vec<CustomReactionAssetView>> {
        self.app_service.list_my_custom_reaction_assets().await
    }

    pub async fn list_recent_reactions(
        &self,
        request: ListRecentReactionsRequest,
    ) -> Result<Vec<RecentReactionView>> {
        self.app_service
            .list_recent_reactions(request.limit.unwrap_or(8))
            .await
    }

    pub async fn create_custom_reaction_asset(
        &self,
        request: CreateCustomReactionAssetRequest,
    ) -> Result<CustomReactionAssetView> {
        let upload = request.upload;
        let raw = BASE64_STANDARD
            .decode(upload.data_base64.as_bytes())
            .context("failed to decode custom reaction upload")?;
        let normalized =
            normalize_custom_reaction_upload(raw, upload.mime.as_str(), &request.crop_rect)?;
        self.app_service
            .create_custom_reaction_asset(CreateCustomReactionAssetInput {
                search_key: request.search_key,
                mime: normalized.mime,
                bytes: normalized.bytes,
                width: 128,
                height: 128,
            })
            .await
    }

    pub async fn list_bookmarked_custom_reactions(
        &self,
    ) -> Result<Vec<BookmarkedCustomReactionView>> {
        self.app_service.list_bookmarked_custom_reactions().await
    }

    pub async fn bookmark_custom_reaction(
        &self,
        request: BookmarkCustomReactionRequest,
    ) -> Result<BookmarkedCustomReactionView> {
        self.app_service
            .bookmark_custom_reaction(CustomReactionAssetSnapshotV1 {
                asset_id: request.asset_id,
                owner_pubkey: request.owner_pubkey.into(),
                blob_hash: BlobHash::new(request.blob_hash),
                search_key: request.search_key,
                mime: request.mime,
                bytes: request.bytes,
                width: request.width,
                height: request.height,
            })
            .await
    }

    pub async fn remove_bookmarked_custom_reaction(
        &self,
        request: RemoveBookmarkedCustomReactionRequest,
    ) -> Result<()> {
        self.app_service
            .remove_bookmarked_custom_reaction(request.asset_id.as_str())
            .await
    }

    pub async fn list_bookmarked_posts_page(
        &self,
        request: ListBookmarkedPostsRequest,
    ) -> Result<BookmarkedPostPageView> {
        self.app_service
            .list_bookmarked_posts_page(request.cursor.as_ref(), request.before)
            .await
    }

    /// Explicit CLI/export listing; UI uses `list_bookmarked_posts_page`.
    pub async fn list_bookmarked_posts(&self) -> Result<Vec<BookmarkedPostView>> {
        self.app_service.list_bookmarked_posts().await
    }

    pub async fn bookmarked_post_ids(
        &self,
        request: BookmarkedPostIdsRequest,
    ) -> Result<Vec<String>> {
        let ids = request
            .object_ids
            .into_iter()
            .map(EnvelopeId::from)
            .collect::<Vec<_>>();
        self.app_service.bookmarked_post_ids(&ids).await
    }

    pub async fn resolve_community_index_posts(
        &self,
        request: ResolveCommunityIndexPostsRequest,
    ) -> Result<CommunityIndexPostResolveResponse> {
        self.app_service
            .resolve_community_index_posts(request.entries)
            .await
    }

    pub async fn bookmark_post(&self, request: BookmarkPostRequest) -> Result<BookmarkedPostView> {
        self.app_service
            .bookmark_post_in_channel(
                request.topic.as_str(),
                request.object_id.as_str(),
                request.channel_ref,
            )
            .await
    }

    pub async fn remove_bookmarked_post(&self, request: RemoveBookmarkedPostRequest) -> Result<()> {
        self.app_service
            .remove_bookmarked_post(request.object_id.as_str())
            .await
    }

    pub async fn list_timeline(&self, request: ListTimelineRequest) -> Result<TimelineView> {
        self.app_service
            .list_timeline_scoped(
                request.topic.as_str(),
                request.scope,
                request.cursor,
                request.limit.unwrap_or(50),
            )
            .await
    }

    pub async fn list_thread(&self, request: ListThreadRequest) -> Result<TimelineView> {
        self.app_service
            .list_thread(
                request.topic.as_str(),
                request.thread_id.as_str(),
                request.cursor,
                request.limit.unwrap_or(50),
            )
            .await
    }

    pub async fn list_profile_timeline(
        &self,
        request: ListProfileTimelineRequest,
    ) -> Result<TimelineView> {
        self.app_service
            .list_profile_timeline(
                request.pubkey.as_str(),
                request.cursor,
                request.limit.unwrap_or(50),
            )
            .await
    }

    pub async fn retry_post_elements(
        &self,
        request: RetryPostElementsRequest,
    ) -> Result<Option<PostView>> {
        self.app_service
            .retry_post_elements(
                request.object_id.as_str(),
                request.body_object_id.as_deref(),
                request.manual,
            )
            .await
    }

    pub async fn get_my_profile(&self) -> Result<Profile> {
        self.app_service.get_my_profile().await
    }

    pub async fn set_my_profile(&self, request: SetMyProfileRequest) -> Result<Profile> {
        let envelope = self.prepare_my_profile(request).await?;
        self.app_service.commit_my_profile(envelope).await
    }

    pub(crate) async fn prepare_my_profile(
        &self,
        request: SetMyProfileRequest,
    ) -> Result<kukuri_core::KukuriEnvelope> {
        self.app_service
            .prepare_my_profile(ProfileInput {
                name: request.name,
                display_name: request.display_name,
                about: request.about,
                picture_upload: request
                    .picture_upload
                    .map(pending_attachment_from_request)
                    .transpose()?,
                clear_picture: request.clear_picture,
            })
            .await
    }

    pub(crate) async fn commit_my_profile(
        &self,
        envelope: kukuri_core::KukuriEnvelope,
    ) -> Result<Profile> {
        self.app_service.commit_my_profile(envelope).await
    }

    pub async fn follow_author(&self, request: AuthorRequest) -> Result<AuthorSocialView> {
        self.app_service
            .follow_author(request.pubkey.as_str())
            .await
    }

    pub async fn unfollow_author(&self, request: AuthorRequest) -> Result<AuthorSocialView> {
        self.app_service
            .unfollow_author(request.pubkey.as_str())
            .await
    }

    pub async fn get_author_social_view(&self, request: AuthorRequest) -> Result<AuthorSocialView> {
        self.app_service
            .get_author_social_view(request.pubkey.as_str())
            .await
    }

    pub async fn mute_author(&self, request: AuthorRequest) -> Result<AuthorSocialView> {
        let view = self
            .app_service
            .mute_author(request.pubkey.as_str())
            .await?;
        self.record_trust_observation(request.pubkey.as_str(), TrustObservationKind::Mute, true)
            .await;
        Ok(view)
    }

    pub async fn unmute_author(&self, request: AuthorRequest) -> Result<AuthorSocialView> {
        let view = self
            .app_service
            .unmute_author(request.pubkey.as_str())
            .await?;
        self.record_trust_observation(request.pubkey.as_str(), TrustObservationKind::Mute, false)
            .await;
        Ok(view)
    }

    pub async fn block_author(&self, request: AuthorRequest) -> Result<AuthorSocialView> {
        let view = self
            .app_service
            .block_author(request.pubkey.as_str())
            .await?;
        self.record_trust_observation(request.pubkey.as_str(), TrustObservationKind::Block, true)
            .await;
        Ok(view)
    }

    pub async fn unblock_author(&self, request: AuthorRequest) -> Result<AuthorSocialView> {
        let view = self
            .app_service
            .unblock_author(request.pubkey.as_str())
            .await?;
        self.record_trust_observation(request.pubkey.as_str(), TrustObservationKind::Block, false)
            .await;
        Ok(view)
    }

    /// #1061: 提供中の CN へ送る観測を積む。ローカル操作は既に成立しているので、失敗は警告に留める。
    async fn record_trust_observation(
        &self,
        target_pubkey: &str,
        kind: TrustObservationKind,
        active: bool,
    ) {
        if let Err(error) = self
            .enqueue_trust_observation(target_pubkey, kind, active)
            .await
        {
            warn!(error = %error, "failed to queue a trust observation for community nodes");
        }
    }

    pub async fn list_social_connections(
        &self,
        request: ListSocialConnectionsRequest,
    ) -> Result<Vec<AuthorSocialView>> {
        self.app_service.list_social_connections(request.kind).await
    }
}
