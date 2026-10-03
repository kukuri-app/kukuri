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
use crate::{ChannelId, KukuriEnvelope, KukuriKeys, Pubkey, ReplicaId, TopicId};

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
    /// private channel の参加（値）と、明示の退会・取消（tombstone）。値は `ChannelMembershipV1`。
    ChannelMembership { channel_id: ChannelId },
    /// private channel の鍵更新の担当端末の記録（意味は W6 が所有する）。
    ChannelController { channel_id: ChannelId },
    /// 担当でない端末から担当への鍵更新の依頼（#1219 AC-3）。値は `ChannelRotationRequestV1`。
    ChannelRotationRequest { channel_id: ChannelId },
    /// 端末の変更の窓の 1 件（ADR 0061 §10）。値は `AccountSyncChangeV1`。merge の対象ではない。
    ChangeSlot { device_id: String, slot: u16 },
    /// 端末の変更の窓の head。値は `AccountSyncChangeV1`（`docs_key` は空）。
    ChangeHead { device_id: String },
}

/// 端末ごとの変更の窓の大きさ（ADR 0061 §5・§10）。
pub const ACCOUNT_SYNC_CHANGE_WINDOW: u64 = 256;

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
            Self::ChannelMembership { channel_id } => {
                format!("channel/{}/membership", hex::encode(channel_id.as_str()))
            }
            Self::ChannelController { channel_id } => {
                format!("channel/{}/controller", hex::encode(channel_id.as_str()))
            }
            Self::ChannelRotationRequest { channel_id } => {
                format!("channel/{}/rotation", hex::encode(channel_id.as_str()))
            }
            Self::ChangeSlot { device_id, slot } => format!("changes/{device_id}/{slot:03}"),
            Self::ChangeHead { device_id } => format!("changes/{device_id}/head"),
        }
    }

    /// 採用の行の key（docs の key）から、merge の対象の item の key を戻す（送り直しで行から item を作る）。
    pub fn from_docs_key(docs_key: &str) -> Result<Self> {
        let text = |value: &str| -> Result<String> {
            String::from_utf8(hex::decode(value)?).context("account sync item id is not UTF-8")
        };
        let key = if docs_key == "profile" {
            Self::Profile
        } else if let Some(author) = docs_key.strip_prefix("trust/always-visible/") {
            Self::TrustAlwaysVisible {
                author: Pubkey::from(author.to_string()),
            }
        } else if let Some(rest) = docs_key.strip_prefix("channel/") {
            let (channel, item) = rest.split_once('/').context("missing channel item")?;
            let channel_id = ChannelId::new(text(channel)?);
            match item.split_once('/') {
                Some(("epoch", epoch)) => Self::ChannelCapability {
                    channel_id,
                    epoch_id: text(epoch)?,
                },
                None if item == "membership" => Self::ChannelMembership { channel_id },
                None if item == "controller" => Self::ChannelController { channel_id },
                None if item == "rotation" => Self::ChannelRotationRequest { channel_id },
                _ => anyhow::bail!("unknown channel item"),
            }
        } else {
            anyhow::bail!("unknown account sync item key");
        };
        key.validate()?;
        ensure!(
            key.docs_key() == docs_key,
            "account sync item key is not canonical"
        );
        Ok(key)
    }

    /// seq の変更が入る、端末の変更の窓の slot。
    pub fn change_slot(device_id: &str, seq: u64) -> Self {
        Self::ChangeSlot {
            device_id: device_id.to_string(),
            slot: u16::try_from(seq % ACCOUNT_SYNC_CHANGE_WINDOW).unwrap_or_default(),
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
            Self::ChannelMembership { channel_id }
            | Self::ChannelController { channel_id }
            | Self::ChannelRotationRequest { channel_id } => id(channel_id.as_str()),
            Self::ChangeSlot { device_id, slot } => {
                ensure!(
                    u64::from(*slot) < ACCOUNT_SYNC_CHANGE_WINDOW,
                    "account sync change slot is outside the window"
                );
                validate_device_id(device_id)
            }
            Self::ChangeHead { device_id } => validate_device_id(device_id),
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
    /// 公開 profile の item。値は署名済みの envelope で、op_id は envelope の ID の先頭 32 桁、編集時刻は
    /// `created_at`。`(updated_at, op_id)` の比較が、ADR 0061 §4 の profile の規則（`created_at`、同じなら ID）になる。
    pub fn profile(envelope: &KukuriEnvelope) -> Result<Self> {
        let op_id = envelope
            .id
            .as_str()
            .get(..32)
            .context("profile envelope id is too short")?
            .to_string();
        let item = Self {
            key: AccountSyncItemKey::Profile,
            op_id,
            updated_at: envelope.created_at,
            value: Some(
                serde_json::to_value(envelope).context("failed to encode profile envelope")?,
            ),
        };
        item.validate()?;
        Ok(item)
    }

    /// private channel の受領済みの世代の鍵の item（ADR 0061 §9）。op_id は (channel, epoch) から決めるので、
    /// 同じ世代を書き直しても変わらない。`updated_at` は鍵を受け取った時刻。
    pub fn channel_epoch(
        channel_id: &ChannelId,
        epoch_id: &str,
        updated_at: i64,
        value: serde_json::Value,
    ) -> Result<Self> {
        let mut seed = b"kukuri channel epoch item v1\0".to_vec();
        seed.extend_from_slice(channel_id.as_str().as_bytes());
        seed.push(0);
        seed.extend_from_slice(epoch_id.as_bytes());
        let item = Self {
            key: AccountSyncItemKey::ChannelCapability {
                channel_id: channel_id.clone(),
                epoch_id: epoch_id.to_string(),
            },
            op_id: blake3::hash(&seed).to_hex()[..32].to_string(),
            updated_at,
            value: Some(value),
        };
        item.validate()?;
        Ok(item)
    }

    /// この端末での編集の item。op_id は新しく作る（再送しても変えない）。
    pub fn edit(
        key: AccountSyncItemKey,
        updated_at: i64,
        value: Option<serde_json::Value>,
    ) -> Self {
        let mut op_id = [0u8; 16];
        rng().fill_bytes(&mut op_id);
        Self {
            key,
            op_id: hex::encode(op_id),
            updated_at,
            value,
        }
    }

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

/// 端末 ID（endpoint ID）は英数字・`-`・`_` だけ（docs の key の区切りを含ませない）。
fn validate_device_id(device_id: &str) -> Result<()> {
    ensure!(
        !device_id.is_empty()
            && device_id.len() <= MAX_ITEM_ID_BYTES
            && device_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "account sync device id must be alphanumeric"
    );
    Ok(())
}

/// 変更の窓の slot・head の値（ADR 0061 §10）。slot は seq と、その変更の item の docs の key。head は seq だけ。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSyncChangeV1 {
    pub seq: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_key: String,
}

/// private channel の参加の item の値（ADR 0061 §9）。参加の行のうち端末に依らない欄で、`current_epoch_id` は参加した
/// ときの現在の世代。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelMembershipV1 {
    pub topic_id: String,
    pub label: String,
    pub creator_pubkey: String,
    pub owner_pubkey: String,
    pub joined_via_pubkey: Option<String>,
    pub audience_kind: crate::ChannelAudienceKind,
    pub current_epoch_id: String,
}

/// 鍵更新の依頼の item の値（#1219 AC-3、ADR 0018 §8）。担当は、現在の世代が `from_epoch_id` のときだけ鍵を更新する
/// （同じ依頼の再受信・既に進んだ世代からの依頼は何もしない）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelRotationRequestV1 {
    pub from_epoch_id: String,
}
