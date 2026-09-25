use super::*;

impl AppService {
    pub(crate) async fn direct_message_send_enabled(&self, peer_pubkey: &str) -> Result<bool> {
        Ok(self
            .services
            .projection_store
            .get_author_relationship(self.current_author_pubkey().as_str(), peer_pubkey)
            .await?
            .as_ref()
            .is_some_and(|relationship| relationship.mutual))
    }

    pub(crate) async fn direct_message_status_view(
        &self,
        peer_pubkey: &str,
    ) -> Result<DirectMessageStatusView> {
        let dm_id = direct_message_id_for_participants(
            &Pubkey::from(self.current_author_pubkey()),
            &Pubkey::from(peer_pubkey),
        );
        let send_enabled = self.direct_message_send_enabled(peer_pubkey).await?;
        let pending_outbox_page = self
            .services
            .projection_store
            .list_direct_message_outbox_for_peer_page(
                peer_pubkey,
                None,
                None,
                kukuri_store::DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT,
            )
            .await?;
        Ok(DirectMessageStatusView {
            peer_pubkey: peer_pubkey.to_string(),
            dm_id,
            mutual: send_enabled,
            send_enabled,
            pending_outbox_count: pending_outbox_page.items.len(),
            pending_outbox_has_more: pending_outbox_page.next_cursor.is_some(),
        })
    }

    pub(crate) async fn ensure_direct_message_conversation_row(
        &self,
        peer_pubkey: &str,
    ) -> Result<()> {
        if self
            .services
            .projection_store
            .get_direct_message_conversation_by_peer(peer_pubkey)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let dm_id = direct_message_id_for_participants(
            &Pubkey::from(self.current_author_pubkey()),
            &Pubkey::from(peer_pubkey),
        );
        self.services
            .projection_store
            .upsert_direct_message_conversation(DirectMessageConversationRow {
                dm_id,
                peer_pubkey: peer_pubkey.to_string(),
                updated_at: Utc::now().timestamp_millis(),
                last_message_at: None,
                last_message_id: None,
                last_message_preview: None,
            })
            .await
    }

    pub(crate) async fn refresh_direct_message_conversation(
        &self,
        peer_pubkey: &str,
    ) -> Result<()> {
        let dm_id = direct_message_id_for_participants(
            &Pubkey::from(self.current_author_pubkey()),
            &Pubkey::from(peer_pubkey),
        );
        let existing = self
            .services
            .projection_store
            .get_direct_message_conversation_by_peer(peer_pubkey)
            .await?;
        let page = self
            .services
            .projection_store
            .list_direct_message_messages(dm_id.as_str(), None, 1)
            .await?;
        let (updated_at, last_message_at, last_message_id, last_message_preview) =
            if let Some(message) = page.items.first() {
                (
                    message.created_at,
                    Some(message.created_at),
                    Some(message.message_id.clone()),
                    Some(direct_message_preview(message)),
                )
            } else if let Some(existing) = existing.as_ref() {
                (existing.updated_at, None, None, None)
            } else if self.direct_message_send_enabled(peer_pubkey).await? {
                (Utc::now().timestamp_millis(), None, None, None)
            } else {
                return Ok(());
            };
        self.services
            .projection_store
            .upsert_direct_message_conversation(DirectMessageConversationRow {
                dm_id,
                peer_pubkey: peer_pubkey.to_string(),
                updated_at,
                last_message_at,
                last_message_id,
                last_message_preview,
            })
            .await
    }

    pub(crate) async fn direct_message_conversation_view(
        &self,
        peer_pubkey: &str,
    ) -> Result<DirectMessageConversationView> {
        let conversation = self
            .services
            .projection_store
            .get_direct_message_conversation_by_peer(peer_pubkey)
            .await?
            .ok_or_else(|| anyhow::anyhow!("direct message conversation is not initialized"))?;
        let profile = self.services.store.get_profile(peer_pubkey).await?;
        let status = self.direct_message_status_view(peer_pubkey).await?;
        Ok(DirectMessageConversationView {
            dm_id: conversation.dm_id,
            peer_pubkey: peer_pubkey.to_string(),
            peer_name: profile.as_ref().and_then(|value| value.name.clone()),
            peer_display_name: profile
                .as_ref()
                .and_then(|value| value.display_name.clone()),
            peer_picture_asset: profile_asset_view_from_ref(
                profile
                    .as_ref()
                    .and_then(|value| value.picture_asset.as_ref()),
            ),
            updated_at: conversation.updated_at,
            last_message_at: conversation.last_message_at,
            last_message_id: conversation.last_message_id,
            last_message_preview: conversation.last_message_preview,
            status,
        })
    }

    pub(crate) async fn direct_message_message_view(
        &self,
        row: DirectMessageMessageRow,
    ) -> Result<DirectMessageMessageView> {
        Ok(DirectMessageMessageView {
            dm_id: row.dm_id,
            message_id: row.message_id,
            sender_pubkey: row.sender_pubkey,
            recipient_pubkey: row.recipient_pubkey,
            created_at: row.created_at,
            text: row.text.unwrap_or_default(),
            reply_to_message_id: row.reply_to_message_id,
            attachments: direct_message_attachment_views(
                self.services.blob_service.as_ref(),
                row.attachment_manifest.as_ref(),
            )
            .await?,
            outgoing: row.outgoing,
            delivered: row.acked_at.is_some() || !row.outgoing,
        })
    }

    pub(crate) async fn notification_view_from_row(
        &self,
        row: NotificationRow,
    ) -> Result<NotificationView> {
        let object_id = row.object_id.clone();
        let is_withdrawn = match object_id.as_ref() {
            Some(object_id) => self
                .services
                .projection_store
                .get_post_withdrawal(object_id)
                .await?
                .is_some(),
            None => false,
        };
        let object_projection = if let Some(object_id) = object_id.as_ref() {
            self.services
                .projection_store
                .get_object_projection(object_id)
                .await?
        } else {
            None
        };
        let thread_root_object_id = object_projection.as_ref().map(|projection| {
            projection
                .root_object_id
                .as_ref()
                .unwrap_or(&projection.object_id)
                .as_str()
                .to_string()
        });
        let content_labels = row.content_labels.clone().or_else(|| {
            object_projection
                .as_ref()
                .map(|projection| projection.content_labels.clone())
        });
        let preview_is_gated = object_id.is_some()
            && !self.adult_content_display_enabled()
            && content_labels
                .as_ref()
                .is_none_or(|labels| kukuri_core::has_adult_content_label(labels));
        let profile = self
            .services
            .store
            .get_profile(row.actor_pubkey.as_str())
            .await?;
        Ok(NotificationView {
            notification_id: row.notification_id,
            kind: row.kind,
            actor_pubkey: row.actor_pubkey,
            actor_name: profile.as_ref().and_then(|value| value.name.clone()),
            actor_display_name: profile
                .as_ref()
                .and_then(|value| value.display_name.clone()),
            actor_picture_asset: profile_asset_view_from_ref(
                profile
                    .as_ref()
                    .and_then(|value| value.picture_asset.as_ref()),
            ),
            source_envelope_id: row
                .source_envelope_id
                .map(|value| value.as_str().to_string()),
            source_replica_id: row
                .source_replica_id
                .map(|value| value.as_str().to_string()),
            topic_id: row.topic_id,
            channel_id: row.channel_id,
            object_id: object_id.map(|value| value.as_str().to_string()),
            thread_root_object_id,
            dm_id: row.dm_id,
            message_id: row.message_id,
            preview_text: if is_withdrawn || preview_is_gated {
                None
            } else {
                row.preview_text
            },
            content_labels,
            created_at: row.created_at,
            received_at: row.received_at,
            read_at: row.read_at,
        })
    }

    pub(crate) async fn notification_status_view(&self) -> Result<NotificationStatusView> {
        Ok(NotificationStatusView {
            unread_count: self
                .services
                .projection_store
                .count_unread_notifications()
                .await?,
        })
    }
}
