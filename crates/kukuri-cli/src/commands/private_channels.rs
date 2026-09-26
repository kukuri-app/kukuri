use std::sync::Arc;

use async_trait::async_trait;
use kukuri_desktop_runtime::{
    CreatePrivateChannelRequest, ExportChannelAccessTokenRequest, ExportFriendOnlyGrantRequest,
    ExportFriendPlusShareRequest, ExportPrivateChannelInviteRequest, FreezePrivateChannelRequest,
    ImportChannelAccessTokenRequest, ImportFriendOnlyGrantRequest, ImportFriendPlusShareRequest,
    ImportPrivateChannelInviteRequest, LeavePrivateChannelRequest,
    ListJoinedPrivateChannelsRequest, PreviewChannelAccessTokenRequest,
    RotatePrivateChannelRequest, SetPrivateChannelEntryDomeRequest,
};
use serde_json::{Value, json};

use super::{command, command_error, decode, encode, host_guards, runtime, schema};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput, SecretOutput, error_code},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};

#[derive(Clone, Copy)]
enum Operation {
    Create,
    ExportInvite,
    ImportInvite,
    ExportAccess,
    PreviewAccess,
    ImportAccess,
    ExportGrant,
    ImportGrant,
    ExportShare,
    ImportShare,
    Freeze,
    Rotate,
    EntryDome,
    Leave,
    List,
}

struct Handler(Operation);

fn token_input(secret: Option<&SecretInput>) -> Result<String, ProtocolError> {
    let secret = secret
        .ok_or_else(|| ProtocolError::new(error_code::INVALID_REQUEST, "secret frameが必要です"))?;
    std::str::from_utf8(secret.expose())
        .map(str::to_owned)
        .map_err(|_| {
            ProtocolError::new(
                error_code::VALIDATION_FAILED,
                "tokenはUTF-8で指定してください",
            )
        })
}

fn token_output(token: String, kind: Value) -> CommandOutput {
    CommandOutput::Secret {
        data: json!({"kind": kind}),
        secret: SecretOutput::new(token.into_bytes()),
    }
}

#[async_trait]
impl CommandHandler for Handler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        payload: Value,
        secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let runtime = runtime(&context)?;
        match self.0 {
            Operation::Create => encode(
                runtime
                    .create_private_channel(decode::<CreatePrivateChannelRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::ExportInvite => Ok(token_output(
                runtime
                    .export_private_channel_invite(decode::<ExportPrivateChannelInviteRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
                json!("invite"),
            )),
            Operation::ExportGrant => Ok(token_output(
                runtime
                    .export_friend_only_grant(decode::<ExportFriendOnlyGrantRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
                json!("grant"),
            )),
            Operation::ExportShare => Ok(token_output(
                runtime
                    .export_friend_plus_share(decode::<ExportFriendPlusShareRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
                json!("share"),
            )),
            Operation::ExportAccess => {
                let result = runtime
                    .export_channel_access_token(decode::<ExportChannelAccessTokenRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?;
                Ok(token_output(result.token, json!(result.kind)))
            }
            Operation::ImportInvite => {
                let result = runtime
                    .import_private_channel_invite(ImportPrivateChannelInviteRequest {
                        token: token_input(secret)?,
                    })
                    .await
                    .map_err(command_error)?;
                // Preview全体をserializeしない。namespace secretは通常JSONへ渡さない。
                encode(
                    json!({"kind": "invite", "topic_id": result.topic_id, "channel_id": result.channel_id,
                    "channel_label": result.channel_label, "owner_pubkey": result.owner_pubkey,
                    "inviter_pubkey": result.inviter_pubkey, "sponsor_pubkey": null, "epoch_id": result.epoch_id}),
                )
            }
            Operation::ImportGrant => {
                let result = runtime
                    .import_friend_only_grant(ImportFriendOnlyGrantRequest {
                        token: token_input(secret)?,
                    })
                    .await
                    .map_err(command_error)?;
                encode(
                    json!({"kind": "grant", "topic_id": result.topic_id, "channel_id": result.channel_id,
                    "channel_label": result.channel_label, "owner_pubkey": result.owner_pubkey,
                    "inviter_pubkey": null, "sponsor_pubkey": null, "epoch_id": result.epoch_id}),
                )
            }
            Operation::ImportShare => {
                let result = runtime
                    .import_friend_plus_share(ImportFriendPlusShareRequest {
                        token: token_input(secret)?,
                    })
                    .await
                    .map_err(command_error)?;
                encode(
                    json!({"kind": "share", "topic_id": result.topic_id, "channel_id": result.channel_id,
                    "channel_label": result.channel_label, "owner_pubkey": result.owner_pubkey,
                    "inviter_pubkey": null, "sponsor_pubkey": result.sponsor_pubkey, "epoch_id": result.epoch_id}),
                )
            }
            Operation::PreviewAccess => encode(
                runtime
                    .preview_channel_access_token(PreviewChannelAccessTokenRequest {
                        token: token_input(secret)?,
                    })
                    .await
                    .map_err(command_error)?,
            ),
            Operation::ImportAccess => encode(
                runtime
                    .import_channel_access_token(ImportChannelAccessTokenRequest {
                        token: token_input(secret)?,
                    })
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Freeze => encode(
                runtime
                    .freeze_private_channel(decode::<FreezePrivateChannelRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Rotate => encode(
                runtime
                    .rotate_private_channel(decode::<RotatePrivateChannelRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::EntryDome => encode(
                runtime
                    .set_private_channel_entry_dome(decode::<SetPrivateChannelEntryDomeRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::Leave => encode(
                runtime
                    .leave_private_channel(decode::<LeavePrivateChannelRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            Operation::List => encode(
                runtime
                    .list_joined_private_channels(decode::<ListJoinedPrivateChannelsRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
            ),
        }
    }
}

pub(super) fn registrations() -> Vec<CommandRegistration> {
    use CommandEffect::{Destructive, Read, Write};
    use Operation::*;
    let channel = || {
        schema::object(
            json!({"topic": {"type": "string"}, "channel_id": {"type": "string"}}),
            &["topic", "channel_id"],
        )
    };
    let export = || {
        schema::object(
            json!({"topic": {"type": "string"}, "channel_id": {"type": "string"}, "expires_at": {"type": "integer"}}),
            &["topic", "channel_id"],
        )
    };
    let token = || {
        let mut input = schema::object(json!({}), &[]);
        input["description"] =
            json!("tokenをUTF-8のsecret frameに指定する。通常payloadには含めない。");
        input
    };
    let exported = || {
        schema::object(
            json!({"kind": {"enum": ["invite", "grant", "share"]}}),
            &["kind"],
        )
    };
    [
        ("create_private_channel", Write, Create, false, false, schema::object(json!({
            "topic": {"type": "string"}, "label": {"type": "string"},
            "audience_kind": {"enum": ["invite_only", "friend_only", "friend_plus"], "default": "invite_only"}
        }), &["topic", "label"]), joined_schema()),
        ("export_private_channel_invite", Write, ExportInvite, false, true, export(), exported()),
        ("import_private_channel_invite", Write, ImportInvite, true, false, token(), preview_schema()),
        ("export_channel_access_token", Write, ExportAccess, false, true, export(), exported()),
        ("preview_channel_access_token", Read, PreviewAccess, true, false, token(), preview_schema()),
        ("import_channel_access_token", Write, ImportAccess, true, false, token(), preview_schema()),
        ("export_friend_only_grant", Write, ExportGrant, false, true, export(), exported()),
        ("import_friend_only_grant", Write, ImportGrant, true, false, token(), preview_schema()),
        ("export_friend_plus_share", Write, ExportShare, false, true, export(), exported()),
        ("import_friend_plus_share", Write, ImportShare, true, false, token(), preview_schema()),
        ("freeze_private_channel", Write, Freeze, false, false, channel(), joined_schema()),
        ("rotate_private_channel", Write, Rotate, false, false, channel(), joined_schema()),
        ("set_private_channel_entry_dome", Write, EntryDome, false, false, schema::object(json!({
            "topic": {"type": "string"}, "channel_id": {"type": "string"}, "entry_dome_instance_id": {"type": "string"}
        }), &["topic", "channel_id"]), joined_schema()),
        ("leave_private_channel", Destructive, Leave, false, false, channel(), json!({"type": "null"})),
        ("list_joined_private_channels", Read, List, false, false, schema::object(json!({"topic": {"type": "string"}}), &["topic"]), schema::array(joined_schema())),
    ].into_iter().map(|(name, effect, operation, secret_input, secret_output, input, output)| {
        command(name, effect, secret_input, secret_output, host_guards(), (input, output), Arc::new(Handler(operation)))
    }).collect()
}

fn preview_schema() -> Value {
    schema::object(
        json!({"kind": {"enum": ["invite", "grant", "share"]},
        "topic_id": {"type": "string"}, "channel_id": {"type": "string"}, "channel_label": {"type": "string"},
        "owner_pubkey": {"type": "string"}, "inviter_pubkey": schema::nullable(json!({"type": "string"})),
        "sponsor_pubkey": schema::nullable(json!({"type": "string"})), "epoch_id": {"type": "string"}}),
        &[
            "kind",
            "topic_id",
            "channel_id",
            "channel_label",
            "owner_pubkey",
            "inviter_pubkey",
            "sponsor_pubkey",
            "epoch_id",
        ],
    )
}

fn joined_schema() -> Value {
    schema::object(
        json!({"topic_id": {"type": "string"}, "channel_id": {"type": "string"}, "label": {"type": "string"},
        "creator_pubkey": {"type": "string"}, "owner_pubkey": {"type": "string"},
        "joined_via_pubkey": schema::nullable(json!({"type": "string"})),
        "audience_kind": {"enum": ["invite_only", "friend_only", "friend_plus"]},
        "is_owner": {"type": "boolean"}, "current_epoch_id": {"type": "string"},
        "archived_epoch_ids": schema::array(json!({"type": "string"})),
        "sharing_state": {"enum": ["open", "frozen"]}, "rotation_required": {"type": "boolean"},
        "participant_count": schema::nullable(json!({"type": "integer", "minimum": 0})),
        "stale_participant_count": {"type": "integer", "minimum": 0},
        "entry_dome_instance_id": schema::nullable(json!({"type": "string"}))}),
        &[
            "topic_id",
            "channel_id",
            "label",
            "creator_pubkey",
            "owner_pubkey",
            "joined_via_pubkey",
            "audience_kind",
            "is_owner",
            "current_epoch_id",
            "archived_epoch_ids",
            "sharing_state",
            "rotation_required",
            "participant_count",
            "stale_participant_count",
            "entry_dome_instance_id",
        ],
    )
}
