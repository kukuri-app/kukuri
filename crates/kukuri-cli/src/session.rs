mod backup;

use crate::protocol::{ProtocolError, error_code};
use kukuri_desktop_runtime::{
    AcceptedAppConsentDocument, ClientHost, ClientHostStart, ClientOperationState,
    ClientStartupState, ClientStartupStatus, DeviceRestorePhase, RestoreActivationFailure,
    RestoreActivationOrchestrationFailure, RestoreStartupAction,
    advance_committed_restore_to_consent, app_consent_status, finalize_pending_device_restore,
    orchestrate_restore_activation, pending_device_restore_phase, persist_restore_activation_phase,
    record_app_consents, recover_device_restore_before_startup, require_consent_acceptance_state,
    restore_startup_action, rollback_pending_device_restore, validate_app_consent_documents,
};
use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};

/// daemonの起動前・同意待ち・復元中も存在する、profile単位の所有状態。
/// 呼出しの直列化と切断後の所有はDispatcherが担う。
pub struct ClientSession {
    pub(crate) app_data_dir: PathBuf,
    startup: ClientStartupState,
    host: RwLock<Option<Arc<ClientHost>>>,
    pub(crate) operation: ClientOperationState,
}

impl ClientSession {
    pub async fn start(app_data_dir: PathBuf) -> Result<Arc<Self>, ProtocolError> {
        let pending = recover_device_restore_before_startup(&app_data_dir).map_err(|_| failed())?;
        let session = Arc::new(Self {
            app_data_dir,
            startup: ClientStartupState::initializing(),
            host: RwLock::new(None),
            operation: ClientOperationState::default(),
        });
        let consent = app_consent_status(&session.consent_db_path()).await;
        match restore_startup_action(pending, consent.satisfied) {
            RestoreStartupAction::ResetConsent => {
                let status = advance_committed_restore_to_consent(
                    &session.app_data_dir,
                    &session.consent_db_path(),
                )
                .await
                .map_err(|_| failed())?;
                session.startup.set_status(status);
            }
            RestoreStartupAction::Activate => session.activate_restore().await?,
            RestoreStartupAction::Normal | RestoreStartupAction::AwaitConsent => {
                session.start_host().await?
            }
            RestoreStartupAction::Reject(_) => return Err(failed()),
        }
        Ok(session)
    }

    pub(crate) fn consent_db_path(&self) -> PathBuf {
        self.app_data_dir.join("kukuri.db")
    }

    pub fn status(&self) -> ClientStartupStatus {
        self.startup.status()
    }

    pub fn host(&self) -> Option<Arc<ClientHost>> {
        if !matches!(self.status(), ClientStartupStatus::Ready) {
            return None;
        }
        self.host.read().expect("session host lock").clone()
    }

    async fn publish_host(&self, next: Arc<ClientHost>) {
        let previous = self.host.write().expect("session host lock").replace(next);
        if let Some(previous) = previous {
            previous.shutdown().await;
        }
        self.startup.set_status(ClientStartupStatus::Ready);
    }

    async fn start_host(&self) -> Result<(), ProtocolError> {
        match ClientHost::start_if_consented(self.app_data_dir.clone())
            .await
            .map_err(|_| failed())?
        {
            ClientHostStart::Ready(host) => self.publish_host(host).await,
            ClientHostStart::ConsentRequired(status) => self.startup.set_status(status),
        }
        Ok(())
    }

    pub(crate) fn set_failed(&self) {
        self.startup
            .set_status(kukuri_desktop_runtime::failed_startup_status(
                kukuri_desktop_runtime::ClientStartupError::unknown(
                    "ローカル状態の復旧が必要です".into(),
                ),
                None,
            ));
    }

    pub(crate) async fn accept_consents(
        &self,
        documents: Vec<AcceptedAppConsentDocument>,
        language: String,
        age_attested: bool,
    ) -> Result<ClientStartupStatus, ProtocolError> {
        validate_app_consent_documents(&documents).map_err(|_| invalid_consent())?;
        require_consent_acceptance_state(&self.status()).map_err(|_| invalid_consent())?;
        let pending = pending_device_restore_phase(&self.app_data_dir).map_err(|_| failed())?;
        if pending.is_some_and(|phase| phase != DeviceRestorePhase::AwaitingConsent) {
            return Err(failed());
        }
        record_app_consents(
            &self.consent_db_path(),
            &documents,
            &language,
            age_attested,
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(|_| invalid_consent())?;
        self.startup.set_status(ClientStartupStatus::Initializing);
        let result = if pending == Some(DeviceRestorePhase::AwaitingConsent) {
            self.activate_restore().await
        } else {
            self.start_host().await
        };
        if let Err(error) = result {
            if !matches!(self.status(), ClientStartupStatus::Ready) {
                self.set_failed();
            }
            return Err(error);
        }
        Ok(self.status())
    }

    async fn activate_restore(&self) -> Result<(), ProtocolError> {
        let activation = orchestrate_restore_activation(
            || async {
                match ClientHost::start_if_consented(self.app_data_dir.clone()).await {
                    Ok(ClientHostStart::Ready(host)) => Ok(host),
                    _ => Err("復元後のruntimeを起動できません".into()),
                }
            },
            |restored: Arc<ClientHost>| async move {
                if let Err(error) = persist_restore_activation_phase(&self.app_data_dir) {
                    restored.shutdown().await;
                    return Err(error);
                }
                if finalize_pending_device_restore(&self.app_data_dir).is_err() {
                    restored.shutdown().await;
                    return Err(RestoreActivationFailure::FinishForward(
                        "復元の確定処理に失敗しました".into(),
                    ));
                }
                self.publish_host(restored).await;
                Ok(())
            },
            || async {
                rollback_pending_device_restore(&self.app_data_dir)
                    .map_err(|_| "復元を取り消せません".to_string())?;
                self.start_host()
                    .await
                    .map_err(|_| "以前のruntimeを起動できません".to_string())?;
                if self.host().is_none() {
                    return Err("以前のruntimeが利用できません".into());
                }
                Ok(())
            },
        )
        .await;
        match activation {
            Ok(()) => Ok(()),
            Err(RestoreActivationOrchestrationFailure::RolledBack(_)) => Err(failed()),
            Err(_) => {
                self.set_failed();
                Err(failed())
            }
        }
    }

    pub async fn shutdown(&self) {
        self.startup.set_status(ClientStartupStatus::Initializing);
        let host = self.host.write().expect("session host lock").take();
        if let Some(host) = host {
            host.shutdown().await;
        }
    }
}

pub(crate) fn failed() -> ProtocolError {
    ProtocolError::new(
        error_code::INTERNAL_ERROR,
        "ローカル状態の操作を完了できませんでした",
    )
}
fn invalid_consent() -> ProtocolError {
    ProtocolError::new(
        error_code::CONSENT_REQUIRED,
        "現行文書への明示的な同意と年齢申告が必要です",
    )
}
