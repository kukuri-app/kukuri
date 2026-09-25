use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use kukuri_desktop_runtime::{ClientEventReceiver, ClientHost};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::protocol::{
    CommandEffect, CommandMetadata, CommandPage, DEFAULT_PAGE_LIMIT, GuardRequirement,
    MAX_PAGE_LIMIT, ProtocolError, SecretInput, SecretOutput, error_code, protocol_schema,
};

pub struct HandlerContext<'a> {
    pub registry: &'a CommandRegistry,
    pub host: Option<&'a Arc<ClientHost>>,
    pub profile: &'a str,
}

pub enum CommandOutput {
    Unary(Value),
    Secret { data: Value, secret: SecretOutput },
    Events(ClientEventReceiver),
}

#[async_trait]
pub trait CommandHandler: Send + Sync {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        payload: Value,
        secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError>;
}

pub struct CommandRegistration {
    pub metadata: CommandMetadata,
    pub handler: Arc<dyn CommandHandler>,
}

impl CommandRegistration {
    pub fn new(metadata: CommandMetadata, handler: Arc<dyn CommandHandler>) -> Self {
        Self { metadata, handler }
    }
}

pub struct CommandRegistry {
    entries: Vec<CommandRegistration>,
}

impl CommandRegistry {
    pub fn builtin() -> Self {
        Self::for_session(None)
    }

    pub(crate) fn for_session(session: Option<Arc<crate::session::ClientSession>>) -> Self {
        let mut entries = vec![
            registration(
                "protocol.schema",
                false,
                Vec::new(),
                Arc::new(ProtocolSchemaHandler),
                empty_object_schema(),
                json!({"type": "object"}),
            ),
            registration(
                "protocol.commands",
                false,
                Vec::new(),
                Arc::new(ProtocolCommandsHandler),
                json!({
                    "type": "object",
                    "properties": {
                        "cursor": {"type": "string"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": MAX_PAGE_LIMIT}
                    },
                    "additionalProperties": false
                }),
                json!({"type": "object"}),
            ),
            registration(
                "client.status",
                false,
                Vec::new(),
                Arc::new(ClientStatusHandler),
                empty_object_schema(),
                json!({"type": "object"}),
            ),
            registration(
                "events.watch",
                true,
                vec![GuardRequirement::HostReady],
                Arc::new(EventsWatchHandler),
                empty_object_schema(),
                json!({"type": "object"}),
            ),
        ];
        entries.extend(crate::commands::registrations(session));
        Self::new(entries).expect("builtin command registry must be valid")
    }

    pub fn new(mut entries: Vec<CommandRegistration>) -> Result<Self, ProtocolError> {
        entries.sort_by(|left, right| left.metadata.name.cmp(&right.metadata.name));
        let mut names = BTreeSet::new();
        for entry in &entries {
            validate_command_schema(&entry.metadata.input_schema, &entry.metadata.name)?;
            if entry
                .metadata
                .input_schema
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind != "object")
            {
                return invalid_schema(
                    &entry.metadata.name,
                    "input root type must be object when specified",
                );
            }
            validate_command_schema(&entry.metadata.output_schema, &entry.metadata.name)?;
            if entry.metadata.name.trim().is_empty() || !names.insert(entry.metadata.name.clone()) {
                return Err(ProtocolError::new(
                    error_code::INTERNAL_ERROR,
                    "command registry contains an empty or duplicate name",
                ));
            }
            if entry.metadata.streaming
                && (entry.metadata.secret_input || entry.metadata.secret_output)
            {
                return Err(ProtocolError::new(
                    error_code::INTERNAL_ERROR,
                    format!(
                        "streaming command `{}` cannot carry secret frames",
                        entry.metadata.name
                    ),
                ));
            }
        }
        Ok(Self { entries })
    }

    pub fn get(&self, name: &str) -> Option<&CommandRegistration> {
        self.entries
            .binary_search_by_key(&name, |entry| entry.metadata.name.as_str())
            .ok()
            .map(|index| &self.entries[index])
    }

    pub fn page(&self, cursor: Option<&str>, limit: u32) -> Result<CommandPage, ProtocolError> {
        if limit == 0 || limit > MAX_PAGE_LIMIT {
            return Err(ProtocolError::new(
                error_code::VALIDATION_FAILED,
                format!("limit must be between 1 and {MAX_PAGE_LIMIT}"),
            ));
        }
        let start = match cursor {
            Some(cursor) => self
                .entries
                .iter()
                .position(|entry| entry.metadata.name == cursor)
                .map(|index| index + 1)
                .ok_or_else(|| {
                    ProtocolError::new(error_code::VALIDATION_FAILED, "invalid command cursor")
                })?,
            None => 0,
        };
        let end = (start + limit as usize).min(self.entries.len());
        let items = self.entries[start..end]
            .iter()
            .map(|entry| entry.metadata.clone())
            .collect();
        let next_cursor =
            (end < self.entries.len()).then(|| self.entries[end - 1].metadata.name.clone());
        Ok(CommandPage { items, next_cursor })
    }

    pub fn schema_document(&self) -> Value {
        let commands = self
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.metadata.name.clone(),
                    json!({
                        "input": entry.metadata.input_schema,
                        "output": entry.metadata.output_schema,
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        json!({
            "protocol": protocol_schema(),
            "commands": commands,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn validate_command_schema(schema: &Value, command: &str) -> Result<(), ProtocolError> {
    let object = schema.as_object().ok_or_else(|| {
        ProtocolError::new(
            error_code::INTERNAL_ERROR,
            format!("command `{command}` has a non-object schema"),
        )
    })?;
    const SUPPORTED: &[&str] = &[
        "type",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "enum",
        "const",
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "title",
        "description",
        "default",
        "examples",
    ];
    if let Some(keyword) = object.keys().find(|key| !SUPPORTED.contains(&key.as_str())) {
        return Err(ProtocolError::new(
            error_code::INTERNAL_ERROR,
            format!("command `{command}` uses unsupported schema keyword `{keyword}`"),
        ));
    }
    if let Some(kind) = object.get("type")
        && !matches!(
            kind.as_str(),
            Some("string" | "integer" | "number" | "boolean" | "object" | "array" | "null")
        )
    {
        return invalid_schema(command, "type must name a supported JSON type");
    }
    let properties = match object.get("properties") {
        Some(Value::Object(properties)) => Some(properties),
        Some(_) => return invalid_schema(command, "properties must be an object"),
        None => None,
    };
    if let Some(required) = object.get("required")
        && !required
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
    {
        return invalid_schema(command, "required must be an array of strings");
    }
    if object
        .get("additionalProperties")
        .is_some_and(|value| !value.is_boolean())
    {
        return invalid_schema(command, "additionalProperties must be a boolean");
    }
    if object.get("items").is_some_and(|value| !value.is_object()) {
        return invalid_schema(command, "items must be an object schema");
    }
    if object
        .get("enum")
        .is_some_and(|value| value.as_array().is_none_or(|items| items.is_empty()))
    {
        return invalid_schema(command, "enum must be a non-empty array");
    }
    for keyword in ["minimum", "maximum"] {
        if object.get(keyword).is_some_and(|value| !value.is_number()) {
            return invalid_schema(command, &format!("{keyword} must be a number"));
        }
    }
    if let (Some(minimum), Some(maximum)) = (
        object.get("minimum").and_then(Value::as_f64),
        object.get("maximum").and_then(Value::as_f64),
    ) && minimum > maximum
    {
        return invalid_schema(command, "minimum must not exceed maximum");
    }
    for (minimum_key, maximum_key) in [("minLength", "maxLength"), ("minItems", "maxItems")] {
        if object
            .get(minimum_key)
            .is_some_and(|value| value.as_u64().is_none())
            || object
                .get(maximum_key)
                .is_some_and(|value| value.as_u64().is_none())
        {
            return invalid_schema(
                command,
                "length and item bounds must be non-negative integers",
            );
        }
        if let (Some(minimum), Some(maximum)) = (
            object.get(minimum_key).and_then(Value::as_u64),
            object.get(maximum_key).and_then(Value::as_u64),
        ) && minimum > maximum
        {
            return invalid_schema(command, "a minimum bound must not exceed its maximum");
        }
    }
    for keyword in ["title", "description"] {
        if object.get(keyword).is_some_and(|value| !value.is_string()) {
            return invalid_schema(command, &format!("{keyword} must be a string"));
        }
    }
    if object
        .get("examples")
        .is_some_and(|value| !value.is_array())
    {
        return invalid_schema(command, "examples must be an array");
    }
    if let Some(properties) = properties {
        for property in properties.values() {
            validate_command_schema(property, command)?;
        }
    }
    if let Some(items) = object.get("items") {
        validate_command_schema(items, command)?;
    }
    Ok(())
}

fn invalid_schema<T>(command: &str, message: &str) -> Result<T, ProtocolError> {
    Err(ProtocolError::new(
        error_code::INTERNAL_ERROR,
        format!("command `{command}` schema is invalid: {message}"),
    ))
}

fn registration(
    name: &str,
    streaming: bool,
    guards: Vec<GuardRequirement>,
    handler: Arc<dyn CommandHandler>,
    input_schema: Value,
    output_schema: Value,
) -> CommandRegistration {
    CommandRegistration::new(
        CommandMetadata {
            name: name.to_string(),
            effect: CommandEffect::Read,
            secret_input: false,
            secret_output: false,
            streaming,
            guards,
            input_schema,
            output_schema,
        },
        handler,
    )
}

fn empty_object_schema() -> Value {
    json!({"type": "object", "additionalProperties": false})
}

struct ProtocolSchemaHandler;

#[async_trait]
impl CommandHandler for ProtocolSchemaHandler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        _payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        Ok(CommandOutput::Unary(context.registry.schema_document()))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandsRequest {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default = "default_page_limit")]
    limit: u32,
}

fn default_page_limit() -> u32 {
    DEFAULT_PAGE_LIMIT
}

struct ProtocolCommandsHandler;

#[async_trait]
impl CommandHandler for ProtocolCommandsHandler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let request: CommandsRequest = serde_json::from_value(payload).map_err(|error| {
            ProtocolError::new(error_code::VALIDATION_FAILED, error.to_string())
        })?;
        let page = context
            .registry
            .page(request.cursor.as_deref(), request.limit)?;
        Ok(CommandOutput::Unary(serde_json::to_value(page).map_err(
            |_| ProtocolError::new(error_code::INTERNAL_ERROR, "failed to encode command page"),
        )?))
    }
}

struct ClientStatusHandler;

#[async_trait]
impl CommandHandler for ClientStatusHandler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        _payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let data = match context.host {
            Some(host) => {
                let runtime = host.runtime();
                json!({
                    "ready": true,
                    "consent_required": false,
                    "profile": context.profile,
                    "account_database": runtime
                        .db_path()
                        .file_name()
                        .and_then(|value| value.to_str()),
                })
            }
            None => json!({
                "ready": false,
                "consent_required": true,
                "profile": context.profile,
            }),
        };
        Ok(CommandOutput::Unary(data))
    }
}

struct EventsWatchHandler;

#[async_trait]
impl CommandHandler for EventsWatchHandler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        _payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let host = context.host.ok_or_else(|| {
            ProtocolError::new(
                error_code::CONSENT_REQUIRED,
                "application consent is required",
            )
        })?;
        Ok(CommandOutput::Events(host.subscribe_events()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_is_sorted_unique_and_introspectable() {
        let registry = CommandRegistry::builtin();
        assert_eq!(registry.len(), 151);
        let mut items = Vec::new();
        let mut cursor = None;
        loop {
            let page = registry.page(cursor.as_deref(), 2).expect("page");
            if page.next_cursor.is_some() {
                assert_eq!(page.items.len(), 2);
            } else {
                assert!(!page.items.is_empty() && page.items.len() <= 2);
            }
            items.extend(page.items);
            assert!(items.len() <= registry.len(), "pagination must advance");
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(items.len(), registry.len());
        assert!(items.windows(2).all(|pair| pair[0].name < pair[1].name));
        for name in [
            "client.status",
            "events.watch",
            "protocol.commands",
            "protocol.schema",
        ] {
            assert!(items.iter().any(|item| item.name == name));
        }
        let schema = registry.schema_document();
        assert_eq!(
            schema["commands"].as_object().expect("schemas").len(),
            items.len()
        );
        for item in items {
            let registered = registry.get(&item.name).expect("registered handler");
            assert_eq!(registered.metadata, item);
            assert_eq!(schema["commands"][&item.name]["input"], item.input_schema);
            assert_eq!(schema["commands"][&item.name]["output"], item.output_schema);
        }
    }

    #[test]
    fn mutating_secret_output_is_registered_without_persisting_a_result() {
        let metadata = CommandMetadata {
            name: "fixture.invalid".to_string(),
            effect: CommandEffect::Write,
            secret_input: false,
            secret_output: true,
            streaming: false,
            guards: Vec::new(),
            input_schema: empty_object_schema(),
            output_schema: json!({"type": "object"}),
        };
        let registry = CommandRegistry::new(vec![CommandRegistration::new(
            metadata,
            Arc::new(ProtocolSchemaHandler),
        )])
        .expect("秘密情報を返す変更操作も登録できる");
        assert_eq!(
            registry.get("fixture.invalid").unwrap().metadata.effect,
            CommandEffect::Write
        );
    }

    #[test]
    fn unsupported_schema_keywords_are_rejected_at_registration() {
        let metadata = CommandMetadata {
            name: "fixture.invalid-schema".to_string(),
            effect: CommandEffect::Read,
            secret_input: false,
            secret_output: false,
            streaming: false,
            guards: Vec::new(),
            input_schema: json!({"type": "object", "oneOf": []}),
            output_schema: empty_object_schema(),
        };
        let error = CommandRegistry::new(vec![CommandRegistration::new(
            metadata,
            Arc::new(ProtocolSchemaHandler),
        )])
        .err()
        .expect("unsupported schema");
        assert!(error.message.contains("unsupported schema keyword"));

        for schema in [
            json!({"type": "unsupported"}),
            json!({"type": "object", "required": "field"}),
            json!({"type": "integer", "minimum": "10"}),
            json!({"type": "array", "items": true}),
        ] {
            let metadata = CommandMetadata {
                name: "fixture.invalid-schema".to_string(),
                effect: CommandEffect::Read,
                secret_input: false,
                secret_output: false,
                streaming: false,
                guards: Vec::new(),
                input_schema: schema,
                output_schema: empty_object_schema(),
            };
            let error = CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(ProtocolSchemaHandler),
            )])
            .err()
            .expect("malformed schema");
            assert!(error.message.contains("schema is invalid"));
        }
    }

    #[test]
    fn input_root_schema_must_match_the_object_payload_contract() {
        let metadata = CommandMetadata {
            name: "fixture.non-object-input".to_string(),
            effect: CommandEffect::Read,
            secret_input: false,
            secret_output: false,
            streaming: false,
            guards: Vec::new(),
            input_schema: json!({"type": "string"}),
            output_schema: json!({"type": "string"}),
        };
        let error = CommandRegistry::new(vec![CommandRegistration::new(
            metadata,
            Arc::new(ProtocolSchemaHandler),
        )])
        .err()
        .expect("non-object input root");
        assert!(error.message.contains("input root type must be object"));
    }
}
