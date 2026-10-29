//! Editor の利用を想定した Sidecar の接続の寿命と複数の profile（ADR 0070 §8.2 と §10 の F-IPC-3。#1976 AC-2）。
//! 実際の `kukuri-sdk-sidecar.exe` を起動し、keyring を使わず loopback だけで runtime を動かす。
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

mod support;

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use kukuri_game_sdk::KukuriHandle;
use serde_json::{Value, json};
use support::{PROFILE, SidecarServer, error_code, finish, request, response_for, send};

struct Sidecar {
    child: Child,
}

impl Sidecar {
    fn start(dir: &Path) -> Self {
        let mut child = sidecar_command("run", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut first = String::new();
        BufReader::new(child.stderr.as_mut().unwrap())
            .read_line(&mut first)
            .unwrap();
        assert_eq!(first.trim(), "ready");
        Self { child }
    }

    /// 起動を試みて、終わるまで待つ（同じ profile の二つ目は起動できない）。
    fn run_to_exit(dir: &Path) -> ExitStatus {
        let mut child = sidecar_command("run", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_exit(&mut child)
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn stop(mut self, dir: &Path) -> ExitStatus {
        let stopped = sidecar_command("stop", dir).output().unwrap();
        assert!(
            stopped.status.success(),
            "stop failed: {}",
            String::from_utf8_lossy(&stopped.stderr)
        );
        wait_exit(&mut self.child)
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn sidecar_command(command: &str, dir: &Path) -> Command {
    let mut process = Command::new(env!("CARGO_BIN_EXE_kukuri-sdk-sidecar"));
    process
        .args([command, "--profile", PROFILE, "--profile-dir"])
        .arg(dir)
        .env_remove("KUKURI_APP_DATA_DIR")
        .env_remove("KUKURI_INSTANCE")
        .env_remove("KUKURI_DISCOVERY_SEEDS")
        .env_remove("KUKURI_ADVERTISE_PORT")
        .env("KUKURI_DISABLE_KEYRING", "1")
        .env("KUKURI_DISCOVERY_MODE", "static_peer")
        .env("KUKURI_BIND_ADDR", "127.0.0.1:0")
        .env("KUKURI_ADVERTISE_HOST", "127.0.0.1");
    process
}

fn wait_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "the sidecar did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn call(handle: *mut KukuriHandle, id: &str, command: &str, payload: Value) -> Value {
    assert_eq!(send(handle, &request(id, command, payload)), 0, "{command}");
    let response = response_for(handle, id, Duration::from_secs(60));
    assert_eq!(response["ok"], true, "{command}: {}", response["error"]);
    response["data"].clone()
}

/// 同意して runtime を動かす（初めての profile だけ）。
fn ready(handle: *mut KukuriHandle) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = call(handle, "startup", "get_desktop_startup_status", json!({}));
        match status["status"].as_str() {
            Some("ready") => return,
            Some("consent_required") => {
                let consent = call(
                    handle,
                    "consent-status",
                    "get_app_consent_status",
                    json!({}),
                );
                let documents: Vec<Value> = consent["documents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|doc| json!({"slug": doc["slug"], "version": doc["currentVersion"]}))
                    .collect();
                call(
                    handle,
                    "consent",
                    "accept_app_consents",
                    json!({"documents": documents, "language": "ja", "age_attested": true}),
                );
            }
            _ => {}
        }
        assert!(Instant::now() < deadline, "the runtime did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 公開鍵と P2P の endpoint id。
fn identity(handle: *mut KukuriHandle, id: &str) -> (Value, Value) {
    let status = call(handle, id, "get_sync_status", json!({}));
    (
        status["local_author_pubkey"].clone(),
        status["discovery"]["local_endpoint_id"].clone(),
    )
}

/// process の handle と thread の数（PowerShell の Get-Process）。
fn resources(pid: u32) -> (u64, u64) {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("$p = Get-Process -Id {pid}; \"$($p.HandleCount) $($p.Threads.Count)\""),
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let mut numbers = text.split_whitespace().map(|n| n.parse::<u64>().unwrap());
    (numbers.next().unwrap(), numbers.next().unwrap())
}

#[test]
fn repeated_attach_and_detach_keep_the_identity_without_accumulating_resources() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = Sidecar::start(dir.path());
    let first = SidecarServer::connect(dir.path());
    ready(first);
    let identity_before = identity(first, "before");
    assert!(identity_before.1.as_str().is_some_and(|id| !id.is_empty()));
    finish(first);

    // Editor の Play と Stop を想定して接続と切断を繰り返す。毎回、要求を残したまま切る回も混ぜる。
    let mut samples = Vec::new();
    for round in 0..30 {
        let handle = SidecarServer::connect(dir.path());
        assert_eq!(identity(handle, &format!("id{round}")), identity_before);
        assert_eq!(
            send(
                handle,
                &request(&format!("left{round}"), "get_sync_status", json!({}))
            ),
            0
        );
        finish(handle);
        if round >= 10 {
            samples.push(resources(sidecar.pid()));
        }
    }
    let (first_handles, first_threads) = samples[0];
    let (last_handles, last_threads) = *samples.last().unwrap();
    // 準備の 10 回の後、20 回の接続と切断で handle と thread が増え続けない（少しの揺れは許す）。
    assert!(
        last_handles <= first_handles + 20,
        "handles grew: {samples:?}"
    );
    assert!(
        last_threads <= first_threads + 2,
        "threads grew: {samples:?}"
    );
    assert!(sidecar.stop(dir.path()).success());
}

#[test]
fn two_profiles_run_side_by_side_and_one_profile_cannot_start_twice() {
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (Sidecar::start(dir_a.path()), Sidecar::start(dir_b.path()));
    let (handle_a, handle_b) = (
        SidecarServer::connect(dir_a.path()),
        SidecarServer::connect(dir_b.path()),
    );
    ready(handle_a);
    ready(handle_b);
    let (key_a, endpoint_a) = identity(handle_a, "a");
    let (key_b, endpoint_b) = identity(handle_b, "b");
    assert_ne!(key_a, key_b);
    assert_ne!(endpoint_a, endpoint_b);

    // 同じ profile の二つ目の Sidecar は起動せずに終わり、動いている Sidecar は使い続けられる。
    let second = Sidecar::run_to_exit(dir_a.path());
    assert_eq!(second.code(), Some(3));
    assert_eq!(identity(handle_a, "a-again"), (key_a, endpoint_a));
    finish(handle_a);
    finish(handle_b);
    assert!(a.stop(dir_a.path()).success());
    assert!(b.stop(dir_b.path()).success());
}

#[test]
fn stopping_while_attached_ends_requests_and_a_restart_restores_the_identity() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = Sidecar::start(dir.path());
    let handle = SidecarServer::connect(dir.path());
    ready(handle);
    let before = identity(handle, "before");

    // 接続中でも stop で止まる。止まった後の要求は Sidecar に届かない。
    assert!(sidecar.stop(dir.path()).success());
    assert_eq!(
        send(handle, &request("after-stop", "get_sync_status", json!({}))),
        0
    );
    let lost = response_for(handle, "after-stop", Duration::from_secs(10));
    assert_eq!(lost["ok"], false);
    assert!(
        ["daemon_unavailable", "operation_outcome_unknown"].contains(&error_code(&lost)),
        "{lost}"
    );
    finish(handle);

    // 起動し直すと、同じ profile の同じ公開鍵と endpoint id に戻る。
    let restarted = Sidecar::start(dir.path());
    let again = SidecarServer::connect(dir.path());
    ready(again);
    assert_eq!(identity(again, "after"), before);
    finish(again);
    assert!(restarted.stop(dir.path()).success());
}

#[test]
fn a_client_that_never_says_hello_does_not_lock_others_out() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = Sidecar::start(dir.path());
    // pipe を開いたまま hello を送らない client。
    let silent = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(kukuri_game_sdk::sidecar::pipe_name(dir.path()))
        .unwrap();
    let started = Instant::now();
    let handle = loop {
        let handle = SidecarServer::connect(dir.path());
        assert_eq!(
            send(handle, &request("status", "client.status", json!({}))),
            0
        );
        let response = response_for(handle, "status", Duration::from_secs(10));
        if response["ok"] == true {
            break handle;
        }
        assert_eq!(error_code(&response), "profile_in_use");
        finish(handle);
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "still locked out"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    finish(handle);
    drop(silent);
    assert!(sidecar.stop(dir.path()).success());
}

#[test]
fn clients_that_open_and_close_at_once_and_refused_clients_do_not_break_the_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = Sidecar::start(dir.path());
    let name = kukuri_game_sdk::sidecar::pipe_name(dir.path());
    // 開いてすぐ閉じる client（Editor の強制終了など）を繰り返しても、Sidecar は止まらない。
    for _ in 0..300 {
        if let Ok(pipe) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
        {
            drop(pipe);
        }
    }
    // 直前に閉じた接続を片付けている間は使用中で断られうるが、Sidecar は動き続け、すぐ接続できるようになる。
    let started = Instant::now();
    let handle = loop {
        let handle = SidecarServer::connect(dir.path());
        assert_eq!(
            send(handle, &request("alive", "client.status", json!({}))),
            0
        );
        let response = response_for(handle, "alive", Duration::from_secs(10));
        if response["ok"] == true {
            break handle;
        }
        assert_eq!(error_code(&response), "profile_in_use", "{response}");
        finish(handle);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the sidecar stopped answering"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        call(handle, "alive-again", "client.status", json!({}))["profile"],
        PROFILE
    );
    // 接続中に来た client は、毎回 profile_in_use の返事を受け取る（返事を捨てずに届ける）。
    for round in 0..30 {
        let busy = SidecarServer::connect(dir.path());
        assert_eq!(send(busy, &request("busy", "client.status", json!({}))), 0);
        assert_eq!(
            error_code(&response_for(busy, "busy", Duration::from_secs(10))),
            "profile_in_use",
            "round {round}"
        );
        finish(busy);
    }
    finish(handle);
    assert!(sidecar.stop(dir.path()).success());
}
