use std::sync::Arc;

use async_trait::async_trait;
use kukuri_desktop_runtime::{
    CreateGameRoomRequest, CreateLiveSessionRequest, FetchGameAssetRequest, JoinGameSessionRequest,
    LeaveGameSessionRequest, ListActionRoomsRequest, ListGameAssetsRequest, ListGameRoomsRequest,
    ListLiveSessionsRequest, LiveMediaRequest, LiveSessionCommandRequest, LiveVolumeRequest,
    PublishGameAssetRequest, SetLiveBroadcastRequest, UpdateGameRoomRequest,
};
use serde_json::{Value, json};

use super::{command, command_error, decode, encode, host_guards, runtime, schema};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};

#[derive(Clone, Copy)]
enum Operation {
    ListSessionCandidates,
    SetSessionDisplay,
    ListLive,
    GetLive,
    CreateLive,
    EndLive,
    JoinLive,
    LeaveLive,
    ListGame,
    ListAction,
    JoinSession,
    LeaveSession,
    PublishAsset,
    ListAssets,
    FetchAsset,
    CreateGame,
    UpdateGame,
    SetBroadcast,
    StopBroadcast,
    WatchMedia,
    StopWatching,
    SetVolume,
}
struct Handler(Operation);

#[async_trait]
impl CommandHandler for Handler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let runtime = runtime(&context)?;
        match self.0 {
            Operation::ListSessionCandidates => encode(
                runtime
                    .list_session_candidates(decode::<ListLiveSessionsRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::SetSessionDisplay => encode(
                runtime
                    .set_session_display(decode::<kukuri_desktop_runtime::SessionDisplayRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::ListLive => encode(
                runtime
                    .list_live_sessions(decode::<ListLiveSessionsRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::GetLive => encode(
                runtime
                    .get_live_session(decode::<LiveSessionCommandRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::CreateLive => encode(
                runtime
                    .create_live_session(decode::<CreateLiveSessionRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::EndLive => encode(
                runtime
                    .end_live_session(decode::<LiveSessionCommandRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::JoinLive => encode(
                runtime
                    .join_live_session(decode::<LiveSessionCommandRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::LeaveLive => encode(
                runtime
                    .leave_live_session(decode::<LiveSessionCommandRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::ListGame => encode(
                runtime
                    .list_game_rooms(decode::<ListGameRoomsRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::ListAction => encode(
                runtime
                    .list_action_rooms(decode::<ListActionRoomsRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::JoinSession => encode(
                runtime
                    .join_game_session(decode::<JoinGameSessionRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::LeaveSession => encode(
                runtime
                    .leave_game_session(decode::<LeaveGameSessionRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::PublishAsset => encode(
                runtime
                    .publish_game_asset(decode::<PublishGameAssetRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::ListAssets => encode(
                runtime
                    .list_game_assets(decode::<ListGameAssetsRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::FetchAsset => encode(
                runtime
                    .fetch_game_asset(decode::<FetchGameAssetRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::CreateGame => encode(
                runtime
                    .create_game_room(decode::<CreateGameRoomRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::UpdateGame => encode(
                runtime
                    .update_game_room(decode::<UpdateGameRoomRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::SetBroadcast => encode(
                runtime
                    .set_live_broadcast(decode::<SetLiveBroadcastRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::StopBroadcast => encode(
                runtime
                    .stop_live_broadcast(decode::<LiveMediaRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::WatchMedia => encode(
                runtime
                    .watch_live_media(decode::<LiveMediaRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::StopWatching => encode(
                runtime
                    .stop_watching_live_media(decode::<LiveMediaRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::SetVolume => encode(
                runtime
                    .set_live_volume(decode::<LiveVolumeRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
        }
    }
}

pub(super) fn registrations() -> Vec<CommandRegistration> {
    use CommandEffect::{Read, Write};
    use Operation::*;
    let session = || {
        schema::object(
            json!({"topic": {"type": "string"}, "session_id": {"type": "string"}}),
            &["topic", "session_id"],
        )
    };
    [
        (
            "list_session_candidates", Read, ListSessionCandidates,
            schema::object(json!({"topic":{"type":"string"}, "scope":schema::timeline_scope()}), &["topic"]),
            schema::array(schema::object(json!({"replica_id":{"type":"string"}, "session_id":{"type":"string"}, "kind":{"type":"string"}}), &["replica_id","session_id","kind"])),
        ),
        (
            "set_session_display", Write, SetSessionDisplay,
            schema::object(json!({"topic":{"type":"string"}, "scope":schema::timeline_scope(), "replica_id":{"type":"string"},
                "session_id":{"type":"string"}, "kind":{"enum":["live","game"]}, "observer":{"type":"string"},
                "visible":{"type":"boolean"}, "retry":{"type":"boolean"}}),
                &["topic","scope","replica_id","session_id","kind","observer","visible"]),
            json!({"type":"null"}),
        ),
        (
            "list_game_rooms",
            Read,
            ListGame,
            schema::object(
                json!({"topic": {"type": "string"}, "scope": schema::timeline_scope()}),
                &["topic"],
            ),
            schema::array(super::game_views::game_room()),
        ),
        // #1978: SDK の action session の room を、application ID ごとに更新の新しい順に 1 頁ずつ読む。
        (
            "list_action_rooms",
            Read,
            ListAction,
            schema::object(
                json!({"topic": {"type": "string"}, "scope": schema::timeline_scope(),
                "application_id": {"type": "string"}, "cursor": schema::nullable(json!({"type": "string"}))}),
                &["topic", "application_id"],
            ),
            schema::object(
                json!({"items": schema::array(super::game_views::game_room()),
                "next_cursor": schema::nullable(json!({"type": "string"}))}),
                &["items", "next_cursor"],
            ),
        ),
        // #1978: action session の room の host へ参加・退出する。断られたら details.reason に理由の code。
        (
            "join_game_session",
            Write,
            JoinSession,
            schema::object(
                json!({"topic": {"type": "string"}, "scope": schema::timeline_scope(),
                "room_id": {"type": "string"}}),
                &["topic", "room_id"],
            ),
            schema::object(json!({"session_id": {"type": "string"}}), &["session_id"]),
        ),
        (
            "leave_game_session",
            Write,
            LeaveSession,
            schema::object(json!({"session_id": {"type": "string"}}), &["session_id"]),
            json!({"type": "null"}),
        ),
        // #1980: ゲームのアセットの発行(CLI と開発用の Sidecar だけ)・一覧・取得。取得できなければ details.reason。
        (
            "publish_game_asset",
            Write,
            PublishAsset,
            schema::object(
                json!({"topic": {"type": "string"}, "application_id": {"type": "string"},
                "asset_id": {"type": "string"}, "format": super::game_views::asset_format(),
                "path": {"type": "string"}}),
                &["topic", "application_id", "asset_id", "format", "path"],
            ),
            super::game_views::asset_release(),
        ),
        (
            "list_game_assets",
            Read,
            ListAssets,
            schema::object(
                json!({"topic": {"type": "string"}, "publisher_pubkey": {"type": "string"},
                "application_id": {"type": "string"},
                "asset_ids": {"type": "array", "items": {"type": "string"}, "minItems": 1,
                    "maxItems": kukuri_desktop_runtime::GAME_ASSET_LIST_LIMIT}}),
                &["topic", "publisher_pubkey", "application_id", "asset_ids"],
            ),
            schema::array(super::game_views::asset_release()),
        ),
        (
            "fetch_game_asset",
            Write,
            FetchAsset,
            schema::object(
                json!({"topic": {"type": "string"}, "publisher_pubkey": {"type": "string"},
                "application_id": {"type": "string"}, "asset_id": {"type": "string"}}),
                &["topic", "publisher_pubkey", "application_id", "asset_id"],
            ),
            super::game_views::asset_file(),
        ),
        (
            "create_game_room",
            Write,
            CreateGame,
            schema::object(
                json!({"topic": {"type": "string"}, "channel_ref": schema::channel_ref(),
                "title": {"type": "string"}, "description": {"type": "string"},
                "participants": schema::array(json!({"type": "string"})),
                "action": schema::nullable(schema::object(
                    json!({"application_id": {"type": "string"}, "max_players": {"type": "integer", "minimum": 1}}),
                    &["application_id", "max_players"],
                ))}),
                &["topic", "title", "description", "participants"],
            ),
            json!({"type": "string", "description": "room_id"}),
        ),
        (
            "update_game_room",
            Write,
            UpdateGame,
            schema::object(
                json!({"topic": {"type": "string"}, "room_id": {"type": "string"},
                "status": super::game_views::status(), "phase_label": {"type": "string"},
                "scores": schema::array(super::game_views::score())}),
                &["topic", "room_id", "status", "scores"],
            ),
            json!({"type": "null"}),
        ),
        (
            "list_live_sessions",
            Read,
            ListLive,
            schema::object(
                json!({"topic": {"type": "string"}, "scope": schema::timeline_scope()}),
                &["topic"],
            ),
            schema::array(live_schema()),
        ),
        (
            "create_live_session",
            Write,
            CreateLive,
            schema::object(
                json!({"topic": {"type": "string"}, "channel_ref": schema::channel_ref(),
            "title": {"type": "string"}, "description": {"type": "string"},
            "start_post": {"type": "string"}}),
                &["topic", "title", "description"],
            ),
            json!({"type": "string", "description": "session_id"}),
        ),
        (
            "get_live_session",
            Read,
            GetLive,
            session(),
            schema::nullable(live_schema()),
        ),
        (
            "end_live_session",
            Write,
            EndLive,
            session(),
            json!({"type": "null"}),
        ),
        (
            "join_live_session",
            Write,
            JoinLive,
            session(),
            json!({"type": "null"}),
        ),
        (
            "leave_live_session",
            Write,
            LeaveLive,
            session(),
            json!({"type": "null"}),
        ),
        // #1982: live の配信・視聴（Game SDK が使う。映像と音声の bytes は SDK の send_binary と poll で渡す）。
        (
            "set_live_broadcast",
            Write,
            SetBroadcast,
            schema::object(
                json!({"topic": {"type": "string"}, "session_id": {"type": "string"},
                "source": {"enum": ["camera", "screen"]}, "microphone": {"type": "boolean"}}),
                &["topic", "session_id", "source", "microphone"],
            ),
            schema::object(
                json!({"microphone": {"type": "boolean"}, "limits": schema::object(
                    json!({"width": {"type": "integer"}, "height": {"type": "integer"},
                    "framerate": {"type": "integer"}, "bitrate": {"type": "integer"}}),
                    &["width", "height", "framerate", "bitrate"],
                )}),
                &["microphone", "limits"],
            ),
        ),
        (
            "stop_live_broadcast",
            Write,
            StopBroadcast,
            session(),
            json!({"type": "null"}),
        ),
        (
            "watch_live_media",
            Write,
            WatchMedia,
            session(),
            json!({"type": "null"}),
        ),
        (
            "stop_watching_live_media",
            Write,
            StopWatching,
            session(),
            json!({"type": "null"}),
        ),
        (
            "set_live_volume",
            Write,
            SetVolume,
            schema::object(
                json!({"session_id": {"type": "string"},
                "volume": {"type": "number", "minimum": 0, "maximum": 1}}),
                &["session_id", "volume"],
            ),
            json!({"type": "null"}),
        ),
    ]
    .into_iter()
    .map(|(name, effect, operation, input, output)| {
        command(
            name,
            effect,
            false,
            false,
            host_guards(),
            (input, output),
            Arc::new(Handler(operation)),
        )
    })
    .collect()
}

fn live_schema() -> Value {
    schema::object(
        json!({"session_id": {"type": "string"}, "host_pubkey": {"type": "string"},
        "title": {"type": "string"}, "description": {"type": "string"},
        "status": {"enum": ["Scheduled", "Live", "Paused", "Ended"]},
        "started_at": {"type": "integer"}, "ended_at": schema::nullable(json!({"type": "integer"})),
        "viewer_count": {"type": "integer", "minimum": 0}, "joined_by_me": {"type": "boolean"},
        "channel_id": schema::nullable(json!({"type": "string"})), "audience_label": {"type": "string"},
        "start_post_id": schema::nullable(json!({"type": "string"}))}),
        &[
            "session_id",
            "host_pubkey",
            "title",
            "description",
            "status",
            "started_at",
            "ended_at",
            "viewer_count",
            "joined_by_me",
            "channel_id",
            "audience_label",
            "start_post_id",
        ],
    )
}
