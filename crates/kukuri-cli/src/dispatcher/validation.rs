use serde_json::Value;

use crate::{
    protocol::{CommandMetadata, ProtocolError, SecretInput, error_code},
    registry::CommandOutput,
};

pub(super) fn validate_payload(schema: &Value, payload: &Value) -> Result<(), ProtocolError> {
    validate_value(schema, payload, "payload")
}

fn validate_value(schema: &Value, value: &Value, path: &str) -> Result<(), ProtocolError> {
    let valid_type = match schema.get("type").and_then(Value::as_str) {
        Some("string") => value.is_string(),
        Some("integer") => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        Some("number") => value.is_number(),
        Some("boolean") => value.is_boolean(),
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("null") => value.is_null(),
        Some(_) | None => true,
    };
    if !valid_type {
        return validation_error(format!("{path} has an invalid type"));
    }
    if let Some(expected) = schema.get("const")
        && expected != value
    {
        return validation_error(format!("{path} does not match the required value"));
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.contains(value)
    {
        return validation_error(format!("{path} is not an allowed value"));
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count() as u64;
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| length < minimum)
            || schema
                .get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|maximum| length > maximum)
        {
            return validation_error(format!("{path} has an invalid length"));
        }
    }
    if let Some(number) = value.as_f64()
        && (schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|minimum| number < minimum)
            || schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|maximum| number > maximum))
    {
        return validation_error(format!("{path} is outside the supported range"));
    }
    if let Some(items) = value.as_array() {
        if schema
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| items.len() < minimum as usize)
            || schema
                .get("maxItems")
                .and_then(Value::as_u64)
                .is_some_and(|maximum| items.len() > maximum as usize)
        {
            return validation_error(format!("{path} has an invalid item count"));
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in items.iter().enumerate() {
                validate_value(item_schema, item, &format!("{path}[{index}]"))?;
            }
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    return validation_error(format!("{path} is missing required field `{key}`"));
                }
            }
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            for key in object.keys() {
                if !properties.is_some_and(|properties| properties.contains_key(key)) {
                    return validation_error(format!("{path} contains unknown field `{key}`"));
                }
            }
        }
        if let Some(properties) = properties {
            for (key, field_schema) in properties {
                if let Some(field) = object.get(key) {
                    validate_value(field_schema, field, &format!("{path}.{key}"))?;
                }
            }
        }
    }
    Ok(())
}

fn validation_error(message: String) -> Result<(), ProtocolError> {
    Err(ProtocolError::new(error_code::VALIDATION_FAILED, message))
}

pub(super) fn validate_command_output(
    metadata: &CommandMetadata,
    output: &CommandOutput,
    secret_input: Option<&SecretInput>,
) -> Result<(), ProtocolError> {
    let (data, secret_output) = match output {
        CommandOutput::Unary(data) if !metadata.secret_output && !metadata.streaming => {
            (data, None)
        }
        CommandOutput::Secret { data, secret }
            if metadata.secret_output
                && !metadata.streaming
                && secret.expose().len() <= crate::protocol::MAX_FRAME_BYTES =>
        {
            (data, Some(secret))
        }
        CommandOutput::Events(_) if metadata.streaming => return Ok(()),
        _ => {
            return Err(ProtocolError::new(
                error_code::INTERNAL_ERROR,
                "command output does not match registry metadata",
            ));
        }
    };
    reject_secret_in_json(data, secret_input.map(SecretInput::expose), "secret input")?;
    reject_secret_in_json(
        data,
        secret_output.map(|secret| secret.expose()),
        "secret output",
    )?;
    validate_value(&metadata.output_schema, data, "output").map_err(|_| {
        ProtocolError::new(
            error_code::INTERNAL_ERROR,
            "command output does not match its registered schema",
        )
    })
}

fn reject_secret_in_json(
    data: &Value,
    secret: Option<&[u8]>,
    label: &str,
) -> Result<(), ProtocolError> {
    let Some(secret) = secret.filter(|secret| !secret.is_empty()) else {
        return Ok(());
    };
    let encoded = serde_json::to_vec(data).map_err(|_| {
        ProtocolError::new(
            error_code::INTERNAL_ERROR,
            "command output could not be encoded",
        )
    })?;
    if encoded.windows(secret.len()).any(|window| window == secret)
        || json_contains_secret(data, secret)
    {
        return Err(ProtocolError::new(
            error_code::INTERNAL_ERROR,
            format!("{label} was also returned in JSON"),
        ));
    }
    Ok(())
}

fn json_contains_secret(value: &Value, secret: &[u8]) -> bool {
    match value {
        Value::String(text) => text
            .as_bytes()
            .windows(secret.len())
            .any(|window| window == secret),
        Value::Array(items) => items.iter().any(|item| json_contains_secret(item, secret)),
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            key.as_bytes()
                .windows(secret.len())
                .any(|window| window == secret)
                || json_contains_secret(value, secret)
        }),
        _ => false,
    }
}

pub(super) fn redact_protocol_error(
    mut error: ProtocolError,
    secret_bearing: bool,
) -> ProtocolError {
    if !secret_bearing {
        return error;
    }
    if !matches!(
        error.code.as_str(),
        error_code::INVALID_REQUEST
            | error_code::VALIDATION_FAILED
            | error_code::CONSENT_REQUIRED
            | error_code::ACTION_REQUIRED
            | error_code::DAEMON_UNAVAILABLE
            | error_code::PROTOCOL_MISMATCH
            | error_code::PROFILE_IN_USE
            | error_code::AUTHORIZATION_FAILED
            | error_code::NOT_FOUND
            | error_code::CONFLICT
            | error_code::OPERATION_OUTCOME_UNKNOWN
            | error_code::NETWORK_UNAVAILABLE
            | error_code::TIMEOUT
            | error_code::INTERRUPTED
            | error_code::INTERNAL_ERROR
            | error_code::BACKPRESSURE
    ) {
        error.code = error_code::INTERNAL_ERROR.to_string();
    }
    error.message = "secret-bearing command failed".to_string();
    error.details = None;
    error
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::validate_command_output;
    use crate::registry::{CommandOutput, CommandRegistry};

    #[test]
    fn dm_status_and_conversation_outputs_accept_the_bounded_count_flag() {
        let registry = CommandRegistry::builtin();
        let status = json!({
            "peer_pubkey": "a", "dm_id": "dm", "mutual": true,
            "send_enabled": true,
            "pending_outbox_count": 64, "pending_outbox_has_more": true
        });
        let conversation = json!({
            "dm_id": "dm", "peer_pubkey": "a", "peer_name": null,
            "peer_display_name": null, "peer_picture_asset": null,
            "updated_at": 1, "last_message_at": null,
            "last_message_id": null, "last_message_preview": null,
            "status": status
        });
        for (command, output) in [
            ("get_direct_message_status", status),
            ("open_direct_message", conversation.clone()),
            ("list_direct_messages", json!([conversation])),
        ] {
            let metadata = &registry.get(command).expect("registered command").metadata;
            validate_command_output(metadata, &CommandOutput::Unary(output), None)
                .unwrap_or_else(|error| panic!("{command} output was rejected: {error}"));
        }
    }
}
