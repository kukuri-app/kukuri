// Rust 1.98 needs more query depth for the layout of `daemon::run()`'s async body.
#![recursion_limit = "256"]

use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use kukuri_cli::protocol::PROTOCOL_VERSION;
#[cfg(target_os = "linux")]
use kukuri_cli::protocol::{
    MAX_FRAME_BYTES, ProtocolError, RequestEnvelope, ResponseEnvelope, SecretInput, error_code,
    exit_code_for,
};
use kukuri_desktop_runtime::{
    AGE_ATTESTATION_VERSION, APP_LEGAL_DOCUMENTS, AgeAttestationRecord, AppBuildProfile,
    AppConsentDocumentRecord, AppConsentStore, ProfileError, ProfileLease, app_consent_satisfied,
    current_unix_seconds, load_app_consent_store, resolve_cli_profile, save_app_consent_store,
};

#[cfg(target_os = "linux")]
mod daemon;

#[derive(Parser)]
#[command(name = "kukuri-cli", version)]
struct Cli {
    /// CLI専用profile。KUKURI_INSTANCEと同じselectorとして扱う。
    #[arg(long)]
    profile: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Daemon(DaemonArgs),
    Consent(ConsentArgs),
    /// daemonへversion付きrequestを送る。
    Call(CallArgs),
}

#[derive(Args)]
struct CallArgs {
    /// registryに登録されたcommand名。
    command: String,
    /// JSON request file。`-`はstdin。
    #[arg(long, conflicts_with = "input_fd")]
    input: Option<String>,
    /// JSON requestを読むfile descriptor。
    #[arg(long)]
    input_fd: Option<u32>,
    /// secretを読む0600 file。`-`はstdin。
    #[arg(long, conflicts_with = "secret_input_fd")]
    secret_input: Option<String>,
    /// secretを読むfile descriptor。
    #[arg(long)]
    secret_input_fd: Option<u32>,
    /// secret responseを書き込む新規0600 file。
    #[arg(long, conflicts_with = "secret_output_fd")]
    secret_output: Option<PathBuf>,
    /// secret responseを書き込むfile descriptor。
    #[arg(long)]
    secret_output_fd: Option<u32>,
    #[arg(long)]
    timeout_ms: Option<u64>,
    #[arg(long, default_value_t = PROTOCOL_VERSION, hide = true)]
    protocol_version: u32,
}

#[derive(Args)]
struct DaemonArgs {
    #[command(subcommand)]
    command: DaemonCommand,
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// foregroundで常駐プロセスを実行する。
    Run,
    /// systemd user instanceを開始する。
    Start,
    /// systemd user instanceを停止する。
    Stop,
    /// systemd user instanceの状態を表示する。
    Status,
}

#[derive(Args)]
struct ConsentArgs {
    #[command(subcommand)]
    command: ConsentCommand,
}

#[derive(Subcommand)]
enum ConsentCommand {
    Status,
    Accept(ConsentAcceptArgs),
}

#[derive(Args)]
struct ConsentAcceptArgs {
    /// 現行利用規約とプライバシーポリシーへの同意を明示する。
    #[arg(long)]
    accept_documents: bool,
    /// 18歳以上であることの自己申告を明示する。
    #[arg(long)]
    age_confirmed: bool,
    #[arg(long, default_value = "ja")]
    language: String,
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                std::process::exit(0);
            }
            #[cfg(target_os = "linux")]
            {
                let response = ResponseEnvelope::failure_parts(
                    "",
                    "",
                    "",
                    ProtocolError::new(error_code::INVALID_REQUEST, error.to_string()),
                );
                println!(
                    "{}",
                    serde_json::to_string(&response).expect("error envelope")
                );
                std::process::exit(2);
            }
            #[cfg(not(target_os = "linux"))]
            error.exit();
        }
    };
    if let Err(error) = run(cli) {
        if !error.reported {
            eprintln!("{}: {}", error.code, error.message);
        }
        std::process::exit(error.exit_code);
    }
}

fn run(cli: Cli) -> Result<(), CliError> {
    let custom_app_data_selected = std::env::var("KUKURI_APP_DATA_DIR")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    let profile = match resolve_profile(cli.profile.as_deref()) {
        Ok(profile) => profile,
        Err(error) => {
            #[cfg(target_os = "linux")]
            if let Command::Call(args) = &cli.command {
                return emit_local_failure(
                    &args.command,
                    "",
                    cli.profile.as_deref().unwrap_or("default"),
                    ProtocolError::new(error_code::VALIDATION_FAILED, error.to_string()),
                );
            }
            return Err(CliError::from_profile(error));
        }
    };
    match cli.command {
        Command::Consent(args) => run_consent(profile, args.command),
        Command::Daemon(args) => run_daemon(profile, args.command, custom_app_data_selected),
        Command::Call(args) => run_call(profile, args),
    }
}

fn resolve_profile(
    argument_profile: Option<&str>,
) -> Result<kukuri_desktop_runtime::ClientProfile, ProfileError> {
    let environment_instance = std::env::var("KUKURI_INSTANCE").ok();
    let environment_app_data_dir = std::env::var("KUKURI_APP_DATA_DIR").ok();
    let xdg_data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve_cli_profile(
        argument_profile,
        environment_instance.as_deref(),
        environment_app_data_dir.as_deref(),
        xdg_data_home.as_deref(),
        home.as_deref(),
    )
}

fn run_consent(
    profile: kukuri_desktop_runtime::ClientProfile,
    command: ConsentCommand,
) -> Result<(), CliError> {
    let lease = ProfileLease::acquire(profile).map_err(CliError::from_profile)?;
    let consent_db_path = lease.profile().app_data_dir.join("kukuri.db");
    match command {
        ConsentCommand::Status => {
            // 同意の file は daemon の外で読み書きする。native の保存先の future は I/O を待たずに終わる。
            let store = n0_future::future::block_on(load_app_consent_store(&consent_db_path));
            println!(
                "{}",
                if app_consent_satisfied(&store) {
                    "satisfied"
                } else {
                    "consent_required"
                }
            );
            Ok(())
        }
        ConsentCommand::Accept(args) => accept_consents(&consent_db_path, args),
    }
}

fn accept_consents(path: &Path, args: ConsentAcceptArgs) -> Result<(), CliError> {
    if !args.accept_documents || !args.age_confirmed {
        return Err(CliError::new(
            "consent_confirmation_required",
            "--accept-documents and --age-confirmed are both required",
            2,
        ));
    }
    let language = args.language.trim();
    if language.is_empty() {
        return Err(CliError::new(
            "invalid_consent_language",
            "--language must not be empty",
            2,
        ));
    }
    let accepted_at = current_unix_seconds();
    let app_version = env!("CARGO_PKG_VERSION").to_string();
    let build_profile = Some(AppBuildProfile::current().as_str().to_string());
    let store = AppConsentStore {
        records: APP_LEGAL_DOCUMENTS
            .iter()
            .map(|(slug, version)| AppConsentDocumentRecord {
                slug: (*slug).to_string(),
                version: *version,
                accepted_at,
                language: language.to_string(),
                app_version: app_version.clone(),
                build_profile: build_profile.clone(),
            })
            .collect(),
        age_attestations: vec![AgeAttestationRecord {
            version: AGE_ATTESTATION_VERSION,
            attested_at: accepted_at,
            language: language.to_string(),
            app_version,
            build_profile,
        }],
    };
    n0_future::future::block_on(save_app_consent_store(path, &store))
        .map_err(|error| CliError::new("consent_persist_failed", error, 1))?;
    println!("accepted");
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_daemon(
    profile: kukuri_desktop_runtime::ClientProfile,
    command: DaemonCommand,
    custom_app_data_selected: bool,
) -> Result<(), CliError> {
    if custom_app_data_selected && !matches!(&command, DaemonCommand::Run) {
        return Err(CliError::new(
            "custom_profile_systemd_unsupported",
            "KUKURI_APP_DATA_DIR can be used only with `daemon run`; systemd actions require a named profile",
            2,
        ));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::new("runtime_start_failed", error.to_string(), 1))?;
    runtime.block_on(daemon::run(profile, command))
}

#[cfg(not(target_os = "linux"))]
fn run_daemon(
    _profile: kukuri_desktop_runtime::ClientProfile,
    _command: DaemonCommand,
    _custom_app_data_selected: bool,
) -> Result<(), CliError> {
    Err(CliError::new(
        "unsupported_platform",
        "kukuri daemon is supported only on Linux",
        2,
    ))
}

#[derive(Debug)]
struct CliError {
    code: &'static str,
    message: String,
    exit_code: i32,
    reported: bool,
}

impl CliError {
    fn new(code: &'static str, message: impl Into<String>, exit_code: i32) -> Self {
        Self {
            code,
            message: message.into(),
            exit_code,
            reported: false,
        }
    }

    #[cfg(target_os = "linux")]
    fn reported(exit_code: i32) -> Self {
        Self {
            code: "protocol_error",
            message: String::new(),
            exit_code,
            reported: true,
        }
    }

    #[cfg(target_os = "linux")]
    fn from_protocol(error: ProtocolError) -> Self {
        let exit_code = exit_code_for(&error.code);
        Self {
            code: "protocol_error",
            message: error.to_string(),
            exit_code,
            reported: false,
        }
    }

    fn from_profile(error: ProfileError) -> Self {
        Self::new(error.code(), error.to_string(), 1)
    }
}

#[cfg(target_os = "linux")]
fn run_call(
    profile: kukuri_desktop_runtime::ClientProfile,
    args: CallArgs,
) -> Result<(), CliError> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            let error = ProtocolError::new(error_code::INTERNAL_ERROR, "runtime startup failed");
            let _ = emit_local_failure(&args.command, &request_id, &profile.name, error);
            CliError::reported(1)
        })?;
    runtime.block_on(run_call_async(profile, args))
}

#[cfg(not(target_os = "linux"))]
fn run_call(
    _profile: kukuri_desktop_runtime::ClientProfile,
    _args: CallArgs,
) -> Result<(), CliError> {
    Err(CliError::new(
        "unsupported_platform",
        "kukuri daemon is supported only on Linux",
        2,
    ))
}

#[cfg(target_os = "linux")]
async fn run_call_async(
    profile: kukuri_desktop_runtime::ClientProfile,
    args: CallArgs,
) -> Result<(), CliError> {
    use kukuri_cli::client::ClientSession;
    use serde_json::json;
    use uuid::Uuid;

    let request_id = Uuid::new_v4().to_string();
    if args.secret_output_fd.is_some_and(|fd| fd <= 2) {
        return emit_local_failure(
            &args.command,
            &request_id,
            &profile.name,
            ProtocolError::new(
                error_code::VALIDATION_FAILED,
                "secret output FD must not be stdin, stdout, or stderr",
            ),
        );
    }
    if args.input.as_deref() == Some("-") && args.secret_input.as_deref() == Some("-") {
        return emit_local_failure(
            &args.command,
            &request_id,
            &profile.name,
            ProtocolError::new(
                error_code::VALIDATION_FAILED,
                "stdin cannot be both JSON input and secret input",
            ),
        );
    }
    let payload = match read_json_input(args.input.as_deref(), args.input_fd) {
        Ok(payload) => payload,
        Err(error) => return emit_local_failure(&args.command, &request_id, &profile.name, error),
    };
    let secret = match read_secret_input(args.secret_input.as_deref(), args.secret_input_fd) {
        Ok(secret) => secret.map(SecretInput::new),
        Err(error) => return emit_local_failure(&args.command, &request_id, &profile.name, error),
    };
    let request = RequestEnvelope {
        protocol_version: args.protocol_version,
        request_id: request_id.clone(),
        command: args.command.clone(),
        profile: profile.name.clone(),
        payload,
        timeout_ms: args.timeout_ms,
        secret_bytes: secret.as_ref().map(|secret| secret.len() as u64),
        accepts_secret_output: args.secret_output.is_some() || args.secret_output_fd.is_some(),
    };
    let timeout_ms = match request.timeout_ms() {
        Ok(timeout) => timeout,
        Err(error) => return emit_local_failure(&args.command, &request_id, &profile.name, error),
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let mut session = match ClientSession::connect(&profile, &request, secret.as_ref()).await {
        Ok(session) => session,
        Err(error) => return emit_local_failure(&args.command, &request_id, &profile.name, error),
    };
    loop {
        let next = tokio::select! {
            result = tokio::time::timeout_at(deadline, session.next()) => match result {
                Ok(result) => result,
                Err(_) => Err(ProtocolError::new(error_code::TIMEOUT, "command timed out")),
            },
            result = tokio::signal::ctrl_c() => {
                let error = match result {
                    Ok(()) => ProtocolError::new(error_code::INTERRUPTED, "command was interrupted"),
                    Err(_) => ProtocolError::new(error_code::INTERNAL_ERROR, "failed to wait for interrupt"),
                };
                return emit_local_failure(&args.command, &request_id, &profile.name, error);
            }
        };
        let next = match next {
            Ok(next) => next,
            Err(error) => {
                return emit_local_failure(&args.command, &request_id, &profile.name, error);
            }
        };
        let Some((response, secret_output)) = next else {
            return emit_local_failure(
                &args.command,
                &request_id,
                &profile.name,
                ProtocolError::new(
                    error_code::NETWORK_UNAVAILABLE,
                    "daemon closed the connection before a complete response",
                ),
            );
        };
        if let Some(secret_output) = secret_output
            && let Err(error) = write_secret_output(
                secret_output.expose(),
                args.secret_output.as_deref(),
                args.secret_output_fd,
            )
        {
            return emit_local_failure(&args.command, &request_id, &profile.name, error);
        }
        println!(
            "{}",
            serde_json::to_string(&response).unwrap_or_else(|_| json!({
                "schema_version": 1,
                "ok": false,
                "command": args.command,
                "request_id": request_id,
                "profile": profile.name,
                "error": {"code": "internal_error", "message": "failed to encode response"}
            })
            .to_string())
        );
        if !response.ok {
            return Err(CliError::reported(exit_code_for(
                response.error_code().unwrap_or(error_code::INTERNAL_ERROR),
            )));
        }
        if !response.more {
            return Ok(());
        }
    }
}

#[cfg(target_os = "linux")]
fn read_json_input(
    path: Option<&str>,
    fd: Option<u32>,
) -> Result<serde_json::Value, ProtocolError> {
    let bytes = match (path, fd) {
        (None, None) => return Ok(serde_json::json!({})),
        (Some("-"), None) => read_stdin()?,
        (Some(path), None) => read_owner_only_file(Path::new(path), "JSON input")?,
        (None, Some(fd)) => read_fd(fd, "JSON")?,
        _ => unreachable!("clap rejects conflicting input sources"),
    };
    ensure_input_bound(&bytes, "JSON input")?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| ProtocolError::new(error_code::INVALID_REQUEST, "input is not valid JSON"))?;
    if !value.is_object() {
        return Err(ProtocolError::new(
            error_code::VALIDATION_FAILED,
            "input JSON must be an object",
        ));
    }
    Ok(value)
}

#[cfg(target_os = "linux")]
fn read_secret_input(
    path: Option<&str>,
    fd: Option<u32>,
) -> Result<Option<Vec<u8>>, ProtocolError> {
    match (path, fd) {
        (None, None) => Ok(None),
        (Some("-"), None) => read_stdin().and_then(|bytes| {
            ensure_input_bound(&bytes, "secret input")?;
            Ok(Some(bytes))
        }),
        (Some(path), None) => read_owner_only_file(Path::new(path), "secret input").map(Some),
        (None, Some(fd)) => read_fd(fd, "secret").and_then(|bytes| {
            ensure_input_bound(&bytes, "secret input")?;
            Ok(Some(bytes))
        }),
        _ => unreachable!("clap rejects conflicting secret sources"),
    }
}

#[cfg(target_os = "linux")]
fn ensure_input_bound(bytes: &[u8], label: &str) -> Result<(), ProtocolError> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::new(
            error_code::VALIDATION_FAILED,
            format!("{label} exceeds the supported size"),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn read_stdin() -> Result<Vec<u8>, ProtocolError> {
    read_bounded(std::io::stdin(), "stdin")
}

#[cfg(target_os = "linux")]
fn read_fd(fd: u32, label: &str) -> Result<Vec<u8>, ProtocolError> {
    let file = std::fs::File::open(format!("/proc/self/fd/{fd}")).map_err(|_| {
        ProtocolError::new(
            error_code::VALIDATION_FAILED,
            format!("failed to read {label} file descriptor"),
        )
    })?;
    read_bounded(file, label)
}

#[cfg(target_os = "linux")]
fn read_owner_only_file(path: &Path, label: &str) -> Result<Vec<u8>, ProtocolError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path).map_err(|_| {
        ProtocolError::new(
            error_code::VALIDATION_FAILED,
            format!("failed to inspect {label} file"),
        )
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(ProtocolError::new(
            error_code::VALIDATION_FAILED,
            format!("{label} file must be a regular owner-only file"),
        ));
    }
    let file = std::fs::File::open(path).map_err(|_| {
        ProtocolError::new(
            error_code::VALIDATION_FAILED,
            format!("failed to read {label} file"),
        )
    })?;
    read_bounded(file, label)
}

#[cfg(target_os = "linux")]
fn read_bounded(reader: impl std::io::Read, label: &str) -> Result<Vec<u8>, ProtocolError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader
        .take((MAX_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            ProtocolError::new(
                error_code::VALIDATION_FAILED,
                format!("failed to read {label}"),
            )
        })?;
    ensure_input_bound(&bytes, label)?;
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn write_secret_output(
    bytes: &[u8],
    path: Option<&Path>,
    fd: Option<u32>,
) -> Result<(), ProtocolError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    match (path, fd) {
        (Some(path), None) => {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .map_err(|_| {
                    ProtocolError::new(
                        error_code::ACTION_REQUIRED,
                        "failed to create secret output file",
                    )
                })?;
            file.write_all(bytes).map_err(|_| {
                ProtocolError::new(error_code::INTERNAL_ERROR, "failed to write secret output")
            })
        }
        (None, Some(fd)) => {
            if fd <= 2 {
                return Err(ProtocolError::new(
                    error_code::VALIDATION_FAILED,
                    "secret output FD must not be stdin, stdout, or stderr",
                ));
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .open(format!("/proc/self/fd/{fd}"))
                .map_err(|_| {
                    ProtocolError::new(error_code::ACTION_REQUIRED, "secret output FD is invalid")
                })?;
            file.write_all(bytes).map_err(|_| {
                ProtocolError::new(error_code::INTERNAL_ERROR, "failed to write secret output")
            })
        }
        (None, None) => Err(ProtocolError::new(
            error_code::ACTION_REQUIRED,
            "secret response requires --secret-output or --secret-output-fd",
        )),
        _ => unreachable!("clap rejects conflicting secret sinks"),
    }
}

#[cfg(target_os = "linux")]
fn emit_local_failure(
    command: &str,
    request_id: &str,
    profile: &str,
    error: ProtocolError,
) -> Result<(), CliError> {
    let exit_code = exit_code_for(&error.code);
    let response = ResponseEnvelope::failure_parts(command, request_id, profile, error);
    println!(
        "{}",
        serde_json::to_string(&response).unwrap_or_else(|_| {
            "{\"schema_version\":1,\"ok\":false,\"command\":\"\",\"request_id\":\"\",\"profile\":\"\",\"error\":{\"code\":\"internal_error\",\"message\":\"failed to encode error\"}}".to_string()
        })
    );
    Err(CliError::reported(exit_code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_requires_both_explicit_confirmations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = accept_consents(
            &dir.path().join("kukuri.db"),
            ConsentAcceptArgs {
                accept_documents: true,
                age_confirmed: false,
                language: "ja".to_string(),
            },
        )
        .expect_err("age confirmation");
        assert_eq!(error.code, "consent_confirmation_required");
    }

    #[test]
    fn consent_acceptance_writes_current_documents_and_age() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("kukuri.db");
        accept_consents(
            &path,
            ConsentAcceptArgs {
                accept_documents: true,
                age_confirmed: true,
                language: "ja".to_string(),
            },
        )
        .expect("accept consent");
        assert!(app_consent_satisfied(&n0_future::future::block_on(
            load_app_consent_store(&path)
        )));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn secret_output_rejects_standard_file_descriptors_without_writing() {
        let sentinel = b"secret-standard-fd-sentinel";
        for fd in 0..=2 {
            let error = write_secret_output(sentinel, None, Some(fd)).expect_err("standard FD");
            assert_eq!(error.code, error_code::VALIDATION_FAILED);
            assert!(!error.to_string().contains("secret-standard-fd-sentinel"));
        }
    }
}
