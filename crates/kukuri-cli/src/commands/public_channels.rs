use std::sync::Arc;

use async_trait::async_trait;
use kukuri_desktop_runtime::{CreatePublicChannelRequest, GetPublicChannelRequest};
use serde_json::{Value, json};

use super::{command, command_error, decode, encode, host_guards, runtime, schema};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};

// 公開 channel(公開スレッド。#1840)の作成と、ID からの取得。
#[derive(Clone, Copy)]
enum Operation {
    Create,
    Get,
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
                    .create_public_channel(decode::<CreatePublicChannelRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Get => encode(
                runtime
                    .get_public_channel(decode::<GetPublicChannelRequest>(payload)?)
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
                "first_post": {"type": "string", "default": ""}}),
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

fn public_channel_schema() -> Value {
    schema::object(
        json!({"channel_id": {"type": "string"}, "topic_id": {"type": "string"},
        "title": {"type": "string"}, "creator_pubkey": {"type": "string"},
        "created_at": {"type": "integer"}, "first_post_id": {"type": "string"}}),
        &[
            "channel_id",
            "topic_id",
            "title",
            "creator_pubkey",
            "created_at",
        ],
    )
}
