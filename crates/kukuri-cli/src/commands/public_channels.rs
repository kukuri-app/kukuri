use std::sync::Arc;

use async_trait::async_trait;
use kukuri_desktop_runtime::{
    CreatePublicChannelRequest, GetPublicChannelRequest, ListPublicChannelsRequest,
    UpdatePublicChannelTitleRequest,
};
use serde_json::{Value, json};

use super::{
    command, command_error, content_views, decode, encode, host_guards, media, runtime, schema,
};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};

// 公開 channel(公開スレッド。#1840)の作成と、ID からの取得と、topic の一覧。作成者のタイトルの変更と削除(#1888)。
#[derive(Clone, Copy)]
enum Operation {
    Create,
    Get,
    List,
    UpdateTitle,
    Delete,
}

struct Handler(Operation);

#[async_trait]
impl CommandHandler for Handler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        payload: Value,
        _: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let runtime = runtime(&context)?;
        match self.0 {
            Operation::Create => encode(
                runtime
                    .create_public_channel(
                        media::decode_with_files::<CreatePublicChannelRequest>(
                            payload,
                            "attachments",
                            true,
                        )
                        .await?,
                    )
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Get => encode(
                runtime
                    .get_public_channel(decode::<GetPublicChannelRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::List => encode(
                runtime
                    .list_public_channels(decode::<ListPublicChannelsRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::UpdateTitle => encode(
                runtime
                    .update_public_channel_title(decode::<UpdatePublicChannelTitleRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Delete => encode(
                runtime
                    .delete_public_channel(decode::<GetPublicChannelRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
        }
    }
}

pub(super) fn registrations() -> Vec<CommandRegistration> {
    [
        (
            "create_public_channel",
            CommandEffect::Write,
            Operation::Create,
            schema::object(
                json!({"topic": {"type": "string"}, "title": {"type": "string"},
                "first_post": {"type": "string", "default": ""},
                "attachments": schema::array(media::input_schema()),
                "content_labels": schema::array(json!({"type": "string"}))}),
                &["topic", "title"],
            ),
            public_channel_schema(),
        ),
        (
            "get_public_channel",
            CommandEffect::Read,
            Operation::Get,
            schema::object(
                json!({"topic": {"type": "string"}, "channel_id": {"type": "string"}}),
                &["topic", "channel_id"],
            ),
            schema::nullable(public_channel_schema()),
        ),
        (
            "update_public_channel_title",
            CommandEffect::Write,
            Operation::UpdateTitle,
            schema::object(
                json!({"topic": {"type": "string"}, "channel_id": {"type": "string"},
                "title": {"type": "string"}}),
                &["topic", "channel_id", "title"],
            ),
            public_channel_schema(),
        ),
        (
            "delete_public_channel",
            CommandEffect::Write,
            Operation::Delete,
            schema::object(
                json!({"topic": {"type": "string"}, "channel_id": {"type": "string"}}),
                &["topic", "channel_id"],
            ),
            public_channel_schema(),
        ),
        (
            "list_public_channels",
            CommandEffect::Read,
            Operation::List,
            schema::object(
                json!({"topic": {"type": "string"},
                "order": {"enum": ["last_post", "created"], "default": "last_post"},
                "cursor": cursor_schema(), "limit": {"type": "integer", "minimum": 0}}),
                &["topic"],
            ),
            schema::object(
                json!({"items": schema::array(public_channel_schema()),
                "next_cursor": schema::nullable(cursor_schema()),
                "unavailable_count": {"type": "integer", "minimum": 0}}),
                &["items", "next_cursor", "unavailable_count"],
            ),
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

/// 共有 DTO の `PublicChannelView`。値の無い `Option` の欄と false の `deleted` は出力に入らない。
fn public_channel_schema() -> Value {
    schema::object(
        json!({"channel_id": {"type": "string"}, "topic_id": {"type": "string"},
        "title": {"type": "string"}, "creator_pubkey": {"type": "string"},
        "created_at": {"type": "integer"}, "first_post_id": {"type": "string"},
        "first_post": content_views::post(), "last_post_at": {"type": "integer"},
        "creator_name": {"type": "string"}, "creator_display_name": {"type": "string"},
        "live_now": {"type": "boolean"}, "deleted": {"type": "boolean"}}),
        &[
            "channel_id",
            "topic_id",
            "title",
            "creator_pubkey",
            "created_at",
            "last_post_at",
            "live_now",
        ],
    )
}

fn cursor_schema() -> Value {
    schema::object(
        json!({"at": {"type": "integer"}, "channel_id": {"type": "string"}}),
        &["at", "channel_id"],
    )
}
