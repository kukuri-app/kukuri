//! SDK のゲームの action session の room の一覧(ADR 0070 §5.2、#1978)。作成・更新は `game.rs` の Game Room と同じ経路。

use crate::service::*;

/// action session の一覧の 1 頁の件数(#1978 の判断)。
pub(crate) const ACTION_ROOM_PAGE_SIZE: usize = 20;

impl AppService {
    /// 1 つの application ID の action session の room を、更新の新しい順に 1 頁読む(#1978)。`cursor` は前の頁の
    /// `next_cursor`。非表示の作者の room は除くが、続きの位置は読んだ最後の行から決めるので、除いた分だけ先が
    /// 読めなくなることはない。
    pub async fn list_action_rooms(
        &self,
        topic_id: &str,
        scope: TimelineScope,
        application_id: &str,
        cursor: Option<&str>,
    ) -> Result<GameRoomPage> {
        let before = cursor
            .map(|cursor| {
                cursor
                    .split_once(':')
                    .and_then(|(updated_at, room_id)| {
                        Some((updated_at.parse::<i64>().ok()?, room_id.to_string()))
                    })
                    .ok_or_else(|| anyhow::anyhow!("invalid action room cursor"))
            })
            .transpose()?;
        let hidden_author_pubkeys = self.current_hidden_author_pubkeys().await?;
        let channel_id = self.allowed_channel_id_for_scope(topic_id, &scope).await?;
        let projection_store = Arc::clone(&self.services.projection_store);
        let first_page = before.is_none();
        let rows = load_projection_rows_with_one_refresh(
            || {
                let projection_store = Arc::clone(&projection_store);
                let before = before.clone();
                let channel_id = channel_id.clone();
                async move {
                    projection_store
                        .list_channel_action_rooms(
                            topic_id,
                            channel_id.as_str(),
                            application_id,
                            before,
                            ACTION_ROOM_PAGE_SIZE,
                        )
                        .await
                }
            },
            |rows| first_page && rows.is_empty(),
            || async {
                self.catch_up_scope_sessions(topic_id, &scope).await?;
                Ok(())
            },
        )
        .await?;
        let next_cursor = (rows.len() == ACTION_ROOM_PAGE_SIZE)
            .then(|| {
                rows.last()
                    .map(|row| format!("{}:{}", row.updated_at, row.room_id))
            })
            .flatten();
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            if !hidden_author_pubkeys.contains(row.host_pubkey.as_str()) {
                items.push(self.game_room_view(topic_id, row, None).await);
            }
        }
        Ok(GameRoomPage { items, next_cursor })
    }
}
