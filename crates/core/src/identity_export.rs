use std::future::Future;

use anyhow::{Context, Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use secp256k1::rand::{RngCore, rng};
use serde::{Deserialize, Serialize};

use crate::Pubkey;
use crate::crypto::KukuriKeys;

pub const ACCOUNT_KEY_EXPORT_PREFIX: &str = "kukuri-account-key.v1.";
pub const ACCOUNT_KEY_EXPORT_VERSION: u32 = 1;
pub const ACCOUNT_KEY_EXPORT_KDF: &str = "argon2id";
pub const ACCOUNT_KEY_EXPORT_MIN_PASSPHRASE_CHARS: usize = 8;

const KDF_M_COST_KIB: u32 = 65536;
const KDF_T_COST: u32 = 3;
const KDF_P_COST: u32 = 1;
// インポート時に envelope 由来の KDF パラメータをそのまま実行するため、
// 細工された巨大パラメータでメモリ/CPU を浪費させられないよう上限を置く。
const KDF_MAX_M_COST_KIB: u32 = 1 << 20;
const KDF_MAX_T_COST: u32 = 16;
const KDF_MAX_P_COST: u32 = 8;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;

/// アカウント鍵エクスポートの暗号化 envelope。
///
/// 秘密鍵そのものは `ciphertext_hex` の中にしか存在しない。メタデータ
/// (version / KDF パラメータ / salt / public_key) は AEAD の AAD に束縛される
/// ため、書き換えると復号が認証エラーで失敗する。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountKeyExportEnvelopeV1 {
    version: u32,
    kdf: String,
    m_cost_kib: u32,
    t_cost: u32,
    p_cost: u32,
    salt_hex: String,
    nonce_hex: String,
    public_key: String,
    ciphertext_hex: String,
}

/// パスフレーズなしで確認できるエクスポートのメタデータ。秘密情報を含まない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountKeyExportPreview {
    pub version: u32,
    pub kdf: String,
    pub public_key: Pubkey,
}

/// passphrase の鍵の導出（argon2id）の入力。導出は重いので、実行する場所を呼出し側が選ぶ（native は blocking の
/// thread、Web は Worker）。passphrase を含むので `Debug` を持たない。
pub struct PassphraseKdf {
    pub passphrase: Vec<u8>,
    pub salt: Vec<u8>,
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl PassphraseKdf {
    pub fn derive(&self) -> Result<[u8; 32]> {
        if self.m_cost_kib > KDF_MAX_M_COST_KIB
            || self.t_cost > KDF_MAX_T_COST
            || self.p_cost > KDF_MAX_P_COST
        {
            bail!("account key export kdf parameters exceed supported bounds");
        }
        let params = Params::new(self.m_cost_kib, self.t_cost, self.p_cost, Some(32))
            .map_err(|_| anyhow!("invalid account key export kdf parameters"))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut key = [0u8; 32];
        argon2
            .hash_password_into(&self.passphrase, &self.salt, &mut key)
            .map_err(|_| anyhow!("failed to derive account key export key"))?;
        Ok(key)
    }
}

/// `derive` は受け取った `PassphraseKdf` の `derive` を呼んだ結果を返す（実行する場所だけを選ぶ）。
pub async fn encrypt_account_key_export<D, F>(
    keys: &KukuriKeys,
    passphrase: &str,
    derive: D,
) -> Result<String>
where
    D: FnOnce(PassphraseKdf) -> F,
    F: Future<Output = Result<[u8; 32]>>,
{
    if passphrase.chars().count() < ACCOUNT_KEY_EXPORT_MIN_PASSPHRASE_CHARS {
        bail!(
            "account key export passphrase must be at least {ACCOUNT_KEY_EXPORT_MIN_PASSPHRASE_CHARS} characters"
        );
    }
    let mut salt = [0u8; SALT_LEN];
    rng().fill_bytes(&mut salt);
    let mut nonce = [0u8; NONCE_LEN];
    rng().fill_bytes(&mut nonce);

    let salt_hex = hex::encode(salt);
    let public_key = keys.public_key_hex();
    let aad = account_key_export_aad(
        KDF_M_COST_KIB,
        KDF_T_COST,
        KDF_P_COST,
        salt_hex.as_str(),
        public_key.as_str(),
    );
    let key = derive(PassphraseKdf {
        passphrase: passphrase.as_bytes().to_vec(),
        salt: salt.to_vec(),
        m_cost_kib: KDF_M_COST_KIB,
        t_cost: KDF_T_COST,
        p_cost: KDF_P_COST,
    })
    .await?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .context("failed to initialize account key export cipher")?;
    let secret_hex = keys.export_secret_hex();
    let plaintext = hex::decode(secret_hex.as_str()).context("invalid account secret key hex")?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext.as_slice(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow!("failed to encrypt account key export"))?;

    let envelope = AccountKeyExportEnvelopeV1 {
        version: ACCOUNT_KEY_EXPORT_VERSION,
        kdf: ACCOUNT_KEY_EXPORT_KDF.to_string(),
        m_cost_kib: KDF_M_COST_KIB,
        t_cost: KDF_T_COST,
        p_cost: KDF_P_COST,
        salt_hex,
        nonce_hex: hex::encode(nonce),
        public_key,
        ciphertext_hex: hex::encode(ciphertext),
    };
    let body = serde_json::to_vec(&envelope).context("failed to encode account key export")?;
    Ok(format!(
        "{ACCOUNT_KEY_EXPORT_PREFIX}{}",
        BASE64_URL.encode(body)
    ))
}

pub fn preview_account_key_export(export: &str) -> Result<AccountKeyExportPreview> {
    let envelope = parse_account_key_export(export)?;
    Ok(AccountKeyExportPreview {
        version: envelope.version,
        kdf: envelope.kdf,
        public_key: Pubkey(envelope.public_key),
    })
}

/// `derive` は `encrypt_account_key_export` と同じ。
pub async fn decrypt_account_key_export<D, F>(
    export: &str,
    passphrase: &str,
    derive: D,
) -> Result<KukuriKeys>
where
    D: FnOnce(PassphraseKdf) -> F,
    F: Future<Output = Result<[u8; 32]>>,
{
    let envelope = parse_account_key_export(export)?;
    let salt = hex::decode(envelope.salt_hex.trim()).context("invalid account key export salt")?;
    if salt.len() != SALT_LEN {
        bail!("account key export salt must be {SALT_LEN} bytes");
    }
    let nonce =
        hex::decode(envelope.nonce_hex.trim()).context("invalid account key export nonce")?;
    if nonce.len() != NONCE_LEN {
        bail!("account key export nonce must be {NONCE_LEN} bytes");
    }
    let ciphertext = hex::decode(envelope.ciphertext_hex.trim())
        .context("invalid account key export ciphertext")?;
    let aad = account_key_export_aad(
        envelope.m_cost_kib,
        envelope.t_cost,
        envelope.p_cost,
        envelope.salt_hex.as_str(),
        envelope.public_key.as_str(),
    );
    let key = derive(PassphraseKdf {
        passphrase: passphrase.as_bytes().to_vec(),
        salt,
        m_cost_kib: envelope.m_cost_kib,
        t_cost: envelope.t_cost,
        p_cost: envelope.p_cost,
    })
    .await?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .context("failed to initialize account key export cipher")?;
    let plaintext = cipher
        .decrypt(
            <&XNonce>::try_from(nonce.as_slice()).expect("nonce length checked"),
            Payload {
                msg: ciphertext.as_slice(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| {
            anyhow!("failed to decrypt account key export: wrong passphrase or corrupted data")
        })?;
    let keys = KukuriKeys::parse(&hex::encode(plaintext))
        .context("account key export payload is not a valid secret key")?;
    if keys.public_key_hex() != envelope.public_key {
        bail!("account key export public key does not match decrypted secret");
    }
    Ok(keys)
}

fn parse_account_key_export(export: &str) -> Result<AccountKeyExportEnvelopeV1> {
    let trimmed = export.trim();
    let Some(body) = trimmed.strip_prefix(ACCOUNT_KEY_EXPORT_PREFIX) else {
        bail!("unsupported account key export format or version");
    };
    let bytes = BASE64_URL
        .decode(body.trim())
        .context("invalid account key export encoding")?;
    let envelope: AccountKeyExportEnvelopeV1 =
        serde_json::from_slice(&bytes).context("invalid account key export payload")?;
    if envelope.version != ACCOUNT_KEY_EXPORT_VERSION {
        bail!(
            "unsupported account key export version `{}`",
            envelope.version
        );
    }
    if envelope.kdf != ACCOUNT_KEY_EXPORT_KDF {
        bail!("unsupported account key export kdf `{}`", envelope.kdf);
    }
    crate::crypto::validate_pubkey(envelope.public_key.as_str())
        .context("invalid account key export public key")?;
    Ok(envelope)
}

fn account_key_export_aad(
    m_cost_kib: u32,
    t_cost: u32,
    p_cost: u32,
    salt_hex: &str,
    public_key: &str,
) -> String {
    format!(
        "kukuri:account-key-export:v{ACCOUNT_KEY_EXPORT_VERSION}:{ACCOUNT_KEY_EXPORT_KDF}:{m_cost_kib}:{t_cost}:{p_cost}:{salt_hex}:{public_key}"
    )
}
