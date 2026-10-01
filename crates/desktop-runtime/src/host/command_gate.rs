//! 起動中・終了中の command の受付（UI adapter に依らない判定。Tauri と web-runtime が同じ判定を使う。ADR 0056 §6）。
//! 判定は引数の解析や command の本体より前に行い、断った command は runtime に触れない。

use super::ClientStartupStatus;

/// 起動が Ready でなくても受け付ける command。
pub const NON_READY_COMMAND_ALLOWLIST: &[&str] = &[
    "get_desktop_startup_status",
    "get_system_locales",
    "get_app_consent_status",
    "accept_app_consents",
    "cancel_device_backup",
    // Window close confirmation is owned by the app shell and must remain
    // answerable while runtime startup or consent is still pending.
    "get_pending_window_close_request",
    "respond_window_close_request",
];

/// 終了の要求の後も受け付ける command。
const EXIT_COMMAND_ALLOWLIST: &[&str] = &["cancel_device_backup", "get_desktop_startup_status"];

/// `command` を受け付けるか。`status` は起動の状態（まだ無ければ `None`）、`exiting` は終了の要求の有無。
/// 断るときは返す文を返す。
pub fn admit_command(
    command: &str,
    status: Option<&ClientStartupStatus>,
    exiting: bool,
) -> Result<(), String> {
    if exiting && !EXIT_COMMAND_ALLOWLIST.contains(&command) {
        return Err("アプリを終了しています。".to_string());
    }
    match status {
        Some(ClientStartupStatus::Ready) => Ok(()),
        Some(_) if NON_READY_COMMAND_ALLOWLIST.contains(&command) => Ok(()),
        Some(status) => Err(format!(
            "desktop command `{command}` requires Ready startup state; current state is {status:?}"
        )),
        None => Err(format!(
            "desktop command `{command}` requires Ready startup state; startup state is unavailable"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent_required_status;

    const READY: Option<&ClientStartupStatus> = Some(&ClientStartupStatus::Ready);

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
            "get_system_locales",
        ] {
            assert!(admit_command(command, READY, true).is_err(), "{command}");
            assert!(admit_command(command, READY, false).is_ok(), "{command}");
        }
        for command in EXIT_COMMAND_ALLOWLIST {
            assert!(admit_command(command, READY, true).is_ok(), "{command}");
        }
    }

    #[test]
    fn consent_required_rejects_network_sink_and_ready_allows_it() {
        let consent_required = consent_required_status(&Default::default());
        for command in ["fetch_community_node_policies", "fetch_link_preview"] {
            assert!(admit_command(command, Some(&consent_required), false).is_err());
            assert!(admit_command(command, READY, false).is_ok());
        }
    }

    #[test]
    fn non_ready_allowlist_is_minimal_and_restore_frontend_state_remains_gated() {
        let status = Some(&ClientStartupStatus::Initializing);
        assert_eq!(
            NON_READY_COMMAND_ALLOWLIST,
            [
                "get_desktop_startup_status",
                "get_system_locales",
                "get_app_consent_status",
                "accept_app_consents",
                "cancel_device_backup",
                "get_pending_window_close_request",
                "respond_window_close_request",
            ]
        );
        for command in NON_READY_COMMAND_ALLOWLIST {
            assert!(admit_command(command, status, false).is_ok(), "{command}");
            assert!(admit_command(command, None, false).is_err(), "{command}");
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
            assert!(admit_command(command, status, false).is_err(), "{command}");
        }
    }
}
