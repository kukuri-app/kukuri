//! C ABI の適合試験（ADR 0070 §10 の F-ABI。#1975 AC-1）。関数は ABI の入口から呼ぶ。
//! 実際の profile を開く試験と、dispatcher に試験用の command を注入する試験がある。
#![allow(clippy::unwrap_used)]

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use kukuri_cli::{
    dispatcher::Dispatcher,
    protocol::{CommandEffect, CommandMetadata, ProtocolError, SecretInput, SecretOutput},
    registry::{
        CommandHandler, CommandOutput, CommandRegistration, CommandRegistry, HandlerContext,
    },
};
use kukuri_game_sdk::{
    ABI_VERSION, KukuriBatch, KukuriHandle, MAX_PENDING, SdkClient, Started, Status,
    kukuri_sdk_batch_release, kukuri_sdk_cancel, kukuri_sdk_close, kukuri_sdk_free,
    kukuri_sdk_open, kukuri_sdk_poll, kukuri_sdk_send,
};
use serde_json::{Value, json};

const PROFILE: &str = "p";

struct Sleep;

#[async_trait]
impl CommandHandler for Sleep {
    async fn execute(
        &self,
        _context: HandlerContext<'_>,
        payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let ms = payload["ms"].as_u64().unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(CommandOutput::Unary(json!({"slept": ms})))
    }
}

struct Panics;

#[async_trait]
impl CommandHandler for Panics {
    async fn execute(
        &self,
        _context: HandlerContext<'_>,
        _payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        panic!("handler panicked");
    }
}

struct Secret;

#[async_trait]
impl CommandHandler for Secret {
    async fn execute(
        &self,
        _context: HandlerContext<'_>,
        _payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        Ok(CommandOutput::Secret {
            data: json!({"exported": true}),
            secret: SecretOutput::new(b"invite-token-sentinel".to_vec()),
        })
    }
}

fn registration(
    name: &str,
    effect: CommandEffect,
    secret_output: bool,
    handler: Arc<dyn CommandHandler>,
) -> CommandRegistration {
    CommandRegistration::new(
        CommandMetadata {
            name: name.into(),
            effect,
            secret_input: false,
            secret_output,
            streaming: false,
            guards: Vec::new(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        },
        handler,
    )
}

/// 試験用の command を持つ dispatcher の handle。
fn fake_handle() -> *mut KukuriHandle {
    let registry = CommandRegistry::new(vec![
        registration("sleep.read", CommandEffect::Read, false, Arc::new(Sleep)),
        registration("sleep.write", CommandEffect::Write, false, Arc::new(Sleep)),
        registration("panic.read", CommandEffect::Read, false, Arc::new(Panics)),
        registration("secret.write", CommandEffect::Write, true, Arc::new(Secret)),
    ])
    .unwrap();
    let client = SdkClient::open_with(PROFILE.into(), async move {
        Ok(Started {
            dispatcher: Dispatcher::new(registry),
            session: None,
        })
    })
    .unwrap();
    KukuriHandle::from_client(client)
}

fn open_real(dir: &std::path::Path) -> *mut KukuriHandle {
    let config = json!({"profile": PROFILE, "profile_dir": dir}).to_string();
    let mut handle = std::ptr::null_mut();
    let status =
        unsafe { kukuri_sdk_open(ABI_VERSION, config.as_ptr(), config.len(), &mut handle) };
    assert_eq!(status, Status::Ok as i32);
    handle
}

fn request(id: &str, command: &str, payload: Value) -> Vec<u8> {
    json!({
        "protocol_version": 1,
        "request_id": id,
        "command": command,
        "profile": PROFILE,
        "payload": payload,
    })
    .to_string()
    .into_bytes()
}

fn send(handle: *mut KukuriHandle, request: &[u8]) -> i32 {
    unsafe { kukuri_sdk_send(handle, request.as_ptr(), request.len(), std::ptr::null(), 0) }
}

#[derive(Debug)]
struct Polled {
    kind: u32,
    json: Value,
    raw: Vec<u8>,
}

/// 1 回 poll して、batch を写してから解放する。
fn poll_once(handle: *mut KukuriHandle, max_items: usize) -> Result<Vec<Polled>, i32> {
    let mut batch: *const KukuriBatch = std::ptr::null();
    let status = unsafe { kukuri_sdk_poll(handle, max_items, 1 << 20, &mut batch) };
    if status != Status::Ok as i32 {
        return Err(status);
    }
    if batch.is_null() {
        return Ok(Vec::new());
    }
    let view = unsafe { &*batch };
    let items = unsafe { std::slice::from_raw_parts(view.items, view.count) };
    let polled = items
        .iter()
        .map(|item| {
            let raw = unsafe { std::slice::from_raw_parts(item.data, item.len) }.to_vec();
            Polled {
                kind: item.kind,
                json: serde_json::from_slice(&raw).unwrap_or(Value::Null),
                raw,
            }
        })
        .collect();
    assert_eq!(
        unsafe { kukuri_sdk_batch_release(handle, batch) },
        Status::Ok as i32
    );
    Ok(polled)
}

/// 先に届いた応答（handle の address と応答）。一度の poll に複数の応答が入るので、探していない応答を残す。
static RECEIVED: Mutex<Vec<(usize, Value)>> = Mutex::new(Vec::new());

/// `request_id` の応答が届くまで poll する。
fn response_for(handle: *mut KukuriHandle, id: &str, within: Duration) -> Value {
    let deadline = Instant::now() + within;
    loop {
        let mut received = RECEIVED.lock().unwrap();
        for item in poll_once(handle, 64).unwrap() {
            if item.kind == 1 {
                received.push((handle as usize, item.json));
            }
        }
        if let Some(index) = received
            .iter()
            .position(|(owner, json)| *owner == handle as usize && json["request_id"] == id)
        {
            return received.swap_remove(index).1;
        }
        drop(received);
        assert!(Instant::now() < deadline, "no response for {id}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn error_code(response: &Value) -> &str {
    response["error"]["code"].as_str().unwrap_or("")
}

fn finish(handle: *mut KukuriHandle) {
    unsafe {
        kukuri_sdk_close(handle);
        kukuri_sdk_free(handle);
    }
}

#[test]
fn a_different_abi_version_creates_no_handle() {
    let dir = tempfile::tempdir().unwrap();
    let config = json!({"profile": PROFILE, "profile_dir": dir.path()}).to_string();
    let mut handle = std::ptr::null_mut();
    let status =
        unsafe { kukuri_sdk_open(ABI_VERSION + 1, config.as_ptr(), config.len(), &mut handle) };
    assert_eq!(status, Status::AbiMismatch as i32);
    assert!(handle.is_null());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "nothing is written"
    );
}

#[test]
fn a_real_profile_starts_answers_and_releases_its_lock_on_close() {
    let dir = tempfile::tempdir().unwrap();
    let handle = open_real(dir.path());
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut started = false;
    while !started {
        started = poll_once(handle, 64)
            .unwrap()
            .iter()
            .any(|item| item.kind == 3 && item.json["type"] == "startup_status_changed");
        assert!(Instant::now() < deadline, "startup did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        send(
            handle,
            &request("status", "get_desktop_startup_status", json!({}))
        ),
        Status::Ok as i32
    );
    let response = response_for(handle, "status", Duration::from_secs(30));
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"]["status"], "consent_required");

    // 同じ profile を同時に開くと、後の handle の要求は所有の衝突で断られる。
    let second = open_real(dir.path());
    assert_eq!(
        send(second, &request("second", "client.status", json!({}))),
        Status::Ok as i32
    );
    assert_eq!(
        error_code(&response_for(second, "second", Duration::from_secs(30))),
        "profile_in_use"
    );
    finish(second);

    assert_eq!(unsafe { kukuri_sdk_close(handle) }, Status::Ok as i32);
    assert_eq!(
        send(handle, &request("late", "client.status", json!({}))),
        Status::Closed as i32
    );

    // close で lock を外したので、free の前でも同じ profile を開ける（閉じた handle は poll できる）。
    let reopened = open_real(dir.path());
    assert_eq!(
        send(reopened, &request("again", "client.status", json!({}))),
        Status::Ok as i32
    );
    assert_eq!(
        response_for(reopened, "again", Duration::from_secs(30))["ok"],
        true
    );
    assert!(poll_once(handle, 64).is_ok());
    unsafe { kukuri_sdk_free(handle) };
    finish(reopened);
}

#[test]
fn responses_follow_their_request_ids_and_duplicates_in_flight_are_rejected() {
    let handle = fake_handle();
    assert_eq!(
        send(handle, &request("a", "sleep.read", json!({"ms": 200}))),
        Status::Ok as i32
    );
    assert_eq!(
        send(handle, &request("b", "sleep.read", json!({"ms": 0}))),
        Status::Ok as i32
    );
    assert_eq!(
        send(handle, &request("a", "sleep.read", json!({"ms": 0}))),
        Status::InvalidArgument as i32
    );
    assert_eq!(
        response_for(handle, "b", Duration::from_secs(5))["data"]["slept"],
        0
    );
    assert_eq!(
        response_for(handle, "a", Duration::from_secs(5))["data"]["slept"],
        200
    );
    // 終わった要求と同じ id は、別の要求として受け付ける。
    assert_eq!(
        send(handle, &request("a", "sleep.read", json!({"ms": 1}))),
        Status::Ok as i32
    );
    assert_eq!(
        response_for(handle, "a", Duration::from_secs(5))["data"]["slept"],
        1
    );
    finish(handle);
}

#[test]
fn a_full_queue_and_an_oversized_request_are_rejected_without_losing_accepted_requests() {
    let handle = fake_handle();
    for index in 0..MAX_PENDING {
        let request = request(&format!("r{index}"), "sleep.read", json!({"ms": 50}));
        assert_eq!(send(handle, &request), Status::Ok as i32);
    }
    assert_eq!(
        send(handle, &request("over", "sleep.read", json!({}))),
        Status::Backpressure as i32
    );
    let oversized = request("big", "sleep.read", json!({"pad": "x".repeat(1 << 20)}));
    assert_eq!(send(handle, &oversized), Status::InvalidArgument as i32);
    let mut answered = std::collections::HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while answered.len() < MAX_PENDING {
        for item in poll_once(handle, 256)
            .unwrap()
            .into_iter()
            .filter(|item| item.kind == 1)
        {
            assert_eq!(item.json["ok"], true);
            answered.insert(item.json["request_id"].as_str().unwrap().to_owned());
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    finish(handle);
}

#[test]
fn only_requests_that_have_not_started_can_be_cancelled() {
    let handle = fake_handle();
    // 実行の枠（16）を長い読み取りで埋め、17 件目を実行前の待ちにする。
    for index in 0..16 {
        let request = request(&format!("busy{index}"), "sleep.read", json!({"ms": 1500}));
        assert_eq!(send(handle, &request), Status::Ok as i32);
    }
    // 16 件が枠を取り終えるのを待ってから送るので、次の要求は枠が空くまで実行前のまま残る。
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        send(handle, &request("queued", "sleep.read", json!({}))),
        Status::Ok as i32
    );
    let cancel = |id: &str| unsafe { kukuri_sdk_cancel(handle, id.as_ptr(), id.len()) };
    assert_eq!(cancel("queued"), Status::Ok as i32);
    assert_eq!(cancel("busy0"), Status::Ok as i32);
    assert_eq!(cancel("unknown"), Status::InvalidArgument as i32);
    assert_eq!(
        error_code(&response_for(handle, "queued", Duration::from_secs(5))),
        "cancelled"
    );
    assert_eq!(
        response_for(handle, "busy0", Duration::from_secs(5))["data"]["slept"],
        1500
    );
    finish(handle);
}

#[test]
fn a_request_past_its_deadline_times_out_even_while_waiting_to_start() {
    let handle = fake_handle();
    let timed = |id: &str, ms: u64| {
        let mut timed =
            serde_json::from_slice::<Value>(&request(id, "sleep.read", json!({"ms": ms}))).unwrap();
        timed["timeout_ms"] = json!(100);
        timed.to_string().into_bytes()
    };
    assert_eq!(send(handle, &timed("t", 2000)), Status::Ok as i32);
    assert_eq!(
        error_code(&response_for(handle, "t", Duration::from_secs(5))),
        "timeout"
    );
    // 実行の枠を埋めると、次の要求は実行前の待ちのまま期限を迎える。期限は受け付けた時から数える。
    for index in 0..16 {
        let request = request(&format!("busy{index}"), "sleep.read", json!({"ms": 1500}));
        assert_eq!(send(handle, &request), Status::Ok as i32);
    }
    std::thread::sleep(Duration::from_millis(300));
    let sent = Instant::now();
    assert_eq!(send(handle, &timed("waiting", 0)), Status::Ok as i32);
    assert_eq!(
        error_code(&response_for(handle, "waiting", Duration::from_secs(5))),
        "timeout"
    );
    assert!(sent.elapsed() < Duration::from_millis(1000));
    finish(handle);
}

#[test]
fn close_interrupts_reads_waits_for_writes_and_rejects_later_requests() {
    let handle = fake_handle();
    assert_eq!(
        send(
            handle,
            &request("read", "sleep.read", json!({"ms": 10_000}))
        ),
        Status::Ok as i32
    );
    assert_eq!(
        send(handle, &request("write", "sleep.write", json!({"ms": 300}))),
        Status::Ok as i32
    );
    // 実行を始めてから close する。
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(unsafe { kukuri_sdk_close(handle) }, Status::Ok as i32);
    assert_eq!(
        error_code(&response_for(handle, "read", Duration::from_secs(1))),
        "interrupted"
    );
    assert_eq!(
        response_for(handle, "write", Duration::from_secs(1))["data"]["slept"],
        300
    );
    assert_eq!(
        send(handle, &request("late", "sleep.read", json!({}))),
        Status::Closed as i32
    );
    unsafe { kukuri_sdk_free(handle) };
}

#[test]
fn a_write_that_outlives_close_reports_an_unknown_outcome() {
    let handle = fake_handle();
    assert_eq!(
        send(
            handle,
            &request("slow", "sleep.write", json!({"ms": 20_000}))
        ),
        Status::Ok as i32
    );
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    assert_eq!(unsafe { kukuri_sdk_close(handle) }, Status::Ok as i32);
    assert!(started.elapsed() < Duration::from_secs(10));
    let response = response_for(handle, "slow", Duration::from_secs(1));
    assert_eq!(error_code(&response), "operation_outcome_unknown");
    unsafe { kukuri_sdk_free(handle) };
}

#[test]
fn a_handler_panic_faults_the_handle_but_not_the_process() {
    let handle = fake_handle();
    assert_eq!(
        send(handle, &request("boom", "panic.read", json!({}))),
        Status::Ok as i32
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match poll_once(handle, 64) {
            Err(status) => {
                assert_eq!(status, Status::Faulted as i32);
                break;
            }
            Ok(_) => assert!(Instant::now() < deadline, "the handle did not fault"),
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        send(handle, &request("after", "sleep.read", json!({}))),
        Status::Faulted as i32
    );
    finish(handle);
}

#[test]
fn polling_with_nothing_returns_no_batch_and_batches_are_released_once() {
    let handle = fake_handle();
    let other = fake_handle();
    // 起動の知らせを読み切ってから、何も届いていない poll を確かめる。
    std::thread::sleep(Duration::from_millis(50));
    poll_once(handle, 64).unwrap();
    let mut batch: *const KukuriBatch = std::ptr::dangling();
    let started = Instant::now();
    assert_eq!(
        unsafe { kukuri_sdk_poll(handle, 64, 1 << 20, &mut batch) },
        Status::Ok as i32
    );
    assert!(batch.is_null());
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "poll does not wait"
    );

    assert_eq!(
        send(handle, &request("x", "sleep.read", json!({}))),
        Status::Ok as i32
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while batch.is_null() {
        assert_eq!(
            unsafe { kukuri_sdk_poll(handle, 64, 1 << 20, &mut batch) },
            Status::Ok as i32
        );
        assert!(Instant::now() < deadline);
    }
    let release = |owner, batch| unsafe { kukuri_sdk_batch_release(owner, batch) };
    assert_eq!(release(other, batch), Status::InvalidArgument as i32);
    assert_eq!(release(handle, batch), Status::Ok as i32);
    assert_eq!(release(handle, batch), Status::InvalidArgument as i32);
    finish(other);
    finish(handle);
}

#[test]
fn a_secret_output_arrives_only_as_its_own_item() {
    let handle = fake_handle();
    let mut secret =
        serde_json::from_slice::<Value>(&request("s", "secret.write", json!({}))).unwrap();
    secret["accepts_secret_output"] = json!(true);
    assert_eq!(
        send(handle, secret.to_string().as_bytes()),
        Status::Ok as i32
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let items = loop {
        let items = poll_once(handle, 64).unwrap();
        if items.iter().any(|item| item.kind == 1) {
            break items;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    let position = items.iter().position(|item| item.kind == 1).unwrap();
    let response = &items[position];
    assert_eq!(response.json["ok"], true);
    assert!(!String::from_utf8_lossy(&response.raw).contains("sentinel"));
    assert_eq!(items[position + 1].kind, 2);
    assert_eq!(items[position + 1].raw, b"invite-token-sentinel");
    finish(handle);
}
