//! Account受信routeの小さい暗号化参照（ADR 0055）。
//! 署名確認とepoch復号はscope権限判定の代替ではない。network I/Oを行わない。

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, ensure};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use secp256k1::rand::{RngCore, rng};
use secp256k1::{XOnlyPublicKey, schnorr::Signature};
use serde::{Deserialize, Serialize};

use crate::crypto::{derive_hkdf_key, pairwise_shared_secret, sha256_digest, validate_pubkey};
use crate::{BlobHash, KukuriKeys, Pubkey};

pub const RECEIVE_OFFER_MAX_BYTES: usize = 2_048;
pub const RECEIVE_PAYLOAD_MAX_BYTES: usize = 65_536;
pub const PRIVATE_RECEIVE_PAYLOAD_MAX_PLAINTEXT_BYTES: usize = 16_384;
pub const RECEIVE_OFFER_MAX_LIFETIME_MS: i64 = 300_000;
const OFFER_MAX_PLAINTEXT_BYTES: usize = 880;
const OFFER_DOMAIN: &str = "kukuri:receive-offer:v1";
const OFFER_KEY_DOMAIN: &[u8] = b"kukuri:receive-offer-key:v1";
const PRIVATE_KEY_DOMAIN: &[u8] = b"kukuri:private-receive-payload:v1\0";
const EPOCH_ID_DOMAIN: &[u8] = b"kukuri:receive-epoch-key-id:v1\0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiveOfferScopeV1 {
    PublicSource,
    DirectMessage,
    DirectMessageFrame {
        dm_id: String,
        message_id: String,
        frame_hash: BlobHash,
    },
    DirectMessageAck {
        dm_id: String,
        message_id: String,
        acked_at: i64,
        signature: String,
    },
    PrivateSource {
        epoch_key_id: String,
    },
    EpochControl {
        epoch_key_id: String,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiveOfferReferenceV1 {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_endpoint_id: String,
    #[serde(default, skip_serializing_if = "is_empty_hash")]
    pub payload_hash: BlobHash,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub payload_bytes: u32,
    pub scope: ReceiveOfferScopeV1,
}

impl std::fmt::Debug for ReceiveOfferReferenceV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReceiveOfferReferenceV1")
            .finish_non_exhaustive()
    }
}

impl ReceiveOfferReferenceV1 {
    pub fn inline(provider_endpoint_id: String, scope: ReceiveOfferScopeV1) -> Result<Self> {
        ensure!(
            matches!(
                &scope,
                ReceiveOfferScopeV1::DirectMessageFrame { .. }
                    | ReceiveOfferScopeV1::DirectMessageAck { .. }
            ),
            "inline receive offer scope is unsupported"
        );
        let reference = Self {
            provider_endpoint_id,
            payload_hash: BlobHash::default(),
            payload_bytes: 0,
            scope,
        };
        reference.validate()?;
        Ok(reference)
    }

    fn validate(&self) -> Result<()> {
        match &self.scope {
            ReceiveOfferScopeV1::DirectMessageFrame { frame_hash, .. } => {
                fixed_hex::<32>(&self.provider_endpoint_id)?;
                fixed_hex::<32>(frame_hash.as_str())?;
                ensure!(
                    self.payload_bytes == 0 && self.payload_hash.as_str().is_empty(),
                    "inline receive offer cannot reference a blob"
                );
            }
            ReceiveOfferScopeV1::DirectMessageAck { signature, .. } => {
                ensure!(
                    self.provider_endpoint_id.is_empty(),
                    "inline ACK cannot name a provider"
                );
                fixed_hex::<64>(signature)?;
                ensure!(
                    self.payload_bytes == 0 && self.payload_hash.as_str().is_empty(),
                    "inline receive offer cannot reference a blob"
                );
            }
            ReceiveOfferScopeV1::PrivateSource { epoch_key_id }
            | ReceiveOfferScopeV1::EpochControl { epoch_key_id } => {
                fixed_hex::<32>(&self.provider_endpoint_id)?;
                fixed_hex::<32>(self.payload_hash.as_str())?;
                fixed_hex::<32>(epoch_key_id)?;
                ensure!(
                    (1..=RECEIVE_PAYLOAD_MAX_BYTES as u32).contains(&self.payload_bytes),
                    "invalid receive payload size"
                );
            }
            _ => {
                fixed_hex::<32>(&self.provider_endpoint_id)?;
                fixed_hex::<32>(self.payload_hash.as_str())?;
                ensure!(
                    (1..=RECEIVE_PAYLOAD_MAX_BYTES as u32).contains(&self.payload_bytes),
                    "invalid receive payload size"
                );
            }
        }
        Ok(())
    }
}

fn is_empty_hash(hash: &BlobHash) -> bool {
    hash.as_str().is_empty()
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedReceiveOffer {
    version: u8,
    sender: Pubkey,
    recipient: Pubkey,
    reference: ReceiveOfferReferenceV1,
    issued_at_ms: i64,
    expires_at_ms: i64,
    signature: String,
}

/// 送信者の署名を検証済み。providerのendpoint binding、参加状態、mutual、
/// private capabilityと失効世代はI/O前に受信側が別途検証する。
pub struct VerifiedReceiveOffer(SignedReceiveOffer);

impl std::fmt::Debug for VerifiedReceiveOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedReceiveOffer")
            .finish_non_exhaustive()
    }
}

impl VerifiedReceiveOffer {
    pub fn sender(&self) -> &Pubkey {
        &self.0.sender
    }
    pub fn recipient(&self) -> &Pubkey {
        &self.0.recipient
    }
    pub fn reference(&self) -> &ReceiveOfferReferenceV1 {
        &self.0.reference
    }
    pub fn expires_at_ms(&self) -> i64 {
        self.0.expires_at_ms
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedReceiveOfferV1 {
    pub version: u8,
    pub ephemeral_pubkey: Pubkey,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
}

pub fn seal_receive_offer(
    sender: &KukuriKeys,
    recipient: &Pubkey,
    reference: ReceiveOfferReferenceV1,
    issued_at_ms: i64,
    expires_at_ms: i64,
) -> Result<SealedReceiveOfferV1> {
    reference.validate()?;
    check_pubkey(recipient)?;
    check_lifetime(issued_at_ms, expires_at_ms)?;
    let mut signed = SignedReceiveOffer {
        version: 1,
        sender: sender.public_key(),
        recipient: recipient.clone(),
        reference,
        issued_at_ms,
        expires_at_ms,
        signature: String::new(),
    };
    signed.signature = sender.sign_schnorr(&signed.digest()?).to_string();
    seal_signed_offer(&signed)
}

fn seal_signed_offer(signed: &SignedReceiveOffer) -> Result<SealedReceiveOfferV1> {
    let plaintext = serde_json::to_vec(signed)?;
    ensure!(
        plaintext.len() <= OFFER_MAX_PLAINTEXT_BYTES,
        "receive offer is too large"
    );
    let ephemeral = KukuriKeys::generate();
    let ephemeral_pubkey = ephemeral.public_key();
    let aad = offer_aad(&ephemeral_pubkey, &signed.recipient)?;
    let shared = pairwise_shared_secret(&ephemeral, &signed.recipient)?;
    let key = derive_hkdf_key(
        OFFER_KEY_DOMAIN,
        &shared.to_secret_bytes(),
        &aad,
        "receive offer",
    )?;
    let (nonce_hex, ciphertext_hex) = encrypt(&key, &aad, &plaintext)?;
    let sealed = SealedReceiveOfferV1 {
        version: 1,
        ephemeral_pubkey,
        nonce_hex,
        ciphertext_hex,
    };
    sealed.encode()?;
    Ok(sealed)
}

impl SealedReceiveOfferV1 {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= RECEIVE_OFFER_MAX_BYTES,
            "receive offer is too large"
        );
        let value: Self = serde_json::from_slice(bytes).context("invalid receive offer")?;
        value.validate()?;
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        ensure!(
            bytes.len() <= RECEIVE_OFFER_MAX_BYTES,
            "receive offer is too large"
        );
        Ok(bytes)
    }

    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported receive offer version");
        check_pubkey(&self.ephemeral_pubkey)?;
        fixed_hex::<24>(&self.nonce_hex)?;
        check_ciphertext(&self.ciphertext_hex, OFFER_MAX_PLAINTEXT_BYTES)?;
        Ok(())
    }

    pub fn open(&self, recipient: &KukuriKeys, now_ms: i64) -> Result<VerifiedReceiveOffer> {
        self.validate()?;
        let recipient_pubkey = recipient.public_key();
        let aad = offer_aad(&self.ephemeral_pubkey, &recipient_pubkey)?;
        let shared = pairwise_shared_secret(recipient, &self.ephemeral_pubkey)?;
        let key = derive_hkdf_key(
            OFFER_KEY_DOMAIN,
            &shared.to_secret_bytes(),
            &aad,
            "receive offer",
        )?;
        let plaintext = decrypt(&key, &aad, &self.nonce_hex, &self.ciphertext_hex)?;
        let signed: SignedReceiveOffer =
            serde_json::from_slice(&plaintext).context("invalid signed receive offer")?;
        signed.verify(&recipient_pubkey, now_ms)?;
        Ok(VerifiedReceiveOffer(signed))
    }
}

impl SignedReceiveOffer {
    fn digest(&self) -> Result<[u8; 32]> {
        Ok(sha256_digest(&serde_json::to_vec(&(
            OFFER_DOMAIN,
            self.version,
            &self.sender,
            &self.recipient,
            &self.reference,
            self.issued_at_ms,
            self.expires_at_ms,
        ))?))
    }

    fn verify(&self, recipient: &Pubkey, now_ms: i64) -> Result<()> {
        ensure!(
            self.version == 1 && &self.recipient == recipient,
            "receive offer recipient/version mismatch"
        );
        check_pubkey(&self.sender)?;
        self.reference.validate()?;
        check_lifetime(self.issued_at_ms, self.expires_at_ms)?;
        ensure!(
            now_ms >= 0 && now_ms < self.expires_at_ms,
            "receive offer expired"
        );
        ensure!(
            self.issued_at_ms <= now_ms.saturating_add(60_000),
            "receive offer is from the future"
        );
        fixed_hex::<64>(&self.signature)?;
        let signature = Signature::from_str(&self.signature)?;
        signature
            .verify(
                &self.digest()?,
                &XOnlyPublicKey::from_str(self.sender.as_str())?,
            )
            .context("receive offer signature verification failed")
    }
}

/// private参照の中身。account宛offerを復号できても、epoch鍵がなければ参照を読めない。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateReceivePayloadV1 {
    pub version: u8,
    pub epoch_key_id: String,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
}

pub fn receive_epoch_key_id(secret: &[u8; 32], channel: &str, epoch: &str) -> Result<String> {
    Ok(hex::encode(epoch_key(
        secret,
        channel,
        epoch,
        EPOCH_ID_DOMAIN,
    )?))
}

pub fn seal_private_receive_payload(
    secret: &[u8; 32],
    channel: &str,
    epoch: &str,
    plaintext: &[u8],
) -> Result<PrivateReceivePayloadV1> {
    ensure!(
        plaintext.len() <= PRIVATE_RECEIVE_PAYLOAD_MAX_PLAINTEXT_BYTES,
        "private receive payload is too large"
    );
    let epoch_key_id = receive_epoch_key_id(secret, channel, epoch)?;
    let key = epoch_key(secret, channel, epoch, PRIVATE_KEY_DOMAIN)?;
    let aad = serde_json::to_vec(&("kukuri:private-receive-payload:v1", 1, &epoch_key_id))?;
    let (nonce_hex, ciphertext_hex) = encrypt(&key, &aad, plaintext)?;
    Ok(PrivateReceivePayloadV1 {
        version: 1,
        epoch_key_id,
        nonce_hex,
        ciphertext_hex,
    })
}

impl PrivateReceivePayloadV1 {
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        ensure!(
            bytes.len() <= RECEIVE_PAYLOAD_MAX_BYTES,
            "private receive payload is too large"
        );
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= RECEIVE_PAYLOAD_MAX_BYTES,
            "private receive payload is too large"
        );
        let value: Self =
            serde_json::from_slice(bytes).context("invalid private receive payload")?;
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported private receive payload version"
        );
        fixed_hex::<32>(&self.epoch_key_id)?;
        fixed_hex::<24>(&self.nonce_hex)?;
        check_ciphertext(
            &self.ciphertext_hex,
            PRIVATE_RECEIVE_PAYLOAD_MAX_PLAINTEXT_BYTES,
        )
    }

    pub fn open(&self, secret: &[u8; 32], channel: &str, epoch: &str) -> Result<Vec<u8>> {
        self.validate()?;
        ensure!(
            self.epoch_key_id == receive_epoch_key_id(secret, channel, epoch)?,
            "private receive epoch mismatch"
        );
        let key = epoch_key(secret, channel, epoch, PRIVATE_KEY_DOMAIN)?;
        let aad = serde_json::to_vec(&(
            "kukuri:private-receive-payload:v1",
            self.version,
            &self.epoch_key_id,
        ))?;
        decrypt(&key, &aad, &self.nonce_hex, &self.ciphertext_hex)
    }
}

fn epoch_key(secret: &[u8; 32], channel: &str, epoch: &str, domain: &[u8]) -> Result<[u8; 32]> {
    ensure!(
        !channel.is_empty() && channel.len() <= 1024 && !epoch.is_empty() && epoch.len() <= 1024,
        "invalid receive epoch identifiers"
    );
    let mut hash = blake3::Hasher::new_keyed(secret);
    hash.update(domain);
    for value in [channel, epoch] {
        hash.update(&(value.len() as u32).to_be_bytes());
        hash.update(value.as_bytes());
    }
    Ok(*hash.finalize().as_bytes())
}

fn offer_aad(ephemeral: &Pubkey, recipient: &Pubkey) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&(
        OFFER_DOMAIN,
        1,
        ephemeral,
        recipient,
    ))?)
}

fn encrypt(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<(String, String)> {
    let mut nonce = [0; 24];
    rng().fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| anyhow!("receive encryption failed"))?;
    Ok((hex::encode(nonce), hex::encode(ciphertext)))
}

fn decrypt(key: &[u8; 32], aad: &[u8], nonce_hex: &str, ciphertext_hex: &str) -> Result<Vec<u8>> {
    let nonce = fixed_hex::<24>(nonce_hex)?;
    let ciphertext = hex::decode(ciphertext_hex)?;
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &ciphertext,
                aad,
            },
        )
        .map_err(|_| anyhow!("receive decryption failed"))
}

fn fixed_hex<const N: usize>(value: &str) -> Result<[u8; N]> {
    ensure!(
        value.len() == N * 2
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid canonical receive hex"
    );
    let mut bytes = [0; N];
    hex::decode_to_slice(value, &mut bytes)?;
    Ok(bytes)
}

fn check_ciphertext(value: &str, plaintext_limit: usize) -> Result<()> {
    ensure!(
        (32..=(plaintext_limit + 16) * 2).contains(&value.len()) && value.len().is_multiple_of(2),
        "invalid receive ciphertext length"
    );
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid receive ciphertext hex"
    );
    Ok(())
}

fn check_pubkey(pubkey: &Pubkey) -> Result<()> {
    fixed_hex::<32>(pubkey.as_str())?;
    validate_pubkey(pubkey.as_str())
}

fn check_lifetime(issued: i64, expires: i64) -> Result<()> {
    ensure!(issued >= 0, "negative receive offer time");
    let lifetime = expires
        .checked_sub(issued)
        .context("receive offer lifetime overflow")?;
    ensure!(
        (1..=RECEIVE_OFFER_MAX_LIFETIME_MS).contains(&lifetime),
        "invalid receive offer lifetime"
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/receive_offer.rs"]
mod tests;
