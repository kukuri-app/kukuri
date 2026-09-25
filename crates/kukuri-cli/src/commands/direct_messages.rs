use std::sync::Arc;

use async_trait::async_trait;
use kukuri_desktop_runtime::{
    DeleteDirectMessageMessageRequest, DirectMessageRequest, ListDirectMessageMessagesRequest,
    SendDirectMessageRequest,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{command, command_error, decode, encode, host_guards, media, runtime, schema};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};

#[derive(Clone, Copy)]
enum Operation {
    Open,
    List,
    Messages,
    Send,
    Delete,
    Clear,
    Status,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendRequest {
    pubkey: String,
    text: Option<String>,
    reply_to_message_id: Option<String>,
    #[serde(default)]
    attachments: Vec<media::FileAttachment>,
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
        // 実際の相互フォロー・block・送信者の判定は既存runtime/serviceに任せる。
        match self.0 {
            Operation::Open => encode(
                runtime
                    .open_direct_message(decode::<DirectMessageRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::List => encode(
                runtime
                    .list_direct_messages()
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Messages => encode(
                runtime
                    .list_direct_message_messages(decode::<ListDirectMessageMessagesRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Send => {
                let request: SendRequest = decode(payload)?;
                encode(
                    runtime
                        .send_direct_message(SendDirectMessageRequest {
                            pubkey: request.pubkey,
                            text: request.text,
                            reply_to_message_id: request.reply_to_message_id,
                            attachments: media::load_attachments(request.attachments).await?,
                        })
                        .await
                        .map_err(command_error)?,
                )
            }
            Operation::Delete => encode(
                runtime
                    .delete_direct_message_message(decode::<DeleteDirectMessageMessageRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Clear => encode(
                runtime
                    .clear_direct_message(decode::<DirectMessageRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Status => encode(
                runtime
                    .get_direct_message_status(decode::<DirectMessageRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
        }
    }
}

pub(super) fn registrations() -> Vec<CommandRegistration> {
    use CommandEffect::{Destructive, Read, Write};
    use Operation::*;
    let peer = || schema::object(json!({"pubkey": {"type": "string"}}), &["pubkey"]);
    [
        ("open_direct_message", Write, Open, peer(), conversation_schema()),
        ("list_direct_messages", Read, List, schema::object(json!({}), &[]), schema::array(conversation_schema())),
        ("list_direct_message_messages", Read, Messages, schema::object(json!({
            "pubkey": {"type": "string"}, "cursor": schema::cursor(),
            "limit": {"type": "integer", "minimum": 0}
        }), &["pubkey"]), schema::object(json!({
            "items": schema::array(message_schema()), "next_cursor": schema::nullable(schema::cursor())
        }), &["items", "next_cursor"])),
        ("send_direct_message", Write, Send, schema::object(json!({
            "pubkey": {"type": "string"}, "text": {"type": "string"},
            "reply_to_message_id": {"type": "string"}, "attachments": schema::array(media::input_schema())
        }), &["pubkey"]), json!({"type": "string", "description": "message_id"})),
        ("delete_direct_message_message", Destructive, Delete, schema::object(json!({
            "pubkey": {"type": "string"}, "message_id": {"type": "string"}
        }), &["pubkey", "message_id"]), json!({"type": "null"})),
        ("clear_direct_message", Destructive, Clear, peer(), json!({"type": "null"})),
        ("get_direct_message_status", Read, Status, peer(), status_schema()),
    ].into_iter().map(|(name, effect, operation, input, output)| {
        command(name, effect, false, false, host_guards(), (input, output), Arc::new(Handler(operation)))
    }).collect()
}

fn status_schema() -> Value {
    schema::object(
        json!({"peer_pubkey": {"type": "string"}, "dm_id": {"type": "string"},
        "mutual": {"type": "boolean"}, "send_enabled": {"type": "boolean"},
        "pending_outbox_count": {"type": "integer", "minimum": 0},
        "pending_outbox_has_more": {"type": "boolean"}}),
        &[
            "peer_pubkey",
            "dm_id",
            "mutual",
            "send_enabled",
            "pending_outbox_count",
            "pending_outbox_has_more",
        ],
    )
}

fn conversation_schema() -> Value {
    schema::object(
        json!({"dm_id": {"type": "string"}, "peer_pubkey": {"type": "string"},
        "peer_name": schema::nullable(json!({"type": "string"})),
        "peer_display_name": schema::nullable(json!({"type": "string"})),
        "peer_picture_asset": schema::nullable(schema::profile_asset()),
        "updated_at": {"type": "integer"}, "last_message_at": schema::nullable(json!({"type": "integer"})),
        "last_message_id": schema::nullable(json!({"type": "string"})),
        "last_message_preview": schema::nullable(json!({"type": "string"})), "status": status_schema()}),
        &[
            "dm_id",
            "peer_pubkey",
            "peer_name",
            "peer_display_name",
            "peer_picture_asset",
            "updated_at",
            "last_message_at",
            "last_message_id",
            "last_message_preview",
            "status",
        ],
    )
}

fn message_schema() -> Value {
    schema::object(
        json!({"dm_id": {"type": "string"}, "message_id": {"type": "string"},
        "sender_pubkey": {"type": "string"}, "recipient_pubkey": {"type": "string"},
        "created_at": {"type": "integer"}, "text": {"type": "string"},
        "reply_to_message_id": schema::nullable(json!({"type": "string"})),
        "attachments": schema::array(schema::attachment()),
        "outgoing": {"type": "boolean"}, "delivered": {"type": "boolean"}}),
        &[
            "dm_id",
            "message_id",
            "sender_pubkey",
            "recipient_pubkey",
            "created_at",
            "text",
            "reply_to_message_id",
            "attachments",
            "outgoing",
            "delivered",
        ],
    )
}
