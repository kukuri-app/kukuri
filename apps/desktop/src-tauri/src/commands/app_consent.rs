use kukuri_desktop_runtime::{DeviceRestorePhase, pending_device_restore_phase};
use tauri::Manager;

use crate::restore_lifecycle::{
    DesktopOperationState, RestoreActivationOrchestrationFailure, activate_pending_restore,
    orchestrate_restore_activation, rollback_pending_restore_and_rebuild,
};
use crate::spawn_desktop_initialization;
use crate::state::{
    CommandError, DesktopStartupState, DesktopStartupStatus, DesktopState, StartupError,
    build_desktop_state, failed_status, resolve_app_data_dir, resolve_db_path,
};
pub use kukuri_desktop_runtime::{AcceptedAppConsentDocument, AppConsentStatus};
use kukuri_desktop_runtime::{
    app_consent_status, record_app_consents, require_consent_acceptance_state,
    validate_app_consent_documents,
};

fn set_activation_failed(
    app_handle: &tauri::AppHandle,
    startup: &DesktopStartupState,
    message: String,
) -> CommandError {
    startup.set_status(failed_status(
        StartupError::unknown(message.clone()),
        resolve_db_path(app_handle).ok(),
    ));
    CommandError::from(message)
}

#[tauri::command]
pub async fn get_app_consent_status(
    app_handle: tauri::AppHandle,
) -> Result<AppConsentStatus, CommandError> {
    let db_path = resolve_db_path(&app_handle)?;
    Ok(app_consent_status(&db_path).await)
}

#[tauri::command]
pub async fn accept_app_consents(
    app_handle: tauri::AppHandle,
    documents: Vec<AcceptedAppConsentDocument>,
    language: String,
    age_attested: bool,
) -> Result<DesktopStartupStatus, CommandError> {
    validate_app_consent_documents(&documents)?;

    let operation = app_handle.state::<DesktopOperationState>();
    let _guard = operation.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app_handle)?;
    let startup_state = app_handle.state::<DesktopStartupState>();
    require_consent_acceptance_state(&startup_state.status()).map_err(CommandError::from)?;
    let app_data_dir = resolve_app_data_dir(&app_handle)?;
    let pending_restore = pending_device_restore_phase(&app_data_dir).map_err(|error| {
        CommandError::from(format!(
            "failed to inspect pending device restore: {error:#}"
        ))
    })?;
    if let Some(phase) = pending_restore
        && phase != DeviceRestorePhase::AwaitingConsent
    {
        return Err(set_activation_failed(
            &app_handle,
            &startup_state,
            format!("device restore cannot accept consent from phase {phase:?}"),
        ));
    }

    let db_path = resolve_db_path(&app_handle)?;
    let app_version = app_handle.package_info().version.to_string();
    record_app_consents(&db_path, &documents, &language, age_attested, &app_version).await?;

    if pending_restore == Some(DeviceRestorePhase::AwaitingConsent) {
        startup_state.set_status(DesktopStartupStatus::Initializing);
        let activation = orchestrate_restore_activation(
            || async {
                build_desktop_state(&app_handle).await.map_err(|error| {
                    format!("failed to start restored account after consent: {error}")
                })
            },
            |restored| activate_pending_restore(&app_handle, &app_data_dir, restored),
            || rollback_pending_restore_and_rebuild(&app_handle, &app_data_dir),
        )
        .await;
        match activation {
            Ok(()) => {
                startup_state.set_status(DesktopStartupStatus::Ready);
                return Ok(DesktopStartupStatus::Ready);
            }
            Err(RestoreActivationOrchestrationFailure::RolledBack(message)) => {
                startup_state.set_status(DesktopStartupStatus::Ready);
                return Err(CommandError::from(message));
            }
            Err(
                RestoreActivationOrchestrationFailure::RollbackFailed(message)
                | RestoreActivationOrchestrationFailure::FinishForward(message),
            ) => {
                return Err(set_activation_failed(&app_handle, &startup_state, message));
            }
        }
    }

    if app_handle.try_state::<DesktopState>().is_some() {
        startup_state.set_status(DesktopStartupStatus::Ready);
        return Ok(DesktopStartupStatus::Ready);
    }

    startup_state.set_status(DesktopStartupStatus::Initializing);
    spawn_desktop_initialization(app_handle.clone())
        .await
        .map_err(|error| error.to_string().into())
}
