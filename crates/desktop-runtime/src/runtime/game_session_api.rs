use super::*;

// SDK のゲームの action session への参加・退出(#1978)。CLI と SDK だけが使う。
impl DesktopRuntime {
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
}
