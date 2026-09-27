use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub struct ProcessClient {
    root: tempfile::TempDir,
    daemon: Option<Child>,
    /// 失敗の文面で daemon を見分ける名前。
    label: String,
}

/// 失敗の文面に載せる daemon の stderr の行数。
const STDERR_TAIL_LINES: usize = 50;

pub struct PendingCall(Option<Child>);

impl PendingCall {
    pub fn finish(mut self) -> Value {
        let result = self
            .0
            .take()
            .expect("CLI")
            .wait_with_output()
            .expect("CLI終了");
        let response: Value =
            serde_json::from_slice(&result.stdout).expect("stdoutはJSON response");
        assert_eq!(
            result.status.success(),
            response["ok"] == true,
            "終了codeとJSONが一致"
        );
        assert!(result.stderr.is_empty(), "通常要求の診断へ本文を複製しない");
        response
    }
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl ProcessClient {
    pub fn start() -> Self {
        Self::start_named("daemon")
    }

    /// `label` は失敗の文面で daemon を見分ける名前。
    pub fn start_named(label: &str) -> Self {
        let root = tempfile::tempdir().expect("一時profile");
        let runtime = root.path().join("run");
        fs::create_dir(&runtime).expect("runtime dir");
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).expect("runtime権限");
        let mut client = Self {
            root,
            daemon: None,
            label: label.to_string(),
        };
        client.restart();
        client
    }

    /// daemon の stderr(内部エラーの診断を含む)の末尾。失敗の文面に載せる。
    fn daemon_stderr_tail(&self) -> String {
        let text = fs::read_to_string(self.path("daemon.stderr")).unwrap_or_default();
        let lines = text.lines().collect::<Vec<_>>();
        let tail = lines[lines.len().saturating_sub(STDERR_TAIL_LINES)..].join("\n");
        format!("\n--- daemon「{}」の stderr の末尾 ---\n{tail}", self.label)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kukuri-cli"));
        command
            .args(["--profile", "test"])
            .env_remove("KUKURI_APP_DATA_DIR")
            .env_remove("KUKURI_INSTANCE")
            .env_remove("KUKURI_DISCOVERY_SEEDS")
            .env_remove("KUKURI_ADVERTISE_PORT")
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_RUNTIME_DIR", self.root.path().join("run"))
            .env("KUKURI_DISABLE_KEYRING", "1")
            .env("KUKURI_DISCOVERY_MODE", "static_peer")
            .env("KUKURI_BIND_ADDR", "127.0.0.1:0")
            .env("KUKURI_ADVERTISE_HOST", "127.0.0.1");
        command
    }

    pub fn restart(&mut self) {
        self.stop();
        self.daemon = Some(
            self.command()
                .args(["daemon", "run"])
                .env("KUKURI_CLI_DEBUG_INTERNAL_ERRORS", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(self.path("daemon.stderr"))
                        .expect("daemon stderr file"),
                )
                .spawn()
                .expect("daemon起動"),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let result = self.raw_call("get_desktop_startup_status", json!({}), None, None);
            if result["ok"] == true {
                break;
            }
            if let Some(status) = self
                .daemon
                .as_mut()
                .expect("daemon")
                .try_wait()
                .expect("起動状態")
            {
                panic!("daemonの起動失敗: {status}{}", self.daemon_stderr_tail());
            }
            assert!(
                Instant::now() < deadline,
                "daemonの起動期限{}",
                self.daemon_stderr_tail()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            // このhelperが起動・所有した子だけを停止する。
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if child.try_wait().expect("daemon終了状態").is_some() {
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("daemonが正常停止しませんでした");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    pub fn app_data_dir(&self) -> PathBuf {
        self.root.path().join("data/kukuri/cli/profiles/test")
    }

    pub fn secret_file(&self, bytes: &[u8]) -> PathBuf {
        let path = self.path(&uuid::Uuid::new_v4().to_string());
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("Secret file")
            .write_all(bytes)
            .expect("Secret入力");
        path
    }

    pub fn raw_call(
        &self,
        name: &str,
        payload: Value,
        input: Option<&Path>,
        output: Option<&Path>,
    ) -> Value {
        self.begin_call(name, payload, input, output).finish()
    }

    pub fn begin_call(
        &self,
        name: &str,
        payload: Value,
        input: Option<&Path>,
        output: Option<&Path>,
    ) -> PendingCall {
        let mut command = self.command();
        command.args(["call", name, "--input", "-", "--timeout-ms", "30000"]);
        if let Some(path) = input {
            command.arg("--secret-input").arg(path);
        }
        if let Some(path) = output {
            command.arg("--secret-output").arg(path);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("CLI起動");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(&serde_json::to_vec(&payload).expect("JSON"))
            .expect("要求入力");
        PendingCall(Some(child))
    }

    pub fn call(&self, name: &str, payload: Value) -> Value {
        let result = self.raw_call(name, payload, None, None);
        assert_eq!(
            result["ok"],
            true,
            "{name}: {}{}",
            result["error"],
            self.daemon_stderr_tail()
        );
        result["data"].clone()
    }

    pub fn consent(&self) {
        let status = self.call("get_app_consent_status", json!({}));
        let documents = status["documents"]
            .as_array()
            .expect("documents")
            .iter()
            .map(|doc| json!({"slug": doc["slug"], "version": doc["currentVersion"]}))
            .collect::<Vec<_>>();
        let result = self.call(
            "accept_app_consents",
            json!({"documents": documents, "language": "ja", "age_attested": true}),
        );
        assert_eq!(result["status"], "ready");
    }

    #[track_caller]
    pub fn wait_for(
        &self,
        name: &str,
        payload: Value,
        predicate: impl Fn(&Value) -> bool,
    ) -> Value {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let data = self.call(name, payload.clone());
            if predicate(&data) {
                return data;
            }
            assert!(
                Instant::now() < deadline,
                "{name}の伝播期限{}",
                self.daemon_stderr_tail()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for ProcessClient {
    fn drop(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
