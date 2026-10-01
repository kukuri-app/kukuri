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
    }
}
