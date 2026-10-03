use super::*;
use crate::accounts::lifecycle;

#[derive(serde::Serialize, serde::Deserialize)]
struct InitialProfileSave {
    request_hash: String,
    envelope: kukuri_core::KukuriEnvelope,
}

impl ClientHost {
    pub async fn create_account(
        &self,
        request: crate::CreateAccountRequest,
    ) -> anyhow::Result<AccountRecord> {
        use crate::accounts::create::{commit_account_creation, prepare_account_creation};
        let _guard = self.operation_guard.lock().await;
        if self.is_stopped() {
            anyhow::bail!("client host is shut down");
        }
        let pending = prepare_account_creation(
            &self.app_data_dir,
            crate::identity::IdentityStorageMode::from_env(),
            &request,
        )
        .await?;
        if list_accounts(&self.app_data_dir).await?.active_account_id == pending.next.id {
            return Ok(pending.next);
        }
        let next = self
            .builder
            .build(&account_db_path(&self.app_data_dir, &pending.next.id))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let previous = self
            .replace_runtime_locked(next)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let commit = commit_account_creation(&self.app_data_dir, &pending).await;
        self.finish_account_change(previous, &pending.previous, pending.next, false, commit)
            .await
    }
    pub async fn save_initial_profile(
        &self,
        request: crate::InitialProfileRequest,
    ) -> anyhow::Result<kukuri_core::Profile> {
        let _guard = self.operation_guard.lock().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            anyhow::bail!("client host is shutting down");
        }
        lifecycle::profile_setup_required(&self.app_data_dir, &request.account_id).await?;
        let request_hash = blake3::hash(&serde_json::to_vec(&request.profile)?)
            .to_hex()
            .to_string();
        let journal = account_db_path(&self.app_data_dir, &request.account_id)
            .with_extension("initial-profile.json");
        if let Some(bytes) = crate::storage::read_file(&journal).await? {
            let pending: InitialProfileSave = serde_json::from_slice(&bytes)?;
            let recovered = self.runtime().commit_my_profile(pending.envelope).await?;
            if pending.request_hash == request_hash {
                lifecycle::complete_profile_setup(&self.app_data_dir, &request.account_id).await?;
                return Ok(recovered);
            }
            // A user may edit after an error. Recover the old operation first,
            // then honor the new explicit save rather than silently discarding it.
        }
        let envelope = self.runtime().prepare_my_profile(request.profile).await?;
        let pending = InitialProfileSave {
            request_hash,
            envelope: envelope.clone(),
        };
        crate::storage::write_file(&journal, &serde_json::to_vec(&pending)?).await?;
        let profile = self.runtime().commit_my_profile(envelope).await?;
        lifecycle::complete_profile_setup(&self.app_data_dir, &request.account_id).await?;
        Ok(profile)
    }

    pub async fn profile_setup_required(&self, account_id: &str) -> anyhow::Result<bool> {
        let _guard = self.operation_guard.lock().await;
        if self.is_stopped() {
            anyhow::bail!("client host is shut down");
        }
        if !lifecycle::profile_setup_required(&self.app_data_dir, account_id).await? {
            return Ok(false);
        }
        let journal =
            account_db_path(&self.app_data_dir, account_id).with_extension("initial-profile.json");
        if let Some(bytes) = crate::storage::read_file(&journal).await? {
            let pending: InitialProfileSave = serde_json::from_slice(&bytes)?;
            self.runtime().commit_my_profile(pending.envelope).await?;
            lifecycle::complete_profile_setup(&self.app_data_dir, account_id).await?;
            return Ok(false);
        }
        if self.runtime().get_my_profile().await?.updated_at > 0 {
            lifecycle::complete_profile_setup(&self.app_data_dir, account_id).await?;
            return Ok(false);
        }
        Ok(true)
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn account_display(&self) -> anyhow::Result<Vec<crate::AccountDisplay>> {
        use crate::accounts::display::{read_profile, set_picture};
        let _guard = self.operation_guard.lock().await;
        let snapshot = list_accounts(&self.app_data_dir).await?;
        let mut result = Vec::new();
        for account in snapshot.accounts {
            let entry = read_profile(&self.app_data_dir, &account).await;
            let (mut display, hash, protected) = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    result.push(crate::AccountDisplay {
                        id: account.id,
                        name: None,
                        display_name: None,
                        picture: None,
                        unavailable: true,
                    });
                    continue;
                }
            };
            // #1221 R5-I: avatar は保護所有先から読む(旧 `iroh-data` の fallback は撤去した)。
            if hash.is_some() {
                set_picture(&mut display, protected);
            }
            result.push(display);
        }
        Ok(result)
    }

    pub async fn logout_account(&self, account_id: &str) -> anyhow::Result<AccountRecord> {
        let _guard = self.operation_guard.lock().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            anyhow::bail!("client host is shutting down");
        }
        let prepared = lifecycle::prepare_logout(
            &self.app_data_dir,
            crate::identity::IdentityStorageMode::from_env(),
            account_id,
        )
        .await?;
        let next = self
            .builder
            .build(&account_db_path(&self.app_data_dir, &prepared.next.id))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let previous = self
            .replace_runtime_locked(next)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let commit = lifecycle::commit_logout(&self.app_data_dir, &prepared).await;
        self.finish_account_change(previous, &prepared.target, prepared.next, true, commit)
            .await
    }

    pub fn is_stopped(&self) -> bool {
        self.shutdown_started.load(Ordering::Acquire)
    }

    pub(super) async fn finish_account_change(
        &self,
        previous: Arc<DesktopRuntime>,
        previous_id: &str,
        next: AccountRecord,
        logout: bool,
        commit: anyhow::Result<()>,
    ) -> anyhow::Result<AccountRecord> {
        if let Err(error) = commit {
            match lifecycle::reconcile_account_commit(
                &self.app_data_dir,
                previous_id,
                &next.id,
                logout,
            )
            .await
            {
                Ok(true) => {
                    if let Err(error) = previous.shutdown_checked().await {
                        self.stop_failed_account_change(previous).await;
                        return Err(error);
                    }
                    return Ok(next);
                }
                Err(recovery) => {
                    self.stop_failed_account_change(previous).await;
                    anyhow::bail!("account commit failed: {error}; recovery failed: {recovery}");
                }
                Ok(false) => {}
            }
            match self.replace_runtime_locked(previous.clone()).await {
                Ok(failed) => failed.shutdown().await,
                Err(rollback) => {
                    self.stop_failed_account_change(previous).await;
                    anyhow::bail!(
                        "account commit failed: {error}; runtime rollback failed: {rollback}"
                    );
                }
            }
            return Err(error);
        }
        if let Err(error) = previous.shutdown_checked().await {
            self.stop_failed_account_change(previous).await;
            return Err(error);
        }
        Ok(next)
    }

    async fn stop_failed_account_change(&self, previous: Arc<DesktopRuntime>) {
        self.shutdown_started.store(true, Ordering::Release);
        if let Some(task) = self.event_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        self.generation.send_modify(|generation| *generation += 1);
        self.runtime().shutdown().await;
        previous.shutdown().await;
    }

    pub async fn import_account_key(
        &self,
        export: String,
        passphrase: String,
        label: Option<String>,
    ) -> anyhow::Result<AccountRecord> {
        let _guard = self.operation_guard.lock().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            anyhow::bail!("client host is shutting down");
        }
        crate::import_account_key_from_env(&self.app_data_dir, &export, &passphrase, label).await
    }

    /// #1211: 移行先としてリンクの移行元へ接続する。受けた必須 bundle はそのアカウントの置き場へ保存し、保存の確定で
    /// 新規のアカウントを登録簿へ足す。
    pub async fn open_account_transfer(
        self: &Arc<Self>,
        request: crate::OpenAccountTransferRequest,
    ) -> anyhow::Result<()> {
        let sink = Arc::new(crate::accounts::transfer::TransferSink {
            host: Arc::downgrade(self),
        });
        self.runtime().open_account_transfer(request, sink).await
    }

    /// #1211: 移行の保存を確定したアカウントを登録簿へ足す。同じ公開鍵が登録済みなら、そのまま返す（二重に登録しない）。
    pub(crate) async fn register_transferred_account(
        &self,
        keys: &kukuri_core::KukuriKeys,
    ) -> anyhow::Result<AccountRecord> {
        let _guard = self.operation_guard.lock().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            anyhow::bail!("client host is shutting down");
        }
        let pubkey = keys.public_key_hex();
        if let Some(record) = list_accounts(&self.app_data_dir)
            .await?
            .accounts
            .into_iter()
            .find(|record| record.pubkey == pubkey)
        {
            return Ok(record);
        }
        // 鍵の置き場は、この端末の今のアカウントと同じ方式にする。
        let mode = self.runtime().identity_mode;
        crate::accounts::add_account(&self.app_data_dir, mode, keys, None, false).await
    }
}
