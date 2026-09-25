#![cfg(target_os = "linux")]

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::ffi::OsStrExt,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use kukuri_desktop_runtime::resolve_cli_profile;

fn base_cli_command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kukuri-cli"));
    command
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("HOME", root.join("home"))
        .env("KUKURI_DISABLE_KEYRING", "1")
        .env_remove("KUKURI_APP_DATA_DIR")
        .env_remove("KUKURI_INSTANCE");
    command
}

fn cli_command(root: &Path, profile: &str) -> Command {
    let mut command = base_cli_command(root);
    command.args(["--profile", profile]);
    command
}

fn custom_cli_command(root: &Path, app_data_dir: &Path) -> Command {
    let mut command = base_cli_command(root);
    command.env("KUKURI_APP_DATA_DIR", app_data_dir);
    command
}

fn daemon_command(root: &Path, profile: &str) -> Command {
    let mut command = cli_command(root, profile);
    command.args(["daemon", "run"]);
    command
}

fn accept_consent(root: &Path, profile: &str) {
    let status = cli_command(root, profile)
        .args(["consent", "accept", "--accept-documents", "--age-confirmed"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .expect("accept consent");
    assert!(status.success(), "consent command exited with {status}");
}

fn accept_custom_consent(root: &Path, app_data_dir: &Path) {
    let status = custom_cli_command(root, app_data_dir)
        .args(["consent", "accept", "--accept-documents", "--age-confirmed"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .expect("accept custom consent");
    assert!(status.success(), "consent command exited with {status}");
}

fn spawn_daemon(root: &Path, profile: &str) -> Child {
    daemon_command(root, profile)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daemon")
}

fn spawn_custom_daemon(root: &Path, app_data_dir: &Path) -> Child {
    custom_cli_command(root, app_data_dir)
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn custom daemon")
}

fn socket_path(root: &Path, profile: &str) -> PathBuf {
    let app_data_dir = root
        .join("data")
        .join("kukuri")
        .join("cli")
        .join("profiles")
        .join(profile);
    socket_path_for_data_dir(root, &app_data_dir)
}

fn socket_path_for_data_dir(root: &Path, app_data_dir: &Path) -> PathBuf {
    let canonical = fs::canonicalize(app_data_dir).expect("canonical profile path");
    let digest = blake3::hash(canonical.as_os_str().as_bytes()).to_hex();
    root.join("runtime")
        .join("kukuri")
        .join(format!("profile-{}.sock", &digest[..32]))
}

fn custom_socket_path(root: &Path, app_data_dir: &Path) -> PathBuf {
    socket_path_for_data_dir(root, app_data_dir)
}

fn wait_for_ready(child: &mut Child, path: &Path) {
    let stderr = child.stderr.take().expect("daemon stderr");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        let result = reader.read_line(&mut line).map(|_| line);
        let _ = sender.send(result);
    });
    let line = match receiver.recv_timeout(Duration::from_secs(20)) {
        Ok(result) => result.expect("read daemon readiness"),
        Err(error) => {
            let _ = child.kill();
            panic!("daemon did not report readiness: {error}");
        }
    };
    assert_eq!(line.trim(), "ready", "unexpected daemon readiness");
    assert!(
        path.exists(),
        "daemon socket does not exist: {}",
        path.display()
    );
    UnixStream::connect(path).expect("connect daemon socket");
}

fn call(root: &Path, profile: &str, args: &[&str]) -> std::process::Output {
    cli_command(root, profile)
        .arg("call")
        .args(args)
        .output()
        .expect("run CLI call")
}

fn terminate(mut child: Child) {
    let result = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    assert_eq!(result, 0, "send SIGTERM");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("wait daemon") {
            assert!(status.success(), "daemon exited with {status}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not stop after SIGTERM"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn process_tcp_listener_count(pid: u32) -> usize {
    let socket_inodes = fs::read_dir(format!("/proc/{pid}/fd"))
        .expect("process fds")
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_link(entry.path()).ok())
        .filter_map(|target| {
            let value = target.to_string_lossy();
            value
                .strip_prefix("socket:[")
                .and_then(|value| value.strip_suffix(']'))
                .map(str::to_string)
        })
        .collect::<std::collections::HashSet<_>>();

    ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .flat_map(|table| {
            table
                .lines()
                .skip(1)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|line| {
            let columns = line.split_ascii_whitespace().collect::<Vec<_>>();
            columns.get(3) == Some(&"0A")
                && columns
                    .get(9)
                    .is_some_and(|inode| socket_inodes.contains(*inode))
        })
        .count()
}

#[test]
fn daemon_enforces_profile_ownership_permissions_and_graceful_restart() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("runtime")).expect("runtime root");
    fs::create_dir_all(root.path().join("home")).expect("home");
    accept_consent(root.path(), "alpha");
    accept_consent(root.path(), "beta");

    let alpha_socket = socket_path(root.path(), "alpha");
    let beta_socket = socket_path(root.path(), "beta");
    let mut alpha = spawn_daemon(root.path(), "alpha");
    wait_for_ready(&mut alpha, &alpha_socket);

    assert_eq!(
        fs::metadata(root.path().join("runtime/kukuri"))
            .expect("runtime metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&alpha_socket)
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(process_tcp_listener_count(alpha.id()), 0);

    let duplicate = daemon_command(root.path(), "alpha")
        .output()
        .expect("duplicate daemon");
    assert!(!duplicate.status.success());
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("profile_in_use"),
        "{}",
        String::from_utf8_lossy(&duplicate.stderr)
    );

    let mut beta = spawn_daemon(root.path(), "beta");
    wait_for_ready(&mut beta, &beta_socket);
    terminate(alpha);
    terminate(beta);
    assert!(!alpha_socket.exists());
    assert!(!beta_socket.exists());

    let mut restarted = spawn_daemon(root.path(), "alpha");
    wait_for_ready(&mut restarted, &alpha_socket);
    terminate(restarted);
    assert!(!alpha_socket.exists());
}

#[test]
fn custom_app_data_daemons_use_distinct_sockets() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("runtime")).expect("runtime root");
    fs::create_dir_all(root.path().join("home")).expect("home");
    let first_data = root.path().join("custom-first");
    let second_data = root.path().join("custom-second");
    accept_custom_consent(root.path(), &first_data);
    accept_custom_consent(root.path(), &second_data);

    let first_socket = custom_socket_path(root.path(), &first_data);
    let second_socket = custom_socket_path(root.path(), &second_data);
    assert_ne!(first_socket, second_socket);

    let mut first = spawn_custom_daemon(root.path(), &first_data);
    wait_for_ready(&mut first, &first_socket);
    let mut second = spawn_custom_daemon(root.path(), &second_data);
    wait_for_ready(&mut second, &second_socket);

    terminate(first);
    assert!(second_socket.exists());
    UnixStream::connect(&second_socket).expect("second custom daemon remains reachable");
    terminate(second);
    assert!(!first_socket.exists());
    assert!(!second_socket.exists());
}

#[test]
fn custom_and_same_named_profile_use_distinct_sockets() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("runtime")).expect("runtime root");
    fs::create_dir_all(root.path().join("home")).expect("home");
    let custom_data = root.path().join("custom");
    accept_custom_consent(root.path(), &custom_data);
    let custom_data_text = custom_data.to_str().expect("UTF-8 custom data path");
    let custom_profile = resolve_cli_profile(None, None, Some(custom_data_text), None, None)
        .expect("resolve custom profile");
    accept_consent(root.path(), &custom_profile.name);

    let custom_socket = custom_socket_path(root.path(), &custom_data);
    let named_socket = socket_path(root.path(), &custom_profile.name);
    assert_ne!(custom_socket, named_socket);

    let mut custom = spawn_custom_daemon(root.path(), &custom_data);
    wait_for_ready(&mut custom, &custom_socket);
    let mut named = spawn_daemon(root.path(), &custom_profile.name);
    wait_for_ready(&mut named, &named_socket);

    terminate(custom);
    assert!(named_socket.exists());
    UnixStream::connect(&named_socket).expect("named daemon remains reachable");
    terminate(named);
    assert!(!custom_socket.exists());
    assert!(!named_socket.exists());
}

#[test]
fn custom_app_data_rejects_systemd_actions() {
    let root = tempfile::tempdir().expect("tempdir");
    let output = custom_cli_command(root.path(), &root.path().join("custom"))
        .args(["daemon", "status"])
        .output()
        .expect("custom daemon status");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("custom_profile_systemd_unsupported"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn protocol_status_registry_and_errors_are_machine_readable() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("runtime")).expect("runtime root");
    fs::create_dir_all(root.path().join("home")).expect("home");

    let usage = call(root.path(), "alpha", &[]);
    assert_eq!(usage.status.code(), Some(2));
    let usage: serde_json::Value =
        serde_json::from_slice(&usage.stdout).expect("usage error envelope");
    assert_eq!(usage["error"]["code"], "invalid_request");

    let unavailable = call(root.path(), "alpha", &["client.status"]);
    assert_eq!(unavailable.status.code(), Some(3));
    let unavailable: serde_json::Value =
        serde_json::from_slice(&unavailable.stdout).expect("unavailable envelope");
    assert_eq!(unavailable["error"]["code"], "daemon_unavailable");

    accept_consent(root.path(), "alpha");
    let socket = socket_path(root.path(), "alpha");
    let mut daemon = spawn_daemon(root.path(), "alpha");
    wait_for_ready(&mut daemon, &socket);

    let status = call(root.path(), "alpha", &["client.status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(
        status.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    let status: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("status envelope");
    assert_eq!(status["schema_version"], 1);
    assert_eq!(status["ok"], true);
    assert_eq!(status["data"]["ready"], true);

    let commands = call(root.path(), "alpha", &["protocol.commands"]);
    assert!(commands.status.success());
    let commands: serde_json::Value =
        serde_json::from_slice(&commands.stdout).expect("commands envelope");
    let registry = kukuri_cli::registry::CommandRegistry::builtin();
    let first_page = registry.page(None, 100).expect("first page");
    assert_eq!(commands["data"], serde_json::to_value(&first_page).unwrap());
    let cursor = first_page.next_cursor.as_deref().expect("second page");
    let page_input = root.path().join("command-page.json");
    fs::write(
        &page_input,
        serde_json::json!({"cursor": cursor}).to_string(),
    )
    .expect("page request");
    fs::set_permissions(&page_input, fs::Permissions::from_mode(0o600))
        .expect("private page request");
    let second = call(
        root.path(),
        "alpha",
        &["protocol.commands", "--input", page_input.to_str().unwrap()],
    );
    assert!(second.status.success());
    let second: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("second page envelope");
    let last_page = registry.page(Some(cursor), 100).expect("last page");
    assert_eq!(second["data"], serde_json::to_value(&last_page).unwrap());
    assert!(last_page.next_cursor.is_none());
    assert_eq!(first_page.items.len() + last_page.items.len(), 151);

    let schema = call(root.path(), "alpha", &["protocol.schema"]);
    assert!(schema.status.success());
    let schema: serde_json::Value =
        serde_json::from_slice(&schema.stdout).expect("schema envelope");
    assert_eq!(
        schema["data"]["protocol"]["$defs"]["request"]["type"],
        "object"
    );

    let invalid_timeout = call(
        root.path(),
        "alpha",
        &["client.status", "--timeout-ms", "0"],
    );
    assert_eq!(invalid_timeout.status.code(), Some(2));
    let invalid_timeout: serde_json::Value =
        serde_json::from_slice(&invalid_timeout.stdout).expect("timeout envelope");
    assert_eq!(invalid_timeout["error"]["code"], "validation_failed");

    let mismatch = call(
        root.path(),
        "alpha",
        &["client.status", "--protocol-version", "99"],
    );
    assert_eq!(mismatch.status.code(), Some(3));
    let mismatch: serde_json::Value =
        serde_json::from_slice(&mismatch.stdout).expect("mismatch envelope");
    assert_eq!(mismatch["error"]["code"], "protocol_mismatch");

    let timeout = call(
        root.path(),
        "alpha",
        &["events.watch", "--timeout-ms", "200"],
    );
    assert_eq!(timeout.status.code(), Some(6));
    let timeout_line = String::from_utf8_lossy(&timeout.stdout)
        .lines()
        .last()
        .expect("timeout envelope")
        .to_string();
    let timeout: serde_json::Value =
        serde_json::from_str(&timeout_line).expect("parse timeout envelope");
    assert_eq!(timeout["error"]["code"], "timeout");

    let mut raw = UnixStream::connect(&socket).expect("raw protocol connection");
    raw.set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout");
    writeln!(
        raw,
        "{}",
        serde_json::json!({
            "protocol_version": 1,
            "request_id": "preflight-secret",
            "command": "events.watch",
            "profile": "alpha",
            "payload": {},
            "timeout_ms": 2000,
            "secret_bytes": 64,
            "accepts_secret_output": false
        })
    )
    .expect("request header");
    let mut line = String::new();
    BufReader::new(raw)
        .read_line(&mut line)
        .expect("preflight response before secret body");
    let preflight: serde_json::Value = serde_json::from_str(&line).expect("preflight envelope");
    assert_eq!(preflight["error"]["code"], "validation_failed");

    let standard_fd = call(
        root.path(),
        "alpha",
        &["client.status", "--secret-output-fd", "1"],
    );
    assert_eq!(standard_fd.status.code(), Some(2));
    let standard_fd: serde_json::Value =
        serde_json::from_slice(&standard_fd.stdout).expect("standard FD rejection envelope");
    assert_eq!(standard_fd["error"]["code"], "validation_failed");

    let input_path = root.path().join("request.json");
    fs::write(&input_path, b"{}").expect("request file");
    fs::set_permissions(&input_path, fs::Permissions::from_mode(0o644))
        .expect("public request permissions");
    let public_input = call(
        root.path(),
        "alpha",
        &[
            "client.status",
            "--input",
            input_path.to_str().expect("input path"),
        ],
    );
    assert_eq!(public_input.status.code(), Some(2));
    fs::set_permissions(&input_path, fs::Permissions::from_mode(0o600))
        .expect("private request permissions");
    assert!(
        call(
            root.path(),
            "alpha",
            &[
                "client.status",
                "--input",
                input_path.to_str().expect("input path"),
            ],
        )
        .status
        .success()
    );

    terminate(daemon);
}

#[test]
fn connection_limit_returns_correlated_backpressure() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("runtime")).expect("runtime root");
    fs::create_dir_all(root.path().join("home")).expect("home");
    accept_consent(root.path(), "alpha");
    let socket = socket_path(root.path(), "alpha");
    let mut daemon = spawn_daemon(root.path(), "alpha");
    wait_for_ready(&mut daemon, &socket);

    let stalled = (0..64)
        .map(|index| {
            let mut stream = UnixStream::connect(&socket).expect("stalled connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("stalled connection timeout");
            writeln!(
                stream,
                "{}",
                serde_json::json!({
                    "protocol_version": 1,
                    "request_id": format!("stalled-{index}"),
                    "command": "events.watch",
                    "profile": "alpha",
                    "payload": {},
                    "timeout_ms": 300_000,
                    "accepts_secret_output": false
                })
            )
            .expect("subscribe stalled connection");
            let mut response = String::new();
            BufReader::new(stream.try_clone().expect("clone stalled connection"))
                .read_line(&mut response)
                .expect("stalled subscription response");
            let response: serde_json::Value =
                serde_json::from_str(&response).expect("stalled subscription envelope");
            assert_eq!(response["ok"], true);
            assert_eq!(response["more"], true);
            stream
        })
        .collect::<Vec<_>>();
    let output = call(
        root.path(),
        "alpha",
        &["client.status", "--timeout-ms", "2000"],
    );
    assert_eq!(output.status.code(), Some(6));
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("backpressure envelope");
    assert_eq!(response["error"]["code"], "backpressure");

    drop(stalled);
    terminate(daemon);
}

#[test]
fn event_watch_sigint_returns_a_parseable_interrupted_error() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("runtime")).expect("runtime root");
    fs::create_dir_all(root.path().join("home")).expect("home");
    accept_consent(root.path(), "alpha");
    let socket = socket_path(root.path(), "alpha");
    let mut daemon = spawn_daemon(root.path(), "alpha");
    wait_for_ready(&mut daemon, &socket);

    let watcher = cli_command(root.path(), "alpha")
        .args(["call", "events.watch"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn event watch");
    thread::sleep(Duration::from_millis(500));
    let result = unsafe { libc::kill(watcher.id() as i32, libc::SIGINT) };
    assert_eq!(result, 0, "send SIGINT");
    let output = watcher.wait_with_output().expect("wait event watch");
    assert_eq!(output.status.code(), Some(130));
    let last = String::from_utf8_lossy(&output.stdout)
        .lines()
        .last()
        .expect("interrupted envelope")
        .to_string();
    let response: serde_json::Value = serde_json::from_str(&last).expect("parse interrupted error");
    assert_eq!(response["error"]["code"], "interrupted");

    terminate(daemon);
}

#[test]
fn secret_input_is_owner_only_and_never_echoed() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("runtime")).expect("runtime root");
    fs::create_dir_all(root.path().join("home")).expect("home");
    accept_consent(root.path(), "alpha");
    let socket = socket_path(root.path(), "alpha");
    let mut daemon = spawn_daemon(root.path(), "alpha");
    wait_for_ready(&mut daemon, &socket);

    let sentinel = "do-not-echo-secret-sentinel";
    let secret_path = root.path().join("secret.txt");
    fs::write(&secret_path, sentinel).expect("write secret");
    fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600)).expect("protect secret");
    let output = call(
        root.path(),
        "alpha",
        &[
            "client.status",
            "--secret-input",
            secret_path.to_str().expect("secret path"),
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains(sentinel));
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("secret rejection envelope");
    assert_eq!(response["error"]["code"], "validation_failed");

    fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o644))
        .expect("relax secret permissions");
    let permission_error = call(
        root.path(),
        "alpha",
        &[
            "client.status",
            "--secret-input",
            secret_path.to_str().expect("secret path"),
        ],
    );
    assert_eq!(permission_error.status.code(), Some(2));
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&permission_error.stdout),
        String::from_utf8_lossy(&permission_error.stderr)
    );
    assert!(!combined.contains(sentinel));

    terminate(daemon);
}
