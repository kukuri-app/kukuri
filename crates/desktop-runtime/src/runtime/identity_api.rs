//! #859: アカウント鍵の安全なエクスポート。
//!
//! 平文秘密鍵は返さない。エクスポートは暗号化 envelope
//! (`kukuri_core::encrypt_account_key_export`)のみを IPC へ出す。

use anyhow::Result;

use crate::accounts::AccountKeyExport;
use crate::requests::ExportAccountKeyRequest;

use super::DesktopRuntime;

impl DesktopRuntime {
    /// この account の公開鍵(hex)。
    pub fn local_author_pubkey(&self) -> String {
        self.author_keys.public_key_hex()
    }

    /// 同期 API。argon2id の鍵導出で数百 ms ブロックするため、呼び出し側
    /// (Tauri コマンド)は `spawn_blocking` で包む。
    pub fn export_account_key(&self, request: ExportAccountKeyRequest) -> Result<AccountKeyExport> {
        let export =
            kukuri_core::encrypt_account_key_export(&self.author_keys, &request.passphrase)?;
        Ok(AccountKeyExport {
            export,
            public_key: self.author_keys.public_key_hex(),
        })
    }
}
