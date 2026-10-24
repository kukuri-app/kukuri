//! 投稿の作成(#1839 で timeline.rs から分けた)。
use crate::service::*;

impl AppService {
    /// `live` は紹介する Live(#1839)。手元に読めている Live だけを紹介でき、限定 channel の Live はその channel でだけ紹介できる。
    #[allow(clippy::too_many_arguments)]
    pub async fn create_post_with_attachments_in_channel(
        &self,
        topic_id: &str,
        channel_ref: ChannelRef,
        content: &str,
        reply_to: Option<&str>,
        attachments: Vec<PendingAttachment>,
        content_labels: Vec<String>,
        live: Option<LiveRefV1>,
    ) -> Result<String> {
        if let Some(live) = &live
            && self
                .get_live_session(live.topic_id.as_str(), live.session_id.as_str())
                .await?
                .is_none_or(|view| {
                    view.channel_id.as_deref() != live.channel_id.as_ref().map(ChannelId::as_str)
                })
        {
            anyhow::bail!("live session not found");
        }
        self.create_post_object(
            topic_id,
            channel_ref,
            content,
            reply_to,
            attachments,
            content_labels,
            live,
        )
        .await
    }

    /// 投稿を作って置く。Live の作成は、まだ置いていない Live の開始 post をここで作る(#1839)。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_post_object(
        &self,
        topic_id: &str,
        channel_ref: ChannelRef,
        content: &str,
        reply_to: Option<&str>,
        attachments: Vec<PendingAttachment>,
        content_labels: Vec<String>,
        live: Option<LiveRefV1>,
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
            let scope = channel_ref.timeline_scope();
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
        // 返信は親の channel に置く(公開 channel の親なら同じ公開 channel、private channel の親なら同じ private channel)。
        let target = if let Some(parent) = parent.as_ref() {
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
            ChannelRef::for_channel(content.channel_id.as_ref())
        } else {
            channel_ref
        };
        let private_state = match &target {
            ChannelRef::PrivateChannel { channel_id } => Some(
                self.private_channel_write_state(topic_id, channel_id)
                    .await?,
            ),
            ChannelRef::PublicChannel { channel_id } => {
                anyhow::ensure!(
                    channel_id.is_public_channel(),
                    "public channel id is invalid"
                );
                None
            }
            ChannelRef::Public => None,
        };
        let effective_channel_id = target.channel_id().cloned();
        if let Some(live) = &live
            && !live.allowed_in(&topic, effective_channel_id.as_ref())
        {
            anyhow::bail!("a limited live session can only be referenced in its channel");
        }
        let now = Utc::now().timestamp_millis();
        let stored_blob = self
            .services
            .blob_service
            .put_blob(content.as_bytes().to_vec(), "text/plain")
            .await?;
        let mut stored_attachments = Vec::new();
        for attachment in attachments {
            if let Some(hash) = attachment.bookmarked_media_hash {
                let saved = self
                    .services
                    .projection_store
                    .get_bookmarked_media(&hash)
                    .await?
                    .context("bookmarked media was not found")?;
                self.services
                    .projection_store
                    .retain_bookmarked_media(&saved)
                    .await?;
                content_labels.extend(saved.content_labels);
                for mut asset in saved.attachments {
                    asset.origin.get_or_insert_with(|| saved.source.clone());
                    stored_attachments.push(asset);
                }
            } else {
                let stored = self
                    .services
                    .blob_service
                    .put_blob(attachment.bytes, &attachment.mime)
                    .await?;
                stored_attachments.push(kukuri_core::AssetRef {
                    hash: stored.hash,
                    mime: stored.mime,
                    bytes: stored.bytes,
                    role: attachment.role,
                    origin: None,
                });
            }
        }
        content_labels.sort();
        content_labels.dedup();
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
                    .map(|stored| MediaManifestItem {
                        blob_hash: stored.hash.clone(),
                        mime: stored.mime.clone(),
                        size: stored.bytes,
                        width: None,
                        height: None,
                        duration_ms: None,
                        codec: None,
                        thumbnail_blob_hash: match stored.role {
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
            stored_attachments,
            manifest
                .iter()
                .map(|(manifest, _)| manifest.manifest_id.clone())
                .collect(),
            parent.as_ref(),
            target.visibility(),
            effective_channel_id.as_ref(),
            content_labels,
            docs_author.as_deref(),
            live,
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
                docs_author: docs_author.clone().filter(|_| private_state.is_none()),
                sent_at: None,
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
}
