//! #859: アカウント鍵の安全なエクスポート。
//!
//! 平文秘密鍵は返さない。エクスポートは暗号化 envelope
//! (`kukuri_core::encrypt_account_key_export`)のみを IPC へ出す。

use anyhow::Result;

use kukuri_core::AccountTransferStatus;

use crate::accounts::{AccountKeyExport, AccountTransferLink};
use crate::requests::{
    DecideAccountTransferRequest, ExportAccountKeyRequest, OpenAccountTransferRequest,
};

use super::DesktopRuntime;

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

    /// #1211: 移行元として招待を出す。前の移行は取り消す。
    pub async fn create_account_transfer_invite(&self) -> Result<AccountTransferLink> {
        let invite = self.iroh_stack.account_transfer().await?.issue()?;
        Ok(AccountTransferLink {
            link: invite.to_link(),
            expires_at_ms: invite.expires_at_ms,
        })
    }

    /// #1211: 移行先としてリンクの移行元へ接続する。前の移行は取り消す。
    pub async fn open_account_transfer(&self, request: OpenAccountTransferRequest) -> Result<()> {
        self.iroh_stack
            .account_transfer()
            .await?
            .open(&request.link)
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
