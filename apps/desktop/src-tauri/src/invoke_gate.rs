use tauri::{Manager, Runtime, ipc::Invoke};

use crate::state::{CommandError, DesktopStartupState};

/// `generate_handler!`へ登録した全app commandを、引数解析やcommand本体より前に
/// host の受付の判定（`kukuri_desktop_runtime::admit_command`）へ通す。
pub(crate) fn with_desktop_startup_gate<R, F>(
    handler: F,
) -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static
where
    R: Runtime,
    F: Fn(Invoke<R>) -> bool + Send + Sync + 'static,
{
    move |invoke| {
        let command = invoke.message.command().to_string();
        let exiting = invoke
            .message
            .webview_ref()
            .try_state::<crate::desktop_lifecycle::DesktopLifecycle>()
            .is_some_and(|lifecycle| lifecycle.requested());
        let status = invoke
            .message
            .webview_ref()
            .try_state::<DesktopStartupState>()
            .map(|startup| startup.status());
        match kukuri_desktop_runtime::admit_command(&command, status.as_ref(), exiting) {
            Ok(()) => handler(invoke),
            Err(message) => {
                invoke.resolver.reject(CommandError::from(message));
                true
            }
        }
    }
}

/// アカウントの操作の調停が使う Tauri の state（排他・起動の状態・終了の要求・OS 通知の後始末）。
struct TauriGate<R: Runtime>(tauri::AppHandle<R>);

impl<R: Runtime> kukuri_desktop_runtime::ClientGate for TauriGate<R> {
    fn host(&self) -> Option<std::sync::Arc<kukuri_desktop_runtime::ClientHost>> {
        self.0
            .try_state::<crate::state::DesktopState>()
            .map(|state| state.host())
    }

    fn startup(&self) -> &DesktopStartupState {
        self.0.state::<DesktopStartupState>().inner()
    }

    fn operation_lock(&self) -> &tokio::sync::Mutex<()> {
        &self
            .0
            .state::<crate::restore_lifecycle::DesktopOperationState>()
            .inner()
            .switch_guard
    }

    fn require_running(&self) -> Result<(), CommandError> {
        crate::desktop_lifecycle::require_running(&self.0)
    }

    fn account_switched(&self) {
        self.0
            .state::<crate::commands::background_notifications::OsNotificationBackground>()
            .reset_for_account_switch();
    }
}

/// desktop-runtime の dispatch 表にある command をそこで呼び、それ以外（Tauri 専用・native だけの command）を
/// `generate_handler!` の handler へ渡す（W1 AC-5、ADR 0056 §6）。旧世代の結果は表が `stale_runtime` にする。
pub(crate) fn with_runtime_dispatch<R, F>(
    handler: F,
) -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static
where
    R: Runtime,
    F: Fn(Invoke<R>) -> bool + Send + Sync + 'static,
{
    let dispatched = kukuri_desktop_runtime::dispatched_commands()
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    move |invoke| {
        if !dispatched.contains(invoke.message.command()) {
            return handler(invoke);
        }
        let command = invoke.message.command().to_string();
        let args = match invoke.message.payload() {
            tauri::ipc::InvokeBody::Json(args) => args.clone(),
            tauri::ipc::InvokeBody::Raw(_) => serde_json::Value::Null,
        };
        let app = invoke.message.webview_ref().app_handle().clone();
        let ctx = kukuri_desktop_runtime::DispatchContext {
            app_version: app.package_info().version.to_string(),
        };
        let gate = TauriGate(app);
        invoke.resolver.respond_async(async move {
            kukuri_desktop_runtime::dispatch_command(&gate, &ctx, &command, args)
                .await
                .map_err(tauri::ipc::InvokeError::from)
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DesktopStartupStatus;
    use crate::state::consent_required_status;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tauri::Manager;

    // W1 AC-5: 表の command も受付の判定を先に通り、Ready の前は表へ届かない。起動の状態は表の command で読める。
    #[test]
    fn table_commands_pass_the_startup_gate_before_the_dispatch() {
        let app = tauri::test::mock_builder()
            .manage(DesktopStartupState::initializing())
            .invoke_handler(with_desktop_startup_gate(with_runtime_dispatch(
                |invoke: tauri::ipc::Invoke<tauri::test::MockRuntime>| {
                    invoke.resolver.resolve(());
                    true
                },
            )))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app without runtime");
        let webview = tauri::WebviewWindowBuilder::new(&app, "dispatch", Default::default())
            .build()
            .expect("mock webview");
        let call = |command: &str| {
            tauri::test::get_ipc_response(
                &webview,
                tauri::webview::InvokeRequest {
                    cmd: command.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: if cfg!(windows) {
                        "http://tauri.localhost"
                    } else {
                        "tauri://localhost"
                    }
                    .parse()
                    .unwrap(),
                    body: tauri::ipc::InvokeBody::Json(serde_json::json!({})),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.into(),
                },
            )
        };
        let rejected = format!("{:?}", call("create_post").unwrap_err());
        assert!(
            rejected.contains("requires Ready startup state"),
            "{rejected}"
        );
        let status: serde_json::Value = call("get_desktop_startup_status")
            .expect("startup status before Ready")
            .deserialize()
            .unwrap();
        assert_eq!(status, serde_json::json!({ "status": "initializing" }));

        app.state::<DesktopStartupState>()
            .set_status(DesktopStartupStatus::Ready);
        let unpublished = format!("{:?}", call("create_post").unwrap_err());
        assert!(
            unpublished.contains("the runtime is not ready"),
            "{unpublished}"
        );
    }

    #[test]
    fn locale_ipc_before_ready_does_not_construct_runtime_or_reach_protected_sinks() {
        let hits = Arc::new(AtomicUsize::new(0));
        let protected_hits = hits.clone();
        let locale_handler: fn(tauri::ipc::Invoke<tauri::test::MockRuntime>) -> bool =
            tauri::generate_handler![crate::commands::system_locale::get_system_locales];
        let app = tauri::test::mock_builder()
            .manage(DesktopStartupState::initializing())
            .invoke_handler(with_desktop_startup_gate(move |invoke| {
                if invoke.message.command() == "get_system_locales" {
                    locale_handler(invoke)
                } else {
                    protected_hits.fetch_add(1, Ordering::SeqCst);
                    invoke.resolver.resolve(());
                    true
                }
            }))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app without runtime");
        let webview = tauri::WebviewWindowBuilder::new(&app, "review", Default::default())
            .build()
            .expect("mock webview");
        let request = |command: &str| tauri::webview::InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: tauri::ipc::InvokeBody::Json(serde_json::json!({})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        };
        for status in [
            DesktopStartupStatus::Initializing,
            consent_required_status(&Default::default()),
            crate::state::failed_status(
                crate::state::StartupError::unknown("fixture".into()),
                None,
            ),
        ] {
            app.state::<DesktopStartupState>().set_status(status);
            let before = serde_json::to_value(app.state::<DesktopStartupState>().status()).unwrap();
            let result = tauri::test::get_ipc_response(&webview, request("get_system_locales"))
                .expect("read-only locale IPC succeeds before Ready");
            let _: Vec<String> = result.deserialize().expect("locale list");
            for command in [
                "create_post",
                "fetch_community_node_policies",
                "fetch_link_preview",
                "verify_profile_domain",
                "get_pending_device_restore_frontend_state",
                "acknowledge_pending_device_restore_frontend_state",
                "set_developer_mode_enabled",
                "read_desktop_logs",
            ] {
                assert!(
                    tauri::test::get_ipc_response(&webview, request(command)).is_err(),
                    "{command}"
                );
            }
            assert_eq!(hits.load(Ordering::SeqCst), 0);
            assert!(app.try_state::<crate::state::DesktopState>().is_none());
            assert_eq!(
                serde_json::to_value(app.state::<DesktopStartupState>().status()).unwrap(),
                before
            );
        }
        // positive control: fixtureの禁止sinkはReadyなら実際に到達できる。
        app.state::<DesktopStartupState>()
            .set_status(DesktopStartupStatus::Ready);
        assert!(tauri::test::get_ipc_response(&webview, request("create_post")).is_ok());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
