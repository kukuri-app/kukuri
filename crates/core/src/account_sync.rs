//! 本人の端末間の account 同期（#1218、ADR 0061）。
//!
//! アカウントの秘密鍵から、用途ごとに別の値（replica の識別子・namespace の秘密・payload の暗号鍵・
//! hint の topic）を導出する。公開鍵だけでは計算できず、同じ秘密鍵を持つ端末は同じ値になる。
//! 同期する item は allowlist の種類だけで、暗号化と認証をして docs へ置く。

use anyhow::{Context, Result, anyhow, ensure};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use secp256k1::rand::{RngCore, rng};
use serde::{Deserialize, Serialize};

use crate::wire::{ACCOUNT_SYNC_REPLICA_PREFIX, ACCOUNT_SYNC_TOPIC_PREFIX};
use crate::{ChannelId, KukuriKeys, Pubkey, ReplicaId, TopicId};

/// 導出の context（ADR 0061 §1）。変えると全アカウントの同期先が変わるので、変更してはならない。
const ID_CONTEXT: &str = "kukuri.app 2026-10-01 account sync id v1";
const NAMESPACE_CONTEXT: &str = "kukuri.app 2026-10-01 account sync namespace v1";
const PAYLOAD_CONTEXT: &str = "kukuri.app 2026-10-01 account sync payload v1";
const RENDEZVOUS_CONTEXT: &str = "kukuri.app 2026-10-01 account sync rendezvous v1";
const ITEM_AAD_DOMAIN: &[u8] = b"kukuri account sync item v1\0";

/// 1 item の平文の上限。
pub const MAX_ACCOUNT_SYNC_ITEM_BYTES: usize = 16 * 1024;
/// 封をした 1 item の上限（hex の暗号文と nonce を含む）。
pub const MAX_SEALED_ACCOUNT_SYNC_ITEM_BYTES: usize = 2 * MAX_ACCOUNT_SYNC_ITEM_BYTES + 256;
/// id（channel id・epoch id・author）の上限。
const MAX_ITEM_ID_BYTES: usize = 256;

/// account 同期の導出値。秘密を含むので `Debug` と log へ中身を出さない。
#[derive(Clone)]
pub struct AccountSyncKeys {
    replica_id: ReplicaId,
    hint_topic: TopicId,
    namespace_secret: [u8; 32],
    payload_key: [u8; 32],
}

impl std::fmt::Debug for AccountSyncKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountSyncKeys").finish_non_exhaustive()
    }
}

impl KukuriKeys {
    /// 本人の端末間の account 同期の値を導出する（ADR 0061 §1）。
    pub fn derive_account_sync(&self) -> AccountSyncKeys {
        let secret = self.secret_bytes();
        let id = blake3::derive_key(ID_CONTEXT, &secret);
        let rendezvous = blake3::derive_key(RENDEZVOUS_CONTEXT, &secret);
        AccountSyncKeys {
            replica_id: ReplicaId::new(format!("{ACCOUNT_SYNC_REPLICA_PREFIX}{}", hex::encode(id))),
            hint_topic: TopicId::new(format!(
                "{ACCOUNT_SYNC_TOPIC_PREFIX}{}",
                hex::encode(rendezvous)
            )),
            namespace_secret: blake3::derive_key(NAMESPACE_CONTEXT, &secret),
            payload_key: blake3::derive_key(PAYLOAD_CONTEXT, &secret),
        }
    }
}

impl AccountSyncKeys {
    /// account 同期の docs の replica。公開の namespace の導出の対象外（private として登録する）。
    pub fn replica_id(&self) -> &ReplicaId {
        &self.replica_id
    }

    /// 変更の手掛かりを流す gossip の topic。replica の識別子とは別の値で、そこから replica は分からない。
    pub fn hint_topic(&self) -> &TopicId {
        &self.hint_topic
    }

    /// docs へ登録する namespace の秘密（hex）。保存・送信・log 出力をしてはならない。
    pub fn expose_namespace_secret_hex(&self) -> String {
        hex::encode(self.namespace_secret)
    }

    /// item を暗号化・認証する。account・item の key を AAD に束縛する。
    pub fn seal(&self, account: &Pubkey, item: &AccountSyncItem) -> Result<SealedAccountSyncItem> {
        item.validate()?;
        let plaintext = serde_json::to_vec(item).context("failed to encode account sync item")?;
        ensure!(
            plaintext.len() <= MAX_ACCOUNT_SYNC_ITEM_BYTES,
            "account sync item exceeds {MAX_ACCOUNT_SYNC_ITEM_BYTES} bytes"
        );
        let mut nonce = [0u8; 24];
        rng().fill_bytes(&mut nonce);
        let aad = item_aad(account, &item.key.docs_key());
        let ciphertext = self
            .cipher()?
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: plaintext.as_slice(),
                    aad: aad.as_slice(),
                },
            )
            .map_err(|_| anyhow!("failed to seal account sync item"))?;
        Ok(SealedAccountSyncItem {
            v: 1,
            nonce_hex: hex::encode(nonce),
            ciphertext_hex: hex::encode(ciphertext),
        })
    }

    /// 封を開く。別の account・別の key・改ざん・上限超過・allowlist の外は失敗にする。
    pub fn open(
        &self,
        account: &Pubkey,
        docs_key: &str,
        sealed: &SealedAccountSyncItem,
    ) -> Result<AccountSyncItem> {
        ensure!(sealed.v == 1, "unsupported account sync item version");
        ensure!(
            sealed.nonce_hex.len() + sealed.ciphertext_hex.len()
                <= MAX_SEALED_ACCOUNT_SYNC_ITEM_BYTES,
            "sealed account sync item is too large"
        );
        let nonce: [u8; 24] = hex::decode(&sealed.nonce_hex)?
            .try_into()
            .map_err(|_| anyhow!("invalid account sync nonce"))?;
        let ciphertext = hex::decode(&sealed.ciphertext_hex)?;
        let aad = item_aad(account, docs_key);
        let plaintext = self
            .cipher()?
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: ciphertext.as_slice(),
                    aad: aad.as_slice(),
                },
            )
            .map_err(|_| anyhow!("account sync item failed authentication"))?;
        ensure!(
            plaintext.len() <= MAX_ACCOUNT_SYNC_ITEM_BYTES,
            "account sync item exceeds {MAX_ACCOUNT_SYNC_ITEM_BYTES} bytes"
        );
        let item: AccountSyncItem =
            serde_json::from_slice(&plaintext).context("invalid account sync item")?;
        item.validate()?;
        ensure!(
            item.key.docs_key() == docs_key,
            "account sync item key does not match its docs key"
        );
        Ok(item)
    }

    fn cipher(&self) -> Result<XChaCha20Poly1305> {
        XChaCha20Poly1305::new_from_slice(&self.payload_key)
            .map_err(|_| anyhow!("failed to initialize account sync cipher"))
    }
}

fn item_aad(account: &Pubkey, docs_key: &str) -> Vec<u8> {
    let mut aad = ITEM_AAD_DOMAIN.to_vec();
    aad.extend_from_slice(account.as_str().as_bytes());
    aad.push(0);
    aad.extend_from_slice(docs_key.as_bytes());
    aad
}

/// 同期する item の種類（allowlist。ADR 0061 §2）。ここに無いものは同期しない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccountSyncItemKey {
    /// 公開 profile の署名済み envelope（公開の正本と同じ ID で運ぶ）。
    Profile,
    /// 著者を常に表示する指定（著者ごとの item）。
    TrustAlwaysVisible { author: Pubkey },
    /// private channel の受領済みの世代の鍵（channel・epoch ごとの item）。
    ChannelCapability {
        channel_id: ChannelId,
        epoch_id: String,
    },
    /// private channel の明示の退会・取消の記録。
    ChannelLeave { channel_id: ChannelId },
    /// private channel の鍵更新の担当端末の記録（意味は W6 が所有する）。
    ChannelController { channel_id: ChannelId },
}

impl AccountSyncItemKey {
    /// docs の key。id は hex にして区切りを含ませない。
    pub fn docs_key(&self) -> String {
        match self {
            Self::Profile => "profile".to_string(),
            Self::TrustAlwaysVisible { author } => {
                format!("trust/always-visible/{}", author.as_str())
            }
            Self::ChannelCapability {
                channel_id,
                epoch_id,
            } => format!(
                "channel/{}/epoch/{}",
                hex::encode(channel_id.as_str()),
                hex::encode(epoch_id)
            ),
            Self::ChannelLeave { channel_id } => {
                format!("channel/{}/leave", hex::encode(channel_id.as_str()))
            }
            Self::ChannelController { channel_id } => {
                format!("channel/{}/controller", hex::encode(channel_id.as_str()))
            }
        }
    }

    fn validate(&self) -> Result<()> {
        let id = |value: &str| {
            ensure!(
                !value.is_empty() && value.len() <= MAX_ITEM_ID_BYTES,
                "account sync item id must contain 1..={MAX_ITEM_ID_BYTES} bytes"
            );
            Ok(())
        };
        match self {
            Self::Profile => Ok(()),
            Self::TrustAlwaysVisible { author } => crate::crypto::validate_pubkey(author.as_str()),
            Self::ChannelCapability {
                channel_id,
                epoch_id,
            } => {
                id(channel_id.as_str())?;
                id(epoch_id)
            }
            Self::ChannelLeave { channel_id } | Self::ChannelController { channel_id } => {
                id(channel_id.as_str())
            }
        }
    }
}

/// 同期する 1 つの item。`value` が無い item は削除（tombstone）。
/// `value` は channel の鍵を含みうるので、`Debug` へ出さない。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSyncItem {
    pub key: AccountSyncItemKey,
    /// 更新ごとに作る操作の ID（再送しても変えない）。
    pub op_id: String,
    /// 編集した端末での編集時刻（ミリ秒）。再受信・restore の時刻を入れない。
    pub updated_at: i64,
    pub value: Option<serde_json::Value>,
}

impl std::fmt::Debug for AccountSyncItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountSyncItem")
            .field("key", &self.key)
            .field("op_id", &self.op_id)
            .field("updated_at", &self.updated_at)
            .field("tombstone", &self.value.is_none())
            .finish_non_exhaustive()
    }
}

impl AccountSyncItem {
    fn validate(&self) -> Result<()> {
        self.key.validate()?;
        ensure!(
            self.op_id.len() == 32 && self.op_id.bytes().all(|b| b.is_ascii_hexdigit()),
            "account sync op id must be 32 hex digits"
        );
        ensure!(
            self.updated_at >= 0,
            "account sync edit time must not be negative"
        );
        Ok(())
    }
}

/// docs へ置く、封をした item。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedAccountSyncItem {
    pub v: u8,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
}
