use crate::service::*;

impl AppService {
    /// Community Node への indexing 申請にだけ使う current epoch secret を返す。
    ///
    /// 呼び出し側は secret を UI / IPC / log へ露出させず、ユーザーが送信先 node と開示の意味を
    /// 明示確認した直後の HTTP request body にだけ載せること。
    pub async fn private_channel_indexing_secret(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<String> {
        Ok(self
            .private_channel_indexing_capability(topic_id, channel_id)
            .await?
            .1)
    }

    pub async fn private_channel_indexing_capability(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<(String, String)> {
        self.maybe_redeem_epoch_handoff_grants_for_channel(topic_id, channel_id)
            .await?;
        let state = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("private channel is not joined"))?;
        if state.current_epoch_secret_hex.trim().is_empty() {
            anyhow::bail!("private channel current epoch secret is unavailable");
        }
        Ok((state.current_epoch_id, state.current_epoch_secret_hex))
    }
}
