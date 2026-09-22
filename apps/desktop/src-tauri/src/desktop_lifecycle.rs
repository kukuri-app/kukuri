use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    restore_lifecycle::DesktopOperationState,
    state::{CommandError, DesktopState},
};

#[derive(Default)]
pub(crate) struct DesktopLifecycle {
    requested: AtomicBool,
    completed: AtomicBool,
    tray_created: AtomicBool,
}

const WINDOW_CLOSE_PREFERENCE_FILE: &str = "window-close-preference.json";
const WINDOW_CLOSE_REQUESTED_EVENT: &str = "kukuri://window-close-requested";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowCloseBehavior {
    Quit,
    Tray,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct WindowClosePreference {
    pub(crate) behavior: Option<WindowCloseBehavior>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WindowClosePrompt {
    pub(crate) request_id: u64,
}

pub(crate) struct WindowCloseState {
    preference: Mutex<WindowClosePreference>,
    preference_path: Option<PathBuf>,
    pending_request: Mutex<Option<WindowClosePrompt>>,
    next_request_id: AtomicU64,
}

impl WindowCloseState {
    pub(crate) fn new(app_data_dir: Option<PathBuf>) -> Self {
        let preference_path = app_data_dir.map(|dir| dir.join(WINDOW_CLOSE_PREFERENCE_FILE));
        let preference = preference_path
            .as_deref()
            .and_then(read_window_close_preference)
            .unwrap_or_default();
        Self {
            preference: Mutex::new(preference),
            preference_path,
            pending_request: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
        }
    }

    fn preference(&self) -> WindowClosePreference {
        *self.preference.lock().expect("window close preference lock poisoned")
    }

    fn set_preference(&self, next: WindowClosePreference) -> Result<(), CommandError> {
        let path = self.preference_path.as_deref().ok_or_else(|| {
            CommandError::from("ウィンドウ終了設定の保存先を利用できません。".to_string())
        })?;
        persist_window_close_preference(path, next).map_err(CommandError::from)?;
        *self.preference.lock().expect("window close preference lock poisoned") = next;
        Ok(())
    }

    fn begin_prompt(&self) -> Option<WindowClosePrompt> {
        let mut pending = self
            .pending_request
            .lock()
            .expect("window close request lock poisoned");
        if pending.is_some() {
            return None;
        }
        let prompt = WindowClosePrompt {
            request_id: self.next_request_id.fetch_add(1, Ordering::SeqCst),
        };
        *pending = Some(prompt);
        Some(prompt)
    }

    fn pending_prompt(&self) -> Option<WindowClosePrompt> {
        *self
            .pending_request
            .lock()
            .expect("window close request lock poisoned")
    }

    fn ensure_pending(&self, request_id: u64) -> Result<(), CommandError> {
        if self.pending_prompt().is_some_and(|pending| pending.request_id == request_id) {
            Ok(())
        } else {
            Err(CommandError::from(
                "ウィンドウ終了の確認要求は期限切れです。".to_string(),
            ))
        }
    }

    fn consume_pending(&self, request_id: u64) -> Result<(), CommandError> {
        let mut pending = self
            .pending_request
            .lock()
            .expect("window close request lock poisoned");
        if pending.is_some_and(|current| current.request_id == request_id) {
            *pending = None;
            Ok(())
        } else {
            Err(CommandError::from(
                "ウィンドウ終了の確認要求は期限切れです。".to_string(),
            ))
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ExitAction {
    Quit,
    Restart,
}

impl DesktopLifecycle {
    pub(crate) fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    pub(crate) fn completed(&self) -> bool {
        self.completed.load(Ordering::SeqCst)
    }

    fn begin(&self) -> bool {
        !self.requested.swap(true, Ordering::SeqCst)
    }

    fn ensure_running(&self) -> Result<(), CommandError> {
        if self.requested() {
            Err(CommandError::from("アプリを終了しています。".to_string()))
        } else {
            Ok(())
        }
    }

    pub(crate) fn set_tray_created(&self, created: bool) {
        self.tray_created.store(created, Ordering::SeqCst);
    }

    async fn drain<F: std::future::Future<Output = ()>>(
        &self,
        operations: &tokio::sync::Mutex<()>,
        shutdown: F,
    ) {
        let _guard = operations.lock().await;
        shutdown.await;
        self.completed.store(true, Ordering::SeqCst);
    }
}

fn read_window_close_preference(path: &Path) -> Option<WindowClosePreference> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "failed to read window close preference");
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(preference) => Some(preference),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "ignored invalid window close preference");
            None
        }
    }
}

fn persist_window_close_preference(
    path: &Path,
    preference: WindowClosePreference,
) -> anyhow::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid preference path `{}`", path.display()))?;
    let temp_path = path.with_file_name(format!("{file_name}.tmp"));
    let bytes = serde_json::to_vec(&preference)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp_path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    replace_file(&temp_path, path)?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(std::io::Error::other)
}

fn should_hide_on_close(created: bool, registered: bool) -> bool {
    created && registered
}

#[cfg(any(target_os = "linux", test))]
fn tray_item_service(item: &str) -> &str {
    // GNOMEはbus@path、KDE系はbus/pathまたはservice名だけを返す。
    item.split(['@', '/']).next().unwrap_or_default()
}

#[cfg(target_os = "linux")]
async fn tray_registered() -> bool {
    // オブジェクト生成成功だけではトレイの表示先が存在するとは限らない。
    let check = async {
        let connection = zbus::Connection::session().await?;
        let watcher = zbus::Proxy::new(
            &connection,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
        )
        .await?;
        if !watcher
            .get_property::<bool>("IsStatusNotifierHostRegistered")
            .await?
        {
            return Ok::<bool, zbus::Error>(false);
        }
        let items = watcher
            .get_property::<Vec<String>>("RegisteredStatusNotifierItems")
            .await?;
        let bus = zbus::Proxy::new(
            &connection,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await?;
        for item in items {
            let service = tray_item_service(&item);
            let pid: Result<u32, zbus::Error> =
                bus.call("GetConnectionUnixProcessID", &(service,)).await;
            if pid.is_ok_and(|pid| pid == std::process::id()) {
                return Ok(true);
            }
        }
        Ok(false)
    };
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), check).await,
        Ok(Ok(true))
    )
}

async fn perform_window_close(window: tauri::Window, behavior: WindowCloseBehavior) {
    let app = window.app_handle();
    let lifecycle = app.state::<DesktopLifecycle>();
    if lifecycle.requested() {
        return;
    }
    if behavior == WindowCloseBehavior::Quit {
        request_exit(app, ExitAction::Quit);
        return;
    }
    let created = lifecycle.tray_created.load(Ordering::SeqCst);
    #[cfg(target_os = "linux")]
    let registered = created && tray_registered().await;
    #[cfg(not(target_os = "linux"))]
    let registered = created;
    if lifecycle.requested() {
        return;
    }
    if should_hide_on_close(created, registered) {
        let _ = window.hide();
    } else {
        request_exit(app, ExitAction::Quit);
    }
}

pub(crate) fn close_window(window: tauri::Window) {
    tauri::async_runtime::spawn(async move {
        let app = window.app_handle();
        let lifecycle = app.state::<DesktopLifecycle>();
        if lifecycle.requested() {
            return;
        }
        let state = app.state::<WindowCloseState>();
        if let Some(behavior) = state.preference().behavior {
            perform_window_close(window, behavior).await;
            return;
        }
        if let Some(prompt) = state.begin_prompt()
            && let Err(error) = window.emit(WINDOW_CLOSE_REQUESTED_EVENT, prompt)
        {
            tracing::warn!(%error, "failed to emit window close confirmation request");
        }
    });
}

#[tauri::command]
pub(crate) fn get_window_close_preference(
    state: tauri::State<'_, WindowCloseState>,
) -> WindowClosePreference {
    state.preference()
}

#[tauri::command]
pub(crate) fn set_window_close_preference(
    state: tauri::State<'_, WindowCloseState>,
    preference: WindowClosePreference,
) -> Result<WindowClosePreference, CommandError> {
    state.set_preference(preference)?;
    Ok(preference)
}

#[tauri::command]
pub(crate) fn get_pending_window_close_request(
    state: tauri::State<'_, WindowCloseState>,
) -> Option<WindowClosePrompt> {
    state.pending_prompt()
}

#[tauri::command]
pub(crate) async fn respond_window_close_request(
    app_handle: AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, WindowCloseState>,
    request_id: u64,
    behavior: Option<WindowCloseBehavior>,
    remember: bool,
) -> Result<(), CommandError> {
    require_running(&app_handle)?;
    state.ensure_pending(request_id)?;
    let Some(behavior) = behavior else {
        state.consume_pending(request_id)?;
        return Ok(());
    };
    if remember {
        state.set_preference(WindowClosePreference {
            behavior: Some(behavior),
        })?;
    }
    state.consume_pending(request_id)?;
    perform_window_close(window, behavior).await;
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn watch_hidden_tray(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if app.state::<DesktopLifecycle>().requested() {
                return;
            }
            if let Some(window) = app.get_webview_window("main")
                && window.is_visible().is_ok_and(|visible| !visible)
                && !tray_registered().await
                && !app.state::<DesktopLifecycle>().requested()
            {
                // 隠した後に表示先を失った場合、復帰・終了操作を失わせない。
                crate::show_main_window(&app);
            }
        }
    });
}

/// 排他待ち中に終了が始まる場合もあるため、各操作はlock取得後にも呼ぶ。
pub(crate) fn require_running<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<(), CommandError> {
    app.state::<DesktopLifecycle>().ensure_running()
}

pub(crate) fn request_exit(app: &AppHandle, action: ExitAction) {
    if !app.state::<DesktopLifecycle>().begin() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let lifecycle = app.state::<DesktopLifecycle>();
        let operations = app.state::<DesktopOperationState>();
        lifecycle
            .drain(&operations.switch_guard, async {
                if let Some(state) = app.try_state::<DesktopState>() {
                    state.host().shutdown().await;
                }
            })
            .await;
        tracing::info!("desktop runtime shutdown completed");
        // TauriのrestartはExitRequestedで延期できないため、停止完了後にだけ要求する。
        match action {
            ExitAction::Quit => app.exit(0),
            ExitAction::Restart => app.request_restart(),
        }
    });
}

#[tauri::command]
pub(crate) fn restart_after_update(app_handle: AppHandle) -> Result<(), CommandError> {
    require_running(&app_handle)?;
    crate::app_update::require_installed(&app_handle).map_err(CommandError::from)?;
    request_exit(&app_handle, ExitAction::Restart);
    Ok(())
}

#[cfg(unix)]
pub(crate) fn watch_signals(app: AppHandle) -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    // handler登録をsetup中に済ませ、runtimeの起動待ちも終了経路へ含める。
    tauri::async_runtime::block_on(async move {
        let mut terminate = signal(SignalKind::terminate())?;
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut hangup = signal(SignalKind::hangup())?;
        tauri::async_runtime::spawn(async move {
            tokio::select! {
                _ = terminate.recv() => {},
                _ = interrupt.recv() => {},
                _ = hangup.recv() => {},
            }
            request_exit(&app, ExitAction::Quit);
        });
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn close_only_hides_when_tray_was_created_and_has_a_registered_host() {
        assert!(should_hide_on_close(true, true));
        assert!(!should_hide_on_close(false, true));
        assert!(!should_hide_on_close(true, false));
        assert!(!should_hide_on_close(false, false));
    }

    #[test]
    fn missing_or_corrupt_preference_remains_an_explicit_question() {
        let dir = tempfile::tempdir().unwrap();
        let state = WindowCloseState::new(Some(dir.path().to_path_buf()));
        assert_eq!(state.preference().behavior, None);

        std::fs::write(
            dir.path().join(WINDOW_CLOSE_PREFERENCE_FILE),
            b"not valid json",
        )
        .unwrap();
        let reloaded = WindowCloseState::new(Some(dir.path().to_path_buf()));
        assert_eq!(reloaded.preference().behavior, None);
    }

    #[test]
    fn preference_is_persisted_and_reloaded_for_each_close_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let state = WindowCloseState::new(Some(dir.path().to_path_buf()));
        for behavior in [WindowCloseBehavior::Quit, WindowCloseBehavior::Tray] {
            state
                .set_preference(WindowClosePreference {
                    behavior: Some(behavior),
                })
                .unwrap();
            let reloaded = WindowCloseState::new(Some(dir.path().to_path_buf()));
            assert_eq!(reloaded.preference().behavior, Some(behavior));
        }
    }

    #[test]
    fn failed_persistence_does_not_change_the_effective_preference() {
        let dir = tempfile::tempdir().unwrap();
        let blocked_parent = dir.path().join("blocked");
        std::fs::write(&blocked_parent, b"file, not directory").unwrap();
        let state = WindowCloseState::new(Some(blocked_parent));
        assert!(
            state
                .set_preference(WindowClosePreference {
                    behavior: Some(WindowCloseBehavior::Tray),
                })
                .is_err()
        );
        assert_eq!(state.preference().behavior, None);
    }

    #[test]
    fn close_prompt_is_bounded_and_consumed_only_by_the_matching_response() {
        let state = WindowCloseState::new(None);
        let prompt = state.begin_prompt().expect("first close opens the prompt");
        assert!(state.begin_prompt().is_none(), "repeated close is coalesced");
        assert!(state.consume_pending(prompt.request_id + 1).is_err());
        assert_eq!(state.pending_prompt(), Some(prompt));
        state.consume_pending(prompt.request_id).unwrap();
        assert!(state.pending_prompt().is_none());
        assert!(state.begin_prompt().is_some());
    }

    #[test]
    fn tray_item_service_accepts_gnome_and_kde_registration_formats() {
        assert_eq!(
            tray_item_service(":1.42@/org/ayatana/NotificationItem/kukuri"),
            ":1.42"
        );
        assert_eq!(tray_item_service(":1.42/StatusNotifierItem"), ":1.42");
        assert_eq!(
            tray_item_service("org.kde.StatusNotifierItem-123-1"),
            "org.kde.StatusNotifierItem-123-1"
        );
        assert_eq!(tray_item_service(""), "");
    }

    #[tokio::test]
    async fn operation_queued_before_exit_does_not_recreate_runtime() {
        let lifecycle = DesktopLifecycle::default();
        let operations = tokio::sync::Mutex::new(());
        let guard = operations.lock().await;
        let mutations = AtomicBool::new(false);
        let pending = async {
            let _guard = operations.lock().await;
            lifecycle.ensure_running()?;
            mutations.store(true, Ordering::SeqCst);
            Ok::<(), CommandError>(())
        };
        assert!(lifecycle.begin());
        drop(guard);
        assert!(pending.await.is_err());
        assert!(!mutations.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn exit_waits_for_initialization_or_restore_before_stopping_latest_runtime() {
        let lifecycle = Arc::new(DesktopLifecycle::default());
        let operations = Arc::new(tokio::sync::Mutex::new(()));
        let guard = operations.lock().await;
        let stopped = Arc::new(AtomicBool::new(false));
        assert!(lifecycle.begin());
        assert!(
            !lifecycle.begin(),
            "duplicate exit must not spawn another shutdown"
        );
        let task = {
            let lifecycle = lifecycle.clone();
            let operations = operations.clone();
            let stopped = stopped.clone();
            tokio::spawn(async move {
                lifecycle
                    .drain(&operations, async {
                        stopped.store(true, Ordering::SeqCst);
                    })
                    .await;
            })
        };
        tokio::task::yield_now().await;
        assert!(lifecycle.requested());
        assert!(!lifecycle.completed());
        assert!(!stopped.load(Ordering::SeqCst));
        drop(guard);
        task.await.unwrap();
        assert!(stopped.load(Ordering::SeqCst));
        assert!(lifecycle.completed());
    }

    #[tokio::test]
    async fn exit_drains_host_published_during_initialization_and_preserves_its_database() {
        use kukuri_core::{ChannelRef, TimelineScope};
        use kukuri_desktop_runtime::{
            ClientHost, CreatePostRequest, DesktopRuntime, ListTimelineRequest,
        };
        use std::{sync::RwLock, time::Duration};

        let dir = std::env::temp_dir().join(format!(
            "kukuri-exit-late-host-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir(&dir).expect("create isolated profile");
        let db_path = dir.join("kukuri.db");
        // A persisted file backend keeps this fixture out of the user's OS keyring,
        // without changing process-wide environment variables or production APIs.
        let keys = kukuri_core::KukuriKeys::generate();
        std::fs::write(
            db_path.with_extension("identity-key"),
            keys.export_secret_hex(),
        )
        .expect("seed fixture identity");
        std::fs::write(db_path.with_extension("identity-store"), b"file")
            .expect("seed fixture backend");

        let lifecycle = DesktopLifecycle::default();
        let operations = tokio::sync::Mutex::new(());
        let operation = operations.lock().await;
        let published = RwLock::new(None::<Arc<ClientHost>>);
        assert!(lifecycle.begin());
        let drain = lifecycle.drain(&operations, async {
            let latest = published.read().expect("published host").clone();
            latest
                .expect("initialization published its host")
                .shutdown()
                .await;
        });
        tokio::pin!(drain);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut drain)
                .await
                .is_err()
        );
        assert!(!lifecycle.completed());

        // Complete an already-running initialization only after exit has begun.
        // The shutdown closure must resolve the host after acquiring the mutex.
        let runtime = Arc::new(
            tokio::time::timeout(Duration::from_secs(60), DesktopRuntime::new(&db_path))
                .await
                .expect("runtime creation timeout")
                .expect("runtime"),
        );
        let host = ClientHost::from_runtime(dir.clone(), runtime)
            .await
            .expect("host");
        let before = host
            .runtime()
            .get_sync_status()
            .await
            .expect("original status");
        let topic = "kukuri:topic:exit-late-host";
        let post = host
            .runtime()
            .create_post(CreatePostRequest {
                topic: topic.into(),
                content: "preserved through queued exit".into(),
                reply_to: None,
                channel_ref: ChannelRef::Public,
                attachments: Vec::new(),
                content_labels: Vec::new(),
            })
            .await
            .expect("fixture post");
        *published.write().expect("publish host") = Some(host.clone());
        drop(operation);
        tokio::time::timeout(Duration::from_secs(60), &mut drain)
            .await
            .expect("shutdown timeout");
        assert!(lifecycle.completed());
        assert!(
            host.restart_runtime(&db_path).await.is_err(),
            "host is stopped"
        );
        published.write().expect("unpublish host").take();
        drop(host);

        let restarted =
            tokio::time::timeout(Duration::from_secs(60), DesktopRuntime::new(&db_path))
                .await
                .expect("database reopen timeout")
                .expect("database reopens after late host shutdown");
        let after = restarted.get_sync_status().await.expect("restarted status");
        assert_eq!(after.local_author_pubkey, before.local_author_pubkey);
        assert_eq!(
            after.discovery.local_endpoint_id,
            before.discovery.local_endpoint_id
        );
        let timeline = restarted
            .list_timeline(ListTimelineRequest {
                topic: topic.into(),
                scope: TimelineScope::Public,
                cursor: None,
                limit: Some(20),
            })
            .await
            .expect("persisted timeline");
        assert!(timeline.items.iter().any(|item| item.object_id == post));
        tokio::time::timeout(Duration::from_secs(60), restarted.shutdown())
            .await
            .expect("restarted shutdown timeout");
        drop(restarted);
        std::fs::remove_dir_all(&dir).expect("remove isolated fixture profile");
    }
}
