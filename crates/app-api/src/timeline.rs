use crate::service::*;

impl AppService {
    pub async fn list_profile_timeline(
        &self,
        author_pubkey: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<TimelineView> {
        let author_pubkey = normalize_author_pubkey(author_pubkey)?;
        let empty_recovery_key = author_empty_recovery_key(author_pubkey.as_str());
        self.ensure_author_subscription(author_pubkey.as_str())
            .await?;
        let load_profile_items = || async {
            let posts = load_profile_posts_from_author_replica(
                self.services.docs_sync.as_ref(),
                author_pubkey.as_str(),
                DocFetchPolicy::LocalOnly,
            )
            .await?;
            let reposts = load_profile_reposts_from_author_replica(
                self.services.docs_sync.as_ref(),
                author_pubkey.as_str(),
                DocFetchPolicy::LocalOnly,
            )
            .await?;
            Ok::<_, anyhow::Error>((posts, reposts))
        };
        let (mut posts, mut reposts) = match load_profile_items().await {
            Ok(items) => items,
            Err(error) => {
                self.maybe_restart_author_subscription(author_pubkey.as_str())
                    .await;
                load_profile_items().await.map_err(|retry_error| {
                    retry_error.context(format!(
                        "failed to reload profile timeline after author subscription restart: {error}"
                    ))
                })?
            }
        };
        if cursor.is_none() && posts.is_empty() && reposts.is_empty() {
            if self
                .should_restart_after_empty_result(empty_recovery_key.as_str())
                .await
            {
                self.maybe_restart_author_subscription(author_pubkey.as_str())
                    .await;
                (posts, reposts) = load_profile_items().await?;
            }
        } else {
            self.clear_empty_result_restart_marker(empty_recovery_key.as_str())
                .await;
        }
        let mut items = Vec::with_capacity(posts.len() + reposts.len());
        items.extend(posts.drain(..).map(ProfileTimelineItem::Post));
        items.extend(reposts.drain(..).map(ProfileTimelineItem::Repost));
        items.sort_by(|left, right| {
            right
                .created_at()
                .cmp(&left.created_at())
                .then_with(|| right.object_id().cmp(left.object_id()))
        });
        let hidden_author_pubkeys = self.current_hidden_author_pubkeys().await?;
        items.retain(|item| !profile_timeline_item_is_hidden(item, &hidden_author_pubkeys));
        let page = profile_timeline_page(items, cursor, limit);
        let mut views = Vec::with_capacity(page.items.len());
        for item in page.items {
            match item {
                ProfileTimelineItem::Post(post) => {
                    views.push(self.profile_post_to_view(post).await?)
                }
                ProfileTimelineItem::Repost(repost) => {
                    views.push(self.profile_repost_to_view(repost).await?)
                }
            }
        }
        Ok(TimelineView {
            items: views,
            next_cursor: page.next_cursor,
        })
    }

    pub async fn create_repost(
        &self,
        target_topic_id: &str,
        source_topic_id: &str,
        source_object_id: &str,
        commentary: Option<&str>,
    ) -> Result<String> {
        ensure_optional_text_within_limit(
            "repost commentary",
            commentary,
            MAX_REPOST_COMMENTARY_CHARS,
        )?;
        self.ensure_topic_subscription(target_topic_id).await?;
        self.ensure_topic_subscription(source_topic_id).await?;

        let normalized_commentary = normalize_repost_commentary(commentary.map(str::to_string));
        if let Some(existing_object_id) = self
            .find_existing_simple_repost(
                target_topic_id,
                source_object_id,
                normalized_commentary.as_deref(),
            )
            .await?
        {
            return Ok(existing_object_id);
        }

        let source_object = self
            .resolve_repost_source(source_topic_id, source_object_id)
            .await?;
        let topic = TopicId::new(target_topic_id);
        let envelope = build_repost_envelope(
            self.services.keys.as_ref(),
            &topic,
            source_object.repost_of.clone(),
            normalized_commentary.as_deref(),
        )?;
        let repost_object = envelope
            .to_post_object()?
            .ok_or_else(|| anyhow::anyhow!("failed to parse repost object"))?;
        self.ingest_event(
            &topic_replica_id(target_topic_id),
            envelope.clone(),
            None,
            Vec::new(),
        )
        .await?;

        let local_author_pubkey = self.current_author_pubkey();
        let profile_repost_envelope = build_profile_repost_envelope(
            self.services.keys.as_ref(),
            &KukuriProfileRepostEnvelopeContentV1 {
                author_pubkey: Pubkey::from(local_author_pubkey.as_str()),
                profile_topic_id: author_profile_topic_id(local_author_pubkey.as_str()),
                published_topic_id: topic.clone(),
                object_id: repost_object.object_id.clone(),
                created_at: repost_object.created_at,
                commentary: normalized_commentary.clone(),
                repost_of: source_object.repost_of,
            },
        )?;
        let profile_repost = parse_profile_repost(&profile_repost_envelope)?
            .ok_or_else(|| anyhow::anyhow!("failed to parse profile repost envelope"))?;
        persist_profile_repost_doc(
            self.services.docs_sync.as_ref(),
            &profile_repost,
            &profile_repost_envelope,
        )
        .await?;

        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(target_topic_id, None),
                GossipHint::TopicObjectsChanged {
                    topic_id: topic,
                    objects: vec![HintObjectRef {
                        object_id: envelope.id.0.clone(),
                        object_kind: envelope.kind.clone(),
                    }],
                },
            )
            .await?;
        Ok(envelope.id.0)
    }

    pub async fn list_bookmarked_posts(&self) -> Result<Vec<BookmarkedPostView>> {
        let hidden_author_pubkeys = self.current_hidden_author_pubkeys().await?;
        let rows = self
            .services
            .projection_store
            .list_bookmarked_posts()
            .await?
            .into_iter()
            .filter(|row| !bookmarked_post_row_is_hidden(row, &hidden_author_pubkeys))
            .collect::<Vec<_>>();
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(self.bookmarked_post_view_from_row(row).await?);
        }
        Ok(items)
    }

    pub async fn bookmark_post(
        &self,
        topic_id: &str,
        source_object_id: &str,
    ) -> Result<BookmarkedPostView> {
        let channel_ref = self
            .services
            .projection_store
            .get_object_projection(&EnvelopeId::from(source_object_id))
            .await?
            .and_then(|projection| channel_id_from_storage(projection.channel_id.as_str()))
            .map(|channel_id| ChannelRef::PrivateChannel { channel_id })
            .unwrap_or(ChannelRef::Public);
        self.bookmark_post_in_channel(topic_id, source_object_id, channel_ref)
            .await
    }

    pub async fn bookmark_post_in_channel(
        &self,
        topic_id: &str,
        source_object_id: &str,
        channel_ref: ChannelRef,
    ) -> Result<BookmarkedPostView> {
        self.ensure_topic_subscription(topic_id).await?;
        let scope = match &channel_ref {
            ChannelRef::Public => TimelineScope::Public,
            ChannelRef::PrivateChannel { channel_id } => TimelineScope::Channel {
                channel_id: channel_id.clone(),
            },
        };
        let source_object_id = EnvelopeId::from(source_object_id);
        self.ensure_object_projection(
            topic_id,
            &scope,
            &source_object_id,
            DocFetchPolicy::LocalOnly,
        )
        .await?;
        let projection = self
            .services
            .projection_store
            .get_object_projection(&source_object_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("bookmark target was not found"))?;
        if projection.topic_id != topic_id {
            anyhow::bail!("bookmark target topic does not match");
        }
        let channel_matches = match &channel_ref {
            ChannelRef::Public => projection.channel_id == PUBLIC_CHANNEL_ID,
            ChannelRef::PrivateChannel { channel_id } => {
                projection.channel_id == channel_id.as_str()
            }
        };
        if !channel_matches {
            anyhow::bail!("bookmark target channel does not match");
        }
        if !matches!(
            projection.object_kind.as_str(),
            "post" | "comment" | "repost"
        ) {
            anyhow::bail!("bookmark target must be a timeline post");
        }
        let attachments = if projection.object_kind == "repost" {
            Vec::new()
        } else {
            fetch_post_object_for_projection(
                self.services.docs_sync.as_ref(),
                &projection.source_replica_id,
                projection.source_key.as_str(),
            )
            .await?
            .map(|post_object| post_object.attachments)
            .unwrap_or_default()
        };
        let row = BookmarkedPostRow {
            source_object_id: projection.object_id.clone(),
            source_envelope_id: projection.source_envelope_id.clone(),
            source_replica_id: projection.source_replica_id.clone(),
            topic_id: projection.topic_id.clone(),
            channel_id: projection.channel_id.clone(),
            author_pubkey: projection.author_pubkey.clone(),
            created_at: projection.created_at,
            object_kind: projection.object_kind.clone(),
            payload_ref: projection.payload_ref.clone(),
            content: projection
                .content
                .clone()
                .or_else(|| content_from_payload_ref(&projection.payload_ref)),
            attachments,
            reply_to_object_id: projection.reply_to_object_id.clone(),
            root_object_id: projection.root_object_id.clone(),
            repost_of: projection.repost_of.clone(),
            bookmarked_at: Utc::now().timestamp_millis(),
        };
        self.services
            .projection_store
            .put_bookmarked_post(row.clone())
            .await?;
        self.bookmarked_post_view_from_row(row).await
    }

    pub async fn remove_bookmarked_post(&self, source_object_id: &str) -> Result<()> {
        self.services
            .projection_store
            .remove_bookmarked_post(&EnvelopeId::from(source_object_id))
            .await
    }

    pub async fn create_post(
        &self,
        topic_id: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<String> {
        self.create_post_in_channel(topic_id, ChannelRef::Public, content, reply_to)
            .await
    }

    pub async fn create_post_with_attachments(
        &self,
        topic_id: &str,
        content: &str,
        reply_to: Option<&str>,
        attachments: Vec<PendingAttachment>,
    ) -> Result<String> {
        self.create_post_with_attachments_in_channel(
            topic_id,
            ChannelRef::Public,
            content,
            reply_to,
            attachments,
            Vec::new(),
        )
        .await
    }

    pub async fn create_post_in_channel(
        &self,
        topic_id: &str,
        channel_ref: ChannelRef,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<String> {
        self.create_post_with_attachments_in_channel(
            topic_id,
            channel_ref,
            content,
            reply_to,
            Vec::new(),
            Vec::new(),
        )
        .await
    }

    pub async fn withdraw_post(
        &self,
        topic_id: &str,
        object_id: &str,
        channel_ref: ChannelRef,
        replacement_object_id: Option<&str>,
        reason_visibility: WithdrawalReasonVisibility,
        reason: Option<PostWithdrawalReason>,
    ) -> Result<String> {
        self.ensure_topic_subscription(topic_id).await?;
        let scope = match &channel_ref {
            ChannelRef::Public => TimelineScope::Public,
            ChannelRef::PrivateChannel { channel_id } => TimelineScope::Channel {
                channel_id: channel_id.clone(),
            },
        };
        let target_object_id = EnvelopeId::from(object_id);
        self.ensure_object_projection(
            topic_id,
            &scope,
            &target_object_id,
            DocFetchPolicy::LocalOnly,
        )
        .await?;
        let target = self
            .resolve_signed_post_envelope(&target_object_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("withdrawal target envelope was not found"))?;
        let target_content = target
            .post_content()?
            .ok_or_else(|| anyhow::anyhow!("withdrawal target must be a post object"))?;
        if target_content.topic_id.as_str() != topic_id {
            anyhow::bail!("withdrawal target topic does not match");
        }

        let private_state = match target_content.channel_id.as_ref() {
            Some(channel_id) => {
                match &channel_ref {
                    ChannelRef::PrivateChannel {
                        channel_id: requested,
                    } if requested == channel_id => {}
                    _ => anyhow::bail!("withdrawal target channel does not match"),
                }
                Some(
                    self.private_channel_write_state(topic_id, channel_id)
                        .await?,
                )
            }
            None => {
                if !matches!(channel_ref, ChannelRef::Public) {
                    anyhow::bail!("withdrawal target channel does not match");
                }
                None
            }
        };
        let replica = private_state
            .as_ref()
            .map(current_private_channel_replica_id)
            .unwrap_or_else(|| topic_replica_id(topic_id));
        let generation = self
            .services
            .projection_store
            .get_post_withdrawal(&target_object_id)
            .await?
            .map(|row| row.generation.saturating_add(1))
            .unwrap_or(1);
        let envelope = build_post_withdrawal_envelope(
            self.services.keys.as_ref(),
            &target,
            generation,
            replacement_object_id.map(EnvelopeId::from),
            reason_visibility,
            reason,
        )?;
        let withdrawal = verify_post_withdrawal(&envelope, &target)?;
        persist_post_withdrawal(
            self.services.docs_sync.as_ref(),
            &replica,
            &target_object_id,
            &envelope,
        )
        .await?;
        self.services
            .projection_store
            .put_post_withdrawal(post_withdrawal_row(withdrawal, &replica))
            .await?;

        let hint_topic = channel_hint_topic_for(topic_id, target_content.channel_id.as_ref());
        if let Err(error) = self
            .services
            .hint_transport
            .publish_hint(
                &hint_topic,
                GossipHint::TopicObjectsChanged {
                    topic_id: TopicId::new(topic_id),
                    objects: vec![HintObjectRef {
                        object_id: target_object_id.0.clone(),
                        object_kind: "post_withdrawal".to_string(),
                    }],
                },
            )
            .await
        {
            warn!(
                topic = %topic_id,
                object_id = %target_object_id.0,
                error = %error,
                "failed to publish withdrawal hint; durable docs state was already persisted"
            );
        }
        if target_content.channel_id.is_none() {
            self.maybe_restart_replica_sync(topic_id, &replica).await;
        }
        Ok(envelope.id.0)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_post_with_attachments_in_channel(
        &self,
        topic_id: &str,
        channel_ref: ChannelRef,
        content: &str,
        reply_to: Option<&str>,
        attachments: Vec<PendingAttachment>,
        content_labels: Vec<String>,
    ) -> Result<String> {
        ensure_text_within_limit("post content", content, MAX_POST_CONTENT_CHARS)?;
        // #858: self-label は既知値だけを受け付ける(現状 `adult` のみ)。
        for label in &content_labels {
            if label != kukuri_core::ADULT_CONTENT_LABEL {
                anyhow::bail!("unknown content label `{label}`");
            }
        }
        let mut content_labels = content_labels;
        content_labels.dedup();
        self.ensure_topic_subscription(topic_id).await?;
        let topic = TopicId::new(topic_id);
        let parent = if let Some(reply_to) = reply_to {
            let scope = match &channel_ref {
                ChannelRef::Public => TimelineScope::Public,
                ChannelRef::PrivateChannel { channel_id } => TimelineScope::Channel {
                    channel_id: channel_id.clone(),
                },
            };
            self.ensure_object_projection(
                topic_id,
                &scope,
                &EnvelopeId::from(reply_to),
                DocFetchPolicy::LocalOnly,
            )
            .await?;
            Some(
                self.resolve_parent_object(&EnvelopeId::from(reply_to))
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("reply target was not found"))?,
            )
        } else {
            None
        };
        let private_state = if let Some(parent) = parent.as_ref() {
            let content = parent
                .post_content()?
                .ok_or_else(|| anyhow::anyhow!("reply target is not a post object"))?;
            if content.object_kind == "repost"
                && normalize_repost_commentary(content_from_payload_ref(&content.payload_ref))
                    .is_none()
            {
                anyhow::bail!("simple repost cannot be a reply parent");
            }
            if content.topic_id.as_str() != topic_id {
                anyhow::bail!("reply target topic does not match");
            }
            if let Some(channel_id) = content.channel_id.clone() {
                Some(
                    self.private_channel_write_state(topic_id, &channel_id)
                        .await?,
                )
            } else {
                None
            }
        } else {
            match channel_ref {
                ChannelRef::Public => None,
                ChannelRef::PrivateChannel { channel_id } => Some(
                    self.private_channel_write_state(topic_id, &channel_id)
                        .await?,
                ),
            }
        };
        let effective_channel_id = private_state.as_ref().map(|state| state.channel_id.clone());
        let write_replica = private_state
            .as_ref()
            .map(current_private_channel_replica_id)
            .unwrap_or_else(|| topic_replica_id(topic_id));
        let now = Utc::now().timestamp_millis();
        let stored_blob = self
            .services
            .blob_service
            .put_blob(content.as_bytes().to_vec(), "text/plain")
            .await?;
        let stored_attachments = futures_util::future::try_join_all(attachments.into_iter().map(
            |attachment| async move {
                let stored = self
                    .services
                    .blob_service
                    .put_blob(attachment.bytes, attachment.mime.as_str())
                    .await?;
                Ok::<_, anyhow::Error>((attachment.role, stored))
            },
        ))
        .await?;
        let manifest_ids = if stored_attachments.is_empty() {
            Vec::new()
        } else {
            let manifest_id = format!(
                "media-{}-{}",
                now,
                short_id_suffix(self.current_author_pubkey().as_str())
            );
            let manifest = KukuriMediaManifestV1 {
                manifest_id: manifest_id.clone(),
                owner_pubkey: Pubkey::from(self.current_author_pubkey()),
                created_at: now,
                items: stored_attachments
                    .iter()
                    .map(|(role, stored)| MediaManifestItem {
                        blob_hash: stored.hash.clone(),
                        mime: stored.mime.clone(),
                        size: stored.bytes,
                        width: None,
                        height: None,
                        duration_ms: None,
                        codec: None,
                        thumbnail_blob_hash: match role {
                            AssetRole::VideoManifest => None,
                            _ => None,
                        },
                    })
                    .collect(),
            };
            let envelope =
                build_media_manifest_envelope(self.services.keys.as_ref(), &topic, &manifest)?;
            persist_media_manifest(
                &write_replica,
                &envelope,
                &manifest,
                self.services.docs_sync.as_ref(),
            )
            .await?;
            vec![manifest_id]
        };
        let envelope = build_post_envelope_with_payload_in_channel(
            self.services.keys.as_ref(),
            &topic,
            PayloadRef::BlobText {
                hash: stored_blob.hash.clone(),
                mime: stored_blob.mime.clone(),
                bytes: stored_blob.bytes,
            },
            stored_attachments
                .iter()
                .map(|(role, stored)| kukuri_core::AssetRef {
                    hash: stored.hash.clone(),
                    mime: stored.mime.clone(),
                    bytes: stored.bytes,
                    role: role.clone(),
                })
                .collect(),
            manifest_ids,
            parent.as_ref(),
            if effective_channel_id.is_some() {
                ObjectVisibility::Private
            } else {
                ObjectVisibility::Public
            },
            effective_channel_id.as_ref(),
            content_labels,
        )?;
        let post_object = envelope
            .to_post_object()?
            .ok_or_else(|| anyhow::anyhow!("failed to parse post object for profile topic"))?;
        self.ingest_event(
            &write_replica,
            envelope.clone(),
            Some(stored_blob.clone()),
            stored_attachments,
        )
        .await?;
        if effective_channel_id.is_none() {
            let local_author_pubkey = self.current_author_pubkey();
            let profile_post_envelope = build_profile_post_envelope(
                self.services.keys.as_ref(),
                &KukuriProfilePostEnvelopeContentV1 {
                    author_pubkey: Pubkey::from(local_author_pubkey.as_str()),
                    profile_topic_id: author_profile_topic_id(local_author_pubkey.as_str()),
                    published_topic_id: topic.clone(),
                    object_id: post_object.object_id.clone(),
                    created_at: post_object.created_at,
                    object_kind: post_object.object_kind.clone(),
                    content: content.to_string(),
                    attachments: post_object.attachments.clone(),
                    reply_to_object_id: post_object.reply_to.clone(),
                    root_id: post_object.root.clone(),
                    content_labels: post_object.content_labels.clone(),
                },
            )?;
            let profile_post = parse_profile_post(&profile_post_envelope)?
                .ok_or_else(|| anyhow::anyhow!("failed to parse profile post envelope"))?;
            persist_profile_post_doc(
                self.services.docs_sync.as_ref(),
                &profile_post,
                &profile_post_envelope,
            )
            .await?;
        }
        let hint_topic = channel_hint_topic_for(topic_id, effective_channel_id.as_ref());
        let hint = GossipHint::TopicObjectsChanged {
            topic_id: topic.clone(),
            objects: vec![HintObjectRef {
                object_id: envelope.id.0.clone(),
                object_kind: envelope.kind.clone(),
            }],
        };
        let mut hint_error = None;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(250 * attempt)).await;
            }
            match self
                .services
                .hint_transport
                .publish_hint(&hint_topic, hint.clone())
                .await
            {
                Ok(()) => hint_error = None,
                Err(error) => hint_error = Some(error),
            }
        }
        if let Some(error) = hint_error {
            warn!(
                topic = %topic_id,
                object_id = %envelope.id.0,
                error = %error,
                "failed to publish post hint; durable docs state was already persisted"
            );
        }
        if effective_channel_id.is_none() {
            self.maybe_restart_replica_sync(topic_id, &topic_replica_id(topic_id))
                .await;
            match self.get_sync_status().await {
                Ok(status) => {
                    if let Some(topic_status) = status
                        .topic_diagnostics
                        .iter()
                        .find(|entry| entry.topic == topic_id)
                    {
                        let connectivity_shape = match topic_status.delivery_state {
                            DeliveryState::Live => "live",
                            DeliveryState::DurableReady => "durable-ready",
                            DeliveryState::DurableRecovering => "durable-recovering",
                            DeliveryState::Offline => "offline",
                        };
                        info!(
                            topic = %topic_id,
                            connectivity_shape,
                            direct_peer_count = topic_status.connected_peers.len(),
                            docs_assist_peer_count = topic_status.docs_assist_peer_ids.len(),
                            "public topic connectivity snapshot after local post"
                        );
                    }
                }
                Err(error) => {
                    warn!(
                        topic = %topic_id,
                        error = %error,
                        "failed to load public topic connectivity snapshot after local post"
                    );
                }
            }
        }
        Ok(envelope.id.0)
    }

    pub async fn list_timeline(
        &self,
        topic_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<TimelineView> {
        self.list_timeline_scoped(topic_id, TimelineScope::Public, cursor, limit)
            .await
    }

    pub async fn list_timeline_scoped(
        &self,
        topic_id: &str,
        scope: TimelineScope,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<TimelineView> {
        let had_topic_subscription = self.has_topic_subscription(topic_id).await;
        let empty_recovery_key = scope_empty_recovery_key(topic_id, &scope);
        self.ensure_scope_subscriptions(topic_id, &scope).await?;
        let hidden_author_pubkeys = self.current_hidden_author_pubkeys().await?;
        let mut page = filtered_timeline_page(
            self.services.projection_store.as_ref(),
            topic_id,
            cursor.clone(),
            limit,
            &self.allowed_channel_ids_for_scope(topic_id, &scope).await?,
            &hidden_author_pubkeys,
        )
        .await?;
        // #1225: 本文が欠けた行は復旧の理由にしない。欠損は行単位の取り直しへ渡す。
        let needs_epoch_hydration = self
            .scope_needs_current_private_epoch_hydration(topic_id, &scope, &page)
            .await;
        let restart_after_empty = had_topic_subscription
            && page.items.is_empty()
            && self
                .should_restart_after_empty_result(empty_recovery_key.as_str())
                .await;
        // #1239: replica を走査しない。利用者が遡ったページ(cursor つき)と、projection が尽きたページ
        // (空を含む)は、その 1 ページぶんの範囲を時系列の索引と照合して、欠けている object だけを key 指定で
        // 反映する。先頭の範囲の追いつきは購読タスクが行う。
        let projection_exhausted = page.next_cursor.is_none();
        if cursor.is_some() || projection_exhausted || needs_epoch_hydration {
            if self
                .reconcile_timeline_range(topic_id, &scope, cursor.as_ref(), limit)
                .await?
                > 0
            {
                *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
                page = filtered_timeline_page(
                    self.services.projection_store.as_ref(),
                    topic_id,
                    cursor,
                    limit,
                    &self.allowed_channel_ids_for_scope(topic_id, &scope).await?,
                    &hidden_author_pubkeys,
                )
                .await?;
            }
            if needs_epoch_hydration || (page.items.is_empty() && restart_after_empty) {
                if had_topic_subscription {
                    self.maybe_restart_scope_subscription(topic_id, &scope)
                        .await;
                }
                self.maybe_restart_scope_replica_sync(topic_id, &scope)
                    .await;
            }
        }
        Box::pin(self.recover_missing_bodies(&mut page.items)).await;
        if !page.items.is_empty() {
            self.clear_empty_result_restart_marker(empty_recovery_key.as_str())
                .await;
        }
        self.ensure_author_subscriptions_for_rows(&page.items)
            .await?;
        let view = self.page_to_view(page).await?;
        let mut last_sync = self.last_sync_ts.lock().await;
        if !view.items.is_empty() && last_sync.is_none() {
            *last_sync = Some(Utc::now().timestamp_millis());
        }
        Ok(view)
    }

    pub async fn list_thread(
        &self,
        topic_id: &str,
        thread_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<TimelineView> {
        let had_topic_subscription = self.has_topic_subscription(topic_id).await;
        let empty_recovery_key = thread_empty_recovery_key(topic_id, thread_id);
        self.ensure_scope_subscriptions(topic_id, &TimelineScope::AllJoined)
            .await?;
        let hidden_author_pubkeys = self.current_hidden_author_pubkeys().await?;
        let thread_root = EnvelopeId::from(thread_id);
        let mut page = filtered_thread_page(
            self.services.projection_store.as_ref(),
            topic_id,
            &thread_root,
            cursor.clone(),
            limit,
            None,
            &hidden_author_pubkeys,
        )
        .await?;
        // #1225: `list_timeline_scoped` と同じく、本文が欠けた行は復旧の理由にしない。
        let restart_after_empty = had_topic_subscription
            && page.items.is_empty()
            && self
                .should_restart_after_empty_result(empty_recovery_key.as_str())
                .await;
        // #1239: replica を走査しない。thread の索引と照合して、欠けている object だけを key 指定で反映する
        // (範囲ごとに間隔を空ける)。thread は途中の返信が欠けうるので、ページが空でなくても照合する。
        let reconciled = self.reconcile_thread(topic_id, &thread_root).await?;
        if reconciled > 0 || page.items.is_empty() {
            if reconciled > 0 {
                *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
                let root_channel = self
                    .services
                    .projection_store
                    .get_object_projection(&thread_root)
                    .await?
                    .map(|row| row.channel_id);
                page = filtered_thread_page(
                    self.services.projection_store.as_ref(),
                    topic_id,
                    &thread_root,
                    cursor,
                    limit,
                    root_channel.as_deref(),
                    &hidden_author_pubkeys,
                )
                .await?;
            }
            if page.items.is_empty() && restart_after_empty {
                if had_topic_subscription {
                    self.maybe_restart_scope_subscription(topic_id, &TimelineScope::AllJoined)
                        .await;
                }
                self.maybe_restart_scope_replica_sync(topic_id, &TimelineScope::AllJoined)
                    .await;
            }
        }
        Box::pin(self.recover_missing_bodies(&mut page.items)).await;
        if !page.items.is_empty() {
            self.clear_empty_result_restart_marker(empty_recovery_key.as_str())
                .await;
        }
        self.ensure_author_subscriptions_for_rows(&page.items)
            .await?;
        let view = self.page_to_view(page).await?;
        let mut last_sync = self.last_sync_ts.lock().await;
        if !view.items.is_empty() && last_sync.is_none() {
            *last_sync = Some(Utc::now().timestamp_millis());
        }
        Ok(view)
    }
}

fn author_empty_recovery_key(author_pubkey: &str) -> String {
    format!("empty-author:{author_pubkey}")
}

fn scope_empty_recovery_key(topic_id: &str, scope: &TimelineScope) -> String {
    match scope {
        TimelineScope::Public => format!("empty-scope:{topic_id}:public"),
        TimelineScope::AllJoined => format!("empty-scope:{topic_id}:all-joined"),
        TimelineScope::Channel { channel_id } => {
            format!("empty-scope:{topic_id}:channel:{}", channel_id.as_str())
        }
    }
}

fn thread_empty_recovery_key(topic_id: &str, thread_id: &str) -> String {
    format!("empty-thread:{topic_id}:{thread_id}")
}
