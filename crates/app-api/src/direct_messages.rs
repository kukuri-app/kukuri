use crate::service::*;
use kukuri_store::DirectMessageOutboxCursor;

pub struct PendingReceiveDestinationPage {
    pub recipients: Vec<Pubkey>,
    pub next_cursor: Option<DirectMessageOutboxCursor>,
    pub cycle_end: Option<DirectMessageOutboxCursor>,
}

impl AppService {
    /// Account-specific CN discovery demand; never materialize every outbox
    /// row or every mutual peer merely to choose a receive route candidate.
    pub async fn pending_receive_destination_recipients(
        &self,
        after: Option<&DirectMessageOutboxCursor>,
        cycle_end: Option<&DirectMessageOutboxCursor>,
    ) -> Result<PendingReceiveDestinationPage> {
        let page = self
            .services
            .projection_store
            .list_direct_message_outbox_candidate_page(after, cycle_end, 4)
            .await?;
        let local = self.current_author_pubkey();
        let mut unique = BTreeSet::new();
        for row in page.items {
            let Ok(peer) = normalize_author_pubkey(&row.peer_pubkey) else {
                continue;
            };
            if self
                .services
                .projection_store
                .get_author_relationship(local.as_str(), peer.as_str())
                .await?
                .as_ref()
                .is_some_and(|relationship| relationship.mutual)
            {
                unique.insert(Pubkey::from(peer));
            }
        }
        Ok(PendingReceiveDestinationPage {
            recipients: unique.into_iter().collect(),
            cycle_end: page.next_cursor.as_ref().and(page.cycle_end),
            next_cursor: page.next_cursor,
        })
    }

    /// 起動時の DM の再開(#1221 R4-D)。会話・outbox・mutual を列挙せず、account の再送 owner を起動するだけ。
    /// 未 ACK の outbox は owner が due 索引から読み、受信は account の受信 route が担う。
    pub async fn resume_direct_message_state(&self) -> Result<()> {
        self.start_direct_message_outbox_retry().await
    }

    pub async fn open_direct_message(
        &self,
        peer_pubkey: &str,
    ) -> Result<DirectMessageConversationView> {
        let peer_pubkey = normalize_author_pubkey(peer_pubkey)?;
        // 表示・送信の需要がある相手だけ author を購読し、自分を指す edge を追いつかせる(関係は読むときに edge から求める)。
        let existing = self
            .services
            .projection_store
            .get_direct_message_conversation_by_peer(peer_pubkey.as_str())
            .await?;
        let can_send = self
            .direct_message_send_enabled(peer_pubkey.as_str())
            .await?;
        if existing.is_none() && !can_send {
            anyhow::bail!("direct message requires a mutual relationship");
        }
        self.ensure_direct_message_conversation_row(peer_pubkey.as_str())
            .await?;
        self.direct_message_conversation_view(peer_pubkey.as_str())
            .await
    }

    pub async fn list_direct_messages(&self) -> Result<Vec<DirectMessageConversationView>> {
        let rows = self
            .services
            .projection_store
            .list_direct_message_conversations()
            .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(
                self.direct_message_conversation_view(row.peer_pubkey.as_str())
                    .await?,
            );
        }
        Ok(items)
    }

    pub async fn list_direct_message_messages(
        &self,
        peer_pubkey: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<DirectMessageTimelineView> {
        let peer_pubkey = normalize_author_pubkey(peer_pubkey)?;
        let existing = self
            .services
            .projection_store
            .get_direct_message_conversation_by_peer(peer_pubkey.as_str())
            .await?;
        let can_send = self
            .direct_message_send_enabled(peer_pubkey.as_str())
            .await?;
        if existing.is_none() && !can_send {
            anyhow::bail!("direct message requires a mutual relationship");
        }
        self.ensure_direct_message_conversation_row(peer_pubkey.as_str())
            .await?;
        let dm_id = direct_message_id_for_participants(
            &Pubkey::from(self.current_author_pubkey()),
            &Pubkey::from(peer_pubkey.as_str()),
        );
        let page = self
            .services
            .projection_store
            .list_direct_message_messages(dm_id.as_str(), cursor, limit)
            .await?;
        let mut items = Vec::with_capacity(page.items.len());
        for row in page.items {
            items.push(self.direct_message_message_view(row).await?);
        }
        Ok(DirectMessageTimelineView {
            items,
            next_cursor: page.next_cursor,
        })
    }

    pub async fn send_direct_message(
        &self,
        peer_pubkey: &str,
        text: Option<&str>,
        reply_to_message_id: Option<&str>,
        attachments: Vec<PendingAttachment>,
    ) -> Result<String> {
        let peer_pubkey = normalize_author_pubkey(peer_pubkey)?;
        // 表示・送信の需要がある相手だけ author を購読し、自分を指す edge を追いつかせる(関係は読むときに edge から求める)。
        if !self
            .direct_message_send_enabled(peer_pubkey.as_str())
            .await?
        {
            anyhow::bail!("direct message requires a mutual relationship");
        }
        self.send_direct_message_internal(
            peer_pubkey.as_str(),
            text,
            reply_to_message_id,
            attachments,
        )
        .await
    }

    pub async fn delete_direct_message_message(
        &self,
        peer_pubkey: &str,
        message_id: &str,
    ) -> Result<()> {
        let peer_pubkey = normalize_author_pubkey(peer_pubkey)?;
        let message_id = message_id.trim();
        if message_id.is_empty() {
            anyhow::bail!("direct message message_id is required");
        }
        let dm_id = direct_message_id_for_participants(
            &Pubkey::from(self.current_author_pubkey()),
            &Pubkey::from(peer_pubkey.as_str()),
        );
        self.services
            .projection_store
            .put_direct_message_tombstone(DirectMessageTombstoneRow {
                dm_id: dm_id.clone(),
                message_id: message_id.to_string(),
                deleted_at: Utc::now().timestamp_millis(),
            })
            .await?;
        self.services
            .projection_store
            .delete_direct_message_message_local(dm_id.as_str(), message_id)
            .await?;
        self.refresh_direct_message_conversation(peer_pubkey.as_str())
            .await?;
        Ok(())
    }

    pub async fn clear_direct_message(&self, peer_pubkey: &str) -> Result<()> {
        let peer_pubkey = normalize_author_pubkey(peer_pubkey)?;
        let dm_id = direct_message_id_for_participants(
            &Pubkey::from(self.current_author_pubkey()),
            &Pubkey::from(peer_pubkey.as_str()),
        );
        let deleted_at = Utc::now().timestamp_millis();
        let mut cursor = None;
        loop {
            let page = self
                .services
                .projection_store
                .list_direct_message_messages(dm_id.as_str(), cursor.clone(), 500)
                .await?;
            for row in &page.items {
                self.services
                    .projection_store
                    .put_direct_message_tombstone(DirectMessageTombstoneRow {
                        dm_id: dm_id.clone(),
                        message_id: row.message_id.clone(),
                        deleted_at,
                    })
                    .await?;
            }
            if page.next_cursor.is_none() {
                break;
            }
            cursor = page.next_cursor;
        }
        self.services
            .projection_store
            .clear_direct_message_local(dm_id.as_str())
            .await?;
        Ok(())
    }

    pub async fn get_direct_message_status(
        &self,
        peer_pubkey: &str,
    ) -> Result<DirectMessageStatusView> {
        let peer_pubkey = normalize_author_pubkey(peer_pubkey)?;
        // 表示・送信の需要がある相手だけ author を購読し、自分を指す edge を追いつかせる(関係は読むときに edge から求める)。
        self.direct_message_status_view(peer_pubkey.as_str()).await
    }
}
