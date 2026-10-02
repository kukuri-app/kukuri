//! #859: アカウント鍵の export / import と複数アカウントの一覧・切替。
//!
//! 平文秘密鍵は IPC に載せない(export は暗号化 envelope のみ)。KDF(argon2id)は
//! 数百 ms ブロックするため `spawn_blocking` で実行する。

use kukuri_core::AccountTransferStatus;
use kukuri_desktop_runtime::{
    AccountKeyExport, AccountKeyImportPreview, AccountRecord, AccountTransferLink,
    AccountsSnapshot, DecideAccountTransferRequest, ExportAccountKeyRequest,
    ImportAccountKeyRequest, OpenAccountTransferRequest, PreviewAccountKeyImportRequest,
    SwitchAccountRequest,
};
use tauri::Manager;

use crate::commands::background_notifications::OsNotificationBackground;
use crate::restore_lifecycle::{DesktopOperationState, require_runtime_operation_ready};
use crate::state::{
    CommandError, DesktopStartupState, DesktopStartupStatus, DesktopState, map_error,
};

#[tauri::command]
pub async fn create_account(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    operation: tauri::State<'_, DesktopOperationState>,
    request: kukuri_desktop_runtime::CreateAccountRequest,
) -> Result<AccountRecord, CommandError> {
    let _guard = operation.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app_handle)?;
    let startup = app_handle.state::<DesktopStartupState>();
    require_runtime_operation_ready(&startup.status()).map_err(CommandError::from)?;
    startup.set_status(DesktopStartupStatus::Initializing);
    let result = state.host().create_account(request).await.map_err(map_error);
    if result.is_ok() { app_handle.state::<OsNotificationBackground>().reset_for_account_switch(); }
    if state.host().is_stopped() {
        startup.set_status(kukuri_desktop_runtime::failed_startup_status(kukuri_desktop_runtime::ClientStartupError::unknown("Account transition requires restart".to_string()), None));
    } else { startup.set_status(DesktopStartupStatus::Ready); }
    result
}

#[tauri::command]
pub async fn export_account_key(
    state: tauri::State<'_, DesktopState>,
    request: ExportAccountKeyRequest,
) -> Result<AccountKeyExport, CommandError> {
    let runtime = state.runtime();
    tauri::async_runtime::spawn_blocking(move || runtime.export_account_key(request))
        .await
        .map_err(|error| CommandError::from(format!("export task failed: {error}")))?
        .map_err(map_error)
}

/// #1211: QR・専用リンクの移行。招待のリンクは log へ出さない。
#[tauri::command]
pub async fn create_account_transfer_invite(
    state: tauri::State<'_, DesktopState>,
) -> Result<AccountTransferLink, CommandError> {
    state
        .runtime()
        .create_account_transfer_invite()
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn open_account_transfer(
    state: tauri::State<'_, DesktopState>,
    request: OpenAccountTransferRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .open_account_transfer(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn get_account_transfer_status(
    state: tauri::State<'_, DesktopState>,
) -> Result<AccountTransferStatus, CommandError> {
    state
        .runtime()
        .account_transfer_status()
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn decide_account_transfer(
    state: tauri::State<'_, DesktopState>,
    request: DecideAccountTransferRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .decide_account_transfer(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn cancel_account_transfer(
    state: tauri::State<'_, DesktopState>,
) -> Result<(), CommandError> {
    state
        .runtime()
        .cancel_account_transfer()
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn preview_account_key_import(
    state: tauri::State<'_, DesktopState>,
    request: PreviewAccountKeyImportRequest,
) -> Result<AccountKeyImportPreview, CommandError> {
    kukuri_desktop_runtime::preview_account_key_import(&state.app_data_dir, &request.export)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn import_account_key(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    operation: tauri::State<'_, DesktopOperationState>,
    request: ImportAccountKeyRequest,
) -> Result<AccountRecord, CommandError> {
    let _guard = operation.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app_handle)?;
    require_runtime_operation_ready(&app_handle.state::<DesktopStartupState>().status())
        .map_err(CommandError::from)?;
    state
        .host()
        .import_account_key(request.export, request.passphrase, request.label)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn logout_account(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    operation: tauri::State<'_, DesktopOperationState>,
    request: SwitchAccountRequest,
) -> Result<AccountRecord, CommandError> {
    let _guard = operation.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app_handle)?;
    let startup = app_handle.state::<DesktopStartupState>();
    require_runtime_operation_ready(&startup.status()).map_err(CommandError::from)?;
    startup.set_status(DesktopStartupStatus::Initializing);
    let result = state
        .host()
        .logout_account(&request.account_id)
        .await
        .map_err(map_error);
    if result.is_ok() {
        app_handle
            .state::<OsNotificationBackground>()
            .reset_for_account_switch();
    }
    if state.host().is_stopped() {
        startup.set_status(kukuri_desktop_runtime::failed_startup_status(
            kukuri_desktop_runtime::ClientStartupError::unknown(
                "Account transition requires restart".to_string(),
            ),
            None,
        ));
    } else {
        startup.set_status(DesktopStartupStatus::Ready);
    }
    result
}

#[tauri::command]
pub async fn get_profile_setup_required(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    operation: tauri::State<'_, DesktopOperationState>,
    request: SwitchAccountRequest,
) -> Result<bool, CommandError> {
    let _guard = operation.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app_handle)?;
    require_runtime_operation_ready(&app_handle.state::<DesktopStartupState>().status())
        .map_err(CommandError::from)?;
    state
        .host()
        .profile_setup_required(&request.account_id)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn get_account_display(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<kukuri_desktop_runtime::AccountDisplay>, CommandError> {
    state.host().account_display().await.map_err(map_error)
}

#[tauri::command]
pub async fn save_initial_profile(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    operation: tauri::State<'_, DesktopOperationState>,
    request: kukuri_desktop_runtime::InitialProfileRequest,
) -> Result<kukuri_core::Profile, CommandError> {
    let _guard = operation.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app_handle)?;
    require_runtime_operation_ready(&app_handle.state::<DesktopStartupState>().status())
        .map_err(CommandError::from)?;
    state
        .host()
        .save_initial_profile(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn list_accounts(
    state: tauri::State<'_, DesktopState>,
) -> Result<AccountsSnapshot, CommandError> {
    kukuri_desktop_runtime::list_accounts(&state.app_data_dir)
        .await
        .map_err(map_error)
}

/// アクティブアカウントを切り替える。新しい runtime の構築に成功してから registry と
/// state を更新し、失敗時は旧 runtime をそのまま生かす(アプリ再起動は不要)。
#[tauri::command]
pub async fn switch_account(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    operation: tauri::State<'_, DesktopOperationState>,
    request: SwitchAccountRequest,
) -> Result<AccountRecord, CommandError> {
    let _guard = operation.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app_handle)?;
    let startup = app_handle.state::<DesktopStartupState>();
    require_runtime_operation_ready(&startup.status()).map_err(CommandError::from)?;

    let snapshot = kukuri_desktop_runtime::list_accounts(&state.app_data_dir)
        .await
        .map_err(map_error)?;
    let record = snapshot
        .accounts
        .iter()
        .find(|record| record.id == request.account_id)
        .cloned()
        .ok_or_else(|| CommandError::from(format!("unknown account `{}`", request.account_id)))?;
    if snapshot.active_account_id == request.account_id {
        return Ok(record);
    }

    startup.set_status(DesktopStartupStatus::Initializing);

    let host = state.host();
    let record = match host.switch_account(request.account_id.as_str()).await {
        Ok(record) => record,
        Err(error) => {
            if host.is_stopped() {
                startup.set_status(kukuri_desktop_runtime::failed_startup_status(
                    kukuri_desktop_runtime::ClientStartupError::unknown(
                        "Account transition requires restart".to_string(),
                    ),
                    None,
                ));
            } else {
                startup.set_status(DesktopStartupStatus::Ready);
            }
            return Err(map_error(error));
        }
    };
    app_handle
        .state::<OsNotificationBackground>()
        .reset_for_account_switch();
    startup.set_status(DesktopStartupStatus::Ready);
    Ok(record)
}
