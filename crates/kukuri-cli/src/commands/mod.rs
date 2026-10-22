mod community_node;
mod community_schema;
mod community_views;
mod content_schema;
mod content_social;
mod content_views;
mod direct_messages;
mod dome_connection_views;
mod errors;
mod game_views;
mod lifecycle;
mod lifecycle_schema;
mod live_game;
mod live_metaverse;
mod media;
mod media_output;
mod metaverse_schema;
mod metaverse_views;
mod network_community_node;
mod network_schema;
mod private_channels;
mod public_channels;
mod schema;
mod topic_retention;

use std::sync::Arc;

use errors::command_error;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

use crate::{
    protocol::{CommandEffect, CommandMetadata, GuardRequirement, ProtocolError, error_code},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};

pub(crate) fn registrations(
    session: Option<Arc<crate::session::ClientSession>>,
) -> Vec<CommandRegistration> {
    let mut entries = Vec::new();
    entries.extend(content_social::registrations());
    entries.extend(direct_messages::registrations());
    entries.extend(live_game::registrations());
    entries.extend(private_channels::registrations());
    entries.extend(public_channels::registrations());
    entries.extend(live_metaverse::registrations());
    entries.extend(network_community_node::registrations());
    entries.extend(community_node::registrations());
    entries.extend(topic_retention::registrations());
    entries.extend(lifecycle::registrations(session));
    entries
}

pub(super) fn command(
    name: &'static str,
    effect: CommandEffect,
    secret_input: bool,
    secret_output: bool,
    guards: Vec<GuardRequirement>,
    schemas: (Value, Value),
    handler: Arc<dyn CommandHandler>,
) -> CommandRegistration {
    CommandRegistration::new(
        CommandMetadata {
            name: name.to_string(),
            effect,
            secret_input,
            secret_output,
            streaming: false,
            guards,
            input_schema: schemas.0,
            output_schema: schemas.1,
        },
        handler,
    )
}

pub(super) fn runtime(
    context: &HandlerContext<'_>,
) -> Result<Arc<kukuri_desktop_runtime::DesktopRuntime>, ProtocolError> {
    context.host.map(|host| host.runtime()).ok_or_else(|| {
        ProtocolError::new(
            error_code::CONSENT_REQUIRED,
            "application consent is required",
        )
    })
}

pub(super) fn decode<T: DeserializeOwned>(payload: Value) -> Result<T, ProtocolError> {
    serde_json::from_value(payload).map_err(|_| {
        ProtocolError::new(
            error_code::VALIDATION_FAILED,
            "入力がcommandのDTO定義に一致しません",
        )
    })
}

pub(super) fn encode<T: Serialize>(value: T) -> Result<CommandOutput, ProtocolError> {
    serde_json::to_value(value)
        .map(CommandOutput::Unary)
        .map_err(|_| {
            ProtocolError::new(
                error_code::INTERNAL_ERROR,
                "failed to encode command response",
            )
        })
}

pub(super) fn host_guards() -> Vec<GuardRequirement> {
    vec![GuardRequirement::HostReady]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Deserialize)]
    enum FixtureMode {
        Allowed,
    }

    #[test]
    fn invalid_dto_does_not_echo_private_input_in_diagnostics() {
        let error = decode::<FixtureMode>(json!("private-body-sentinel")).unwrap_err();
        assert_eq!(error.code, error_code::VALIDATION_FAILED);
        assert!(!error.message.contains("sentinel"));
    }

    #[test]
    fn runtime_error_does_not_echo_private_input_in_diagnostics() {
        let error = command_error(anyhow::anyhow!("private-body-sentinel"));
        assert!(!error.message.contains("sentinel"));
        assert!(error.details.is_none());
    }
}
