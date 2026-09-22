use std::sync::Arc;

mod operations;
mod validation;

use async_trait::async_trait;
use kukuri_desktop_runtime::ClientHost;
use serde_json::Value;

use crate::{
    protocol::{
        CommandEffect, GuardRequirement, PROTOCOL_VERSION, ProtocolError, RequestEnvelope,
        ResponseEnvelope, SecretInput, error_code,
    },
    registry::{CommandOutput, CommandRegistry, HandlerContext},
};
use validation::{redact_protocol_error, validate_command_output, validate_payload};

pub enum DispatchReply {
    Unary(ResponseEnvelope, Option<crate::protocol::SecretOutput>),
    Events {
        request: RequestEnvelope,
        receiver: kukuri_desktop_runtime::ClientEventReceiver,
    },
}

#[derive(Clone)]
pub struct Dispatcher {
    registry: Arc<CommandRegistry>,
    mutation_guard: Arc<tokio::sync::Mutex<()>>,
    guard_evaluator: Arc<dyn GuardEvaluator>,
    operations: Arc<operations::OperationTasks>,
    session: Option<Arc<crate::session::ClientSession>>,
}

pub struct GuardContext<'a> {
    pub request: &'a RequestEnvelope,
    pub host: Option<&'a Arc<ClientHost>>,
}

#[async_trait]
pub trait GuardEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        requirement: GuardRequirement,
        context: &GuardContext<'_>,
    ) -> Result<(), ProtocolError>;
}

struct HostGuardEvaluator;

#[async_trait]
impl GuardEvaluator for HostGuardEvaluator {
    async fn evaluate(
        &self,
        requirement: GuardRequirement,
        context: &GuardContext<'_>,
    ) -> Result<(), ProtocolError> {
        match requirement {
            GuardRequirement::HostReady if context.host.is_none() => Err(ProtocolError::new(
                error_code::CONSENT_REQUIRED,
                "application consent is required",
            )),
            GuardRequirement::HostReady => Ok(()),
            _ => Err(ProtocolError::new(
                error_code::ACTION_REQUIRED,
                "the command guard is not available in this protocol version",
            )),
        }
    }
}

impl Dispatcher {
    pub fn builtin() -> Self {
        Self {
            registry: Arc::new(CommandRegistry::builtin()),
            mutation_guard: Arc::new(tokio::sync::Mutex::new(())),
            guard_evaluator: Arc::new(HostGuardEvaluator),
            operations: Arc::new(operations::OperationTasks::new()),
            session: None,
        }
    }

    pub fn new(registry: CommandRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
            mutation_guard: Arc::new(tokio::sync::Mutex::new(())),
            guard_evaluator: Arc::new(HostGuardEvaluator),
            operations: Arc::new(operations::OperationTasks::new()),
            session: None,
        }
    }

    pub fn with_guard_evaluator(mut self, evaluator: Arc<dyn GuardEvaluator>) -> Self {
        self.guard_evaluator = evaluator;
        self
    }

    pub fn for_session(session: Arc<crate::session::ClientSession>) -> Self {
        let mut dispatcher = Self::new(CommandRegistry::for_session(Some(session.clone())));
        dispatcher.session = Some(session);
        dispatcher
    }

    fn resolve_host(&self, fallback: Option<&Arc<ClientHost>>) -> Option<Arc<ClientHost>> {
        match &self.session {
            Some(session) => session.host(),
            None => fallback.cloned(),
        }
    }

    pub fn registry(&self) -> &CommandRegistry {
        &self.registry
    }

    pub async fn preflight(
        &self,
        request: &RequestEnvelope,
        expected_profile: &str,
        host: Option<&Arc<ClientHost>>,
    ) -> Result<(), ProtocolError> {
        let registration = self.registration_for(request, expected_profile)?;
        if registration.metadata.secret_output && !request.accepts_secret_output {
            return Err(ProtocolError::new(
                error_code::ACTION_REQUIRED,
                "secret response requires an explicit output sink",
            ));
        }
        let resolved = self.resolve_host(host);
        let context = GuardContext {
            request,
            host: resolved.as_ref(),
        };
        let secret_bearing =
            registration.metadata.secret_input || registration.metadata.secret_output;
        for requirement in &registration.metadata.guards {
            self.guard_evaluator
                .evaluate(*requirement, &context)
                .await
                .map_err(|error| redact_protocol_error(error, secret_bearing))?;
        }
        Ok(())
    }

    async fn dispatch_inner(
        &self,
        request: &RequestEnvelope,
        secret: Option<SecretInput>,
        expected_profile: &str,
        host: Option<&Arc<ClientHost>>,
    ) -> Result<CommandOutput, ProtocolError> {
        let registration = self.registration_for(request, expected_profile)?;
        if request.secret_bytes.is_some() != secret.is_some() {
            return Err(ProtocolError::new(
                error_code::INVALID_REQUEST,
                "secret frame is missing or unexpected",
            ));
        }
        // runtime利用中のreadも停止・切替と直列化する。取消だけは進行中のbackupへ届く。
        let needs_lock = request.command != "cancel_device_backup"
            && (matches!(
                registration.metadata.effect,
                CommandEffect::Write | CommandEffect::Destructive
            ) || (self.session.is_some()
                && registration
                    .metadata
                    .guards
                    .contains(&GuardRequirement::HostReady)));
        let _mutation_guard = if needs_lock {
            Some(self.mutation_guard.lock().await)
        } else {
            None
        };
        // lock待機中の切替・復元を反映し、停止済みruntimeへ新しい要求を渡さない。
        let resolved = self.resolve_host(host);
        let host = resolved.as_ref();
        self.preflight(request, expected_profile, host).await?;
        let result = registration
            .handler
            .execute(
                HandlerContext {
                    registry: &self.registry,
                    host,
                    profile: expected_profile,
                },
                request.payload.clone(),
                secret.as_ref(),
            )
            .await
            .map_err(|error| {
                redact_protocol_error(
                    error,
                    registration.metadata.secret_input || registration.metadata.secret_output,
                )
            })?;
        validate_command_output(&registration.metadata, &result, secret.as_ref())?;
        Ok(result)
    }

    fn registration_for<'a>(
        &'a self,
        request: &RequestEnvelope,
        expected_profile: &str,
    ) -> Result<&'a crate::registry::CommandRegistration, ProtocolError> {
        if request.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolError::new(
                error_code::PROTOCOL_MISMATCH,
                format!(
                    "unsupported protocol version {}; expected {PROTOCOL_VERSION}",
                    request.protocol_version
                ),
            ));
        }
        if request.request_id.trim().is_empty() || request.command.trim().is_empty() {
            return Err(ProtocolError::new(
                error_code::VALIDATION_FAILED,
                "request_id and command must not be empty",
            ));
        }
        if request.profile != expected_profile {
            return Err(ProtocolError::new(
                error_code::AUTHORIZATION_FAILED,
                "request profile does not match the daemon profile",
            ));
        }
        if !request.payload.is_object() {
            return Err(ProtocolError::new(
                error_code::VALIDATION_FAILED,
                "payload must be a JSON object",
            ));
        }
        let registration = self
            .registry
            .get(&request.command)
            .ok_or_else(|| ProtocolError::new(error_code::NOT_FOUND, "unknown command"))?;
        validate_payload(&registration.metadata.input_schema, &request.payload).map_err(
            |error| {
                redact_protocol_error(
                    error,
                    registration.metadata.secret_input || registration.metadata.secret_output,
                )
            },
        )?;
        if request.secret_bytes.is_some() != registration.metadata.secret_input {
            return Err(ProtocolError::new(
                error_code::VALIDATION_FAILED,
                "secret input does not match command metadata",
            ));
        }
        Ok(registration)
    }
}

pub fn decode_request(bytes: &[u8]) -> Result<RequestEnvelope, ProtocolError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| ProtocolError::new(error_code::INVALID_REQUEST, "invalid JSON request"))?;
    serde_json::from_value(value)
        .map_err(|error| ProtocolError::new(error_code::INVALID_REQUEST, error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use kukuri_desktop_runtime::DesktopRuntime;
    use serde_json::json;

    use super::*;
    use crate::{
        protocol::{CommandEffect, CommandMetadata, GuardRequirement},
        registry::{CommandHandler, CommandRegistration},
    };

    fn request(command: &str) -> RequestEnvelope {
        RequestEnvelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: "req-1".to_string(),
            command: command.to_string(),
            profile: "test".to_string(),
            payload: json!({}),
            timeout_ms: None,
            secret_bytes: None,
            accepts_secret_output: false,
        }
    }

    #[tokio::test]
    async fn mismatch_and_unknown_command_are_typed_errors() {
        let dispatcher = Dispatcher::builtin();
        let mut mismatch = request("client.status");
        mismatch.protocol_version = 99;
        let DispatchReply::Unary(response, _) =
            dispatcher.dispatch(mismatch, None, "test", None).await
        else {
            panic!("expected unary response")
        };
        assert_eq!(response.error_code(), Some(error_code::PROTOCOL_MISMATCH));

        let DispatchReply::Unary(response, _) = dispatcher
            .dispatch(request("missing.command"), None, "test", None)
            .await
        else {
            panic!("expected unary response")
        };
        assert_eq!(response.error_code(), Some(error_code::NOT_FOUND));
    }

    // #1280: 複数の channel をまたぐ scope(`all_joined`)は API から閉じた。渡されたら入力の検証で失敗する。
    #[tokio::test]
    async fn all_joined_timeline_scope_is_rejected_by_the_input_schema() {
        let dispatcher = Dispatcher::builtin();
        for command in ["list_timeline", "list_live_sessions", "list_game_rooms"] {
            let mut rejected = request(command);
            rejected.payload = json!({"topic": "kukuri:topic:t", "scope": {"kind": "all_joined"}});
            let DispatchReply::Unary(response, _) =
                dispatcher.dispatch(rejected, None, "test", None).await
            else {
                panic!("expected unary response")
            };
            assert_eq!(
                response.error_code(),
                Some(error_code::VALIDATION_FAILED),
                "{command}"
            );
        }
    }

    #[tokio::test]
    async fn status_is_available_before_consent_but_events_are_guarded() {
        let dispatcher = Dispatcher::builtin();
        let DispatchReply::Unary(status, _) = dispatcher
            .dispatch(request("client.status"), None, "test", None)
            .await
        else {
            panic!("expected status")
        };
        assert!(status.ok);
        assert_eq!(status.data.expect("data")["ready"], false);

        let DispatchReply::Unary(events, _) = dispatcher
            .dispatch(request("events.watch"), None, "test", None)
            .await
        else {
            panic!("expected guarded response")
        };
        assert_eq!(events.error_code(), Some(error_code::CONSENT_REQUIRED));
    }

    struct CountingWriteHandler(Arc<AtomicUsize>);

    #[async_trait]
    impl CommandHandler for CountingWriteHandler {
        async fn execute(
            &self,
            _context: crate::registry::HandlerContext<'_>,
            payload: Value,
            _secret: Option<&SecretInput>,
        ) -> Result<CommandOutput, ProtocolError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(CommandOutput::Unary(json!({"saved": payload})))
        }
    }

    struct SecretEchoHandler;

    #[async_trait]
    impl CommandHandler for SecretEchoHandler {
        async fn execute(
            &self,
            _context: crate::registry::HandlerContext<'_>,
            _payload: Value,
            secret: Option<&SecretInput>,
        ) -> Result<CommandOutput, ProtocolError> {
            let bytes = secret.expect("secret input").expose().to_vec();
            Ok(CommandOutput::Secret {
                data: json!({"written": true}),
                secret: crate::protocol::SecretOutput::new(bytes),
            })
        }
    }

    struct SecretErrorHandler;

    #[async_trait]
    impl CommandHandler for SecretErrorHandler {
        async fn execute(
            &self,
            _context: crate::registry::HandlerContext<'_>,
            _payload: Value,
            secret: Option<&SecretInput>,
        ) -> Result<CommandOutput, ProtocolError> {
            let secret =
                std::str::from_utf8(secret.expect("secret input").expose()).expect("fixture UTF-8");
            Err(
                ProtocolError::new(error_code::VALIDATION_FAILED, format!("rejected {secret}"))
                    .with_details(json!({"reason": secret})),
            )
        }
    }

    struct SecretOutputLeakHandler;

    #[async_trait]
    impl CommandHandler for SecretOutputLeakHandler {
        async fn execute(
            &self,
            _context: crate::registry::HandlerContext<'_>,
            _payload: Value,
            _secret: Option<&SecretInput>,
        ) -> Result<CommandOutput, ProtocolError> {
            Ok(CommandOutput::Secret {
                data: json!({"leak": "secret-output-\"-\\-sentinel"}),
                secret: crate::protocol::SecretOutput::new(
                    b"secret-output-\"-\\-sentinel".to_vec(),
                ),
            })
        }
    }

    struct SecretOutputErrorHandler;

    #[async_trait]
    impl CommandHandler for SecretOutputErrorHandler {
        async fn execute(
            &self,
            _context: crate::registry::HandlerContext<'_>,
            _payload: Value,
            _secret: Option<&SecretInput>,
        ) -> Result<CommandOutput, ProtocolError> {
            Err(
                ProtocolError::new(error_code::INTERNAL_ERROR, "secret-output-error-sentinel")
                    .with_details(json!({"leak": "secret-output-error-sentinel"})),
            )
        }
    }

    struct AllowAllGuards;

    #[async_trait]
    impl GuardEvaluator for AllowAllGuards {
        async fn evaluate(
            &self,
            _requirement: GuardRequirement,
            _context: &GuardContext<'_>,
        ) -> Result<(), ProtocolError> {
            Ok(())
        }
    }

    struct LeakingGuard;

    #[async_trait]
    impl GuardEvaluator for LeakingGuard {
        async fn evaluate(
            &self,
            _requirement: GuardRequirement,
            _context: &GuardContext<'_>,
        ) -> Result<(), ProtocolError> {
            Err(
                ProtocolError::new(error_code::AUTHORIZATION_FAILED, "guard-secret-sentinel")
                    .with_details(json!({"credential": "guard-secret-sentinel"})),
            )
        }
    }

    fn write_dispatcher(counter: Arc<AtomicUsize>) -> Dispatcher {
        let metadata = CommandMetadata {
            name: "fixture.write".to_string(),
            effect: CommandEffect::Write,
            secret_input: false,
            secret_output: false,
            streaming: false,
            guards: vec![GuardRequirement::HostReady],
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        };
        Dispatcher::new(
            CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(CountingWriteHandler(counter)),
            )])
            .expect("registry"),
        )
    }

    #[tokio::test]
    async fn guard_runs_before_mutation_sink() {
        let counter = Arc::new(AtomicUsize::new(0));
        let dispatcher = write_dispatcher(counter.clone());
        let request = request("fixture.write");
        let DispatchReply::Unary(response, _) =
            dispatcher.dispatch(request, None, "test", None).await
        else {
            panic!("expected guarded response")
        };
        assert_eq!(response.error_code(), Some(error_code::CONSENT_REQUIRED));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn each_explicit_mutation_executes_even_with_identical_request_id() {
        let root = tempfile::tempdir().expect("tempdir");
        let runtime = Arc::new(
            DesktopRuntime::new(root.path().join("kukuri.db"))
                .await
                .expect("runtime"),
        );
        let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
            .await
            .expect("host");
        let counter = Arc::new(AtomicUsize::new(0));
        let dispatcher = write_dispatcher(counter.clone());
        let mut first = request("fixture.write");
        first.payload = json!({"value": 1});
        let DispatchReply::Unary(response, _) = dispatcher
            .dispatch(first.clone(), None, "test", Some(&host))
            .await
        else {
            panic!("expected first response")
        };
        assert!(response.ok);
        let DispatchReply::Unary(second, _) = dispatcher
            .dispatch(first.clone(), None, "test", Some(&host))
            .await
        else {
            panic!("expected second response")
        };
        assert!(second.ok);
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        first.payload = json!({"value": 2});
        let DispatchReply::Unary(third, _) =
            dispatcher.dispatch(first, None, "test", Some(&host)).await
        else {
            panic!("expected third response")
        };
        assert!(third.ok);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
        host.shutdown().await;
    }

    #[tokio::test]
    async fn secret_is_kept_out_of_the_json_response() {
        let metadata = CommandMetadata {
            name: "fixture.secret".to_string(),
            effect: CommandEffect::Read,
            secret_input: true,
            secret_output: true,
            streaming: false,
            guards: Vec::new(),
            input_schema: json!({"type": "object", "additionalProperties": false}),
            output_schema: json!({"type": "object"}),
        };
        let dispatcher = Dispatcher::new(
            CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(SecretEchoHandler),
            )])
            .expect("registry"),
        );
        let sentinel = b"secret-sentinel";
        let mut request = request("fixture.secret");
        request.secret_bytes = Some(sentinel.len() as u64);
        request.accepts_secret_output = true;
        let DispatchReply::Unary(response, secret) = dispatcher
            .dispatch(
                request,
                Some(SecretInput::new(sentinel.to_vec())),
                "test",
                None,
            )
            .await
        else {
            panic!("expected secret response")
        };
        let encoded = serde_json::to_string(&response).expect("response json");
        assert!(!encoded.contains("secret-sentinel"));
        assert_eq!(secret.expect("secret frame").expose(), sentinel);
    }

    #[tokio::test]
    async fn preflight_rejects_guarded_secret_before_a_secret_frame_is_needed() {
        let metadata = CommandMetadata {
            name: "fixture.guarded-secret".to_string(),
            effect: CommandEffect::Read,
            secret_input: true,
            secret_output: false,
            streaming: false,
            guards: vec![GuardRequirement::HostReady],
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        };
        let dispatcher = Dispatcher::new(
            CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(SecretEchoHandler),
            )])
            .expect("registry"),
        );
        let mut request = request("fixture.guarded-secret");
        request.secret_bytes = Some(64);
        let error = dispatcher
            .preflight(&request, "test", None)
            .await
            .expect_err("guard blocks before secret read");
        assert_eq!(error.code, error_code::CONSENT_REQUIRED);
    }

    #[tokio::test]
    async fn domain_guard_has_an_injectable_success_path() {
        let metadata = CommandMetadata {
            name: "fixture.domain-read".to_string(),
            effect: CommandEffect::Read,
            secret_input: false,
            secret_output: false,
            streaming: false,
            guards: vec![GuardRequirement::DomainAuthorization],
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        };
        let dispatcher = Dispatcher::new(
            CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(CountingWriteHandler(Arc::new(AtomicUsize::new(0)))),
            )])
            .expect("registry"),
        )
        .with_guard_evaluator(Arc::new(AllowAllGuards));
        let DispatchReply::Unary(response, _) = dispatcher
            .dispatch(request("fixture.domain-read"), None, "test", None)
            .await
        else {
            panic!("expected unary response")
        };
        assert!(response.ok);
    }

    #[tokio::test]
    async fn secret_is_redacted_from_handler_errors_and_details() {
        let metadata = CommandMetadata {
            name: "fixture.secret-error".to_string(),
            effect: CommandEffect::Read,
            secret_input: true,
            secret_output: false,
            streaming: false,
            guards: Vec::new(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        };
        let dispatcher = Dispatcher::new(
            CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(SecretErrorHandler),
            )])
            .expect("registry"),
        );
        let sentinel = b"secret-error-sentinel";
        let mut request = request("fixture.secret-error");
        request.secret_bytes = Some(sentinel.len() as u64);
        let DispatchReply::Unary(response, _) = dispatcher
            .dispatch(
                request,
                Some(SecretInput::new(sentinel.to_vec())),
                "test",
                None,
            )
            .await
        else {
            panic!("expected error response")
        };
        let encoded = serde_json::to_string(&response).expect("response JSON");
        assert!(!encoded.contains("secret-error-sentinel"));
        assert!(encoded.contains("secret-bearing command failed"));
    }

    #[tokio::test]
    async fn handler_cannot_return_an_unregistered_secret_output() {
        let metadata = CommandMetadata {
            name: "fixture.secret-leak".to_string(),
            effect: CommandEffect::Read,
            secret_input: true,
            secret_output: false,
            streaming: false,
            guards: Vec::new(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        };
        let dispatcher = Dispatcher::new(
            CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(SecretEchoHandler),
            )])
            .expect("registry"),
        );
        let sentinel = b"unregistered-secret";
        let mut request = request("fixture.secret-leak");
        request.secret_bytes = Some(sentinel.len() as u64);
        let DispatchReply::Unary(response, secret) = dispatcher
            .dispatch(
                request,
                Some(SecretInput::new(sentinel.to_vec())),
                "test",
                None,
            )
            .await
        else {
            panic!("expected guarded response")
        };
        assert_eq!(response.error_code(), Some(error_code::INTERNAL_ERROR));
        assert!(secret.is_none());
        assert!(
            !serde_json::to_string(&response)
                .expect("response JSON")
                .contains("unregistered-secret")
        );
    }

    #[test]
    fn nested_schema_constraints_are_enforced() {
        let schema = json!({
            "type": "object",
            "required": ["mode", "items"],
            "properties": {
                "mode": {"type": "string", "enum": ["safe"]},
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {"name": {"type": "string", "minLength": 1}},
                        "additionalProperties": false
                    }
                }
            },
            "additionalProperties": false
        });
        assert!(
            validate_payload(&schema, &json!({"mode": "safe", "items": [{"name": "a"}]})).is_ok()
        );
        assert!(validate_payload(&schema, &json!({"mode": "unsafe", "items": []})).is_err());
        assert!(
            validate_payload(&schema, &json!({"mode": "safe", "items": [{"name": ""}]})).is_err()
        );
    }

    #[tokio::test]
    async fn secret_output_cannot_also_appear_in_json() {
        let metadata = CommandMetadata {
            name: "fixture.secret-output-leak".to_string(),
            effect: CommandEffect::Read,
            secret_input: false,
            secret_output: true,
            streaming: false,
            guards: Vec::new(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        };
        let dispatcher = Dispatcher::new(
            CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(SecretOutputLeakHandler),
            )])
            .expect("registry"),
        );
        let mut request = request("fixture.secret-output-leak");
        request.accepts_secret_output = true;
        let DispatchReply::Unary(response, secret) =
            dispatcher.dispatch(request, None, "test", None).await
        else {
            panic!("expected error response")
        };
        assert_eq!(response.error_code(), Some(error_code::INTERNAL_ERROR));
        assert!(secret.is_none());
        assert!(
            !serde_json::to_string(&response)
                .expect("response JSON")
                .contains("secret-output-\\\"-\\\\-sentinel")
        );
    }

    #[tokio::test]
    async fn secret_output_only_errors_are_redacted() {
        let metadata = CommandMetadata {
            name: "fixture.secret-output-error".to_string(),
            effect: CommandEffect::Read,
            secret_input: false,
            secret_output: true,
            streaming: false,
            guards: Vec::new(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        };
        let dispatcher = Dispatcher::new(
            CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(SecretOutputErrorHandler),
            )])
            .expect("registry"),
        );
        let mut request = request("fixture.secret-output-error");
        request.accepts_secret_output = true;
        let DispatchReply::Unary(response, _) =
            dispatcher.dispatch(request, None, "test", None).await
        else {
            panic!("expected error response")
        };
        let encoded = serde_json::to_string(&response).expect("response JSON");
        assert!(!encoded.contains("secret-output-error-sentinel"));
        assert!(encoded.contains("secret-bearing command failed"));
    }

    #[tokio::test]
    async fn secret_bearing_guard_errors_are_redacted() {
        let metadata = CommandMetadata {
            name: "fixture.secret-guard".to_string(),
            effect: CommandEffect::Read,
            secret_input: true,
            secret_output: false,
            streaming: false,
            guards: vec![GuardRequirement::Credential],
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        };
        let dispatcher = Dispatcher::new(
            CommandRegistry::new(vec![CommandRegistration::new(
                metadata,
                Arc::new(SecretEchoHandler),
            )])
            .expect("registry"),
        )
        .with_guard_evaluator(Arc::new(LeakingGuard));
        let mut request = request("fixture.secret-guard");
        request.secret_bytes = Some(1);
        let error = dispatcher
            .preflight(&request, "test", None)
            .await
            .expect_err("guard rejection");
        assert_eq!(error.code, error_code::AUTHORIZATION_FAILED);
        assert_eq!(error.message, "secret-bearing command failed");
        assert!(error.details.is_none());
    }
}
