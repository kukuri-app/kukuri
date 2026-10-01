use std::path::Path;

use super::{ClientStartupError, ClientStartupStatus, reset_app_consent_at_path};
use crate::{
    DeviceBackupCancellation, DeviceRestorePhase, mark_device_restore_activated,
    mark_device_restore_awaiting_consent, pending_device_restore_phase,
    recover_interrupted_restore,
};

/// runtimeがまだ存在しない同意待ちでも、account switch・backup・restore activationを
/// 同じ直列化境界で扱うためのprocess-lifetime state。
#[derive(Default)]
pub struct ClientOperationState {
    pub switch_guard: tokio::sync::Mutex<()>,
    device_backup_cancellation: DeviceBackupCancellation,
    device_backup_cancel_allowed: std::sync::Mutex<bool>,
}

impl ClientOperationState {
    pub fn begin_cancellable_device_backup(&self) {
        let mut allowed = self
            .device_backup_cancel_allowed
            .lock()
            .expect("device backup cancel gate poisoned");
        self.device_backup_cancellation.reset();
        *allowed = true;
    }

    pub fn device_backup_cancellation(&self) -> DeviceBackupCancellation {
        self.device_backup_cancellation.clone()
    }

    /// cancel commandと同じmutex下でflagを確認してgateを閉じる。これが成功した後に
    /// 到着したcancelはInstalling transactionへ伝播しない。
    pub fn close_device_backup_cancel_gate(&self) -> anyhow::Result<()> {
        let mut allowed = self
            .device_backup_cancel_allowed
            .lock()
            .expect("device backup cancel gate poisoned");
        self.device_backup_cancellation.check()?;
        *allowed = false;
        Ok(())
    }

    pub fn cancel_device_backup(&self) {
        let allowed = self
            .device_backup_cancel_allowed
            .lock()
            .expect("device backup cancel gate poisoned");
        if *allowed {
            self.device_backup_cancellation.cancel();
        }
    }
}

/// runtimeへ触れてよいのは、公開済みruntimeとstartup statusの両方がReadyの時だけ。
pub fn runtime_access_allowed(status: &ClientStartupStatus) -> bool {
    matches!(status, ClientStartupStatus::Ready)
}

pub fn require_runtime_operation_ready(status: &ClientStartupStatus) -> Result<(), String> {
    if runtime_access_allowed(status) {
        Ok(())
    } else {
        Err(format!(
            "desktop runtime operation requires Ready startup state; current state is {status:?}"
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreStartupAction {
    Normal,
    ResetConsent,
    AwaitConsent,
    Activate,
    Reject(DeviceRestorePhase),
}

impl RestoreStartupAction {
    pub fn initializes_runtime(self) -> bool {
        matches!(self, Self::Normal | Self::Activate)
    }
}

pub fn restore_startup_action(
    pending_phase: Option<DeviceRestorePhase>,
    consent_satisfied: bool,
) -> RestoreStartupAction {
    match pending_phase {
        None if consent_satisfied => RestoreStartupAction::Normal,
        None => RestoreStartupAction::AwaitConsent,
        Some(DeviceRestorePhase::Committed) => RestoreStartupAction::ResetConsent,
        Some(DeviceRestorePhase::AwaitingConsent) if consent_satisfied => {
            RestoreStartupAction::Activate
        }
        Some(DeviceRestorePhase::AwaitingConsent) => RestoreStartupAction::AwaitConsent,
        Some(unexpected) => RestoreStartupAction::Reject(unexpected),
    }
}

/// 起動時の最初の永続状態操作。未完了installをrollbackしてから、残ったpending
/// phaseを返す。同意pathの解決・読取はこの関数の完了後にだけ行う。
pub fn recover_device_restore_before_startup(
    app_data_dir: &Path,
) -> Result<Option<DeviceRestorePhase>, ClientStartupError> {
    recover_interrupted_restore(app_data_dir).map_err(|error| {
        ClientStartupError::unknown(format!("device restore recovery failed: {error:#}"))
    })?;
    pending_device_restore_phase(app_data_dir).map_err(|error| {
        ClientStartupError::unknown(format!(
            "failed to inspect pending device restore: {error:#}"
        ))
    })
}

pub async fn advance_committed_restore_to_consent(
    app_data_dir: &Path,
    consent_db_path: &Path,
) -> Result<ClientStartupStatus, ClientStartupError> {
    advance_committed_restore_to_consent_with(
        reset_app_consent_at_path(consent_db_path).await,
        || mark_device_restore_awaiting_consent(app_data_dir).map_err(|error| format!("{error:#}")),
    )
}

fn advance_committed_restore_to_consent_with<Mark>(
    reset_consent: Result<ClientStartupStatus, String>,
    mark_awaiting_consent: Mark,
) -> Result<ClientStartupStatus, ClientStartupError>
where
    Mark: FnOnce() -> Result<(), String>,
{
    let status = reset_consent.map_err(|error| {
        ClientStartupError::unknown(format!("device restore consent reset failed: {error}"))
    })?;
    mark_awaiting_consent().map_err(|error| {
        ClientStartupError::unknown(format!(
            "failed to persist device restore consent gate: {error}"
        ))
    })?;
    Ok(status)
}

pub enum RestoreActivationFailure {
    RollbackAllowed(String),
    FinishForward(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RestoreActivationOrchestrationFailure {
    RolledBack(String),
    RollbackFailed(String),
    FinishForward(String),
}

async fn roll_back_failed_activation<Rollback, RollbackFuture>(
    operation_error: String,
    rollback: Rollback,
) -> RestoreActivationOrchestrationFailure
where
    Rollback: FnOnce() -> RollbackFuture,
    RollbackFuture: std::future::Future<Output = Result<(), String>>,
{
    match rollback().await {
        Ok(()) => RestoreActivationOrchestrationFailure::RolledBack(operation_error),
        Err(rollback_error) => RestoreActivationOrchestrationFailure::RollbackFailed(format!(
            "{operation_error}; additionally {rollback_error}"
        )),
    }
}

/// startup recoveryと明示的な同意commandのruntime構築、durable activation、
/// activation前rollbackを同じ順序へ集約する。rollbackできるのは`RollbackAllowed`だけで、
/// Activatedへ進んだtransactionは必ずfinish-forwardする。
pub async fn orchestrate_restore_activation<
    Restored,
    Build,
    BuildFuture,
    Activate,
    ActivateFuture,
    Rollback,
    RollbackFuture,
>(
    build: Build,
    activate: Activate,
    rollback: Rollback,
) -> Result<(), RestoreActivationOrchestrationFailure>
where
    Build: FnOnce() -> BuildFuture,
    BuildFuture: std::future::Future<Output = Result<Restored, String>>,
    Activate: FnOnce(Restored) -> ActivateFuture,
    ActivateFuture: std::future::Future<Output = Result<(), RestoreActivationFailure>>,
    Rollback: FnOnce() -> RollbackFuture,
    RollbackFuture: std::future::Future<Output = Result<(), String>>,
{
    let restored = match build().await {
        Ok(restored) => restored,
        Err(message) => {
            return Err(roll_back_failed_activation(message, rollback).await);
        }
    };
    match activate(restored).await {
        Ok(()) => Ok(()),
        Err(RestoreActivationFailure::RollbackAllowed(message)) => {
            Err(roll_back_failed_activation(message, rollback).await)
        }
        Err(RestoreActivationFailure::FinishForward(message)) => Err(
            RestoreActivationOrchestrationFailure::FinishForward(message),
        ),
    }
}

pub fn persist_restore_activation_phase(
    app_data_dir: &Path,
) -> Result<(), RestoreActivationFailure> {
    mark_device_restore_activated(app_data_dir).map_err(|error| {
        RestoreActivationFailure::RollbackAllowed(format!(
            "failed to persist restored runtime activation: {error:#}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{
        AGE_ATTESTATION_VERSION, APP_LEGAL_DOCUMENTS, AgeAttestationRecord,
        AppConsentDocumentRecord, AppConsentStore, ClientStartupErrorKind, app_consent_satisfied,
        consent_required_status, failed_startup_status as failed_status, load_app_consent_store,
        reset_app_consent_at_path, save_app_consent_store,
    };

    #[test]
    fn runtime_access_is_allowed_only_for_ready() {
        assert!(runtime_access_allowed(&ClientStartupStatus::Ready));
        assert!(!runtime_access_allowed(&ClientStartupStatus::Initializing));
        assert!(!runtime_access_allowed(&consent_required_status(
            &Default::default()
        )));
        assert!(!runtime_access_allowed(&failed_status(
            ClientStartupError {
                kind: ClientStartupErrorKind::Unknown,
                message: "failed".to_string(),
            },
            None,
        )));

        assert!(require_runtime_operation_ready(&ClientStartupStatus::Ready).is_ok());
        assert!(require_runtime_operation_ready(&ClientStartupStatus::Initializing).is_err());
    }

    #[test]
    fn startup_action_covers_every_safe_restore_boundary() {
        assert_eq!(
            restore_startup_action(None, true),
            RestoreStartupAction::Normal
        );
        assert_eq!(
            restore_startup_action(None, false),
            RestoreStartupAction::AwaitConsent
        );
        assert_eq!(
            restore_startup_action(Some(DeviceRestorePhase::Committed), true),
            RestoreStartupAction::ResetConsent
        );
        assert_eq!(
            restore_startup_action(Some(DeviceRestorePhase::AwaitingConsent), false),
            RestoreStartupAction::AwaitConsent
        );
        assert!(
            !restore_startup_action(Some(DeviceRestorePhase::AwaitingConsent), false)
                .initializes_runtime()
        );
        assert_eq!(
            restore_startup_action(Some(DeviceRestorePhase::AwaitingConsent), true),
            RestoreStartupAction::Activate
        );
        for unexpected in [
            DeviceRestorePhase::Installing,
            DeviceRestorePhase::Installed,
            DeviceRestorePhase::Activated,
        ] {
            assert_eq!(
                restore_startup_action(Some(unexpected), false),
                RestoreStartupAction::Reject(unexpected)
            );
        }
    }

    #[test]
    fn install_cancel_gate_serializes_cancel_at_the_boundary() {
        let operation = ClientOperationState::default();
        operation.begin_cancellable_device_backup();
        operation.cancel_device_backup();
        assert!(operation.close_device_backup_cancel_gate().is_err());

        operation.begin_cancellable_device_backup();
        operation
            .close_device_backup_cancel_gate()
            .expect("close cancel gate before install");
        operation.cancel_device_backup();
        operation
            .device_backup_cancellation()
            .check()
            .expect("cancel arriving after install boundary must be ignored");
    }

    #[tokio::test]
    async fn restore_consent_reset_is_persisted_before_consent_required_status() {
        let dir = std::env::temp_dir().join(format!(
            "kukuri-consent-restore-reset-test-{}-{}",
            std::process::id(),
            crate::host::current_unix_seconds()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("kukuri.db");
        let accepted = AppConsentStore {
            records: APP_LEGAL_DOCUMENTS
                .iter()
                .map(|(slug, version)| AppConsentDocumentRecord {
                    slug: (*slug).to_string(),
                    version: *version,
                    accepted_at: 1,
                    language: "ja".to_string(),
                    app_version: "0.1.8".to_string(),
                    build_profile: None,
                })
                .collect(),
            age_attestations: vec![AgeAttestationRecord {
                version: AGE_ATTESTATION_VERSION,
                attested_at: 1,
                language: "ja".to_string(),
                app_version: "0.1.8".to_string(),
                build_profile: None,
            }],
        };
        save_app_consent_store(&db_path, &accepted)
            .await
            .expect("save accepted consent");

        let status = reset_app_consent_at_path(&db_path)
            .await
            .expect("reset restored consent");
        assert!(matches!(
            status,
            ClientStartupStatus::ConsentRequired { .. }
        ));
        assert!(!app_consent_satisfied(
            &load_app_consent_store(&db_path).await
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn consent_reset_failure_leaves_committed_phase_unchanged() {
        let dir = std::env::temp_dir().join(format!(
            "kukuri-consent-reset-failure-test-{}-{}",
            std::process::id(),
            crate::host::current_unix_seconds()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let blocked_parent = dir.join("not-a-directory");
        std::fs::write(&blocked_parent, b"block directory creation").expect("create blocking file");
        let db_path = blocked_parent.join("kukuri.db");
        let phase = std::cell::Cell::new(DeviceRestorePhase::Committed);
        let result = advance_committed_restore_to_consent_with(
            reset_app_consent_at_path(&db_path).await,
            || {
                phase.set(DeviceRestorePhase::AwaitingConsent);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert_eq!(phase.get(), DeviceRestorePhase::Committed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn accept_activation_success_and_phase_write_failure_use_the_same_rollback_boundary() {
        let rollback_count = std::cell::Cell::new(0);
        let success = orchestrate_restore_activation(
            || std::future::ready(Ok::<_, String>(())),
            |_| std::future::ready(Ok::<_, RestoreActivationFailure>(())),
            || {
                rollback_count.set(rollback_count.get() + 1);
                std::future::ready(Ok(()))
            },
        )
        .await;
        assert_eq!(success, Ok(()));
        assert_eq!(rollback_count.get(), 0);

        let missing_journal_dir = std::env::temp_dir().join(format!(
            "kukuri-missing-restore-journal-test-{}-{}",
            std::process::id(),
            crate::host::current_unix_seconds()
        ));
        let phase_write_failure = orchestrate_restore_activation(
            || std::future::ready(Ok::<_, String>(())),
            |_| std::future::ready(persist_restore_activation_phase(&missing_journal_dir)),
            || {
                rollback_count.set(rollback_count.get() + 1);
                std::future::ready(Ok(()))
            },
        )
        .await;
        assert_eq!(
            phase_write_failure,
            Err(RestoreActivationOrchestrationFailure::RolledBack(
                "failed to persist restored runtime activation: device restore journal is missing"
                    .to_string()
            ))
        );
        assert_eq!(rollback_count.get(), 1);
    }
}
