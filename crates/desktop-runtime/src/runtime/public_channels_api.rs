use super::*;

// 公開 channel(公開スレッド。#1840)の command。
impl DesktopRuntime {
    pub async fn create_public_channel(
        &self,
        request: CreatePublicChannelRequest,
    ) -> Result<kukuri_app_api::PublicChannelView> {
        let attachments = pending_attachments_from_requests(request.attachments).await?;
        self.app_service
            .create_public_channel(
                &request.topic,
                &request.title,
                &request.first_post,
                attachments,
            )
            .await
    }

    pub async fn get_public_channel(
        &self,
        request: GetPublicChannelRequest,
    ) -> Result<Option<kukuri_app_api::PublicChannelView>> {
        self.app_service
            .get_public_channel(&request.topic, &request.channel_id)
            .await
    }

    pub async fn list_public_channels(
        &self,
        request: ListPublicChannelsRequest,
    ) -> Result<kukuri_app_api::PublicChannelListView> {
        self.app_service
            .list_public_channels(
                &request.topic,
                request.order,
                request.cursor,
                request.limit.unwrap_or(20),
            )
            .await
    }
}
