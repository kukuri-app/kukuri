use std::{collections::BTreeMap, path::PathBuf};

use kukuri_desktop_runtime::{
    AccountsSnapshot, CreateDeviceBackupRequest, DeviceBackupPhase, DeviceBackupPreview,
    DeviceBackupProgress, DeviceBackupRestoreResult, DeviceBackupSummary,
    PreviewDeviceBackupRequest, RestoreDeviceBackupRequest,
    acknowledge_pending_device_restore_frontend_state as acknowledge_restore_frontend_state,
    commit_device_restore, create_device_backup, install_prepared_device_restore, list_accounts,
    mark_device_restore_awaiting_consent,
    pending_device_restore_frontend_state as read_pending_restore_frontend_state,
    pending_device_restore_phase, prepare_device_restore, preview_device_backup,
    rollback_pending_device_restore, validate_prepared_device_restore,
};
use tauri::{Emitter, Manager};

use crate::commands::background_notifications::OsNotificationBackground;
use crate::restore_lifecycle::{DesktopOperationState, require_runtime_operation_ready};
use crate::state::{
    CommandError, DesktopStartupState, DesktopStartupStatus, DesktopState, StartupError,
    build_runtime, failed_status, map_error, reset_app_consent_after_device_restore,
};

const PROGRESS_EVENT: &str = "kukuri://device-backup-progress";

async fn rebuild_runtime(
    state: &DesktopState,
    db_path: PathBuf,
) -> Result<(), CommandError> {
    let runtime = build_runtime(db_path).await.map_err(|error| {
        CommandError::from(format!("failed to restart desktop runtime: {error}"))
    })?;
    let previous = state
        .host()
        .replace_runtime(runtime)
        .await
        .map_err(|error| {
            CommandError::from(format!("failed to activate desktop runtime: {error}"))
        })?;
    drop(previous);
    Ok(())
}

async fn restore_previous_runtime(
    app_handle: &tauri::AppHandle,
    state: &DesktopState,
    startup: &DesktopStartupState,
    db_path: PathBuf,
    operation_error: CommandError,
) -> CommandError {
    let failed_db_path = db_path.clone();
    match rebuild_runtime(state, db_path).await {
        Ok(()) => {
            app_handle
                .state::<OsNotificationBackground>()
                .reset_for_account_switch();
            startup.set_status(DesktopStartupStatus::Ready);
            operation_error
        }
        Err(restart_error) => {
            let message = format!(
                "{}; additionally failed to restart the previous runtime: {}",
                operation_error.message, restart_error.message
            );
            startup.set_status(failed_status(
                StartupError::unknown(message.clone()),
                Some(failed_db_path),
            ));
            CommandError::from(message)
        }
    }
}

fn verify_previous_account_restored(
    expected_accounts: &AccountsSnapshot,
    pending_phase: Option<kukuri_desktop_runtime::DeviceRestorePhase>,
    actual_accounts: &AccountsSnapshot,
) -> Result<(), String> {
    if let Some(phase) = pending_phase {
        return Err(format!(
            "device restore rollback left pending journal phase {phase:?}"
        ));
    }
    if actual_accounts != expected_accounts {
        return Err(
            "device restore rollback did not restore the previous account registry".to_string(),
        );
    }
    Ok(())
}

/// restoreの失敗経路はjournalとregistryの両方が旧状態へ戻ったことを確認できた場合だけ
/// 旧runtimeを再構築する。確認不能・rollback失敗では停止済みのままFailedへ閉じる。
async fn restore_previous_runtime_after_rollback(
    app_handle: &tauri::AppHandle,
    state: &DesktopState,
    startup: &DesktopStartupState,
    previous_db_path: PathBuf,
    previous_accounts: &AccountsSnapshot,
    operation_error: CommandError,
) -> CommandError {
    let rollback_result = rollback_pending_device_restore(&state.app_data_dir);
    let verification = rollback_result
        .map_err(|error| format!("failed to roll back pending device restore: {error:#}"))
        .and_then(|()| {
            let pending = pending_device_restore_phase(&state.app_data_dir)
                .map_err(|error| format!("failed to verify restore journal cleanup: {error:#}"))?;
            let snapshot = list_accounts(&state.app_data_dir).map_err(|error| {
                format!("failed to verify restored account registry: {error:#}")
            })?;
            verify_previous_account_restored(previous_accounts, pending, &snapshot)
        });

    if let Err(rollback_error) = verification {
        let message = format!("{}; additionally {rollback_error}", operation_error.message);
        startup.set_status(failed_status(StartupError::unknown(message.clone()), None));
        return CommandError::from(message);
    }

    restore_previous_runtime(
        app_handle,
        state,
        startup,
        previous_db_path,
        operation_error,
    )
    .await
}

#[tauri::command]
pub async fn create_device_backup_command(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    operation: tauri::State<'_, DesktopOperationState>,
    request: CreateDeviceBackupRequest,
) -> Result<DeviceBackupSummary, CommandError> {
    let _guard = operation.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app_handle)?;
    let startup = app_handle.state::<DesktopStartupState>();
    require_runtime_operation_ready(&startup.status()).map_err(CommandError::from)?;
    operation.begin_cancellable_device_backup();
    startup.set_status(DesktopStartupStatus::Initializing);

    let current = state.runtime();
    let db_path = current.db_path().to_path_buf();
    // #1221 R5-G: backup は保護所有先だけを含めるため、旧 `iroh-data` からの移行の残りを先に写す。
    let migrated = current.finish_protected_migration().await;
    current.shutdown().await;

    let restart_path = db_path.clone();
    if let Err(error) = migrated {
        return Err(restore_previous_runtime(
            &app_handle,
            &state,
            &startup,
            restart_path,
            map_error(error),
        )
        .await);
    }
    let app_data_dir = state.app_data_dir.clone();
    let cancellation = operation.device_backup_cancellation();
    let progress_app = app_handle.clone();
    let operation = tauri::async_runtime::spawn_blocking(move || {
        create_device_backup(
            &app_data_dir,
            &db_path,
            &request,
            &cancellation,
            |progress| {
                let _ = progress_app.emit(PROGRESS_EVENT, progress);
            },
        )
    })
    .await;
    let operation = match operation {
        Ok(operation) => operation,
        Err(error) => {
            return Err(restore_previous_runtime(
                &app_handle,
                &state,
                &startup,
                restart_path,
                CommandError::from(format!("device backup task failed: {error}")),
            )
            .await);
        }
    };

    match operation.map_err(map_error) {
        Ok(summary) => match rebuild_runtime(&state, restart_path.clone()).await {
            Ok(()) => {
                app_handle
                    .state::<OsNotificationBackground>()
                    .reset_for_account_switch();
                startup.set_status(DesktopStartupStatus::Ready);
                Ok(summary)
            }
            Err(error) => {
                Err(
                    restore_previous_runtime(&app_handle, &state, &startup, restart_path, error)
                        .await,
                )
            }
        },
        Err(error) => {
            Err(restore_previous_runtime(&app_handle, &state, &startup, restart_path, error).await)
        }
    }
}

#[tauri::command]
pub async fn preview_device_backup_command(
    state: tauri::State<'_, DesktopState>,
    request: PreviewDeviceBackupRequest,
) -> Result<DeviceBackupPreview, CommandError> {
    let app_data_dir = state.app_data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || preview_device_backup(&app_data_dir, &request))
        .await
        .map_err(|error| CommandError::from(format!("device backup preview task failed: {error}")))?
        .map_err(map_error)
}

#[tauri::command]
pub async fn restore_device_backup_command(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    operation: tauri::State<'_, DesktopOperationState>,
    request: RestoreDeviceBackupRequest,
) -> Result<DeviceBackupRestoreResult, CommandError> {
    let _guard = operation.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app_handle)?;
    let startup = app_handle.state::<DesktopStartupState>();
    require_runtime_operation_ready(&startup.status()).map_err(CommandError::from)?;
    match pending_device_restore_phase(&state.app_data_dir).map_err(map_error)? {
        None => {}
        Some(phase) => {
            return Err(CommandError::from(format!(
                "device restore transaction is already pending in phase {phase:?}"
            )));
        }
    }
    operation.begin_cancellable_device_backup();

    let previous_accounts = list_accounts(&state.app_data_dir).map_err(map_error)?;
    startup.set_status(DesktopStartupStatus::Initializing);

    let current = state.runtime();
    let previous_db_path = current.db_path().to_path_buf();
    current.shutdown().await;

    let app_data_dir = state.app_data_dir.clone();
    let cancellation = operation.device_backup_cancellation();
    let progress_app = app_handle.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        prepare_device_restore(&app_data_dir, &request, &cancellation, |progress| {
            let _ = progress_app.emit(PROGRESS_EVENT, progress);
        })
    })
    .await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            return Err(restore_previous_runtime_after_rollback(
                &app_handle,
                &state,
                &startup,
                previous_db_path,
                &previous_accounts,
                CommandError::from(format!("device restore task failed: {error}")),
            )
            .await);
        }
    };
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            return Err(restore_previous_runtime_after_rollback(
                &app_handle,
                &state,
                &startup,
                previous_db_path,
                &previous_accounts,
                map_error(error),
            )
            .await);
        }
    };

    // Staging検証はDBとarchive整合性だけを確認し、Iroh/runtime/taskを構築しない。
    if let Err(error) = validate_prepared_device_restore(&prepared).await {
        drop(prepared);
        return Err(restore_previous_runtime_after_rollback(
            &app_handle,
            &state,
            &startup,
            previous_db_path,
            &previous_accounts,
            CommandError::from(format!("restored account validation failed: {error:#}")),
        )
        .await);
    }

    if let Err(error) = operation.close_device_backup_cancel_gate() {
        drop(prepared);
        return Err(restore_previous_runtime_after_rollback(
            &app_handle,
            &state,
            &startup,
            previous_db_path,
            &previous_accounts,
            CommandError::from(format!(
                "device restore canceled before installation: {error:#}"
            )),
        )
        .await);
    }

    let _ = app_handle.emit(
        PROGRESS_EVENT,
        DeviceBackupProgress {
            phase: DeviceBackupPhase::Installing,
            completed_bytes: 0,
            total_bytes: 0,
        },
    );

    let install_app_data = state.app_data_dir.clone();
    let install_cancellation = operation.device_backup_cancellation();
    let installed = tauri::async_runtime::spawn_blocking(move || {
        // durable installへ入る最後の境界。ここより後に到着したcancelはinstall完了後に
        // journalからrollbackし、部分適用を残さない。
        install_cancellation.check()?;
        install_prepared_device_restore(&install_app_data, prepared)
    })
    .await;
    let installed = match installed {
        Ok(installed) => installed,
        Err(error) => {
            return Err(restore_previous_runtime_after_rollback(
                &app_handle,
                &state,
                &startup,
                previous_db_path,
                &previous_accounts,
                CommandError::from(format!("device restore install task failed: {error}")),
            )
            .await);
        }
    };
    let installed = match installed {
        Ok(installed) => installed,
        Err(error) => {
            return Err(restore_previous_runtime_after_rollback(
                &app_handle,
                &state,
                &startup,
                previous_db_path,
                &previous_accounts,
                map_error(error),
            )
            .await);
        }
    };

    if let Err(error) = operation.device_backup_cancellation().check() {
        return Err(restore_previous_runtime_after_rollback(
            &app_handle,
            &state,
            &startup,
            previous_db_path,
            &previous_accounts,
            CommandError::from(format!(
                "device restore canceled after installation: {error:#}"
            )),
        )
        .await);
    }

    let result = match commit_device_restore(&installed) {
        Ok(result) => result,
        Err(error) => {
            return Err(restore_previous_runtime_after_rollback(
                &app_handle,
                &state,
                &startup,
                previous_db_path,
                &previous_accounts,
                CommandError::from(format!("failed to commit restored account: {error:#}")),
            )
            .await);
        }
    };

    let consent_status = match reset_app_consent_after_device_restore(&app_handle) {
        Ok(status) => status,
        Err(error) => {
            // registry commit後は旧runtimeへ戻さない。Committed journalを残し、次回起動で
            // consent resetを再試行するまでfail-closedにする。
            let message = format!("failed to reset consent after device restore: {error}");
            startup.set_status(failed_status(
                StartupError::unknown(message.clone()),
                Some(installed.db_path()),
            ));
            return Err(CommandError::from(message));
        }
    };

    if let Err(error) = mark_device_restore_awaiting_consent(&state.app_data_dir) {
        // consent resetは完了済みでもjournalがCommittedなら、startup recoveryが同じresetを
        // 安全に再実行してAwaitingConsentへ進める。
        let message = format!("failed to persist restore consent gate: {error:#}");
        startup.set_status(failed_status(
            StartupError::unknown(message.clone()),
            Some(installed.db_path()),
        ));
        return Err(CommandError::from(message));
    }

    // 復元runtimeはここでは構築・公開しない。明示的な再同意後のactivationだけが
    // AwaitingConsentをReadyへ遷移させる。
    startup.set_status(consent_status);
    let _ = app_handle.emit(
        PROGRESS_EVENT,
        DeviceBackupProgress {
            phase: DeviceBackupPhase::Installing,
            completed_bytes: 1,
            total_bytes: 1,
        },
    );
    Ok(result)
}

#[tauri::command]
pub fn cancel_device_backup(operation: tauri::State<'_, DesktopOperationState>) {
    operation.cancel_device_backup();
}

#[tauri::command]
pub fn get_pending_device_restore_frontend_state(
    app_handle: tauri::AppHandle,
) -> Result<Option<BTreeMap<String, String>>, CommandError> {
    let app_data_dir = crate::state::resolve_app_data_dir(&app_handle)?;
    read_pending_restore_frontend_state(&app_data_dir).map_err(map_error)
}

#[tauri::command]
pub fn acknowledge_pending_device_restore_frontend_state(
    app_handle: tauri::AppHandle,
) -> Result<(), CommandError> {
    let app_data_dir = crate::state::resolve_app_data_dir(&app_handle)?;
    acknowledge_restore_frontend_state(&app_data_dir).map_err(map_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kukuri_desktop_runtime::DeviceRestorePhase;

    #[test]
    fn previous_runtime_restart_requires_clean_journal_and_original_registry() {
        let old = AccountsSnapshot {
            active_account_id: "old".to_string(),
            accounts: Vec::new(),
        };
        let restored = AccountsSnapshot {
            active_account_id: "restored".to_string(),
            accounts: Vec::new(),
        };
        assert!(verify_previous_account_restored(&old, None, &old).is_ok());
        assert!(
            verify_previous_account_restored(&old, Some(DeviceRestorePhase::Installed), &old)
                .is_err()
        );
        assert!(verify_previous_account_restored(&old, None, &restored).is_err());
    }
}
