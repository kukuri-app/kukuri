//! SDK のゲームの action session の参加の認可(ADR 0070 §5.2、#1978 AC-2)。
//!
//! 参加者は room の host へ、topic・channel と host に束縛した 10 秒の proof(`DomeSpatialAccessProofV1`)を送る。host は
//! 自分の room の状態と、公開範囲(公開 topic なら gossip、private channel なら今の epoch の参加者か)と block を自分の
//! 状態で確かめる。接続と session の記録は desktop-runtime が持つ。

use crate::service::*;
use kukuri_core::{DomeSpatialAccessProofV1, SpatialContextV1};
use serde::Deserialize;

/// 参加を断った・続けられない理由の code(参加の応答と `game_session_closed` に載る)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameSessionRejected(pub String);

impl std::fmt::Display for GameSessionRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "game session rejected: {}", self.0)
    }
}

impl std::error::Error for GameSessionRejected {}

pub const GAME_SESSION_ROOM_NOT_FOUND: &str = "room_not_found";
pub const GAME_SESSION_ROOM_ENDED: &str = "room_ended";
pub const GAME_SESSION_ACCESS_DENIED: &str = "access_denied";
pub const GAME_SESSION_ACCESS_REVOKED: &str = "access_revoked";
pub const GAME_SESSION_ROOM_FULL: &str = "room_full";

fn rejected(code: &str) -> anyhow::Error {
    anyhow::anyhow!(GameSessionRejected(code.to_string()))
}

/// host へ送る参加の要求。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GameSessionJoinRequestV1 {
    topic_id: TopicId,
    room_id: String,
    proof: DomeSpatialAccessProofV1,
}

/// 参加者の端末で作った、host への参加の要求。
pub struct GameSessionJoin {
    pub host_endpoint_id: String,
    pub request: Vec<u8>,
}

/// host が受け付けた参加。評価し直しに使う。
#[derive(Clone, Debug)]
pub struct AdmittedGameSession {
    pub topic_id: String,
    pub room_id: String,
    pub participant: Pubkey,
    pub max_players: u32,
    context: SpatialContextV1,
}

fn room_context(topic_id: &str, row: &GameRoomProjectionRow) -> SpatialContextV1 {
    match channel_id_for_view(row.channel_id.as_str()) {
        Some(channel_id) => SpatialContextV1::Channel {
            topic_id: TopicId::new(topic_id),
            channel_id: ChannelId::new(channel_id),
        },
        None => SpatialContextV1::Topic {
            topic_id: TopicId::new(topic_id),
        },
    }
}

impl AppService {
    /// 一覧に出ている action session の room の host への、参加の要求を作る。
    pub async fn game_session_join_request(
        &self,
        topic_id: &str,
        scope: TimelineScope,
        room_id: &str,
    ) -> Result<GameSessionJoin> {
        // 参加していない private channel の room は、公開範囲の外として断る。
        let channel_id = self
            .allowed_channel_id_for_scope(topic_id, &scope)
            .await
            .map_err(|_| rejected(GAME_SESSION_ACCESS_DENIED))?;
        let row = self
            .services
            .projection_store
            .get_game_room(topic_id, room_id)
            .await?
            .filter(|row| row.channel_id == channel_id)
            .ok_or_else(|| rejected(GAME_SESSION_ROOM_NOT_FOUND))?;
        let action = row
            .action
            .clone()
            .ok_or_else(|| rejected(GAME_SESSION_ROOM_NOT_FOUND))?;
        if row.status == GameRoomStatus::Ended {
            return Err(rejected(GAME_SESSION_ROOM_ENDED));
        }
        let proof = self
            .build_dome_access_proof(room_context(topic_id, &row), Pubkey::from(row.host_pubkey))
            .await
            .map_err(|_| rejected(GAME_SESSION_ACCESS_DENIED))?;
        Ok(GameSessionJoin {
            host_endpoint_id: action.host_endpoint_id,
            request: serde_json::to_vec(&GameSessionJoinRequestV1 {
                topic_id: TopicId::new(topic_id),
                room_id: room_id.to_string(),
                proof,
            })?,
        })
    }

    /// host の端末で参加の要求を確かめる。room が自分の終わっていない action session で、相手が公開範囲を満たすとき
    /// だけ受け付ける(人数の上限は session の記録を持つ側が数える)。断るときは理由の code を返す。
    pub async fn authorize_game_session(
        &self,
        request: &[u8],
    ) -> std::result::Result<AdmittedGameSession, &'static str> {
        let request: GameSessionJoinRequestV1 =
            serde_json::from_slice(request).map_err(|_| GAME_SESSION_ACCESS_DENIED)?;
        let topic_id = request.topic_id.as_str();
        let owner = self.services.keys.public_key();
        let row = self
            .services
            .projection_store
            .get_game_room(topic_id, &request.room_id)
            .await
            .ok()
            .flatten()
            .filter(|row| row.host_pubkey == owner.as_str())
            .ok_or(GAME_SESSION_ROOM_NOT_FOUND)?;
        let max_players = row
            .action
            .as_ref()
            .map(|action| action.max_players)
            .ok_or(GAME_SESSION_ROOM_NOT_FOUND)?;
        if row.status == GameRoomStatus::Ended {
            return Err(GAME_SESSION_ROOM_ENDED);
        }
        let context = room_context(topic_id, &row);
        let participant = self
            .authorize_media_listener(&request.proof, &context, &owner)
            .await
            .ok()
            .flatten()
            .ok_or(GAME_SESSION_ACCESS_DENIED)?;
        Ok(AdmittedGameSession {
            topic_id: topic_id.to_string(),
            room_id: request.room_id,
            participant,
            max_players,
            context,
        })
    }

    /// 受け付けた参加を host が評価し直す。続けられないときは閉じる理由の code を返す。
    pub async fn game_session_close_reason(
        &self,
        session: &AdmittedGameSession,
    ) -> Option<&'static str> {
        let room = self
            .services
            .projection_store
            .get_game_room(&session.topic_id, &session.room_id)
            .await
            .ok()
            .flatten();
        if room.is_none_or(|room| room.status == GameRoomStatus::Ended) {
            return Some(GAME_SESSION_ROOM_ENDED);
        }
        let owner = self.services.keys.public_key();
        match self
            .media_listener_allowed(&session.context, &owner, &session.participant)
            .await
        {
            Ok(true) => None,
            _ => Some(GAME_SESSION_ACCESS_REVOKED),
        }
    }
}
