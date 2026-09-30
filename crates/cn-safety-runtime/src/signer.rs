//! moderation event の本番実鍵署名（secp256k1 schnorr）と検証（#405）。
//!
//! `cn-safety` の [`ModerationEventSigner`] trait の本番実装。mock signer（非暗号 FNV-1a、
//! `cn-safety` の `mock` feature 限定）を置き換え、issuer node の secp256k1 鍵で
//! [`ModerationEventBody`] の canonical digest に schnorr 署名する。
//!
//! 署名対象は `sha256(body.canonical_bytes())`（32 byte digest）。`canonical_bytes()` は
//! object キーを辞書順へ正規化したクロス実装安定 canonical であり、同一論理内容なら
//! 署名対象 digest も安定する。
//!
//! 署名・検証は `kukuri-core` の secp256k1 schnorr（`KukuriEnvelope` と同じ方式）を再利用し、
//! 新しい暗号スタックを持ち込まない。
//!
//! issuer 同一性: moderation event の `issuer_node_id` は **署名鍵の x-only 公開鍵 hex** と
//! 一致する必要がある。[`verify_signed_event`] は `body.issuer_node_id` を公開鍵として検証する
//! ため、別鍵で署名した event や issuer を詐称した event は検証に失敗する。

use std::str::FromStr;

use kukuri_cn_safety::ModerationEventSigner;
use kukuri_cn_safety::event::{ModerationEventBody, SignedModerationEvent};
use kukuri_core::KukuriKeys;
use secp256k1::XOnlyPublicKey;
use secp256k1::schnorr::Signature;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// moderation event signing key を注入する env var。
///
/// Secret Manager の secret（operator-config の `safety.events.signing_key_secret_id` が指す）を
/// runtime に注入する経路。値は secp256k1 秘密鍵（hex / `nsec` bech32）。
pub const SAFETY_SIGNING_KEY_ENV: &str = "COMMUNITY_NODE_SAFETY_SIGNING_KEY";
/// signed event 無効時に risk signal issuer として使う node id を注入する env var。
pub const SAFETY_ISSUER_NODE_ID_ENV: &str = "COMMUNITY_NODE_SAFETY_ISSUER_NODE_ID";

/// この配備でリスク判定の `issuer_node_id` になる値を、署名鍵(優先)または明示指定から導出する。
///
/// - 署名鍵があれば x-only 公開鍵 hex(`Secp256k1ModerationEventSigner` と同一の導出)
/// - 無ければ明示指定の識別子(前後空白除去)
/// - どちらも無ければ `None`
///
/// cn-indexer が signal を発行する値と、公開ノード情報 `node_id` の一致検査(cn-user-api 起動時)、
/// 運用者向けの導出コマンド(`cn-cli moderation issuer-node-id`)が同じ関数を共有する(#706)。
pub fn expected_issuer_node_id(
    signing_key: Option<&str>,
    explicit_issuer_node_id: Option<&str>,
) -> Result<Option<String>, SignerKeyError> {
    if let Some(secret) = signing_key.map(str::trim).filter(|value| !value.is_empty()) {
        let signer = Secp256k1ModerationEventSigner::from_secret(secret)?;
        return Ok(Some(signer.issuer_node_id().to_string()));
    }
    Ok(explicit_issuer_node_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string))
}

/// [`expected_issuer_node_id`] を env var([`SAFETY_SIGNING_KEY_ENV`] / [`SAFETY_ISSUER_NODE_ID_ENV`])
/// から評価する。
pub fn expected_issuer_node_id_from_env() -> Result<Option<String>, SignerKeyError> {
    let signing_key = std::env::var(SAFETY_SIGNING_KEY_ENV).ok();
    let explicit = std::env::var(SAFETY_ISSUER_NODE_ID_ENV).ok();
    expected_issuer_node_id(signing_key.as_deref(), explicit.as_deref())
}

/// 本番 [`ModerationEventSigner`] 実装（secp256k1 schnorr）。
///
/// issuer node の鍵を保持し、`issuer_node_id()` として鍵の x-only 公開鍵 hex を返す。
/// `sign()` は `sha256(canonical_bytes)` への schnorr 署名を hex 文字列で返す。
#[derive(Clone)]
pub struct Secp256k1ModerationEventSigner {
    keys: KukuriKeys,
    issuer_node_id: String,
}

impl std::fmt::Debug for Secp256k1ModerationEventSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 秘密鍵を出力しない。issuer（公開鍵）のみ。
        f.debug_struct("Secp256k1ModerationEventSigner")
            .field("issuer_node_id", &self.issuer_node_id)
            .finish_non_exhaustive()
    }
}

impl Secp256k1ModerationEventSigner {
    /// 既存の鍵から signer を作る。`issuer_node_id` は鍵の x-only 公開鍵 hex。
    pub fn new(keys: KukuriKeys) -> Self {
        let issuer_node_id = keys.public_key_hex();
        Self {
            keys,
            issuer_node_id,
        }
    }

    /// secret 文字列（hex / `nsec` bech32）から signer を作る。
    ///
    /// placeholder（`change-me` 等）は本番経路で誤用しないよう拒否する。
    pub fn from_secret(secret: &str) -> Result<Self, SignerKeyError> {
        let trimmed = secret.trim();
        if trimmed.is_empty() {
            return Err(SignerKeyError::Missing);
        }
        reject_placeholder(trimmed)?;
        let keys = KukuriKeys::parse(trimmed).map_err(|err| SignerKeyError::InvalidKey {
            detail: err.to_string(),
        })?;
        Ok(Self::new(keys))
    }

    /// 注入された env var（[`SAFETY_SIGNING_KEY_ENV`]）から signer を作る。
    ///
    /// Secret Manager の signing key を runtime に注入する本番経路。未設定 / 空 / placeholder /
    /// 不正鍵はいずれもエラー。鍵を読めたときのみ signer を構築する。
    pub fn from_env() -> Result<Self, SignerKeyError> {
        let secret = std::env::var(SAFETY_SIGNING_KEY_ENV).map_err(|_| SignerKeyError::Missing)?;
        Self::from_secret(&secret)
    }
}

/// placeholder の signing key を拒否する。
///
/// 判定は `kukuri-core` の `is_placeholder_secret` を単一の真実源として共有し、JWT secret 検証
/// （`cn-core` の `validate_jwt_secret`）と marker リストが drift しないようにする。
fn reject_placeholder(secret: &str) -> Result<(), SignerKeyError> {
    if kukuri_core::is_placeholder_secret(secret) {
        return Err(SignerKeyError::Placeholder);
    }
    Ok(())
}

impl ModerationEventSigner for Secp256k1ModerationEventSigner {
    fn issuer_node_id(&self) -> &str {
        &self.issuer_node_id
    }

    fn sign(&self, body: &ModerationEventBody) -> String {
        let digest = canonical_digest(body);
        self.keys.sign_schnorr(&digest).to_string()
    }
}

/// 鍵の読み込み・解釈エラー。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SignerKeyError {
    /// signing key が注入されていない（env 未設定 / 空）。
    #[error(
        "moderation event signing key is not configured (set {SAFETY_SIGNING_KEY_ENV} from Secret Manager)"
    )]
    Missing,
    /// placeholder の signing key（本番で誤用させない）。
    #[error("moderation event signing key still contains a placeholder value; set a real key")]
    Placeholder,
    /// secret 文字列を secp256k1 鍵として解釈できない。
    #[error("invalid moderation event signing key: {detail}")]
    InvalidKey { detail: String },
}

/// signed moderation event の署名検証エラー。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SignatureError {
    /// `issuer_node_id` を x-only 公開鍵として解釈できない。
    #[error("invalid issuer_node_id (not an x-only public key)")]
    InvalidIssuer,
    /// signature 文字列を schnorr 署名として解釈できない。
    #[error("invalid signature encoding")]
    InvalidSignature,
    /// 署名が body / issuer に対して検証に失敗した（改竄 / 別鍵 / issuer 詐称）。
    #[error("signature verification failed")]
    VerificationFailed,
}

/// `sha256(body.canonical_bytes())` の 32 byte digest を返す（署名対象）。
fn canonical_digest(body: &ModerationEventBody) -> [u8; 32] {
    Sha256::digest(body.canonical_bytes()).into()
}

/// signed moderation event の署名を検証する。
///
/// `body.issuer_node_id` を x-only 公開鍵として用い、`sha256(canonical_bytes)` に対する
/// schnorr 署名を検証する。body を 1 byte でも改竄した場合、別鍵で署名した場合、issuer を
/// 詐称した場合はいずれも `Err` を返す。
pub fn verify_signed_event(event: &SignedModerationEvent) -> Result<(), SignatureError> {
    let public_key = XOnlyPublicKey::from_str(event.body.issuer_node_id.as_str())
        .map_err(|_| SignatureError::InvalidIssuer)?;
    let signature = Signature::from_str(event.signature.as_str())
        .map_err(|_| SignatureError::InvalidSignature)?;
    let digest = canonical_digest(&event.body);
    signature
        .verify(&digest, &public_key)
        .map_err(|_| SignatureError::VerificationFailed)
}
