use super::*;

pub(crate) struct DirectMessageHintServices<'a> {
    pub(crate) services: &'a ServiceHandles,
    pub(crate) local_author_pubkey: &'a str,
    pub(crate) peer_pubkey: &'a str,
    pub(crate) topic: &'a TopicId,
}

impl AppService {
    pub(crate) async fn handle_direct_message_hint(
        services: DirectMessageHintServices<'_>,
        hint: &GossipHint,
    ) -> Result<bool> {
        let DirectMessageHintServices {
            services,
            local_author_pubkey,
            peer_pubkey,
            topic,
        } = services;
        let projection_store = services.projection_store.as_ref();
        match hint {
            GossipHint::DirectMessageFrame {
                dm_id,
                message_id,
                frame_hash,
                ..
            } => {
                AppService::ingest_direct_message_frame(
                    services,
                    local_author_pubkey,
                    peer_pubkey,
                    topic,
                    dm_id.as_str(),
                    message_id.as_str(),
                    frame_hash,
                )
                .await
            }
            GossipHint::DirectMessageAck { ack, .. } => {
                ack.verify()?;
                if ack.sender.as_str() != peer_pubkey
                    || ack.recipient.as_str() != local_author_pubkey
                {
                    return Ok(false);
                }
                projection_store
                    .set_direct_message_acked_at(
                        ack.dm_id.as_str(),
                        ack.message_id.as_str(),
                        ack.acked_at,
                    )
                    .await?;
                projection_store
                    .remove_direct_message_outbox(ack.dm_id.as_str(), ack.message_id.as_str())
                    .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(crate) async fn maybe_create_notification_for_remote_object_event(
        projection_store: &dyn ProjectionStore,
        docs_sync: &dyn DocsSync,
        blob_service: &dyn BlobService,
        local_author_pubkey: &str,
        topic_id: &str,
        notification_baseline: &NotificationDocEventBaseline,
        event: &DocEvent,
    ) -> Result<bool> {
        if notification_baseline.contains(event) {
            return Ok(false);
        }
        let Some(candidate) = notification_candidate_from_object_event(
            projection_store,
            docs_sync,
            blob_service,
            local_author_pubkey,
            topic_id,
            event,
        )
        .await?
        else {
            return Ok(false);
        };
        Self::put_notification_candidate(projection_store, local_author_pubkey, candidate).await
    }

    pub(crate) async fn maybe_create_notification_for_remote_follow_event(
        store: &dyn Store,
        projection_store: &dyn ProjectionStore,
        docs_sync: &dyn DocsSync,
        local_author_pubkey: &str,
        author_pubkey: &str,
        notification_baseline: &NotificationDocEventBaseline,
        event: &DocEvent,
    ) -> Result<bool> {
        if notification_baseline.contains(event) {
            return Ok(false);
        }
        let Some(candidate) = notification_candidate_from_follow_event(
            store,
            docs_sync,
            local_author_pubkey,
            author_pubkey,
            event,
        )
        .await?
        else {
            return Ok(false);
        };
        Self::put_notification_candidate(projection_store, local_author_pubkey, candidate).await
    }

    pub(crate) async fn put_notification_candidate(
        projection_store: &dyn ProjectionStore,
        recipient_pubkey: &str,
        candidate: NotificationCandidate,
    ) -> Result<bool> {
        let notification_id = if let (Some(dm_id), Some(message_id)) =
            (candidate.dm_id.as_deref(), candidate.message_id.as_deref())
        {
            direct_message_notification_id(recipient_pubkey, &candidate.kind, dm_id, message_id)
        } else {
            let source_envelope_id = candidate
                .source_envelope_id
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("notification is missing source envelope id"))?;
            document_notification_id(recipient_pubkey, &candidate.kind, source_envelope_id)
        };
        projection_store
            .put_notification_if_absent(NotificationRow {
                notification_id,
                recipient_pubkey: recipient_pubkey.to_string(),
                kind: candidate.kind,
                actor_pubkey: candidate.actor_pubkey,
                source_envelope_id: candidate.source_envelope_id,
                source_replica_id: candidate.source_replica_id,
                topic_id: candidate.topic_id,
                channel_id: candidate.channel_id,
                object_id: candidate.object_id,
                dm_id: candidate.dm_id,
                message_id: candidate.message_id,
                preview_text: candidate.preview_text,
                content_labels: candidate.content_labels,
                created_at: candidate.created_at,
                received_at: candidate.received_at,
                read_at: None,
            })
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn ingest_direct_message_frame(
        services: &ServiceHandles,
        local_author_pubkey: &str,
        peer_pubkey: &str,
        topic: &TopicId,
        dm_id: &str,
        message_id: &str,
        frame_hash: &kukuri_core::BlobHash,
    ) -> Result<bool> {
        let projection_store = services.projection_store.as_ref();
        let blob_service = services.blob_service.as_ref();
        let hint_transport = services.hint_transport.as_ref();
        let keys = services.keys.as_ref();
        let expected_dm_id = direct_message_id_for_participants(
            &Pubkey::from(local_author_pubkey),
            &Pubkey::from(peer_pubkey),
        );
        if dm_id != expected_dm_id {
            return Ok(false);
        }
        let Some(frame_bytes) = blob_service.fetch_blob(frame_hash).await? else {
            return Ok(false);
        };
        let frame: DirectMessageFrameV1 = serde_json::from_slice(frame_bytes.as_slice())
            .context("failed to decode direct message frame blob")?;
        if frame.message_id != message_id || frame.dm_id != dm_id {
            return Ok(false);
        }
        if frame.sender.as_str() != peer_pubkey || frame.recipient.as_str() != local_author_pubkey {
            return Ok(false);
        }
        let payload = decrypt_direct_message_frame(keys, &frame)?;
        let ack = build_direct_message_ack(
            keys,
            dm_id,
            message_id,
            &frame.sender,
            Utc::now().timestamp_millis(),
        )?;
        if projection_store
            .has_direct_message_tombstone(dm_id, message_id)
            .await?
        {
            hint_transport
                .publish_hint(
                    topic,
                    GossipHint::DirectMessageAck {
                        topic_id: topic.clone(),
                        ack,
                    },
                )
                .await?;
            return Ok(false);
        }
        if projection_store
            .get_direct_message_message(dm_id, message_id)
            .await?
            .is_some()
        {
            hint_transport
                .publish_hint(
                    topic,
                    GossipHint::DirectMessageAck {
                        topic_id: topic.clone(),
                        ack,
                    },
                )
                .await?;
            return Ok(false);
        }
        let local_manifest = materialize_direct_message_manifest(
            blob_service,
            keys,
            &frame.sender,
            frame.message_id.as_str(),
            payload.attachment_manifest.as_ref(),
        )
        .await?;
        let message_row = DirectMessageMessageRow {
            dm_id: dm_id.to_string(),
            message_id: message_id.to_string(),
            sender_pubkey: frame.sender.as_str().to_string(),
            recipient_pubkey: frame.recipient.as_str().to_string(),
            created_at: frame.created_at,
            text: payload.text,
            reply_to_message_id: payload.reply_to,
            attachment_manifest: local_manifest,
            outgoing: false,
            acked_at: None,
        };
        let preview_text = notification_preview_text(Some(direct_message_preview(&message_row)));
        projection_store
            .put_direct_message_message(message_row)
            .await?;
        projection_store
            .upsert_direct_message_conversation(DirectMessageConversationRow {
                dm_id: dm_id.to_string(),
                peer_pubkey: peer_pubkey.to_string(),
                updated_at: frame.created_at,
                last_message_at: Some(frame.created_at),
                last_message_id: Some(message_id.to_string()),
                last_message_preview: preview_text.clone(),
            })
            .await?;
        Self::put_notification_candidate(
            projection_store,
            local_author_pubkey,
            NotificationCandidate {
                kind: NotificationKind::DirectMessage,
                actor_pubkey: peer_pubkey.to_string(),
                source_envelope_id: None,
                source_replica_id: None,
                topic_id: None,
                channel_id: None,
                object_id: None,
                dm_id: Some(dm_id.to_string()),
                message_id: Some(message_id.to_string()),
                preview_text,
                content_labels: None,
                created_at: frame.created_at,
                received_at: Utc::now().timestamp_millis(),
            },
        )
        .await?;
        hint_transport
            .publish_hint(
                topic,
                GossipHint::DirectMessageAck {
                    topic_id: topic.clone(),
                    ack,
                },
            )
            .await?;
        Ok(true)
    }

    pub(crate) async fn flush_direct_message_outbox_for_peer(
        services: &ServiceHandles,
        local_author_pubkey: &str,
        peer_pubkey: &str,
    ) -> Result<usize> {
        let projection_store = services.projection_store.as_ref();
        let hint_transport = services.hint_transport.as_ref();
        let transport = services.transport.as_ref();
        let keys = services.keys.as_ref();
        let relationship = projection_store
            .get_author_relationship(local_author_pubkey, peer_pubkey)
            .await?;
        if !relationship.as_ref().is_some_and(|value| value.mutual) {
            return Ok(0);
        }
        let topic = derive_direct_message_topic(keys, &Pubkey::from(peer_pubkey))?;
        let peer_count = direct_message_topic_peer_count(transport, &topic).await?;
        let topic_has_connected_peer = peer_count > 0;
        let mut published = 0usize;
        let attempted_at = Utc::now().timestamp_millis();
        for row in projection_store.list_direct_message_outbox().await? {
            if row.peer_pubkey != peer_pubkey {
                continue;
            }
            if topic_has_connected_peer {
                projection_store
                    .touch_direct_message_outbox_attempt(
                        row.dm_id.as_str(),
                        row.message_id.as_str(),
                        attempted_at,
                    )
                    .await?;
            }
            let publish_result = hint_transport
                .publish_hint(
                    &topic,
                    GossipHint::DirectMessageFrame {
                        topic_id: topic.clone(),
                        dm_id: row.dm_id.clone(),
                        message_id: row.message_id.clone(),
                        frame_hash: row.frame_blob_hash.clone(),
                    },
                )
                .await;
            if let Err(error) = publish_result {
                if topic_has_connected_peer {
                    return Err(error);
                }
                continue;
            }
            published += 1;
        }
        Ok(published)
    }

    pub(crate) async fn direct_message_topic_peer_count(&self, peer_pubkey: &str) -> Result<usize> {
        let topic =
            derive_direct_message_topic(self.services.keys.as_ref(), &Pubkey::from(peer_pubkey))?;
        direct_message_topic_peer_count(self.services.transport.as_ref(), &topic).await
    }

    pub(crate) async fn send_direct_message_internal(
        &self,
        peer_pubkey: &str,
        text: Option<&str>,
        reply_to_message_id: Option<&str>,
        attachments: Vec<PendingAttachment>,
    ) -> Result<String> {
        let text = normalize_optional_text(text.map(str::to_string));
        let dm_id = direct_message_id_for_participants(
            &Pubkey::from(self.current_author_pubkey()),
            &Pubkey::from(peer_pubkey),
        );
        if text.is_none() && attachments.is_empty() {
            anyhow::bail!("direct message text or attachment is required");
        }
        let message_id = format!(
            "dm-message-{}-{}",
            Utc::now().timestamp_millis(),
            short_id_suffix(self.current_author_pubkey().as_str())
        );
        if let Some(reply_to_message_id) = reply_to_message_id
            && self
                .services
                .projection_store
                .get_direct_message_message(dm_id.as_str(), reply_to_message_id.trim())
                .await?
                .is_none()
        {
            anyhow::bail!("direct message reply target was not found");
        }
        let (local_manifest, encrypted_manifest) = self
            .prepare_direct_message_manifests(peer_pubkey, message_id.as_str(), attachments)
            .await?;
        let created_at = Utc::now().timestamp_millis();
        let frame = encrypt_direct_message_frame(
            self.services.keys.as_ref(),
            &Pubkey::from(peer_pubkey),
            dm_id.as_str(),
            message_id.as_str(),
            created_at,
            &DirectMessagePayloadV1 {
                text: text.clone(),
                reply_to: normalize_optional_text(reply_to_message_id.map(str::to_string)),
                attachment_manifest: encrypted_manifest,
            },
        )?;
        let frame_bytes =
            serde_json::to_vec(&frame).context("failed to encode direct message frame blob")?;
        let frame_blob = self
            .services
            .blob_service
            .put_blob(frame_bytes, DIRECT_MESSAGE_FRAME_MIME)
            .await?;
        self.services
            .projection_store
            .put_direct_message_message(DirectMessageMessageRow {
                dm_id: dm_id.clone(),
                message_id: message_id.clone(),
                sender_pubkey: self.current_author_pubkey(),
                recipient_pubkey: peer_pubkey.to_string(),
                created_at,
                text,
                reply_to_message_id: normalize_optional_text(
                    reply_to_message_id.map(str::to_string),
                ),
                attachment_manifest: local_manifest,
                outgoing: true,
                acked_at: None,
            })
            .await?;
        self.services
            .projection_store
            .put_direct_message_outbox(DirectMessageOutboxRow {
                dm_id: dm_id.clone(),
                message_id: message_id.clone(),
                peer_pubkey: peer_pubkey.to_string(),
                frame_blob_hash: frame_blob.hash,
                created_at,
                last_attempt_at: None,
            })
            .await?;
        self.refresh_direct_message_conversation(peer_pubkey)
            .await?;
        let _ = Self::flush_direct_message_outbox_for_peer(
            &self.services,
            self.current_author_pubkey().as_str(),
            peer_pubkey,
        )
        .await?;
        Ok(message_id)
    }

    pub(crate) async fn prepare_direct_message_manifests(
        &self,
        peer_pubkey: &str,
        message_id: &str,
        attachments: Vec<PendingAttachment>,
    ) -> Result<(
        Option<DirectMessageAttachmentManifestV1>,
        Option<DirectMessageAttachmentManifestV1>,
    )> {
        if attachments.is_empty() {
            return Ok((None, None));
        }
        let image = attachments
            .iter()
            .find(|attachment| attachment.role == AssetRole::ImageOriginal);
        let video = attachments
            .iter()
            .find(|attachment| attachment.role == AssetRole::VideoManifest);
        let poster = attachments
            .iter()
            .find(|attachment| attachment.role == AssetRole::VideoPoster);
        match (image, video, poster) {
            (Some(image), None, None) => {
                if attachments.len() != 1 || !image.mime.starts_with("image/") {
                    anyhow::bail!(
                        "direct message image attachment must be a single image/* payload"
                    );
                }
                let local_blob = self
                    .services
                    .blob_service
                    .put_blob(image.bytes.clone(), image.mime.as_str())
                    .await?;
                let encrypted = encrypt_direct_message_attachment(
                    self.services.keys.as_ref(),
                    &Pubkey::from(peer_pubkey),
                    message_id,
                    "original",
                    image.bytes.as_slice(),
                )?;
                let encrypted_blob = self
                    .services
                    .blob_service
                    .put_blob(
                        serde_json::to_vec(&encrypted)
                            .context("failed to encode encrypted direct message attachment")?,
                        DIRECT_MESSAGE_ATTACHMENT_MIME,
                    )
                    .await?;
                Ok((
                    Some(DirectMessageAttachmentManifestV1 {
                        attachment_id: "attachment-1".into(),
                        kind: DirectMessageAttachmentKind::Image,
                        original: DirectMessageEncryptedBlobRefV1 {
                            blob_id: "original".into(),
                            hash: local_blob.hash,
                            mime: image.mime.clone(),
                            bytes: image.bytes.len() as u64,
                            nonce_hex: String::new(),
                        },
                        poster: None,
                    }),
                    Some(DirectMessageAttachmentManifestV1 {
                        attachment_id: "attachment-1".into(),
                        kind: DirectMessageAttachmentKind::Image,
                        original: DirectMessageEncryptedBlobRefV1 {
                            blob_id: "original".into(),
                            hash: encrypted_blob.hash,
                            mime: image.mime.clone(),
                            bytes: image.bytes.len() as u64,
                            nonce_hex: encrypted.nonce_hex,
                        },
                        poster: None,
                    }),
                ))
            }
            (None, Some(video), Some(poster)) => {
                if attachments.len() != 2
                    || !video.mime.starts_with("video/")
                    || !poster.mime.starts_with("image/")
                {
                    anyhow::bail!(
                        "direct message video attachment must contain one video/* payload and one image/* poster"
                    );
                }
                let local_video = self
                    .services
                    .blob_service
                    .put_blob(video.bytes.clone(), video.mime.as_str())
                    .await?;
                let local_poster = self
                    .services
                    .blob_service
                    .put_blob(poster.bytes.clone(), poster.mime.as_str())
                    .await?;
                let encrypted_video = encrypt_direct_message_attachment(
                    self.services.keys.as_ref(),
                    &Pubkey::from(peer_pubkey),
                    message_id,
                    "original",
                    video.bytes.as_slice(),
                )?;
                let encrypted_poster = encrypt_direct_message_attachment(
                    self.services.keys.as_ref(),
                    &Pubkey::from(peer_pubkey),
                    message_id,
                    "poster",
                    poster.bytes.as_slice(),
                )?;
                let encrypted_video_blob = self
                    .services
                    .blob_service
                    .put_blob(
                        serde_json::to_vec(&encrypted_video)
                            .context("failed to encode encrypted direct message video")?,
                        DIRECT_MESSAGE_ATTACHMENT_MIME,
                    )
                    .await?;
                let encrypted_poster_blob = self
                    .services
                    .blob_service
                    .put_blob(
                        serde_json::to_vec(&encrypted_poster)
                            .context("failed to encode encrypted direct message poster")?,
                        DIRECT_MESSAGE_ATTACHMENT_MIME,
                    )
                    .await?;
                Ok((
                    Some(DirectMessageAttachmentManifestV1 {
                        attachment_id: "attachment-1".into(),
                        kind: DirectMessageAttachmentKind::Video,
                        original: DirectMessageEncryptedBlobRefV1 {
                            blob_id: "original".into(),
                            hash: local_video.hash,
                            mime: video.mime.clone(),
                            bytes: video.bytes.len() as u64,
                            nonce_hex: String::new(),
                        },
                        poster: Some(DirectMessageEncryptedBlobRefV1 {
                            blob_id: "poster".into(),
                            hash: local_poster.hash,
                            mime: poster.mime.clone(),
                            bytes: poster.bytes.len() as u64,
                            nonce_hex: String::new(),
                        }),
                    }),
                    Some(DirectMessageAttachmentManifestV1 {
                        attachment_id: "attachment-1".into(),
                        kind: DirectMessageAttachmentKind::Video,
                        original: DirectMessageEncryptedBlobRefV1 {
                            blob_id: "original".into(),
                            hash: encrypted_video_blob.hash,
                            mime: video.mime.clone(),
                            bytes: video.bytes.len() as u64,
                            nonce_hex: encrypted_video.nonce_hex,
                        },
                        poster: Some(DirectMessageEncryptedBlobRefV1 {
                            blob_id: "poster".into(),
                            hash: encrypted_poster_blob.hash,
                            mime: poster.mime.clone(),
                            bytes: poster.bytes.len() as u64,
                            nonce_hex: encrypted_poster.nonce_hex,
                        }),
                    }),
                ))
            }
            _ => anyhow::bail!(
                "direct message attachment must be one image or one video with a poster"
            ),
        }
    }
}
