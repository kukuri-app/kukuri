use super::{ClientSession, failed};
use crate::protocol::ProtocolError;
use kukuri_desktop_runtime::{
    AccountsSnapshot, ClientHost, ClientStartupStatus, CreateDeviceBackupRequest,
    DeviceBackupRestoreResult, DeviceBackupSummary, RestoreDeviceBackupRequest,
    advance_committed_restore_to_consent, commit_device_restore, create_device_backup,
    install_prepared_device_restore, list_accounts, pending_device_restore_phase,
    prepare_device_restore, rollback_pending_device_restore, validate_prepared_device_restore,
};
use std::{path::PathBuf, sync::Arc};

impl ClientSession {
    async fn rebuild(&self, host: &Arc<ClientHost>, db: PathBuf) -> Result<(), ProtocolError> {
        let result = async {
            let runtime = ClientHost::build_detached_runtime(db)
                .await
                .map_err(|_| failed())?;
            host.replace_runtime(runtime).await.map_err(|_| failed())?;
            Ok(())
        }
        .await;
        if result.is_ok() {
            self.startup.set_status(ClientStartupStatus::Ready);
        } else {
            self.set_failed();
        }
        result
    }

    pub(crate) async fn create_backup(
        &self,
        request: CreateDeviceBackupRequest,
    ) -> Result<DeviceBackupSummary, ProtocolError> {
        let host = self.host().ok_or_else(failed)?;
        let runtime = host.runtime();
        let db = runtime.db_path().to_path_buf();
        self.operation.begin_cancellable_device_backup();
        self.startup.set_status(ClientStartupStatus::Initializing);
        // #1221 R5-G: backup は保護所有先だけを含めるため、旧 `iroh-data` からの移行の残りを写してから止める。
        let migrated = runtime.finish_protected_migration().await;
        runtime.shutdown().await;
        if migrated.is_err() {
            self.rebuild(&host, db).await?;
            return Err(failed());
        }
        let dir = self.app_data_dir.clone();
        let backup_db = db.clone();
        let cancellation = self.operation.device_backup_cancellation();
        let result = tokio::task::spawn_blocking(move || {
            create_device_backup(&dir, &backup_db, &request, &cancellation, |_| {})
        })
        .await;
        // 書込み失敗や取消でも、停止したruntimeを再構築してから応答する。
        self.rebuild(&host, db).await?;
        result.map_err(|_| failed())?.map_err(|_| failed())
    }

    async fn rollback_and_rebuild(
        &self,
        host: &Arc<ClientHost>,
        db: PathBuf,
        previous: &AccountsSnapshot,
    ) -> Result<(), ProtocolError> {
        let verification = (|| {
            rollback_pending_device_restore(&self.app_data_dir).map_err(|_| failed())?;
            if pending_device_restore_phase(&self.app_data_dir)
                .map_err(|_| failed())?
                .is_some()
                || &list_accounts(&self.app_data_dir).map_err(|_| failed())? != previous
            {
                return Err(failed());
            }
            Ok(())
        })();
        if verification.is_err() {
            self.set_failed();
            return verification;
        }
        self.rebuild(host, db).await
    }

    pub(crate) async fn restore_backup(
        &self,
        request: RestoreDeviceBackupRequest,
    ) -> Result<DeviceBackupRestoreResult, ProtocolError> {
        if pending_device_restore_phase(&self.app_data_dir)
            .map_err(|_| failed())?
            .is_some()
        {
            return Err(failed());
        }
        let host = self.host().ok_or_else(failed)?;
        let previous = list_accounts(&self.app_data_dir).map_err(|_| failed())?;
        let runtime = host.runtime();
        let db = runtime.db_path().to_path_buf();
        self.operation.begin_cancellable_device_backup();
        self.startup.set_status(ClientStartupStatus::Initializing);
        runtime.shutdown().await;
        let dir = self.app_data_dir.clone();
        let cancellation = self.operation.device_backup_cancellation();
        let before_commit = async {
            let prepared = tokio::task::spawn_blocking(move || {
                prepare_device_restore(&dir, &request, &cancellation, |_| {})
            })
            .await
            .map_err(|_| failed())?
            .map_err(|_| failed())?;
            validate_prepared_device_restore(&prepared)
                .await
                .map_err(|_| failed())?;
            self.operation
                .close_device_backup_cancel_gate()
                .map_err(|_| failed())?;
            let dir = self.app_data_dir.clone();
            let cancellation = self.operation.device_backup_cancellation();
            let installed = tokio::task::spawn_blocking(move || {
                cancellation.check()?;
                install_prepared_device_restore(&dir, prepared)
            })
            .await
            .map_err(|_| failed())?
            .map_err(|_| failed())?;
            self.operation
                .device_backup_cancellation()
                .check()
                .map_err(|_| failed())?;
            commit_device_restore(&installed).map_err(|_| failed())
        }
        .await;
        let result = match before_commit {
            Ok(result) => result,
            Err(error) => {
                self.rollback_and_rebuild(&host, db, &previous).await?;
                return Err(error);
            }
        };
        // commit後は旧runtimeを公開しない。失敗時はjournalからfinish-forwardする。
        match advance_committed_restore_to_consent(&self.app_data_dir, &self.consent_db_path()) {
            Ok(status) => {
                self.startup.set_status(status);
                Ok(result)
            }
            Err(_) => {
                self.set_failed();
                Err(failed())
            }
        }
    }
}
