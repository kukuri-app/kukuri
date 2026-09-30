//! Account受信routeとendpointの署名付き対応（ADR 0055）。
//! 発見候補を認証済み接続先と混同しない。権限・配送成功の証明ではない。

use std::str::FromStr;

use anyhow::{Context, Result, ensure};
use secp256k1::{XOnlyPublicKey, schnorr::Signature};
use serde::{Deserialize, Serialize};

use crate::crypto::{sha256_digest, validate_pubkey};
use crate::{KukuriKeys, Pubkey, TopicId};

pub const RECEIVE_ENDPOINT_BINDING_MAX_BYTES: usize = 1_024;
pub const RECEIVE_ENDPOINT_BINDING_MAX_LIFETIME_MS: i64 = 300_000;
const MAX_FUTURE_SKEW_MS: i64 = 60_000;
const ROUTE_DOMAIN: &[u8] = b"kukuri:account-receive-route:v1\0";
const BINDING_DOMAIN: &str = "kukuri:receive-endpoint-binding:v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiveEndpointBindingV1 {
    pub version: u8,
    pub account: Pubkey,
    pub route: TopicId,
    pub endpoint_id: String,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub signature: String,
}

/// 指定したaccountと実接続endpointに対して検証済みの値。
/// raw wireから直接deserializeしたり、外部で構築したりできない。
#[derive(Clone, Debug)]
pub struct VerifiedReceiveEndpointBinding {
    binding: ReceiveEndpointBindingV1,
}

impl VerifiedReceiveEndpointBinding {
    pub fn account(&self) -> &Pubkey {
        &self.binding.account
    }

    pub fn route(&self) -> &TopicId {
        &self.binding.route
    }

    pub fn endpoint_id(&self) -> &str {
        &self.binding.endpoint_id
    }

    pub fn expires_at_ms(&self) -> i64 {
        self.binding.expires_at_ms
    }
}

pub fn receive_route_for_account(account: &Pubkey) -> Result<TopicId> {
    ensure_canonical_hex(account.as_str(), 32)?;
    validate_pubkey(account.as_str())?;
    let mut hash = blake3::Hasher::new();
    hash.update(ROUTE_DOMAIN);
    hash.update(&hex::decode(account.as_str())?);
    Ok(TopicId::new(format!(
        "receive::v1::{}",
        hash.finalize().to_hex()
    )))
}

impl ReceiveEndpointBindingV1 {
    pub fn sign(
        keys: &KukuriKeys,
        endpoint_id: &str,
        issued_at_ms: i64,
        expires_at_ms: i64,
    ) -> Result<Self> {
        // 確保の前にwire上限と同じendpoint長を検査する。
        ensure_canonical_hex(endpoint_id, 32)?;
        let account = keys.public_key();
        let mut binding = Self {
            version: 1,
            route: receive_route_for_account(&account)?,
            account,
            endpoint_id: endpoint_id.to_owned(),
            issued_at_ms,
            expires_at_ms,
            signature: String::new(),
        };
        binding.validate_fields()?;
        binding.signature = keys.sign_schnorr(&binding.digest()?).to_string();
        Ok(binding)
    }

    /// decodeだけでは署名・期限・接続先を検証しない。利用前にverify_forを呼ぶ。
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= RECEIVE_ENDPOINT_BINDING_MAX_BYTES,
            "receive binding is too large"
        );
        serde_json::from_slice(bytes).context("invalid receive endpoint binding")
    }

    /// actual_endpoint_idは候補/自己申告でなく、transportが認証したQUIC相手IDを渡す。
    pub fn verify_for(
        &self,
        expected_account: &Pubkey,
        actual_endpoint_id: &str,
        now_ms: i64,
    ) -> Result<VerifiedReceiveEndpointBinding> {
        self.validate_fields()?;
        ensure!(
            &self.account == expected_account,
            "receive binding account mismatch"
        );
        ensure!(
            self.endpoint_id == actual_endpoint_id,
            "receive binding endpoint mismatch"
        );
        ensure!(
            now_ms >= 0 && now_ms < self.expires_at_ms,
            "receive binding expired"
        );
        ensure!(
            self.issued_at_ms <= now_ms.saturating_add(MAX_FUTURE_SKEW_MS),
            "receive binding is from the future"
        );
        ensure_canonical_hex(&self.signature, 64)?;
        let signature =
            Signature::from_str(&self.signature).context("invalid binding signature")?;
        let account = XOnlyPublicKey::from_str(self.account.as_str())?;
        signature
            .verify(&self.digest()?, &account)
            .context("receive binding signature verification failed")?;
        Ok(VerifiedReceiveEndpointBinding {
            binding: self.clone(),
        })
    }

    fn validate_fields(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported receive binding version");
        ensure_canonical_hex(&self.endpoint_id, 32)?;
        ensure!(
            self.route == receive_route_for_account(&self.account)?,
            "receive binding route mismatch"
        );
        ensure!(self.issued_at_ms >= 0, "negative binding issue time");
        let lifetime = self
            .expires_at_ms
            .checked_sub(self.issued_at_ms)
            .context("receive binding lifetime overflow")?;
        ensure!(
            (1..=RECEIVE_ENDPOINT_BINDING_MAX_LIFETIME_MS).contains(&lifetime),
            "invalid binding lifetime"
        );
        Ok(())
    }

    fn digest(&self) -> Result<[u8; 32]> {
        // JSON object順序や追加fieldに依存しない、版付きの固定tupleを署名する。
        let canonical = serde_json::to_vec(&(
            BINDING_DOMAIN,
            self.version,
            &self.account,
            &self.route,
            &self.endpoint_id,
            self.issued_at_ms,
            self.expires_at_ms,
        ))?;
        Ok(sha256_digest(&canonical))
    }
}

fn ensure_canonical_hex(value: &str, bytes: usize) -> Result<()> {
    ensure!(
        value.len() == bytes * 2
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid canonical receive binding hex"
    );
    Ok(())
}
