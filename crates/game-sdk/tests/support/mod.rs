//! game-sdk の試験が共用する ABI の呼び出し。
#![allow(dead_code, clippy::unwrap_used)]

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use kukuri_game_sdk::{
    ABI_VERSION, KukuriBatch, KukuriHandle, Status, kukuri_sdk_batch_release, kukuri_sdk_close,
    kukuri_sdk_free, kukuri_sdk_open, kukuri_sdk_poll, kukuri_sdk_send,
};
use serde_json::{Value, json};

pub const PROFILE: &str = "p";

pub fn open_real(dir: &std::path::Path) -> *mut KukuriHandle {
    let config = json!({"profile": PROFILE, "profile_dir": dir}).to_string();
    let mut handle = std::ptr::null_mut();
    let status =
        unsafe { kukuri_sdk_open(ABI_VERSION, config.as_ptr(), config.len(), &mut handle) };
    assert_eq!(status, Status::Ok as i32);
    handle
}

pub fn request(id: &str, command: &str, payload: Value) -> Vec<u8> {
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

pub fn send(handle: *mut KukuriHandle, request: &[u8]) -> i32 {
    unsafe { kukuri_sdk_send(handle, request.as_ptr(), request.len(), std::ptr::null(), 0) }
}

#[derive(Debug)]
pub struct Polled {
    pub kind: u32,
    pub json: Value,
    pub raw: Vec<u8>,
}

/// 1 回 poll して、batch を写してから解放する。
pub fn poll_once(handle: *mut KukuriHandle, max_items: usize) -> Result<Vec<Polled>, i32> {
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
    let polled: Vec<Polled> = items
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
    SEEN.lock().unwrap().extend(
        polled
            .iter()
            .map(|item| (handle as usize, item.kind, item.raw.clone())),
    );
    Ok(polled)
}

/// poll で受けたすべての項目（handle の address、種類、bytes）。秘密が出ていないことを後から確かめる。
static SEEN: Mutex<Vec<(usize, u32, Vec<u8>)>> = Mutex::new(Vec::new());

/// handle がこれまでに poll で受けた応答と event（秘密の出力の項目は除く）。
pub fn seen_json(handle: *mut KukuriHandle) -> Vec<Vec<u8>> {
    SEEN.lock()
        .unwrap()
        .iter()
        .filter(|(owner, kind, _)| *owner == handle as usize && *kind != 2)
        .map(|(_, _, raw)| raw.clone())
        .collect()
}

/// 先に届いた応答（handle の address と応答）。一度の poll に複数の応答が入るので、探していない応答を残す。
static RECEIVED: Mutex<Vec<(usize, Value)>> = Mutex::new(Vec::new());

/// `request_id` の応答が届くまで poll する。
pub fn response_for(handle: *mut KukuriHandle, id: &str, within: Duration) -> Value {
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

pub fn error_code(response: &Value) -> &str {
    response["error"]["code"].as_str().unwrap_or("")
}

pub fn finish(handle: *mut KukuriHandle) {
    unsafe {
        kukuri_sdk_close(handle);
        kukuri_sdk_free(handle);
    }
}

/// 秘密の入力つきで送る（`secret_bytes` を秘密の長さにする）。
pub fn send_secret(handle: *mut KukuriHandle, request: &[u8], secret: &[u8]) -> i32 {
    let mut envelope: Value = serde_json::from_slice(request).expect("request JSON");
    envelope["secret_bytes"] = json!(secret.len());
    let request = envelope.to_string();
    unsafe {
        kukuri_sdk_send(
            handle,
            request.as_ptr(),
            request.len(),
            secret.as_ptr(),
            secret.len(),
        )
    }
}

/// process の中で `client` を Sidecar として公開し、Sidecar の mode の handle で接続する（Windows だけ）。
/// drop で server を止め、client を close する。
#[cfg(windows)]
pub struct SidecarServer {
    pub handle: *mut KukuriHandle,
    pub profile_dir: tempfile::TempDir,
    stop: tokio::sync::watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl SidecarServer {
    pub fn start(client: kukuri_game_sdk::SdkClient) -> Self {
        let profile_dir = tempfile::tempdir().unwrap();
        let dir = profile_dir.path().to_path_buf();
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut client = client;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime
                .block_on(kukuri_game_sdk::sidecar::serve(
                    &client,
                    &dir,
                    stopped,
                    || {
                        let _ = ready_tx.send(());
                    },
                ))
                .unwrap();
            client.close();
        });
        ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let handle = Self::connect(profile_dir.path());
        Self {
            handle,
            profile_dir,
            stop,
            thread: Some(thread),
        }
    }

    /// 同じ Sidecar へ新しい handle で接続する。
    pub fn connect(profile_dir: &std::path::Path) -> *mut KukuriHandle {
        let config =
            json!({"profile": PROFILE, "profile_dir": profile_dir, "mode": "sidecar"}).to_string();
        let mut handle = std::ptr::null_mut();
        let status =
            unsafe { kukuri_sdk_open(ABI_VERSION, config.as_ptr(), config.len(), &mut handle) };
        assert_eq!(status, Status::Ok as i32);
        handle
    }
}

#[cfg(windows)]
impl Drop for SidecarServer {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
