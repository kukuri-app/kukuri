use super::*;

// SDK のゲームの action session への参加・退出(#1978)と packet の送受信(#1979)、アセットの配布(#1980)。CLI と SDK
// だけが使う。
impl DesktopRuntime {
    pub async fn publish_game_asset(
        &self,
        request: crate::PublishGameAssetRequest,
    ) -> Result<kukuri_app_api::GameAssetReleaseView> {
        anyhow::ensure!(request.path.is_absolute(), "path must be absolute");
        self.app_service
            .publish_game_asset(
                request.topic.as_str(),
                kukuri_app_api::PublishGameAssetInput {
                    application_id: request.application_id,
                    asset_id: request.asset_id,
                    format: request.format,
                    path: request.path,
                },
            )
            .await
    }

    pub async fn list_game_assets(
        &self,
        request: crate::ListGameAssetsRequest,
    ) -> Result<Vec<kukuri_app_api::GameAssetReleaseView>> {
        self.app_service
            .list_game_assets(
                request.topic.as_str(),
                request.publisher_pubkey.as_str(),
                request.application_id.as_str(),
                &request.asset_ids,
            )
            .await
    }

    pub async fn fetch_game_asset(
        &self,
        request: crate::FetchGameAssetRequest,
    ) -> Result<kukuri_app_api::GameAssetFileView> {
        self.app_service
            .fetch_game_asset(
                request.topic.as_str(),
                request.publisher_pubkey.as_str(),
                request.application_id.as_str(),
                request.asset_id.as_str(),
            )
            .await
    }

    pub async fn join_game_session(
        &self,
        request: crate::JoinGameSessionRequest,
    ) -> Result<crate::JoinedGameSession> {
        let node = self
            .iroh_stack
            .current_node()
            .await
            .context("the network is not running")?;
        let session_id = self
            .game_sessions
            .join(
                &self.app_service,
                &node,
                request.topic.as_str(),
                request.scope,
                request.room_id.as_str(),
            )
            .await?;
        Ok(crate::JoinedGameSession { session_id })
    }

    pub async fn leave_game_session(&self, request: crate::LeaveGameSessionRequest) -> Result<()> {
        self.game_sessions.leave(request.session_id.as_str());
        Ok(())
    }

    /// 開いている session に packet を送り待ちに置いて即座に戻る(network を待たない)。
    pub fn send_game_packet(
        &self,
        session_id: &str,
        delivery: crate::GameDelivery,
        bytes: &[u8],
    ) -> std::result::Result<(), crate::GameSendRefused> {
        self.game_sessions.send(session_id, delivery, bytes)
    }

    /// 受け取った packet を、`fits` が大きさを許す間だけ取り出す。`accept` が false なら datagram を捨て、stream の
    /// 読み取りを止めたままにする(呼出側の未解放の bytes が上限に達したとき。ADR 0070 §3)。
    pub fn take_game_packets(
        &self,
        accept: bool,
        fits: impl FnMut(usize) -> bool,
    ) -> Vec<(String, crate::GamePacket)> {
        self.game_sessions.take(accept, fits)
    }
}
