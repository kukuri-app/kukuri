use super::*;

#[async_trait]
impl DirectMessageStore for SqliteStore {
    async fn upsert_direct_message_conversation(
        &self,
        row: DirectMessageConversationRow,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dm_conversations (
              dm_id, peer_pubkey, updated_at, last_message_at, last_message_id, last_message_preview
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(dm_id) DO UPDATE SET
              peer_pubkey = excluded.peer_pubkey,
              updated_at = excluded.updated_at,
              last_message_at = excluded.last_message_at,
              last_message_id = excluded.last_message_id,
              last_message_preview = excluded.last_message_preview
            "#,
        )
        .bind(row.dm_id.as_str())
        .bind(row.peer_pubkey.as_str())
        .bind(row.updated_at)
        .bind(row.last_message_at)
        .bind(row.last_message_id.as_deref())
        .bind(row.last_message_preview.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_direct_message_conversation_by_peer(
        &self,
        peer_pubkey: &str,
    ) -> Result<Option<DirectMessageConversationRow>> {
        let row = sqlx::query(
            r#"
            SELECT dm_id, peer_pubkey, updated_at, last_message_at, last_message_id, last_message_preview
            FROM dm_conversations
            WHERE peer_pubkey = ?1
            "#,
        )
        .bind(peer_pubkey)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_direct_message_conversation).transpose()
    }

    async fn get_direct_message_conversation_by_dm_id(
        &self,
        dm_id: &str,
    ) -> Result<Option<DirectMessageConversationRow>> {
        let row = sqlx::query(
            r#"
            SELECT dm_id, peer_pubkey, updated_at, last_message_at, last_message_id, last_message_preview
            FROM dm_conversations
            WHERE dm_id = ?1
            "#,
        )
        .bind(dm_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_direct_message_conversation).transpose()
    }

    async fn list_direct_message_conversations(&self) -> Result<Vec<DirectMessageConversationRow>> {
        let rows = sqlx::query(
            r#"
            SELECT dm_id, peer_pubkey, updated_at, last_message_at, last_message_id, last_message_preview
            FROM dm_conversations
            ORDER BY updated_at DESC, dm_id DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(row_to_direct_message_conversation)
            .collect()
    }

    async fn put_direct_message_message(&self, row: DirectMessageMessageRow) -> Result<()> {
        let tombstoned = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT 1
            FROM dm_message_tombstones
            WHERE dm_id = ?1 AND message_id = ?2
            LIMIT 1
            "#,
        )
        .bind(row.dm_id.as_str())
        .bind(row.message_id.as_str())
        .fetch_optional(&self.pool)
        .await?
        .is_some();
        if tombstoned {
            return Ok(());
        }
        let attachment_manifest_json = row
            .attachment_manifest
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        sqlx::query(
            r#"
            INSERT INTO dm_messages (
              dm_id, message_id, sender_pubkey, recipient_pubkey, created_at, text,
              reply_to_message_id, attachment_manifest_json, outgoing, acked_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(dm_id, message_id) DO UPDATE SET
              sender_pubkey = excluded.sender_pubkey,
              recipient_pubkey = excluded.recipient_pubkey,
              created_at = excluded.created_at,
              text = excluded.text,
              reply_to_message_id = excluded.reply_to_message_id,
              attachment_manifest_json = excluded.attachment_manifest_json,
              outgoing = excluded.outgoing,
              acked_at = excluded.acked_at
            "#,
        )
        .bind(row.dm_id.as_str())
        .bind(row.message_id.as_str())
        .bind(row.sender_pubkey.as_str())
        .bind(row.recipient_pubkey.as_str())
        .bind(row.created_at)
        .bind(row.text.as_deref())
        .bind(row.reply_to_message_id.as_deref())
        .bind(attachment_manifest_json.as_deref())
        .bind(if row.outgoing { 1_i64 } else { 0_i64 })
        .bind(row.acked_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_direct_message_message(
        &self,
        dm_id: &str,
        message_id: &str,
    ) -> Result<Option<DirectMessageMessageRow>> {
        let row = sqlx::query(
            r#"
            SELECT dm_id, message_id, sender_pubkey, recipient_pubkey, created_at, text,
                   reply_to_message_id, attachment_manifest_json, outgoing, acked_at
            FROM dm_messages
            WHERE dm_id = ?1 AND message_id = ?2
            "#,
        )
        .bind(dm_id)
        .bind(message_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_direct_message_message).transpose()
    }

    async fn list_direct_message_messages(
        &self,
        dm_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<DirectMessageMessageRow>> {
        let rows = sqlx::query(
            r#"
            SELECT dm_id, message_id, sender_pubkey, recipient_pubkey, created_at, text,
                   reply_to_message_id, attachment_manifest_json, outgoing, acked_at
            FROM dm_messages
            WHERE dm_id = ?1
              AND (
                ?2 IS NULL
                OR created_at < ?2
                OR (created_at = ?2 AND message_id < ?3)
              )
            ORDER BY created_at DESC, message_id DESC
            LIMIT ?4
            "#,
        )
        .bind(dm_id)
        .bind(cursor.as_ref().map(|value| value.created_at))
        .bind(cursor.as_ref().map(|value| value.object_id.as_str()))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        direct_message_page_from_rows(rows, limit)
    }

    async fn set_direct_message_acked_at(
        &self,
        dm_id: &str,
        message_id: &str,
        acked_at: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE dm_messages
            SET acked_at = COALESCE(acked_at, ?3)
            WHERE dm_id = ?1 AND message_id = ?2
            "#,
        )
        .bind(dm_id)
        .bind(message_id)
        .bind(acked_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn put_direct_message_outbox(&self, row: DirectMessageOutboxRow) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dm_outbox (
              dm_id, message_id, peer_pubkey, frame_blob_hash, created_at, last_attempt_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(dm_id, message_id) DO UPDATE SET
              peer_pubkey = excluded.peer_pubkey,
              frame_blob_hash = excluded.frame_blob_hash,
              created_at = excluded.created_at,
              last_attempt_at = excluded.last_attempt_at
            "#,
        )
        .bind(row.dm_id.as_str())
        .bind(row.message_id.as_str())
        .bind(row.peer_pubkey.as_str())
        .bind(row.frame_blob_hash.as_str())
        .bind(row.created_at)
        .bind(row.last_attempt_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_direct_message_outbox(
        &self,
        dm_id: &str,
        message_id: &str,
    ) -> Result<Option<DirectMessageOutboxRow>> {
        let row = sqlx::query(
            r#"
            SELECT dm_id, message_id, peer_pubkey, frame_blob_hash, created_at, last_attempt_at
            FROM dm_outbox
            WHERE dm_id = ?1 AND message_id = ?2
            "#,
        )
        .bind(dm_id)
        .bind(message_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_direct_message_outbox).transpose()
    }

    async fn list_direct_message_outbox(&self) -> Result<Vec<DirectMessageOutboxRow>> {
        let rows = sqlx::query(
            r#"
            SELECT dm_id, message_id, peer_pubkey, frame_blob_hash, created_at, last_attempt_at
            FROM dm_outbox
            ORDER BY created_at ASC, message_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_direct_message_outbox).collect()
    }

    async fn list_direct_message_outbox_candidate_page(
        &self,
        after: Option<&DirectMessageOutboxCursor>,
        cycle_end: Option<&DirectMessageOutboxCursor>,
        limit: usize,
    ) -> Result<DirectMessageOutboxPage> {
        anyhow::ensure!(
            (1..=DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT).contains(&limit),
            "invalid direct message outbox candidate page limit"
        );
        anyhow::ensure!(
            after.is_none() || cycle_end.is_some(),
            "missing candidate cycle end"
        );
        let end = match cycle_end {
            Some(end) => Some(end.clone()),
            None => sqlx::query(
                "SELECT created_at, message_id, dm_id FROM dm_outbox \
                 ORDER BY created_at DESC, message_id DESC, dm_id DESC LIMIT 1",
            )
            .fetch_optional(&self.pool)
            .await?
            .map(|row| -> Result<DirectMessageOutboxCursor> {
                Ok(DirectMessageOutboxCursor {
                    created_at: row.try_get("created_at")?,
                    message_id: row.try_get("message_id")?,
                    dm_id: row.try_get("dm_id")?,
                })
            })
            .transpose()?,
        };
        let Some(end) = end else {
            return Ok(DirectMessageOutboxPage {
                items: Vec::new(),
                next_cursor: None,
                cycle_end: None,
            });
        };
        let rows = if let Some(after) = after {
            sqlx::query(
                "SELECT dm_id, message_id, peer_pubkey, frame_blob_hash, created_at, last_attempt_at \
                 FROM dm_outbox WHERE (created_at, message_id, dm_id) > (?1, ?2, ?3) \
                 AND (created_at, message_id, dm_id) <= (?4, ?5, ?6) \
                 ORDER BY created_at, message_id, dm_id LIMIT ?7",
            )
            .bind(after.created_at)
            .bind(after.message_id.as_str())
            .bind(after.dm_id.as_str())
            .bind(end.created_at)
            .bind(end.message_id.as_str())
            .bind(end.dm_id.as_str())
            .bind((limit + 1) as i64)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT dm_id, message_id, peer_pubkey, frame_blob_hash, created_at, last_attempt_at \
                 FROM dm_outbox WHERE (created_at, message_id, dm_id) <= (?1, ?2, ?3) \
                 ORDER BY created_at, message_id, dm_id LIMIT ?4",
            )
            .bind(end.created_at)
            .bind(end.message_id.as_str())
            .bind(end.dm_id.as_str())
            .bind((limit + 1) as i64)
            .fetch_all(&self.pool)
            .await?
        };
        let has_more = rows.len() > limit;
        let items = rows
            .into_iter()
            .take(limit)
            .map(row_to_direct_message_outbox)
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = has_more.then(|| {
            let last = items.last().expect("nonempty candidate page");
            DirectMessageOutboxCursor {
                created_at: last.created_at,
                message_id: last.message_id.clone(),
                dm_id: last.dm_id.clone(),
            }
        });
        Ok(DirectMessageOutboxPage {
            items,
            next_cursor,
            cycle_end: Some(end),
        })
    }

    async fn list_direct_message_outbox_for_peer_page(
        &self,
        peer_pubkey: &str,
        after: Option<&DirectMessageOutboxCursor>,
        cycle_end: Option<&DirectMessageOutboxCursor>,
        limit: usize,
    ) -> Result<DirectMessageOutboxPage> {
        anyhow::ensure!(
            (1..=DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT).contains(&limit),
            "invalid direct message outbox page limit"
        );
        anyhow::ensure!(
            after.is_none() || cycle_end.is_some(),
            "missing outbox cycle end"
        );
        let cycle_end = match cycle_end {
            Some(end) => Some(end.clone()),
            None => sqlx::query(
                "SELECT created_at, message_id, dm_id FROM dm_outbox \
                 WHERE peer_pubkey = ?1 ORDER BY created_at DESC, message_id DESC, dm_id DESC LIMIT 1",
            )
            .bind(peer_pubkey)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| -> Result<DirectMessageOutboxCursor> {
                Ok(DirectMessageOutboxCursor {
                    created_at: row.try_get("created_at")?,
                    message_id: row.try_get("message_id")?,
                    dm_id: row.try_get("dm_id")?,
                })
            })
            .transpose()?,
        };
        let Some(end) = cycle_end.as_ref() else {
            return Ok(DirectMessageOutboxPage {
                items: Vec::new(),
                next_cursor: None,
                cycle_end: None,
            });
        };
        let fetch_limit = (limit + 1) as i64;
        let rows = if let Some(after) = after {
            sqlx::query(
                r#"
                SELECT dm_id, message_id, peer_pubkey, frame_blob_hash, created_at, last_attempt_at
                FROM dm_outbox
                WHERE peer_pubkey = ?1
                  AND (created_at, message_id, dm_id) > (?2, ?3, ?4)
                  AND (created_at, message_id, dm_id) <= (?5, ?6, ?7)
                ORDER BY created_at ASC, message_id ASC, dm_id ASC
                LIMIT ?8
                "#,
            )
            .bind(peer_pubkey)
            .bind(after.created_at)
            .bind(after.message_id.as_str())
            .bind(after.dm_id.as_str())
            .bind(end.created_at)
            .bind(end.message_id.as_str())
            .bind(end.dm_id.as_str())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT dm_id, message_id, peer_pubkey, frame_blob_hash, created_at, last_attempt_at
                FROM dm_outbox
                WHERE peer_pubkey = ?1
                  AND (created_at, message_id, dm_id) <= (?2, ?3, ?4)
                ORDER BY created_at ASC, message_id ASC, dm_id ASC
                LIMIT ?5
                "#,
            )
            .bind(peer_pubkey)
            .bind(end.created_at)
            .bind(end.message_id.as_str())
            .bind(end.dm_id.as_str())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await?
        };
        let has_more = rows.len() > limit;
        let items = rows
            .into_iter()
            .take(limit)
            .map(row_to_direct_message_outbox)
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = if has_more {
            items.last().map(|last| DirectMessageOutboxCursor {
                created_at: last.created_at,
                message_id: last.message_id.clone(),
                dm_id: last.dm_id.clone(),
            })
        } else {
            None
        };
        Ok(DirectMessageOutboxPage {
            items,
            next_cursor,
            cycle_end,
        })
    }

    async fn list_due_direct_message_outbox(
        &self,
        retry_due_at_or_before: i64,
        new_limit: usize,
        retry_limit: usize,
    ) -> Result<Vec<DirectMessageOutboxRow>> {
        anyhow::ensure!(
            new_limit <= 3 && retry_limit <= 1 && new_limit + retry_limit > 0,
            "invalid direct message outbox due limits"
        );
        let mut selected = Vec::with_capacity(new_limit + retry_limit);
        if new_limit > 0 {
            let rows = sqlx::query(
                r#"
                SELECT dm_id, message_id, peer_pubkey, frame_blob_hash, created_at, last_attempt_at
                FROM dm_outbox
                WHERE last_attempt_at IS NULL
                ORDER BY created_at ASC, message_id ASC, dm_id ASC, peer_pubkey ASC
                LIMIT ?1
                "#,
            )
            .bind(new_limit as i64)
            .fetch_all(&self.pool)
            .await?;
            selected.extend(
                rows.into_iter()
                    .map(row_to_direct_message_outbox)
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        if retry_limit > 0 {
            let rows = sqlx::query(
                r#"
                SELECT dm_id, message_id, peer_pubkey, frame_blob_hash, created_at, last_attempt_at
                FROM dm_outbox
                WHERE last_attempt_at IS NOT NULL AND last_attempt_at <= ?1
                ORDER BY last_attempt_at ASC, created_at ASC, message_id ASC, dm_id ASC, peer_pubkey ASC
                LIMIT ?2
                "#,
            )
            .bind(retry_due_at_or_before)
            .bind(retry_limit as i64)
            .fetch_all(&self.pool)
            .await?;
            selected.extend(
                rows.into_iter()
                    .map(row_to_direct_message_outbox)
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        Ok(selected)
    }

    async fn touch_direct_message_outbox_attempt(
        &self,
        dm_id: &str,
        message_id: &str,
        attempted_at: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE dm_outbox
            SET last_attempt_at = ?3
            WHERE dm_id = ?1 AND message_id = ?2
            "#,
        )
        .bind(dm_id)
        .bind(message_id)
        .bind(attempted_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn remove_direct_message_outbox(&self, dm_id: &str, message_id: &str) -> Result<()> {
        // #1221 R5-G: ACK で送信待ちを消す transaction の中で、frame と暗号化添付の保護を外す。
        let mut update = self.begin_protected_ref_update().await?;
        sqlx::query("DELETE FROM dm_outbox WHERE dm_id = ?1 AND message_id = ?2")
            .bind(dm_id)
            .bind(message_id)
            .execute(&mut *update.tx)
            .await?;
        self.set_refs_in(&mut update, &format!("dm_outbox:{dm_id}/{message_id}"), &[])
            .await?;
        self.commit_protected_ref_update(update).await
    }

    async fn put_direct_message_tombstone(&self, row: DirectMessageTombstoneRow) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dm_message_tombstones (dm_id, message_id, deleted_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(dm_id, message_id) DO UPDATE SET
              deleted_at = excluded.deleted_at
            "#,
        )
        .bind(row.dm_id.as_str())
        .bind(row.message_id.as_str())
        .bind(row.deleted_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_direct_message_tombstones(
        &self,
        dm_id: &str,
    ) -> Result<Vec<DirectMessageTombstoneRow>> {
        let rows = sqlx::query(
            r#"
            SELECT dm_id, message_id, deleted_at
            FROM dm_message_tombstones
            WHERE dm_id = ?1
            ORDER BY deleted_at DESC, message_id DESC
            "#,
        )
        .bind(dm_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(row_to_direct_message_tombstone)
            .collect()
    }

    async fn has_direct_message_tombstone(&self, dm_id: &str, message_id: &str) -> Result<bool> {
        let exists = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT 1
            FROM dm_message_tombstones
            WHERE dm_id = ?1 AND message_id = ?2
            LIMIT 1
            "#,
        )
        .bind(dm_id)
        .bind(message_id)
        .fetch_optional(&self.pool)
        .await?
        .is_some();
        Ok(exists)
    }

    async fn delete_direct_message_message_local(
        &self,
        dm_id: &str,
        message_id: &str,
    ) -> Result<()> {
        let mut update = self.begin_protected_ref_update().await?;
        for table in ["dm_messages", "dm_outbox"] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE dm_id = ?1 AND message_id = ?2"
            ))
            .bind(dm_id)
            .bind(message_id)
            .execute(&mut *update.tx)
            .await?;
        }
        // #1221 R5-G: 手元から消した message の添付と送信待ちの保護を外す。
        for kind in ["dm_message", "dm_outbox"] {
            self.set_refs_in(&mut update, &format!("{kind}:{dm_id}/{message_id}"), &[])
                .await?;
        }
        self.commit_protected_ref_update(update).await
    }

    async fn clear_direct_message_local(&self, dm_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM dm_messages
            WHERE dm_id = ?1
            "#,
        )
        .bind(dm_id)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            DELETE FROM dm_outbox
            WHERE dm_id = ?1
            "#,
        )
        .bind(dm_id)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            DELETE FROM dm_conversations
            WHERE dm_id = ?1
            "#,
        )
        .bind(dm_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
