//! Windows の開発用 Sidecar の IPC（ADR 0070 §8.2 と §10 の F-IPC。#1976 AC-1）。
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

mod support;

use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use kukuri_cli::{
    dispatcher::Dispatcher,
    protocol::{CommandEffect, CommandMetadata, ProtocolError, SecretInput},
    registry::{
        CommandHandler, CommandOutput, CommandRegistration, CommandRegistry, HandlerContext,
    },
};
use kukuri_game_sdk::{ABI_VERSION, SdkClient, Started, Status, kukuri_sdk_close, sidecar};
use serde_json::{Value, json};
use support::{
    PROFILE, SidecarServer, error_code, finish, poll_once, request, response_for, seen_json, send,
    send_secret,
};

/// 実行した回数を数える handler（切断で自動の再実行が起きないことを確かめる）。
struct Counting(Arc<AtomicUsize>);

#[async_trait]
impl CommandHandler for Counting {
    async fn execute(
        &self,
        _context: HandlerContext<'_>,
        payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let ms = payload["ms"].as_u64().unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(CommandOutput::Unary(json!({"slept": ms})))
    }
}

fn counting_client(count: Arc<AtomicUsize>) -> SdkClient {
    let registration = |name: &str, effect| {
        CommandRegistration::new(
            CommandMetadata {
                name: name.into(),
                effect,
                secret_input: false,
                secret_output: false,
                streaming: false,
                guards: Vec::new(),
                input_schema: json!({"type": "object"}),
                output_schema: json!({"type": "object"}),
            },
            Arc::new(Counting(count.clone())),
        )
    };
    let registry = CommandRegistry::new(vec![
        registration("count.read", CommandEffect::Read),
        registration("count.write", CommandEffect::Write),
    ])
    .unwrap();
    SdkClient::open_with(PROFILE.into(), async move {
        Ok(Started {
            dispatcher: Dispatcher::new(registry),
            session: None,
        })
    })
    .unwrap()
}

/// pipe へ hello だけを送り、返事を読む。
fn raw_hello(profile_dir: &std::path::Path, hello: Value) -> Value {
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(sidecar::pipe_name(profile_dir))
        .unwrap();
    let frame = hello.to_string();
    pipe.write_all(&(frame.len() as u32).to_le_bytes()).unwrap();
    pipe.write_all(frame.as_bytes()).unwrap();
    let mut len = [0; 4];
    pipe.read_exact(&mut len).unwrap();
    let mut reply = vec![0; u32::from_le_bytes(len) as usize];
    pipe.read_exact(&mut reply).unwrap();
    serde_json::from_slice(&reply).unwrap()
}

#[test]
fn a_different_version_or_profile_is_refused_before_any_handler_runs() {
    let count = Arc::new(AtomicUsize::new(0));
    let server = SidecarServer::start(counting_client(count.clone()));
    let dir = server.profile_dir.path();
    // 最初の接続は fixture の handle が持っているので、いったん切って hello を直接送る。
    finish(server.handle);
    let mismatch = raw_hello(
        dir,
        json!({"op": "hello", "abi_version": ABI_VERSION + 1, "profile": PROFILE, "profile_dir": dir}),
    );
    assert_eq!(mismatch["status"], Status::AbiMismatch as i32);
    assert_eq!(mismatch["code"], "protocol_mismatch");
    let other_profile = raw_hello(
        dir,
        json!({"op": "hello", "abi_version": ABI_VERSION, "profile": "other", "profile_dir": dir}),
    );
    assert_eq!(other_profile["code"], "authorization_failed");
    let other_dir = tempfile::tempdir().unwrap();
    let other_dir_reply = raw_hello(
        dir,
        json!({"op": "hello", "abi_version": ABI_VERSION, "profile": PROFILE, "profile_dir": other_dir.path()}),
    );
    assert_eq!(other_dir_reply["code"], "authorization_failed");
    assert_eq!(count.load(Ordering::SeqCst), 0, "no handler ran");

    // 正しい hello の後は要求が handler まで届く。
    let handle = SidecarServer::connect(dir);
    assert_eq!(send(handle, &request("ok", "count.read", json!({}))), 0);
    assert_eq!(
        response_for(handle, "ok", Duration::from_secs(5))["ok"],
        true
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    finish(handle);
}

#[test]
fn a_lost_connection_ends_reads_and_writes_without_running_them_again() {
    let count = Arc::new(AtomicUsize::new(0));
    let server = SidecarServer::start(counting_client(count.clone()));
    let handle = server.handle;
    assert_eq!(
        send(handle, &request("read", "count.read", json!({"ms": 1000}))),
        0
    );
    assert_eq!(
        send(
            handle,
            &request("write", "count.write", json!({"ms": 1000}))
        ),
        0
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(unsafe { kukuri_sdk_close(handle) }, Status::Ok as i32);
    assert_eq!(
        error_code(&response_for(handle, "read", Duration::from_secs(1))),
        "interrupted"
    );
    assert_eq!(
        error_code(&response_for(handle, "write", Duration::from_secs(1))),
        "operation_outcome_unknown"
    );
    finish(handle);

    // 接続し直しても、前の接続の応答は届かず、要求も実行し直さない。
    std::thread::sleep(Duration::from_millis(1200));
    let again = SidecarServer::connect(server.profile_dir.path());
    std::thread::sleep(Duration::from_millis(100));
    for item in poll_once(again, 256).unwrap() {
        assert_ne!(item.kind, 1, "a stale response arrived: {}", item.json);
    }
    assert_eq!(count.load(Ordering::SeqCst), 2, "each request ran once");
    finish(again);
}

fn netstat_listens(pid: u32) -> Vec<String> {
    let output = Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("LISTENING") && line.trim_end().ends_with(&format!(" {pid}")))
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_sidecar_binary_listens_on_no_tcp_port_and_logs_no_secret() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kukuri-sdk-sidecar"))
        .args(["run", "--profile", PROFILE, "--profile-dir"])
        .arg(dir.path())
        .env_remove("KUKURI_APP_DATA_DIR")
        .env_remove("KUKURI_INSTANCE")
        .env_remove("KUKURI_DISCOVERY_SEEDS")
        .env_remove("KUKURI_ADVERTISE_PORT")
        .env("KUKURI_DISABLE_KEYRING", "1")
        .env("KUKURI_DISCOVERY_MODE", "static_peer")
        .env("KUKURI_BIND_ADDR", "127.0.0.1:0")
        .env("KUKURI_ADVERTISE_HOST", "127.0.0.1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut first = String::new();
    stderr.read_line(&mut first).unwrap();
    assert_eq!(first.trim(), "ready");

    let handle = SidecarServer::connect(dir.path());
    // 同意して runtime（node）を動かしてから、秘密を扱う。
    let call = |id: &str, command: &str, payload: Value| {
        assert_eq!(send(handle, &request(id, command, payload)), 0);
        let response = response_for(handle, id, Duration::from_secs(60));
        assert_eq!(response["ok"], true, "{command}: {}", response["error"]);
        response["data"].clone()
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while call("startup", "get_desktop_startup_status", json!({}))["status"] == "initializing" {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = call("consent-status", "get_app_consent_status", json!({}));
    let documents: Vec<Value> = status["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|doc| json!({"slug": doc["slug"], "version": doc["currentVersion"]}))
        .collect();
    call(
        "consent",
        "accept_app_consents",
        json!({"documents": documents, "language": "ja", "age_attested": true}),
    );
    let passphrase = "sidecar-passphrase-sentinel";
    let mut export =
        serde_json::from_slice::<Value>(&request("export", "export_account_key", json!({})))
            .unwrap();
    export["accepts_secret_output"] = json!(true);
    assert_eq!(
        send_secret(handle, export.to_string().as_bytes(), passphrase.as_bytes()),
        0
    );
    assert_eq!(
        response_for(handle, "export", Duration::from_secs(60))["ok"],
        true
    );
    poll_once(handle, 256).unwrap();
    for raw in seen_json(handle) {
        assert!(!String::from_utf8_lossy(&raw).contains("sentinel"));
    }

    // 制御の経路は named pipe だけで、TCP の listen は無い（P2P は UDP の QUIC）。
    assert_eq!(netstat_listens(child.id()), Vec::<String>::new());

    // 接続中に別の client が来ても、Sidecar が持つ profile を FFI で開いても、二重の所有として断る（F-PRO-5）。
    let second = SidecarServer::connect(dir.path());
    assert_eq!(
        send(second, &request("second", "client.status", json!({}))),
        0
    );
    assert_eq!(
        error_code(&response_for(second, "second", Duration::from_secs(10))),
        "profile_in_use"
    );
    finish(second);
    let ffi = support::open_real(dir.path());
    assert_eq!(send(ffi, &request("ffi", "client.status", json!({}))), 0);
    assert_eq!(
        error_code(&response_for(ffi, "ffi", Duration::from_secs(30))),
        "profile_in_use"
    );
    finish(ffi);

    finish(handle);
    let stopped = Command::new(env!("CARGO_BIN_EXE_kukuri-sdk-sidecar"))
        .args(["stop", "--profile", PROFILE, "--profile-dir"])
        .arg(dir.path())
        .status()
        .unwrap();
    assert!(stopped.success());
    let deadline = Instant::now() + Duration::from_secs(30);
    let exit = loop {
        if let Some(exit) = child.try_wait().unwrap() {
            break exit;
        }
        assert!(Instant::now() < deadline, "the sidecar did not stop");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(exit.success());
    let mut rest = String::new();
    stderr.read_to_string(&mut rest).unwrap();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut rest)
        .unwrap();
    assert!(!rest.contains("sentinel"), "{rest}");
}
