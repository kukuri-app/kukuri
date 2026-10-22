use super::*;

// 公開 topic の保持(#1909)。CLI daemon だけが使う。
impl DesktopRuntime {
    pub async fn set_topic_retention(&self, request: SetTopicRetentionRequest) -> Result<()> {
        self.app_service
            .set_topic_retention(&request.topic, request.retained)
            .await
    }

    pub async fn get_topic_retention(&self) -> Result<kukuri_app_api::TopicRetentionView> {
        self.app_service.topic_retention().await
    }
}
