//! #859: アカウント鍵の安全なエクスポート。#1211: QR・専用リンクの移行。
//!
//! 平文秘密鍵は返さない。エクスポートは暗号化 envelope
//! (`kukuri_core::encrypt_account_key_export`)のみを IPC へ出す。移行の鍵は、両端末で確認した接続でだけ送る。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use kukuri_app_api::AppService;
use kukuri_core::AccountTransferStatus;
use kukuri_iroh_node::AccountBundleSink;

use crate::accounts::transfer::{RuntimeBundleSource, merge_staged};
use crate::accounts::{AccountKeyExport, AccountTransferLink};
use crate::requests::{
    DecideAccountTransferRequest, ExportAccountKeyRequest, OpenAccountTransferRequest,
};

use super::DesktopRuntime;
use super::private_channel_controller::claim_restored_controllers;

impl DesktopRuntime {
    /// この account の公開鍵(hex)。
    pub fn local_author_pubkey(&self) -> String {
        self.author_keys.public_key_hex()
    }

    pub async fn export_account_key(
        &self,
        request: ExportAccountKeyRequest,
    ) -> Result<AccountKeyExport> {
        let export = kukuri_core::encrypt_account_key_export(
            &self.author_keys,
            &request.passphrase,
            crate::kdf::derive_passphrase_key,
        )
        .await?;
        Ok(AccountKeyExport {
            export,
            public_key: self.author_keys.public_key_hex(),
        })
    }

    /// #1211: 移行元として招待を出す。確認の後に、この account の必須 bundle を送る。前の移行は取り消す。
    pub async fn create_account_transfer_invite(&self) -> Result<AccountTransferLink> {
        let source = Arc::new(RuntimeBundleSource {
            app: self.app_service.account_transfer_handle(),
            keys: self.author_keys.clone(),
        });
        let invite = self.iroh_stack.account_transfer().await?.issue(source)?;
        Ok(AccountTransferLink {
            link: invite.to_link(),
            expires_at_ms: invite.expires_at_ms,
        })
    }

    /// #1211: 移行先としてリンクの移行元へ接続し、受けた必須 bundle を `sink` へ保存する。前の移行は取り消す。
    pub(crate) async fn open_account_transfer(
        &self,
        request: OpenAccountTransferRequest,
        sink: Arc<dyn AccountBundleSink>,
    ) -> Result<()> {
        self.iroh_stack
            .account_transfer()
            .await?
            .open(&request.link, sink)
    }

    /// #1211 AC-2: 移行で保存した必須 bundle を、背景で chunk ごとに反映する（起動時と、使っているアカウントへの
    /// 移行の確定の後）。反映の途中の task は止めて作り直す（反映はやり直せる）。
    pub(crate) async fn merge_account_transfer(&self) {
        let task = spawn_account_transfer_merge(
            self.db_path.clone(),
            self.app_service.account_transfer_handle(),
        );
        if let Some(previous) = self.account_transfer_task.lock().await.replace(task) {
            previous.abort();
        }
    }

    /// 反映の task を止める（使っているアカウントへの新しい移行の前と、停止のとき）。
    pub(crate) async fn stop_account_transfer_merge(&self) {
        if let Some(task) = self.account_transfer_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
    }

    pub async fn account_transfer_status(&self) -> Result<AccountTransferStatus> {
        Ok(self.iroh_stack.account_transfer().await?.status())
    }

    pub async fn decide_account_transfer(
        &self,
        request: DecideAccountTransferRequest,
    ) -> Result<()> {
        self.iroh_stack
            .account_transfer()
            .await?
            .decide(request.accept)
    }

    pub async fn cancel_account_transfer(&self) -> Result<()> {
        self.iroh_stack.account_transfer().await?.cancel();
        Ok(())
    }
}

pub(super) fn spawn_account_transfer_merge(
    db_path: PathBuf,
    app: AppService,
) -> n0_future::task::JoinHandle<()> {
    n0_future::task::spawn(async move {
        if let Err(error) = merge_staged(&db_path, &app).await {
            tracing::warn!(%error, "the transferred account bundle is merged again at the next start");
        }
        if let Err(error) = claim_restored_controllers(&db_path, &app).await {
            tracing::warn!(%error, "the restored device takes over its private channels at the next start");
        }
    })
}
