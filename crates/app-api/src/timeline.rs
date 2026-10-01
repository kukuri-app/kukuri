use crate::service::*;
use anyhow::ensure;

impl AppService {
    pub async fn list_profile_timeline(
        &self,
        author_pubkey: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<TimelineView> {
        let author_pubkey = normalize_author_pubkey(author_pubkey)?;
        // #1239: replica は走査しない。プロフィールの索引から、ページの行だけを読む。
        // #1221 R5-C: 手元で埋まらないページだけを、author 本人を含む有界な provider から読む。
        let hidden_author_pubkeys = self.current_hidden_author_pubkeys().await?;
        let local_author = self.current_author_pubkey();
        let docs_author = known_docs_author(
            &self.services,
            local_author.as_str(),
            author_pubkey.as_str(),
        )
        .await?;
        let page = profile_timeline_page(
            &self.services,
            author_pubkey.as_str(),
            docs_author.as_deref(),
            cursor,
            limit,
            &hidden_author_pubkeys,
        )
        .await?;
        self.reflect_reply_targets_for_profile_items(&page.items)
            .await;
        let mut views = Vec::with_capacity(page.items.len());
        for item in page.items {
            match item {
                ProfileTimelineItem::Post(post) => {
                    views.push(self.profile_post_to_view(post).await?)
                }
                ProfileTimelineItem::Repost(repost) => {
                    views.push(self.profile_repost_to_view(repost).await?)
                }
                ProfileTimelineItem::Projection(row) => views.extend(
                    self.page_to_view(Page {
                        items: vec![*row],
                        next_cursor: None,
                    })
                    .await?
                    .items,
                ),
            }
        }
        Ok(TimelineView {
            items: views,
            next_cursor: page.next_cursor,
            unavailable_count: 0,
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
        // ADR 0053 §2: docs へ書く docs author を、署名の対象の tag と hint に入れる。
        let docs_author = self.services.docs_sync.local_docs_author().await?;
        let envelope = build_repost_envelope_with_docs_author(
            self.services.keys.as_ref(),
            &topic,
            source_object.repost_of.clone(),
            normalized_commentary.as_deref(),
            docs_author.as_deref(),
        )?;
        let repost_object = envelope
            .to_post_object()?
            .ok_or_else(|| anyhow::anyhow!("failed to parse repost object"))?;
        let write_replica =
            self.services
                .scope_write_replica(target_topic_id, None, envelope.created_at)?;
        self.ingest_event(&write_replica, envelope.clone()).await?;

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
            &self
                .services
                .author_index_replica(&local_author_pubkey, profile_repost.created_at)?,
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
                        docs_author,
                    }],
                },
            )
            .await?;
        self.queue_public_repost_offer(
            target_topic_id,
            &envelope,
            repost_object.repost_of.as_ref(),
            normalized_commentary.as_deref(),
        )
        .await;
        Ok(envelope.id.0)
    }

    pub async fn list_bookmarked_posts_page(
        &self,
        cursor: Option<&kukuri_store::BookmarkCursor>,
        before: bool,
    ) -> Result<BookmarkedPostPageView> {
        let mut rows = self
            .services
            .projection_store
            .list_bookmarked_posts_page(cursor, before)
            .await?;
        let has_more = rows.len() > 20;
        rows.truncate(20);
        if before {
            rows.reverse();
        }
        let first = rows.first().map(kukuri_store::BookmarkCursor::from);
        let last = rows.last().map(kukuri_store::BookmarkCursor::from);
        let bookmark_at_cursor_exists = if before {
            if let Some(cursor) = cursor {
                !self
                    .services
                    .projection_store
                    .bookmarked_post_ids(std::slice::from_ref(&cursor.source_object_id))
                    .await?
                    .is_empty()
            } else {
                false
            }
        } else {
            false
        };
        let (newer_cursor, older_cursor) = if before {
            (
                has_more.then_some(first).flatten(),
                bookmark_at_cursor_exists.then_some(last).flatten(),
            )
        } else {
            (
                cursor.is_some().then_some(first).flatten(),
                has_more.then_some(last).flatten(),
            )
        };
        let hidden_authors = self.current_hidden_author_pubkeys().await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            if !bookmarked_post_row_is_hidden(&row, &hidden_authors) {
                items.push(self.bookmarked_post_view_from_row(row).await?);
            }
        }
        Ok(BookmarkedPostPageView {
            items,
            newer_cursor,
            older_cursor,
        })
    }

    pub async fn bookmarked_post_ids(&self, ids: &[EnvelopeId]) -> Result<Vec<String>> {
        ensure!(ids.len() <= 200, "bookmark status page exceeds 200 objects");
        Ok(self
            .services
            .projection_store
            .bookmarked_post_ids(ids)
            .await?
            .into_iter()
            .map(|id| id.as_str().to_owned())
            .collect())
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
        // 添付は検証済みの行から写す(#1248)。docs の `state` は読まない。
        let attachments = if projection.object_kind == "repost" {
            Vec::new()
        } else {
            projection.attachments.clone()
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
        // 操作時の位置と、切替後は元投稿の位置(private は元 epoch)の 2 か所を outbox へ積んでから書く(ADR 0054 §2)。
        let replica = self.services.scope_write_replica(
            topic_id,
            private_state.as_ref(),
            envelope.created_at,
        )?;
        let target_replica = match self.services.writes_buckets() {
            true => self
                .services
                .projection_store
                .get_object_projection(&target_object_id)
                .await?
                .map(|row| row.source_replica_id)
                .filter(|source| source != &replica),
            false => None,
        };
        let row =
            |replica_id: &ReplicaId, target_replica_id: Option<ReplicaId>| WithdrawalWriteRow {
                withdrawal_envelope_id: envelope.id.clone(),
                replica_id: replica_id.clone(),
                target_object_id: target_object_id.clone(),
                envelope: envelope.clone(),
                target_replica_id,
            };
        let mut writes = vec![row(&replica, target_replica.clone())];
        writes.extend(target_replica.iter().map(|source| row(source, None)));
        self.services
            .projection_store
            .queue_withdrawal_writes(writes)
            .await?;
        self.services
            .projection_store
            .put_post_withdrawal(post_withdrawal_row(withdrawal, &replica))
            .await?;
        self.resume_withdrawal_writes().await?;

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
                        // 取り下げを書いた docs author(ADR 0053 §2)。private channel の hint には載せない。
                        docs_author: match target_content.channel_id {
                            Some(_) => None,
                            None => self.services.docs_sync.local_docs_author().await?,
                        },
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
        let manifest = if stored_attachments.is_empty() {
            None
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
            Some((manifest, envelope))
        };
        // ADR 0053 §2: docs へ書く docs author を、署名の対象の tag と hint に入れる。
        let docs_author = self.services.docs_sync.local_docs_author().await?;
        let envelope = build_post_envelope_with_docs_author(
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
            manifest
                .iter()
                .map(|(manifest, _)| manifest.manifest_id.clone())
                .collect(),
            parent.as_ref(),
            if effective_channel_id.is_some() {
                ObjectVisibility::Private
            } else {
                ObjectVisibility::Public
            },
            effective_channel_id.as_ref(),
            content_labels,
            docs_author.as_deref(),
        )?;
        let post_object = envelope
            .to_post_object()?
            .ok_or_else(|| anyhow::anyhow!("failed to parse post object for profile topic"))?;
        // manifest は投稿と同じ bucket(署名した `created_at` から決める)へ置く。
        let write_replica = self.services.scope_write_replica(
            topic_id,
            private_state.as_ref(),
            envelope.created_at,
        )?;
        if let Some((manifest, manifest_envelope)) = &manifest {
            persist_media_manifest(
                &write_replica,
                manifest_envelope,
                manifest,
                self.services.docs_sync.as_ref(),
            )
            .await?;
        }
        self.ingest_event(&write_replica, envelope.clone()).await?;
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
                &self
                    .services
                    .author_index_replica(&local_author_pubkey, profile_post.created_at)?,
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
                // private channel の hint の topic は epoch の秘密に依存しないので、docs author を載せない
                // (ADR 0053 §2)。channel の参加者は、docs の event と索引の entry から同じ手がかりを得る。
                docs_author: docs_author
                    .clone()
                    .filter(|_| effective_channel_id.is_none()),
            }],
        };
        let mut hint_error = None;
        for attempt in 0..3 {
            if attempt > 0 {
                n0_future::time::sleep(std::time::Duration::from_millis(250 * attempt)).await;
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
            // この topic の neighbor と補助の数だけを読む(通信状態の全体は作らない。#1221 R2-D)。
            let direct_peer_count = self
                .services
                .transport
                .peer_page(
                    ConnectivityPeerKind::Connected,
                    Some(kukuri_core::wire::hint_topic_id(&topic).as_str()),
                    None,
                    crate::CONNECTIVITY_PEER_PAGE_LIMIT,
                )
                .await
                .map_or(0, |page| page.peer_ids.len());
            let docs_assist_peer_count = self
                .docs_assisted_peer_ids()
                .await
                .map_or(0, |peers| peers.len());
            let last_docs_activity_at = self
                .public_topic_delivery_status(topic_id)
                .await
                .and_then(|status| status.last_docs_activity_at);
            let connectivity_shape = match delivery_state_for_topic(
                direct_peer_count,
                docs_assist_peer_count,
                last_docs_activity_at,
            ) {
                DeliveryState::Live => "live",
                DeliveryState::DurableReady => "durable-ready",
                DeliveryState::DurableRecovering => "durable-recovering",
                DeliveryState::Offline => "offline",
            };
            info!(
                topic = %topic_id,
                connectivity_shape,
                direct_peer_count,
                docs_assist_peer_count,
                "public topic connectivity snapshot after local post"
            );
        }
        self.queue_post_offer(
            &write_replica,
            &envelope,
            content,
            parent.as_ref(),
            private_state.as_ref(),
        )
        .await;
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
        let hidden_author_pubkeys = self.current_hidden_author_pubkeys().await?;
        let mut page = filtered_timeline_page(
            self.services.projection_store.as_ref(),
            topic_id,
            cursor.clone(),
            limit,
            &self.allowed_channel_id_for_scope(topic_id, &scope).await?,
            &hidden_author_pubkeys,
        )
        .await?;
        // The visible page (including its first page) checks only its bounded
        // index range; this also sees new buckets before local sync exists.
        let unavailable;
        {
            let reconcile = self
                .reconcile_timeline_range_checked(topic_id, &scope, cursor.as_ref(), limit, true)
                .await?;
            if reconcile.hydrated > 0 {
                self.last_sync_ts.set(Utc::now().timestamp_millis()).await;
            }
            // 照合した範囲に、最初のページより多くの object が projection に在ると分かったときだけ、ページを
            // 読み直す(今回反映した、または購読タスクが同じ範囲を先に反映していた)。それ以外は読み直さない。
            if reconcile.page_is_stale(page.items.len(), !hidden_author_pubkeys.is_empty()) {
                page = filtered_timeline_page(
                    self.services.projection_store.as_ref(),
                    topic_id,
                    cursor,
                    limit,
                    &self.allowed_channel_id_for_scope(topic_id, &scope).await?,
                    &hidden_author_pubkeys,
                )
                .await?;
            }
            unavailable = reconcile.unavailable;
            continue_past_unavailable(&mut page, &reconcile, PageOrder::NewestFirst, None);
        }
        Box::pin(self.recover_missing_bodies(&mut page.items)).await;
        self.reflect_reply_targets_for_rows(&page.items).await;
        let mut view = self.page_to_view(page).await?;
        view.unavailable_count = u32::try_from(unavailable).unwrap_or(u32::MAX);
        if !view.items.is_empty() && self.last_sync_ts.get().await.is_none() {
            self.last_sync_ts.set(Utc::now().timestamp_millis()).await;
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
        // #1239: replica を走査しない。このページの範囲を thread の索引と照合して、欠けている object だけを
        // key 指定で反映する(範囲ごとに間隔を空ける)。thread は途中の返信が欠けうるので、ページが空でなくても
        // 照合する。
        let reconcile = self
            .reconcile_thread_checked(topic_id, &thread_root, cursor.as_ref(), limit)
            .await?;
        let page_is_stale =
            reconcile.page_is_stale(page.items.len(), !hidden_author_pubkeys.is_empty());
        let unavailable = reconcile.unavailable;
        if page_is_stale || page.items.is_empty() {
            if reconcile.hydrated > 0 {
                self.last_sync_ts.set(Utc::now().timestamp_millis()).await;
            }
            // 照合した範囲に、最初のページより多くの object が在ると分かったときだけ読み直す
            // (`list_timeline_scoped` と同じ)。
            if page_is_stale {
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
        }
        Box::pin(self.recover_missing_bodies(&mut page.items)).await;
        continue_past_unavailable(
            &mut page,
            &reconcile,
            PageOrder::OldestFirst,
            Some(&thread_root),
        );
        self.reflect_reply_targets_for_rows(&page.items).await;
        let mut view = self.page_to_view(page).await?;
        view.unavailable_count = u32::try_from(unavailable).unwrap_or(u32::MAX);
        if !view.items.is_empty() && self.last_sync_ts.get().await.is_none() {
            self.last_sync_ts.set(Utc::now().timestamp_millis()).await;
        }
        Ok(view)
    }
}

/// ページの並び(続きの位置の向き)。
#[derive(Clone, Copy)]
enum PageOrder {
    /// タイムライン(新しい順)。続きは古い側。
    NewestFirst,
    /// thread(古い順)。続きは新しい側。
    OldestFirst,
}

/// 照合の結果を、取得したページへ映す(#1239 AC-4)。
///
/// projection が尽きたページで、照合が反映できない entry の続く範囲を読み終えていなければ、読み進めた位置を続きの位置にし、
/// その位置より先(続きの側)の行はこのページから外す(利用者が、取得できない投稿の先へ進めるように)。外した行は次のページが
/// 返す。外さないと、次のページが同じ行をもう一度返し、そのあいだに届いた投稿が後ろに並んで、画面の並びが崩れる(独立監査 B1)。
/// 続きの位置の行そのものは残す(次のページはその位置を含まないので、外すと欠ける。delta 監査 N1)。thread の root 行
/// (最初のページで先頭に 1 行引きする)は、時刻に関わらず外さない。
fn continue_past_unavailable(
    page: &mut Page<ObjectProjectionRow>,
    reconcile: &RangeReconcile,
    order: PageOrder,
    thread_root: Option<&EnvelopeId>,
) {
    if page.next_cursor.is_some() {
        return;
    }
    let Some(read_past) = reconcile.read_past.as_ref() else {
        return;
    };
    let edge = (read_past.created_at, read_past.object_id.as_str());
    page.items.retain(|row| {
        if thread_root.is_some_and(|root| *root == row.object_id) {
            return true;
        }
        let position = (row.created_at, row.object_id.as_str());
        match order {
            PageOrder::NewestFirst => position >= edge,
            PageOrder::OldestFirst => position <= edge,
        }
    });
    page.next_cursor = Some(TimelineCursor {
        created_at: read_past.created_at,
        object_id: EnvelopeId::from(read_past.object_id.as_str()),
    });
}
