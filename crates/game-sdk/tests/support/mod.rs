//! game-sdk の試験が共用する ABI の呼び出し。
#![allow(dead_code, clippy::unwrap_used)]

use std::{
    sync::{Mutex, Once},
    time::{Duration, Instant},
};

use kukuri_game_sdk::{
    ABI_VERSION, KukuriBatch, KukuriHandle, Status, kukuri_sdk_batch_release, kukuri_sdk_close,
    kukuri_sdk_free, kukuri_sdk_open, kukuri_sdk_poll, kukuri_sdk_send, kukuri_sdk_send_binary,
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

pub fn send_binary(handle: *mut KukuriHandle, kind: u32, target: &str, bytes: &[u8]) -> i32 {
    unsafe {
        kukuri_sdk_send_binary(
            handle,
            kind,
            target.as_ptr(),
            target.len(),
            bytes.as_ptr(),
            bytes.len(),
        )
    }
}

/// 1 回 poll して、batch を解放せずに写す（届いていなければ None）。
pub fn poll_hold(
    handle: *mut KukuriHandle,
    max_items: usize,
    max_bytes: usize,
) -> Option<(*const KukuriBatch, Vec<Polled>)> {
    let mut batch: *const KukuriBatch = std::ptr::null();
    let status = unsafe { kukuri_sdk_poll(handle, max_items, max_bytes, &mut batch) };
    assert_eq!(status, Status::Ok as i32);
    if batch.is_null() {
        return None;
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
    Some((batch, polled))
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

/// 同意の後は実際の runtime を起動するので、CLI の process 試験と同じく keyring を使わず loopback だけで動かす。
pub fn isolated_environment() {
    static SET: Once = Once::new();
    SET.call_once(|| unsafe {
        for name in [
            "KUKURI_APP_DATA_DIR",
            "KUKURI_INSTANCE",
            "KUKURI_DISCOVERY_SEEDS",
            "KUKURI_ADVERTISE_PORT",
        ] {
            std::env::remove_var(name);
        }
        std::env::set_var("KUKURI_DISABLE_KEYRING", "1");
        std::env::set_var("KUKURI_DISCOVERY_MODE", "static_peer");
        std::env::set_var("KUKURI_BIND_ADDR", "127.0.0.1:0");
        std::env::set_var("KUKURI_ADVERTISE_HOST", "127.0.0.1");
    });
}

/// 秘密の出力（招待の token・移行のリンク）を返す command を送り、秘密を受け取る。
pub fn secret_output(
    handle: *mut KukuriHandle,
    request_id: &str,
    command: &str,
    payload: Value,
) -> String {
    let mut envelope: Value =
        serde_json::from_slice(&request(request_id, command, payload)).unwrap();
    envelope["accepts_secret_output"] = json!(true);
    assert_eq!(send(handle, envelope.to_string().as_bytes()), 0);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let items = poll_once(handle, 64).unwrap();
        if let Some(position) = items
            .iter()
            .position(|item| item.kind == 1 && item.json["request_id"] == request_id)
        {
            assert_eq!(items[position].json["ok"], true, "{}", items[position].json);
            assert_eq!(
                items[position + 1].kind,
                2,
                "the secret follows as its own item"
            );
            return String::from_utf8(items[position + 1].raw.clone()).unwrap();
        }
        assert!(Instant::now() < deadline, "{command}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 応答の data を返す。失敗なら error を見せて止まる。
pub fn call(handle: *mut KukuriHandle, id: &str, command: &str, payload: Value) -> Value {
    assert_eq!(send(handle, &request(id, command, payload)), 0, "{command}");
    let response = response_for(handle, id, Duration::from_secs(60));
    assert_eq!(response["ok"], true, "{command}: {}", response["error"]);
    response["data"].clone()
}

/// これまでに受け取った項目（応答を待つ間の poll で受けたものを含む）に、`pick` が値を返す JSON か bytes が届くまで
/// poll する。
pub fn wait_for<T>(
    handle: *mut KukuriHandle,
    what: &str,
    mut pick: impl FnMut(&Value, &[u8]) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        poll_once(handle, 64).unwrap();
        let found = seen_json(handle)
            .iter()
            .find_map(|raw| pick(&serde_json::from_slice(raw).unwrap_or(Value::Null), raw));
        if let Some(value) = found {
            return value;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// topic の gossip を購読する。
pub fn subscribe(handle: *mut KukuriHandle, topic: &str) {
    call(
        handle,
        "subscribe",
        "set_topic_gossip_enabled",
        json!({"topic": topic, "enabled": true}),
    );
}

/// 2 つの profile に互いの peer ticket を入れる（loopback の static peer）。
pub fn introduce(left: *mut KukuriHandle, right: *mut KukuriHandle) {
    let ticket = |handle| {
        call(handle, "ticket", "get_local_peer_ticket", json!({}))
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (left_ticket, right_ticket) = (ticket(left), ticket(right));
    call(
        left,
        "import",
        "import_peer_ticket",
        json!({"ticket": right_ticket}),
    );
    call(
        right,
        "import",
        "import_peer_ticket",
        json!({"ticket": left_ticket}),
    );
}

pub fn wait_for_startup(handle: *mut KukuriHandle) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !poll_once(handle, 64)
        .unwrap()
        .iter()
        .any(|item| item.kind == 3 && item.json["type"] == "startup_status_changed")
    {
        assert!(Instant::now() < deadline, "startup did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 表示された現行の文書と 18 歳以上の申告に同意する（ゲームが同意の画面で行う操作）。
pub fn consent(handle: *mut KukuriHandle) {
    let status = call(
        handle,
        "consent-status",
        "get_app_consent_status",
        json!({}),
    );
    let documents: Vec<Value> = status["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|doc| json!({"slug": doc["slug"], "version": doc["currentVersion"]}))
        .collect();
    let accepted = call(
        handle,
        "consent",
        "accept_app_consents",
        json!({"documents": documents, "language": "ja", "age_attested": true}),
    );
    assert_eq!(accepted["status"], "ready");
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

/// 実際の `kukuri-sdk-sidecar.exe` の process（Windows だけ）。keyring を使わず loopback だけで runtime を動かす。
#[cfg(windows)]
pub mod sidecar_process {
    use std::{
        io::{BufRead, BufReader},
        path::Path,
        process::{Child, Command, ExitStatus, Stdio},
        time::{Duration, Instant},
    };

    use serde_json::{Value, json};

    use super::{KukuriHandle, PROFILE, call};

    pub struct Sidecar {
        child: Child,
    }

    impl Sidecar {
        pub fn start(dir: &Path) -> Self {
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
        pub fn run_to_exit(dir: &Path) -> ExitStatus {
            let mut child = sidecar_command("run", dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            wait_exit(&mut child)
        }

        pub fn pid(&self) -> u32 {
            self.child.id()
        }

        pub fn stop(mut self, dir: &Path) -> ExitStatus {
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

    pub fn sidecar_command(command: &str, dir: &Path) -> Command {
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

    pub fn wait_exit(child: &mut Child) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the sidecar did not exit");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 同意して runtime を動かす（初めての profile だけ）。
    pub fn ready(handle: *mut KukuriHandle) {
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
}
