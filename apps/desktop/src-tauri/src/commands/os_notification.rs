use tauri::AppHandle;
#[cfg(target_os = "linux")]
use tauri::{Emitter, Manager};

#[cfg(windows)]
#[path = "os_notification_windows.rs"]
mod windows;

use crate::state::CommandError;

#[cfg(target_os = "android")]
#[path = "os_notification_android.rs"]
pub(crate) mod android;

/// Event payload emitted to the frontend when an OS toast is clicked.
#[derive(Clone, serde::Serialize)]
#[cfg(target_os = "linux")]
struct ActivationPayload {
    notification_id: String,
}

/// Bring the main window forward and tell the frontend which notification was
/// activated. The frontend resolves the id back to a notification and opens the
/// target post via the existing in-app handler.
#[cfg(target_os = "linux")]
fn activate_main_window(app: &AppHandle, notification_id: String) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit(
        "os-notification://activated",
        ActivationPayload { notification_id },
    );
}

/// Linuxでは通知サービスへの接続だけを確認する。OS側の表示許可は保証しない。
#[tauri::command]
#[cfg(not(target_os = "android"))]
pub async fn get_os_notification_permission() -> &'static str {
    platform_notification_permission().await
}

/// Linuxにはこのprotocolの権限要求がないため、設定変更や通知送信なしで再確認する。
#[tauri::command]
#[cfg(not(target_os = "android"))]
pub async fn request_os_notification_permission() -> &'static str {
    platform_notification_permission().await
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
async fn platform_notification_permission() -> &'static str {
    // 既存Windows desktop backendの結果は維持する。
    "granted"
}

#[cfg(target_os = "android")]
#[tauri::command]
pub async fn get_os_notification_permission(app: AppHandle) -> String {
    android::permission(app, false).await
}

#[cfg(target_os = "android")]
#[tauri::command]
pub async fn request_os_notification_permission(app: AppHandle) -> String {
    android::permission(app, true).await
}

#[cfg(target_os = "linux")]
async fn platform_notification_permission() -> &'static str {
    let probe = async {
        let connection = zbus::Connection::session().await?;
        let service = zbus::Proxy::new(
            &connection,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await?;
        let _: (String, String, String, String) = service.call("GetServerInformation", &()).await?;
        Ok::<(), zbus::Error>(())
    };
    match tokio::time::timeout(std::time::Duration::from_secs(2), probe).await {
        Ok(Ok(())) => "available",
        // サービス欠落・接続拒否・不正応答・timeoutを許可済みとは表示しない。
        _ => "unavailable",
    }
}

/// Show an OS toast for an incoming notification. On platforms that support it,
/// clicking the toast focuses the window and emits `os-notification://activated`
/// so the frontend can open the related post.
///
/// This replaces the direct `plugin:notification|notify` invocation because the
/// `tauri-plugin-notification` desktop backend never reports click/activation
/// events back to the app (those are mobile-only).
#[tauri::command]
#[cfg(not(target_os = "android"))]
pub fn show_os_notification(
    app: AppHandle,
    id: String,
    title: String,
    body: Option<String>,
    silent: bool,
) -> Result<(), CommandError> {
    // cfg 付きのプラットフォーム別ヘルパは String のまま(From<String> で封筒化)。
    Ok(show_platform_notification(app, id, title, body, silent)?)
}

#[cfg(target_os = "android")]
#[tauri::command]
pub async fn show_os_notification(
    app: AppHandle,
    id: String,
    title: String,
    body: Option<String>,
    silent: bool,
) -> Result<(), CommandError> {
    Ok(show_platform_notification(app, id, title, body, silent).await?)
}

#[cfg(windows)]
pub(crate) fn show_platform_notification(
    app: AppHandle,
    id: String,
    title: String,
    body: Option<String>,
    silent: bool,
) -> Result<(), String> {
    // NSIS apps use protocol activation (including Notification Center clicks).
    // The existing single-instance handler shows the resident window, and the
    // frontend resolves this ID against the current account's notifications.
    windows::show(
        &app.config().identifier,
        &id,
        &title,
        body.as_deref(),
        silent,
    )
}

#[cfg(target_os = "linux")]
fn linux_notification(title: &str, body: Option<&str>, silent: bool) -> notify_rust::Notification {
    let mut notification = notify_rust::Notification::new();
    notification.summary(title);
    if let Some(body) = body {
        notification.body(body);
    }
    notification.hint(notify_rust::Hint::SuppressSound(silent));
    notification.action("default", "Open");
    notification
}

#[cfg(target_os = "linux")]
pub(crate) fn show_platform_notification(
    app: AppHandle,
    id: String,
    title: String,
    body: Option<String>,
    silent: bool,
) -> Result<(), String> {
    let notification = linux_notification(&title, body.as_deref(), silent);
    let handle = notification.show().map_err(|error| error.to_string())?;

    // `wait_for_action` blocks until the notification is actioned or closed, so
    // run it off the command thread.
    let activate_app = app.clone();
    std::thread::spawn(move || {
        handle.wait_for_action(|action| {
            if action == "default" {
                activate_main_window(&activate_app, id);
            }
        });
    });
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn show_platform_notification(
    _app: AppHandle,
    _id: String,
    title: String,
    body: Option<String>,
    _silent: bool,
) -> Result<(), String> {
    use notify_rust::Notification;

    // notify-rust routes through mac-notification-sys on macOS, whose action
    // callbacks require a signed application bundle. For now we only display the
    // toast; click-through activation can be added later via the UserNotifications
    // framework (UNUserNotificationCenter).
    let mut notification = Notification::new();
    notification.summary(&title);
    if let Some(body) = body.as_deref() {
        notification.body(body);
    }
    notification.show().map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(target_os = "android")]
pub(crate) async fn show_platform_notification(
    app: AppHandle,
    id: String,
    title: String,
    body: Option<String>,
    silent: bool,
) -> Result<(), String> {
    android::show(app, id, title, body, silent).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pins the permission contract inherited from tauri-plugin-notification's
    // always-granted desktop backend; ReleasePanel branches on this string.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    #[tokio::test]
    async fn permission_commands_report_granted() {
        assert_eq!(get_os_notification_permission().await, "granted");
        assert_eq!(request_os_notification_permission().await, "granted");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn permission_commands_require_notification_service() {
        // 子processだけを無効なbusへ向け、他のtestやホストsessionを変更しない。
        run_permission_probe("unix:path=/dev/null", "unavailable");
    }

    #[cfg(target_os = "linux")]
    fn run_permission_probe(address: &str, expected: &str) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::os_notification::tests::permission_probe_child",
                "--nocapture",
            ])
            .env("KUKURI_889_PERMISSION_EXPECTED", expected)
            .env("DBUS_SESSION_BUS_ADDRESS", address)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn permission_probe_child() {
        let Ok(expected) = std::env::var("KUKURI_889_PERMISSION_EXPECTED") else {
            return;
        };
        assert_eq!(get_os_notification_permission().await, expected);
        assert_eq!(request_os_notification_permission().await, expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_notification_honors_silent_without_changing_body() {
        for silent in [true, false] {
            let notification = linux_notification("test", Some("preview"), silent);
            assert_eq!(notification.summary, "test");
            assert_eq!(notification.body, "preview");
            assert!(
                notification
                    .hints
                    .contains(&notify_rust::Hint::SuppressSound(silent))
            );
        }
        assert!(linux_notification("test", None, true).body.is_empty());
    }

    #[cfg(target_os = "linux")]
    mod isolated_service {
        use super::*;
        use std::{
            io::{BufRead, BufReader},
            process::{Child, Command, Stdio},
            sync::{
                Arc,
                atomic::{AtomicU8, AtomicUsize, Ordering},
            },
        };

        struct TestBus(Child);

        impl Drop for TestBus {
            fn drop(&mut self) {
                // このtestが起動した隔離busだけを終了する。
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        struct TestNotifications {
            mode: Arc<AtomicU8>,
            probes: Arc<AtomicUsize>,
            notifications: Arc<AtomicUsize>,
        }

        #[zbus::interface(name = "org.freedesktop.Notifications")]
        impl TestNotifications {
            async fn get_server_information(
                &self,
            ) -> zbus::fdo::Result<(String, String, String, String)> {
                self.probes.fetch_add(1, Ordering::SeqCst);
                match self.mode.load(Ordering::SeqCst) {
                    1 => return Err(zbus::fdo::Error::AccessDenied("fixture".into())),
                    2 => std::future::pending::<()>().await,
                    _ => {}
                }
                Ok((
                    "fixture".into(),
                    "kukuri-test".into(),
                    "1".into(),
                    "1.3".into(),
                ))
            }

            #[allow(clippy::too_many_arguments)]
            fn notify(
                &self,
                _app_name: &str,
                _replaces_id: u32,
                _app_icon: &str,
                _summary: &str,
                _body: &str,
                _actions: Vec<String>,
                _hints: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
                _expire_timeout: i32,
            ) -> u32 {
                self.notifications.fetch_add(1, Ordering::SeqCst);
                1
            }
        }

        #[tokio::test]
        async fn permission_probe_handles_service_failure_timeout_and_recovery_without_toasts() {
            let mut bus = TestBus(
                Command::new("dbus-daemon")
                    .args(["--session", "--nofork", "--print-address=1"])
                    .stdout(Stdio::piped())
                    .spawn()
                    .expect("Linux notification contract requires dbus-daemon"),
            );
            let mut address = String::new();
            BufReader::new(bus.0.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            let address = address.trim().to_owned();
            let mode = Arc::new(AtomicU8::new(0));
            let probes = Arc::new(AtomicUsize::new(0));
            let notifications = Arc::new(AtomicUsize::new(0));
            let _service = zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .name("org.freedesktop.Notifications")
                .unwrap()
                .serve_at(
                    "/org/freedesktop/Notifications",
                    TestNotifications {
                        mode: mode.clone(),
                        probes: probes.clone(),
                        notifications: notifications.clone(),
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();

            for (state, expected) in [
                (0, "available"),
                (1, "unavailable"),
                (2, "unavailable"),
                (0, "available"),
            ] {
                mode.store(state, Ordering::SeqCst);
                let address = address.clone();
                tokio::task::spawn_blocking(move || run_permission_probe(&address, expected))
                    .await
                    .unwrap();
            }
            assert_eq!(probes.load(Ordering::SeqCst), 8);
            assert_eq!(notifications.load(Ordering::SeqCst), 0);
        }
    }
}
