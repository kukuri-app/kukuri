use tauri::{Runtime, ipc::Invoke};

use crate::state::{CommandError, DesktopStartupState, DesktopStartupStatus};

const NON_READY_COMMAND_ALLOWLIST: &[&str] = &[
    "get_desktop_startup_status",
    "get_system_locales",
    "get_app_consent_status",
    "accept_app_consents",
    "cancel_device_backup",
    "get_pending_window_close_request",
    "respond_window_close_request",
];

fn command_allowed(command: &str, status: &DesktopStartupStatus) -> bool {
    matches!(status, DesktopStartupStatus::Ready) || NON_READY_COMMAND_ALLOWLIST.contains(&command)
}

fn command_allowed_during_exit(command: &str, stopping: bool) -> bool {
    !stopping
        || matches!(
            command,
            "cancel_device_backup" | "get_desktop_startup_status"
        )
}

/// `generate_handler!`へ登録した全app commandを、引数解析やcommand本体より前に
/// 同じstartup gateへ通す。
pub(crate) fn with_desktop_startup_gate<R, F>(
    handler: F,
) -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static
where
    R: Runtime,
    F: Fn(Invoke<R>) -> bool + Send + Sync + 'static,
{
    move |invoke| {
        let command = invoke.message.command().to_string();
        let stopping = invoke
            .message
            .state_ref()
            .try_get::<crate::desktop_lifecycle::DesktopLifecycle>()
            .is_some_and(|lifecycle| lifecycle.requested());
        if !command_allowed_during_exit(&command, stopping) {
            invoke
                .resolver
                .reject(CommandError::from("アプリを終了しています。".to_string()));
            return true;
        }
        let status = invoke
            .message
            .state_ref()
            .try_get::<DesktopStartupState>()
            .map(|startup| startup.status());
        let allowed = status
            .as_ref()
            .is_some_and(|status| command_allowed(&command, status));
        if allowed {
            return handler(invoke);
        }

        let detail = status
            .map(|status| format!("current state is {status:?}"))
            .unwrap_or_else(|| "startup state is unavailable".to_string());
        invoke.resolver.reject(CommandError::from(format!(
            "desktop command `{command}` requires Ready startup state; {detail}"
        )));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::consent_required_status;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tauri::Manager;

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
        assert!(!command_allowed_during_exit("get_system_locales", true));
    }

    #[test]
    fn exit_rejects_new_operations_but_keeps_backup_cancellation_available() {
        for command in [
            "accept_app_consents",
            "switch_account",
            "restore_device_backup_command",
            "create_post",
            "restart_after_update",
            "check_app_update",
            "download_app_update",
            "install_app_update",
            "set_developer_mode_enabled",
            "read_desktop_logs",
        ] {
            assert!(!command_allowed_during_exit(command, true));
            assert!(command_allowed_during_exit(command, false));
        }
        assert!(command_allowed_during_exit("cancel_device_backup", true));
        assert!(command_allowed_during_exit(
            "get_desktop_startup_status",
            true
        ));
    }

    #[test]
    fn consent_required_rejects_network_sink_and_ready_allows_it() {
        let consent_required = consent_required_status(&Default::default());

        assert!(!command_allowed(
            "fetch_community_node_policies",
            &consent_required
        ));
        assert!(command_allowed(
            "fetch_community_node_policies",
            &DesktopStartupStatus::Ready
        ));
        assert!(!command_allowed("fetch_link_preview", &consent_required));
        assert!(command_allowed(
            "fetch_link_preview",
            &DesktopStartupStatus::Ready
        ));
    }

    #[test]
    fn non_ready_allowlist_is_minimal_and_restore_frontend_state_remains_gated() {
        let status = DesktopStartupStatus::Initializing;
        assert_eq!(
            NON_READY_COMMAND_ALLOWLIST,
            [
                "get_desktop_startup_status",
                "get_system_locales",
                "get_app_consent_status",
                "accept_app_consents",
                "cancel_device_backup",
                // Window close confirmation is owned by the app shell and must remain
                // answerable while runtime startup or consent is still pending.
                "get_pending_window_close_request",
                "respond_window_close_request",
            ]
        );
        for command in NON_READY_COMMAND_ALLOWLIST {
            assert!(command_allowed(command, &status), "{command}");
        }
        for command in [
            "get_pending_device_restore_frontend_state",
            "acknowledge_pending_device_restore_frontend_state",
            "preview_device_backup_command",
            "list_accounts",
            "check_app_update",
            "fetch_link_preview",
            "download_app_update",
            "install_app_update",
            "set_developer_mode_enabled",
            "read_desktop_logs",
        ] {
            assert!(!command_allowed(command, &status), "{command}");
        }
    }
}
