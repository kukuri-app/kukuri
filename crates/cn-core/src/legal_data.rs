//! 権利案件・通報の機微区分を用途分離した鍵で暗号化する。

use anyhow::{Context, Result, anyhow, bail};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use chrono::{DateTime, Utc};
use secp256k1::rand::{RngCore, rng};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

const LEGAL_DATA_AAD_PREFIX: &[u8] = b"kukuri-cn-legal:data:v1:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensitiveDataCategory {
    ReportContact,
    RightsRequestContact,
    RightsRequestIdentity,
    RightsRequestEvidence,
}

impl SensitiveDataCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReportContact => "report_contact",
            Self::RightsRequestContact => "rights_request_contact",
            Self::RightsRequestIdentity => "rights_request_identity",
            Self::RightsRequestEvidence => "rights_request_evidence",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "report_contact" => Ok(Self::ReportContact),
            "rights_request_contact" => Ok(Self::RightsRequestContact),
            "rights_request_identity" => Ok(Self::RightsRequestIdentity),
            "rights_request_evidence" => Ok(Self::RightsRequestEvidence),
            _ => bail!("unknown sensitive data category `{value}`"),
        }
    }
}

/// 起動時の鍵の照合が読む 1 行（#1705）。暗号文を書くのは照合を通った鍵だけなので、全行が同じ鍵で
/// 書かれている。改ざんは各行を読むときの AEAD の認証で検出する。
pub const LEGAL_DATA_KEY_CHECK_SQL: &str =
    "SELECT owner_kind, owner_id, data_category, nonce, ciphertext
     FROM cn_legal.sensitive_items ORDER BY id LIMIT 1";

/// 設定した鍵で暗号文を 1 行復号し、誤鍵を通常API公開前に検出する。暗号文が無ければ照合しない。
pub async fn verify_legal_data_key(pool: &PgPool, cipher: &LegalDataCipher) -> Result<()> {
    let Some(row) = sqlx::query(LEGAL_DATA_KEY_CHECK_SQL)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(());
    };
    let owner_kind: String = row.try_get("owner_kind")?;
    let owner_id: String = row.try_get("owner_id")?;
    let category_name: String = row.try_get("data_category")?;
    let category = SensitiveDataCategory::parse(&category_name)?;
    decrypt_row::<serde_json::Value>(cipher, &owner_kind, &owner_id, category, &row).with_context(
        || {
            format!(
                "failed to verify encrypted legal data for {owner_kind}/{owner_id}/{category_name}"
            )
        },
    )?;
    Ok(())
}

#[derive(Clone)]
pub struct LegalDataCipher {
    key: [u8; 32],
}

impl std::fmt::Debug for LegalDataCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegalDataCipher").finish_non_exhaustive()
    }
}

impl LegalDataCipher {
    pub fn from_key_material(material: &str) -> Result<Self> {
        let material = material.trim();
        if material.len() < 32 {
            bail!("legal data encryption key must be at least 32 bytes");
        }
        if kukuri_core::is_placeholder_secret(material) {
            bail!("legal data encryption key still contains a placeholder value");
        }
        let mut hasher = Sha256::new();
        hasher.update(b"kukuri-cn-legal:data-key:v1");
        hasher.update(material.as_bytes());
        Ok(Self {
            key: hasher.finalize().into(),
        })
    }

    pub fn encrypt_json<T: Serialize>(
        &self,
        owner_kind: &str,
        owner_id: &str,
        category: SensitiveDataCategory,
        value: &T,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let plaintext = serde_json::to_vec(value)?;
        let mut nonce = [0u8; 24];
        rng().fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher()?
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &plaintext,
                    aad: &aad(owner_kind, owner_id, category),
                },
            )
            .map_err(|_| anyhow!("failed to encrypt legal data"))?;
        Ok((nonce.to_vec(), ciphertext))
    }

    pub fn decrypt_json<T: DeserializeOwned>(
        &self,
        owner_kind: &str,
        owner_id: &str,
        category: SensitiveDataCategory,
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<T> {
        if nonce.len() != 24 {
            bail!("legal data nonce must be 24 bytes");
        }
        let plaintext = self
            .cipher()?
            .decrypt(
                <&XNonce>::try_from(nonce).expect("nonce length checked"),
                Payload {
                    msg: ciphertext,
                    aad: &aad(owner_kind, owner_id, category),
                },
            )
            .map_err(|_| anyhow!("failed to decrypt legal data"))?;
        serde_json::from_slice(&plaintext).context("decrypted legal data is invalid JSON")
    }

    fn cipher(&self) -> Result<XChaCha20Poly1305> {
        XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|_| anyhow!("invalid legal data encryption key length"))
    }
}

fn aad(owner_kind: &str, owner_id: &str, category: SensitiveDataCategory) -> Vec<u8> {
    let mut result = LEGAL_DATA_AAD_PREFIX.to_vec();
    result.extend_from_slice(owner_kind.as_bytes());
    result.push(b':');
    result.extend_from_slice(owner_id.as_bytes());
    result.push(b':');
    result.extend_from_slice(category.as_str().as_bytes());
    result
}

pub async fn upsert_sensitive_json_in_tx<T: Serialize>(
    tx: &mut Transaction<'_, Postgres>,
    cipher: &LegalDataCipher,
    owner_kind: &str,
    owner_id: &str,
    category: SensitiveDataCategory,
    value: &T,
    expires_at: DateTime<Utc>,
) -> Result<()> {
    let (nonce, ciphertext) = cipher.encrypt_json(owner_kind, owner_id, category, value)?;
    sqlx::query(
        "INSERT INTO cn_legal.sensitive_items
            (id, owner_kind, owner_id, data_category, nonce, ciphertext, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (owner_kind, owner_id, data_category) DO UPDATE
         SET nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext,
             expires_at = EXCLUDED.expires_at",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(owner_kind)
    .bind(owner_id)
    .bind(category.as_str())
    .bind(nonce)
    .bind(ciphertext)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn load_sensitive_json<T: DeserializeOwned>(
    pool: &PgPool,
    cipher: &LegalDataCipher,
    owner_kind: &str,
    owner_id: &str,
    category: SensitiveDataCategory,
    now: DateTime<Utc>,
) -> Result<Option<T>> {
    let row = sqlx::query(
        "SELECT nonce, ciphertext FROM cn_legal.sensitive_items
         WHERE owner_kind = $1 AND owner_id = $2 AND data_category = $3
           AND expires_at > $4",
    )
    .bind(owner_kind)
    .bind(owner_id)
    .bind(category.as_str())
    .bind(now)
    .fetch_optional(pool)
    .await?;
    row.as_ref()
        .map(|row| decrypt_row(cipher, owner_kind, owner_id, category, row))
        .transpose()
}

pub(crate) fn decrypt_row<T: DeserializeOwned>(
    cipher: &LegalDataCipher,
    owner_kind: &str,
    owner_id: &str,
    category: SensitiveDataCategory,
    row: &PgRow,
) -> Result<T> {
    let nonce: Vec<u8> = row.try_get("nonce")?;
    let ciphertext: Vec<u8> = row.try_get("ciphertext")?;
    cipher.decrypt_json(owner_kind, owner_id, category, &nonce, &ciphertext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> LegalDataCipher {
        LegalDataCipher::from_key_material("unit-test-legal-data-key-0123456789abcdef")
            .expect("cipher")
    }

    #[test]
    fn roundtrip_and_aad_binding() {
        let cipher = cipher();
        let value = serde_json::json!({"email": "person@example.test"});
        let (nonce, ciphertext) = cipher
            .encrypt_json(
                "rights_request",
                "case-1",
                SensitiveDataCategory::RightsRequestContact,
                &value,
            )
            .expect("encrypt");
        let decoded: serde_json::Value = cipher
            .decrypt_json(
                "rights_request",
                "case-1",
                SensitiveDataCategory::RightsRequestContact,
                &nonce,
                &ciphertext,
            )
            .expect("decrypt");
        assert_eq!(decoded, value);
        assert!(
            cipher
                .decrypt_json::<serde_json::Value>(
                    "rights_request",
                    "case-2",
                    SensitiveDataCategory::RightsRequestContact,
                    &nonce,
                    &ciphertext
                )
                .is_err()
        );
    }

    #[test]
    fn tampering_and_weak_keys_are_rejected() {
        assert!(LegalDataCipher::from_key_material("short").is_err());
        let cipher = cipher();
        let (nonce, mut ciphertext) = cipher
            .encrypt_json("report", "r-1", SensitiveDataCategory::ReportContact, &"x")
            .expect("encrypt");
        ciphertext[0] ^= 1;
        assert!(
            cipher
                .decrypt_json::<String>(
                    "report",
                    "r-1",
                    SensitiveDataCategory::ReportContact,
                    &nonce,
                    &ciphertext
                )
                .is_err()
        );
    }
}
