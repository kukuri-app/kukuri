use super::*;

impl DesktopRuntime {
    pub async fn list_notifications_page(
        &self,
        request: ListNotificationsPageRequest,
    ) -> Result<NotificationPageView> {
        self.app_service
            .list_notifications_page(request.cursor.as_ref(), request.before)
            .await
    }

    pub async fn list_notifications(&self) -> Result<Vec<NotificationView>> {
        self.app_service.list_notifications().await
    }

    pub async fn list_notification_dispatch_after(
        &self,
        after_sequence: i64,
    ) -> Result<Vec<(i64, NotificationView)>> {
        self.app_service
            .list_notification_dispatch_after(after_sequence)
            .await
    }

    pub async fn notification_dispatch_head(&self) -> Result<i64> {
        self.app_service.notification_dispatch_head().await
    }

    pub async fn mark_notification_read(
        &self,
        request: NotificationIdRequest,
    ) -> Result<NotificationStatusView> {
        let status = self
            .app_service
            .mark_notification_read(request.notification_id.as_str())
            .await?;
        self.emit_event(RuntimeEvent::NotificationStatusChanged);
        Ok(status)
    }

    pub async fn mark_all_notifications_read(&self) -> Result<NotificationStatusView> {
        let status = self.app_service.mark_all_notifications_read().await?;
        self.emit_event(RuntimeEvent::NotificationStatusChanged);
        Ok(status)
    }

    pub async fn get_notification_status(&self) -> Result<NotificationStatusView> {
        self.app_service.get_notification_status().await
    }

    pub async fn open_direct_message(
        &self,
        request: DirectMessageRequest,
    ) -> Result<DirectMessageConversationView> {
        self.app_service
            .open_direct_message(request.pubkey.as_str())
            .await
    }

    pub async fn list_direct_messages(&self) -> Result<Vec<DirectMessageConversationView>> {
        self.app_service.list_direct_messages().await
    }

    pub async fn list_direct_message_messages(
        &self,
        request: ListDirectMessageMessagesRequest,
    ) -> Result<DirectMessageTimelineView> {
        self.app_service
            .list_direct_message_messages(
                request.pubkey.as_str(),
                request.cursor,
                request.limit.unwrap_or(50),
            )
            .await
    }

    pub async fn send_direct_message(&self, request: SendDirectMessageRequest) -> Result<String> {
        let attachments = request
            .attachments
            .into_iter()
            .map(pending_attachment_from_request)
            .collect::<Result<Vec<_>>>()?;
        self.app_service
            .send_direct_message(
                request.pubkey.as_str(),
                request.text.as_deref(),
                request.reply_to_message_id.as_deref(),
                attachments,
            )
            .await
    }

    pub async fn delete_direct_message_message(
        &self,
        request: DeleteDirectMessageMessageRequest,
    ) -> Result<()> {
        self.app_service
            .delete_direct_message_message(request.pubkey.as_str(), request.message_id.as_str())
            .await
    }

    pub async fn clear_direct_message(&self, request: DirectMessageRequest) -> Result<()> {
        self.app_service
            .clear_direct_message(request.pubkey.as_str())
            .await
    }

    pub async fn get_direct_message_status(
        &self,
        request: DirectMessageRequest,
    ) -> Result<DirectMessageStatusView> {
        self.app_service
            .get_direct_message_status(request.pubkey.as_str())
            .await
    }
}
