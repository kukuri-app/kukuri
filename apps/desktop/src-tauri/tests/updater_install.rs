//! Real Linux updater/verifier/filesystem contract; no GUI, restart or user profile.
#![cfg(target_os = "linux")]

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tauri_plugin_updater::UpdaterExt;

struct FixtureServer {
    endpoint: String,
    stop: Arc<AtomicBool>,
    task: Option<std::thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start(bundle: Vec<u8>, signature: String) -> Self {
        Self::start_for_target(bundle, signature, "linux-x86_64")
    }

    fn start_for_target(bundle: Vec<u8>, signature: String, target: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        // updater 2.13 以降は、署名に記録された版と告知する版の一致を要求する。
        // fixture は同じ crate の版で build・署名した bundle なので、その版を告知する。
        let manifest = serde_json::to_vec(&serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "platforms": { target: {
                "url": format!("{base}/bundle"), "signature": signature.trim()
            }}
        }))
        .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let task = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut request = Vec::new();
                        let mut byte = [0];
                        while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                            if stream.read(&mut byte).unwrap_or(0) == 0 {
                                break;
                            }
                            request.push(byte[0]);
                        }
                        let (status, body) = if request.starts_with(b"GET /manifest ") {
                            ("200 OK", manifest.as_slice())
                        } else if request.starts_with(b"GET /bundle ") {
                            ("200 OK", bundle.as_slice())
                        } else {
                            ("404 Not Found", &b""[..])
                        };
                        write!(
                            stream,
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .unwrap();
                        let _ = stream.write_all(body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture listener failed: {error}"),
                }
            }
        });
        Self {
            endpoint: format!("{base}/manifest"),
            stop,
            task: Some(task),
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}

struct ReadOnlyDirectory(PathBuf);

impl ReadOnlyDirectory {
    fn lock(path: &Path) -> Self {
        fs::set_permissions(path, fs::Permissions::from_mode(0o500)).unwrap();
        Self(path.to_owned())
    }
}

impl Drop for ReadOnlyDirectory {
    fn drop(&mut self) {
        fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
#[ignore = "requires signed AppImage fixture; run in Linux package CI as a non-root user"]
fn verified_appimage_install_failure_preserves_old_file_and_profile() {
    let bundle = fs::read(std::env::var("KUKURI_UPDATER_BUNDLE").unwrap()).unwrap();
    assert!(
        bundle.starts_with(b"\x7fELF"),
        "fixture must be a real AppImage"
    );
    let signature = fs::read_to_string(std::env::var("KUKURI_UPDATER_SIGNATURE").unwrap()).unwrap();
    let pubkey =
        fs::read_to_string(std::env::var("KUKURI_UPDATER_PUBLIC_KEY_FILE").unwrap()).unwrap();
    let server = FixtureServer::start(bundle.clone(), signature);
    let temporary = tempfile::tempdir().unwrap();
    let appdir = temporary.path().join("installation");
    let profile = temporary.path().join("profile");
    fs::create_dir(&appdir).unwrap();
    fs::create_dir(&profile).unwrap();
    let executable = appdir.join("kukuri.AppImage");
    // Sentinel is never executed. Only the replacement uses a verified real bundle.
    let original = b"old executable sentinel";
    fs::write(&executable, original).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let profile_file = profile.join("preserved-fixture");
    let profile_bytes = b"profile fixture must not be touched by installer";
    fs::write(&profile_file, profile_bytes).unwrap();

    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": pubkey.trim(), "dangerousInsecureTransportProtocol": true
        }),
    );
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    let updater = app
        .updater_builder()
        .executable_path(&executable)
        .endpoints(vec![server.endpoint.parse().unwrap()])
        .unwrap()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let update = tauri::async_runtime::block_on(updater.check())
        .unwrap()
        .unwrap();
    let verified = tauri::async_runtime::block_on(update.download(|_, _| {}, || {})).unwrap();
    assert_eq!(verified, bundle);

    let locked = ReadOnlyDirectory::lock(&appdir);
    assert!(
        fs::write(appdir.join("permission-probe"), b"probe").is_err(),
        "run as non-root: the fixture must really deny writes"
    );
    assert!(update.install(&verified).is_err(), "replacement must fail");
    assert_eq!(fs::read(&executable).unwrap(), original);
    assert_eq!(
        fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(fs::read(&profile_file).unwrap(), profile_bytes);
    drop(locked);

    // An explicit retry after restoring this fixture's permissions succeeds.
    update.install(&verified).unwrap();
    assert_eq!(fs::read(&executable).unwrap(), bundle);
    assert_eq!(
        fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(fs::read(&profile_file).unwrap(), profile_bytes);
}

#[test]
#[ignore = "requires signed Deb fixture; run in Linux package CI"]
fn real_deb_signature_tampering_and_missing_target_fail_closed() {
    let bundle = fs::read(std::env::var("KUKURI_DEB_UPDATER_BUNDLE").unwrap()).unwrap();
    assert!(bundle.starts_with(b"!<arch>\n"));
    let signature =
        fs::read_to_string(std::env::var("KUKURI_DEB_UPDATER_SIGNATURE").unwrap()).unwrap();
    let pubkey =
        fs::read_to_string(std::env::var("KUKURI_UPDATER_PUBLIC_KEY_FILE").unwrap()).unwrap();
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": pubkey.trim(), "dangerousInsecureTransportProtocol": true
        }),
    );
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    for (target, tamper) in [
        ("linux-x86_64-deb", false),
        ("linux-x86_64-deb", true),
        ("linux-x86_64", false),
    ] {
        let mut served = bundle.clone();
        if tamper {
            let last = served.len() - 1;
            served[last] ^= 1;
        }
        let server = FixtureServer::start_for_target(served, signature.clone(), target);
        let updater = app
            .updater_builder()
            .target("linux-x86_64-deb")
            .endpoints(vec![server.endpoint.parse().unwrap()])
            .unwrap()
            .no_proxy()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap();
        let checked = tauri::async_runtime::block_on(updater.check());
        if target != "linux-x86_64-deb" {
            assert!(
                checked.is_err(),
                "AppImage entry must not serve as Deb fallback"
            );
            continue;
        }
        let update = checked.unwrap().unwrap();
        let downloaded = tauri::async_runtime::block_on(update.download(|_, _| {}, || {}));
        if tamper {
            assert!(
                downloaded.is_err(),
                "one changed byte must reject real signature"
            );
        } else {
            assert_eq!(downloaded.unwrap(), bundle);
        }
    }
}
