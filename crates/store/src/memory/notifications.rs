use super::*;

fn visible_notification(state: &MemoryNotificationRows, row: &NotificationRow) -> NotificationRow {
    let mut row = row.clone();
    if row.read_at.is_none()
        && state.sequence_by_id[&row.notification_id] <= state.read_through_sequence
    {
        row.read_at = state.read_through_at;
    }
    row
}

#[async_trait]
impl NotificationStore for MemoryStore {
    async fn put_notification_if_absent(&self, row: NotificationRow) -> Result<bool> {
        let mut notifications = self.notification_rows.write().await;
        if notifications
            .rows
            .contains_key(row.notification_id.as_str())
        {
            return Ok(false);
        }
        let next = notifications
            .last_sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("notification dispatch sequence exhausted"))?;
        notifications.last_sequence = next;
        notifications
            .by_sequence
            .insert(next, row.notification_id.clone());
        notifications
            .by_received_at
            .insert((row.received_at, row.notification_id.clone()));
        notifications
            .sequence_by_id
            .insert(row.notification_id.clone(), next);
        if row.read_at.is_none() {
            notifications.unread_count += 1;
        }
        notifications.rows.insert(row.notification_id.clone(), row);
        Ok(true)
    }

    async fn list_notifications_page(
        &self,
        cursor: Option<&NotificationCursor>,
        before: bool,
    ) -> Result<Vec<NotificationRow>> {
        use std::ops::Bound::{Excluded, Unbounded};
        let notifications = self.notification_rows.read().await;
        let key = cursor.map(|cursor| (cursor.received_at, cursor.notification_id.clone()));
        let ids: Vec<_> = if before {
            let key =
                key.ok_or_else(|| anyhow::anyhow!("newer notification page requires cursor"))?;
            notifications
                .by_received_at
                .range((Excluded(key), Unbounded))
                .take(NOTIFICATION_PAGE_SIZE + 1)
                .collect()
        } else {
            notifications
                .by_received_at
                .range((Unbounded, key.map(Excluded).unwrap_or(Unbounded)))
                .rev()
                .take(NOTIFICATION_PAGE_SIZE + 1)
                .collect()
        };
        Ok(ids
            .into_iter()
            .map(|(_, id)| visible_notification(&notifications, &notifications.rows[id]))
            .collect())
    }

    async fn list_notification_dispatch_after(
        &self,
        after_sequence: i64,
    ) -> Result<Vec<(i64, NotificationRow)>> {
        use std::ops::Bound::{Excluded, Unbounded};
        let notifications = self.notification_rows.read().await;
        Ok(notifications
            .by_sequence
            .range((Excluded(after_sequence), Unbounded))
            .take(crate::NOTIFICATION_DISPATCH_PAGE_SIZE)
            .map(|(sequence, id)| {
                (
                    *sequence,
                    visible_notification(
                        &notifications,
                        notifications
                            .rows
                            .get(id)
                            .expect("notification dispatch index must reference a row"),
                    ),
                )
            })
            .collect())
    }

    async fn notification_dispatch_head(&self) -> Result<i64> {
        Ok(self.notification_rows.read().await.last_sequence)
    }

    async fn mark_notification_read(&self, notification_id: &str, read_at: i64) -> Result<()> {
        let mut notifications = self.notification_rows.write().await;
        let active = notifications
            .sequence_by_id
            .get(notification_id)
            .is_some_and(|sequence| *sequence > notifications.read_through_sequence);
        if active
            && let Some(row) = notifications.rows.get_mut(notification_id)
            && row.read_at.is_none()
        {
            row.read_at = Some(read_at);
            notifications.unread_count -= 1;
        }
        Ok(())
    }

    async fn mark_all_notifications_read(&self, read_at: i64) -> Result<()> {
        let mut notifications = self.notification_rows.write().await;
        if notifications.unread_count > 0 {
            notifications.read_through_sequence = notifications.last_sequence;
            notifications.read_through_at = Some(read_at);
            notifications.unread_count = 0;
        }
        Ok(())
    }

    async fn count_unread_notifications(&self) -> Result<usize> {
        Ok(self.notification_rows.read().await.unread_count)
    }
}
