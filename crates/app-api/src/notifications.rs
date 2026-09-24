use crate::NotificationPageView;
use crate::service::*;

impl AppService {
    pub async fn list_notifications_page(
        &self,
        cursor: Option<&kukuri_store::NotificationCursor>,
        before: bool,
    ) -> Result<NotificationPageView> {
        let mut rows = self
            .services
            .projection_store
            .list_notifications_page(cursor, before)
            .await?;
        let has_more = rows.len() > kukuri_store::NOTIFICATION_PAGE_SIZE;
        rows.truncate(kukuri_store::NOTIFICATION_PAGE_SIZE);
        if before {
            rows.reverse();
        }
        let first = rows.first().map(kukuri_store::NotificationCursor::from);
        let last = rows.last().map(kukuri_store::NotificationCursor::from);
        let (newer_cursor, older_cursor) = if before {
            (
                has_more.then_some(first).flatten(),
                cursor.is_some().then_some(last).flatten(),
            )
        } else {
            (
                cursor.is_some().then_some(first).flatten(),
                has_more.then_some(last).flatten(),
            )
        };
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(self.notification_view_from_row(row).await?);
        }
        Ok(NotificationPageView {
            items,
            newer_cursor,
            older_cursor,
        })
    }

    pub async fn list_notifications(&self) -> Result<Vec<NotificationView>> {
        let mut items = Vec::new();
        for row in self.services.projection_store.list_notifications().await? {
            items.push(self.notification_view_from_row(row).await?);
        }
        Ok(items)
    }

    pub async fn list_notification_dispatch_after(
        &self,
        after_sequence: i64,
    ) -> Result<Vec<(i64, NotificationView)>> {
        let mut items = Vec::new();
        for (sequence, row) in self
            .services
            .projection_store
            .list_notification_dispatch_after(after_sequence)
            .await?
        {
            items.push((sequence, self.notification_view_from_row(row).await?));
        }
        Ok(items)
    }

    pub async fn notification_dispatch_head(&self) -> Result<i64> {
        self.services
            .projection_store
            .notification_dispatch_head()
            .await
    }

    pub async fn mark_notification_read(
        &self,
        notification_id: &str,
    ) -> Result<NotificationStatusView> {
        self.services
            .projection_store
            .mark_notification_read(notification_id, Utc::now().timestamp_millis())
            .await?;
        self.notification_status_view().await
    }

    pub async fn mark_all_notifications_read(&self) -> Result<NotificationStatusView> {
        self.services
            .projection_store
            .mark_all_notifications_read(Utc::now().timestamp_millis())
            .await?;
        self.notification_status_view().await
    }

    pub async fn get_notification_status(&self) -> Result<NotificationStatusView> {
        self.notification_status_view().await
    }
}
