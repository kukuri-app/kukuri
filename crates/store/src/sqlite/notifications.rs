use super::*;

#[async_trait]
impl NotificationStore for SqliteStore {
    async fn put_notification_if_absent(&self, row: NotificationRow) -> Result<bool> {
        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO notifications (
              notification_id,
              recipient_pubkey,
              kind,
              actor_pubkey,
              source_envelope_id,
              source_replica_id,
              topic_id,
              channel_id,
              object_id,
              dm_id,
              message_id,
              preview_text,
              content_labels_json,
              created_at,
              received_at,
              read_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            "#,
        )
        .bind(row.notification_id.as_str())
        .bind(row.recipient_pubkey.as_str())
        .bind(notification_kind_name(&row.kind))
        .bind(row.actor_pubkey.as_str())
        .bind(row.source_envelope_id.as_ref().map(EnvelopeId::as_str))
        .bind(row.source_replica_id.as_ref().map(ReplicaId::as_str))
        .bind(row.topic_id.as_deref())
        .bind(row.channel_id.as_deref())
        .bind(row.object_id.as_ref().map(EnvelopeId::as_str))
        .bind(row.dm_id.as_deref())
        .bind(row.message_id.as_deref())
        .bind(row.preview_text.as_deref())
        .bind(
            row.content_labels
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(row.created_at)
        .bind(row.received_at)
        .bind(row.read_at)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_notifications_page(
        &self,
        cursor: Option<&NotificationCursor>,
        before: bool,
    ) -> Result<Vec<NotificationRow>> {
        anyhow::ensure!(
            !before || cursor.is_some(),
            "newer notification page requires cursor"
        );
        let comparison = if before { ">" } else { "<" };
        let order = if before { "ASC" } else { "DESC" };
        let sql = format!(
            r#"
            SELECT
              notification_id,
              recipient_pubkey,
              kind,
              actor_pubkey,
              source_envelope_id,
              source_replica_id,
              topic_id,
              channel_id,
              object_id,
              dm_id,
              message_id,
              preview_text,
              content_labels_json,
              created_at,
              received_at,
              read_at
            FROM notification_inbox_rows {}
            ORDER BY received_at {order}, notification_id {order}
            LIMIT {}
            "#,
            if cursor.is_some() {
                format!("WHERE (received_at, notification_id) {comparison} (?1, ?2)")
            } else {
                String::new()
            },
            NOTIFICATION_PAGE_SIZE + 1,
        );
        let mut query = sqlx::query(&sql);
        if let Some(cursor) = cursor {
            query = query
                .bind(cursor.received_at)
                .bind(cursor.notification_id.as_str());
        }
        let rows = query.fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_notification).collect()
    }

    async fn list_notification_dispatch_after(
        &self,
        after_sequence: i64,
    ) -> Result<Vec<(i64, NotificationRow)>> {
        let rows = sqlx::query(
            r#"
            SELECT
              dispatch_seq,
              notification_id,
              recipient_pubkey,
              kind,
              actor_pubkey,
              source_envelope_id,
              source_replica_id,
              topic_id,
              channel_id,
              object_id,
              dm_id,
              message_id,
              preview_text,
              content_labels_json,
              created_at,
              received_at,
              read_at
            FROM notification_inbox_rows
            WHERE dispatch_seq > ?1
            ORDER BY dispatch_seq ASC
            LIMIT ?2
            "#,
        )
        .bind(after_sequence)
        .bind(crate::NOTIFICATION_DISPATCH_PAGE_SIZE as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| Ok((row.try_get("dispatch_seq")?, row_to_notification(row)?)))
            .collect()
    }

    async fn notification_dispatch_head(&self) -> Result<i64> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT last_seq FROM notification_dispatch_clock WHERE singleton = 1",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    async fn mark_notification_read(&self, notification_id: &str, read_at: i64) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE notifications
            SET read_at = ?2
            WHERE notification_id = ?1 AND read_at IS NULL AND
              (dispatch_seq IS NULL AND
                (SELECT legacy_read_at FROM notification_inbox_state WHERE singleton = 1) IS NULL
               OR dispatch_seq >
                (SELECT read_through_seq FROM notification_inbox_state WHERE singleton = 1))
            "#,
        )
        .bind(notification_id)
        .bind(read_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_all_notifications_read(&self, read_at: i64) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE notification_inbox_state
            SET unread_count = 0,
                read_through_seq = (SELECT last_seq FROM notification_dispatch_clock WHERE singleton = 1),
                read_through_at = ?1,
                legacy_read_at = ?1
            WHERE singleton = 1 AND unread_count > 0
            "#,
        )
        .bind(read_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn count_unread_notifications(&self) -> Result<usize> {
        let count = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT unread_count FROM notification_inbox_state WHERE singleton = 1
            "#,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(count as usize)
    }
}
