use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use secp256k1::rand::{RngCore, rng};
use secp256k1::schnorr::Signature;
use secp256k1::{SECP256K1, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::crypto::{derive_hkdf_key, pairwise_shared_secret, sha256_digest, validate_pubkey};
use crate::{BlobHash, KukuriKeys, Pubkey, TopicId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectMessageAttachmentKind {
    Image,
    Video,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageEncryptedBlobRefV1 {
    pub blob_id: String,
    pub hash: BlobHash,
    pub mime: String,
    pub bytes: u64,
    pub nonce_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageAttachmentManifestV1 {
    pub attachment_id: String,
    pub kind: DirectMessageAttachmentKind,
    pub original: DirectMessageEncryptedBlobRefV1,
    #[serde(default)]
    pub poster: Option<DirectMessageEncryptedBlobRefV1>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessagePayloadV1 {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub attachment_manifest: Option<DirectMessageAttachmentManifestV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageFrameV1 {
    pub dm_id: String,
    pub message_id: String,
    pub sender: Pubkey,
    pub recipient: Pubkey,
    pub created_at: i64,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageAckV1 {
    pub dm_id: String,
    pub message_id: String,
    pub sender: Pubkey,
    pub recipient: Pubkey,
    pub acked_at: i64,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageEncryptedAttachmentV1 {
    pub blob_id: String,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
}

impl DirectMessageFrameV1 {
    pub fn verify(&self) -> Result<()> {
        validate_pubkey(self.sender.as_str()).context("invalid direct message sender pubkey")?;
        validate_pubkey(self.recipient.as_str())
            .context("invalid direct message recipient pubkey")?;
        if self.dm_id.trim().is_empty() {
            bail!("direct message frame dm_id is required");
        }
        if self.message_id.trim().is_empty() {
            bail!("direct message frame message_id is required");
        }
        let nonce =
            hex::decode(self.nonce_hex.trim()).context("invalid direct message frame nonce")?;
        if nonce.len() != 24 {
            bail!("direct message frame nonce must be 24 bytes");
        }
        let _ = hex::decode(self.ciphertext_hex.trim())
            .context("invalid direct message frame ciphertext")?;
        let signature = Signature::from_str(self.signature.as_str())
            .context("invalid direct message frame signature")?;
        let sender =
            XOnlyPublicKey::from_str(self.sender.as_str()).context("invalid frame sender")?;
        let digest = sha256_digest(canonical_direct_message_frame_payload(self)?.as_bytes());
        SECP256K1
            .verify_schnorr(&signature, &digest, &sender)
            .context("direct message frame signature verification failed")?;
        Ok(())
    }
}

impl DirectMessageAckV1 {
    pub fn verify(&self) -> Result<()> {
        validate_pubkey(self.sender.as_str())
            .context("invalid direct message ack sender pubkey")?;
        validate_pubkey(self.recipient.as_str())
            .context("invalid direct message ack recipient pubkey")?;
        if self.dm_id.trim().is_empty() {
            bail!("direct message ack dm_id is required");
        }
        if self.message_id.trim().is_empty() {
            bail!("direct message ack message_id is required");
        }
        let signature = Signature::from_str(self.signature.as_str())
            .context("invalid direct message ack signature")?;
        let sender =
            XOnlyPublicKey::from_str(self.sender.as_str()).context("invalid ack sender")?;
        let digest = sha256_digest(canonical_direct_message_ack_payload(self)?.as_bytes());
        SECP256K1
            .verify_schnorr(&signature, &digest, &sender)
            .context("direct message ack signature verification failed")?;
        Ok(())
    }
}

pub fn direct_message_id_for_participants(left: &Pubkey, right: &Pubkey) -> String {
    let mut participants = [left.as_str().to_string(), right.as_str().to_string()];
    participants.sort();
    let digest = blake3::hash(
        format!(
            "kukuri:direct-message:{}:{}",
            participants[0], participants[1]
        )
        .as_bytes(),
    );
    format!("dm-{}", digest.to_hex())
}

pub(crate) fn derive_direct_message_secret(
    local_keys: &KukuriKeys,
    remote_pubkey: &Pubkey,
) -> Result<[u8; 32]> {
    let shared = pairwise_shared_secret(local_keys, remote_pubkey)?;
    let mut participants = [
        local_keys.public_key_hex(),
        remote_pubkey.as_str().to_string(),
    ];
    participants.sort();
    let hkdf = Hkdf::<Sha256>::new(
        Some(b"kukuri/direct-message/root"),
        shared.secret_bytes().as_slice(),
    );
    let mut secret = [0u8; 32];
    hkdf.expand(
        format!(
            "kukuri:direct-message:{}:{}",
            participants[0], participants[1]
        )
        .as_bytes(),
        &mut secret,
    )
    .map_err(|_| anyhow!("failed to derive direct message root secret"))?;
    Ok(secret)
}

pub fn derive_direct_message_topic(
    local_keys: &KukuriKeys,
    remote_pubkey: &Pubkey,
) -> Result<TopicId> {
    let secret = derive_direct_message_secret(local_keys, remote_pubkey)?;
    let hkdf = Hkdf::<Sha256>::new(Some(b"kukuri/direct-message/topic"), secret.as_slice());
    let mut topic_key = [0u8; 32];
    hkdf.expand(b"kukuri:direct-message:pairwise-topic", &mut topic_key)
        .map_err(|_| anyhow!("failed to derive direct message topic"))?;
    Ok(TopicId::new(format!(
        "{}{}",
        crate::wire::DM_TOPIC_PREFIX,
        hex::encode(topic_key)
    )))
}

pub fn encrypt_direct_message_frame(
    local_keys: &KukuriKeys,
    recipient_pubkey: &Pubkey,
    dm_id: &str,
    message_id: &str,
    created_at: i64,
    payload: &DirectMessagePayloadV1,
) -> Result<DirectMessageFrameV1> {
    if dm_id.trim().is_empty() {
        bail!("direct message dm_id is required");
    }
    if message_id.trim().is_empty() {
        bail!("direct message message_id is required");
    }
    validate_pubkey(recipient_pubkey.as_str())
        .context("invalid direct message recipient pubkey")?;
    let sender = local_keys.public_key();
    let plaintext =
        serde_json::to_vec(payload).context("failed to encode direct message payload")?;
    let mut nonce = [0u8; 24];
    rng().fill_bytes(&mut nonce);
    let key = derive_direct_message_frame_key(
        &derive_direct_message_secret(local_keys, recipient_pubkey)?,
        dm_id,
        message_id,
        sender.as_str(),
        recipient_pubkey.as_str(),
        created_at,
    )?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .context("failed to initialize direct message frame cipher")?;
    let aad = direct_message_frame_aad(
        dm_id,
        message_id,
        sender.as_str(),
        recipient_pubkey.as_str(),
        created_at,
    );
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext.as_slice(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow!("failed to encrypt direct message frame"))?;
    let mut frame = DirectMessageFrameV1 {
        dm_id: dm_id.trim().to_string(),
        message_id: message_id.trim().to_string(),
        sender,
        recipient: recipient_pubkey.clone(),
        created_at,
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(ciphertext),
        signature: String::new(),
    };
    let digest = sha256_digest(canonical_direct_message_frame_payload(&frame)?.as_bytes());
    frame.signature = local_keys.sign_schnorr(&digest).to_string();
    Ok(frame)
}

pub fn decrypt_direct_message_frame(
    local_keys: &KukuriKeys,
    frame: &DirectMessageFrameV1,
) -> Result<DirectMessagePayloadV1> {
    frame.verify()?;
    if local_keys.public_key() != frame.recipient {
        bail!("direct message frame recipient pubkey must match decrypting author");
    }
    open_direct_message_frame(local_keys, frame, &frame.sender)
}

/// 送信者が自分の送った frame を開く(#1221 R5-G: 暗号化添付の hash は frame の中にしか無い)。
pub fn open_sent_direct_message_frame(
    local_keys: &KukuriKeys,
    frame: &DirectMessageFrameV1,
) -> Result<DirectMessagePayloadV1> {
    frame.verify()?;
    if local_keys.public_key() != frame.sender {
        bail!("direct message frame sender pubkey must match opening author");
    }
    open_direct_message_frame(local_keys, frame, &frame.recipient)
}

fn open_direct_message_frame(
    local_keys: &KukuriKeys,
    frame: &DirectMessageFrameV1,
    remote: &Pubkey,
) -> Result<DirectMessagePayloadV1> {
    let nonce =
        hex::decode(frame.nonce_hex.trim()).context("invalid direct message frame nonce")?;
    if nonce.len() != 24 {
        bail!("direct message frame nonce must be 24 bytes");
    }
    let ciphertext = hex::decode(frame.ciphertext_hex.trim())
        .context("invalid direct message frame ciphertext")?;
    let key = derive_direct_message_frame_key(
        &derive_direct_message_secret(local_keys, remote)?,
        frame.dm_id.as_str(),
        frame.message_id.as_str(),
        frame.sender.as_str(),
        frame.recipient.as_str(),
        frame.created_at,
    )?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .context("failed to initialize direct message frame cipher")?;
    let aad = direct_message_frame_aad(
        frame.dm_id.as_str(),
        frame.message_id.as_str(),
        frame.sender.as_str(),
        frame.recipient.as_str(),
        frame.created_at,
    );
    let plaintext = cipher
        .decrypt(
            <&XNonce>::try_from(nonce.as_slice()).expect("nonce length checked"),
            Payload {
                msg: ciphertext.as_slice(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow!("failed to decrypt direct message frame"))?;
    serde_json::from_slice(&plaintext).context("failed to decode direct message payload")
}

pub fn build_direct_message_ack(
    local_keys: &KukuriKeys,
    dm_id: &str,
    message_id: &str,
    recipient_pubkey: &Pubkey,
    acked_at: i64,
) -> Result<DirectMessageAckV1> {
    if dm_id.trim().is_empty() {
        bail!("direct message ack dm_id is required");
    }
    if message_id.trim().is_empty() {
        bail!("direct message ack message_id is required");
    }
    validate_pubkey(recipient_pubkey.as_str())
        .context("invalid direct message ack recipient pubkey")?;
    let mut ack = DirectMessageAckV1 {
        dm_id: dm_id.trim().to_string(),
        message_id: message_id.trim().to_string(),
        sender: local_keys.public_key(),
        recipient: recipient_pubkey.clone(),
        acked_at,
        signature: String::new(),
    };
    let digest = sha256_digest(canonical_direct_message_ack_payload(&ack)?.as_bytes());
    ack.signature = local_keys.sign_schnorr(&digest).to_string();
    Ok(ack)
}

pub fn encrypt_direct_message_attachment(
    local_keys: &KukuriKeys,
    recipient_pubkey: &Pubkey,
    message_id: &str,
    blob_id: &str,
    plaintext: &[u8],
) -> Result<DirectMessageEncryptedAttachmentV1> {
    if message_id.trim().is_empty() {
        bail!("direct message attachment message_id is required");
    }
    if blob_id.trim().is_empty() {
        bail!("direct message attachment blob_id is required");
    }
    let mut nonce = [0u8; 24];
    rng().fill_bytes(&mut nonce);
    let key = derive_direct_message_attachment_key(
        &derive_direct_message_secret(local_keys, recipient_pubkey)?,
        message_id,
        blob_id,
    )?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .context("failed to initialize direct message attachment cipher")?;
    let aad = direct_message_attachment_aad(message_id, blob_id);
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow!("failed to encrypt direct message attachment"))?;
    Ok(DirectMessageEncryptedAttachmentV1 {
        blob_id: blob_id.trim().to_string(),
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(ciphertext),
    })
}

pub fn decrypt_direct_message_attachment(
    local_keys: &KukuriKeys,
    remote_pubkey: &Pubkey,
    message_id: &str,
    attachment: &DirectMessageEncryptedAttachmentV1,
) -> Result<Vec<u8>> {
    if message_id.trim().is_empty() {
        bail!("direct message attachment message_id is required");
    }
    if attachment.blob_id.trim().is_empty() {
        bail!("direct message attachment blob_id is required");
    }
    let nonce = hex::decode(attachment.nonce_hex.trim())
        .context("invalid direct message attachment nonce")?;
    if nonce.len() != 24 {
        bail!("direct message attachment nonce must be 24 bytes");
    }
    let ciphertext = hex::decode(attachment.ciphertext_hex.trim())
        .context("invalid direct message attachment ciphertext")?;
    let key = derive_direct_message_attachment_key(
        &derive_direct_message_secret(local_keys, remote_pubkey)?,
        message_id,
        attachment.blob_id.as_str(),
    )?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .context("failed to initialize direct message attachment cipher")?;
    let aad = direct_message_attachment_aad(message_id, attachment.blob_id.as_str());
    cipher
        .decrypt(
            <&XNonce>::try_from(nonce.as_slice()).expect("nonce length checked"),
            Payload {
                msg: ciphertext.as_slice(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow!("failed to decrypt direct message attachment"))
}

fn derive_direct_message_frame_key(
    root_secret: &[u8; 32],
    dm_id: &str,
    message_id: &str,
    sender: &str,
    recipient: &str,
    created_at: i64,
) -> Result<[u8; 32]> {
    derive_hkdf_key(
        b"kukuri/direct-message/frame",
        root_secret.as_slice(),
        direct_message_frame_aad(dm_id, message_id, sender, recipient, created_at).as_bytes(),
        "direct message frame key",
    )
}

fn derive_direct_message_attachment_key(
    root_secret: &[u8; 32],
    message_id: &str,
    blob_id: &str,
) -> Result<[u8; 32]> {
    derive_hkdf_key(
        b"kukuri/direct-message/attachment",
        root_secret.as_slice(),
        direct_message_attachment_aad(message_id, blob_id).as_bytes(),
        "direct message attachment key",
    )
}

pub(crate) fn canonical_direct_message_frame_payload(
    frame: &DirectMessageFrameV1,
) -> Result<String> {
    serde_json::to_string(&serde_json::json!([
        0,
        frame.dm_id,
        frame.message_id,
        frame.sender,
        frame.recipient,
        frame.created_at,
        frame.nonce_hex,
        frame.ciphertext_hex
    ]))
    .context("failed to encode canonical direct message frame payload")
}

pub(crate) fn canonical_direct_message_ack_payload(ack: &DirectMessageAckV1) -> Result<String> {
    serde_json::to_string(&serde_json::json!([
        0,
        ack.dm_id,
        ack.message_id,
        ack.sender,
        ack.recipient,
        ack.acked_at
    ]))
    .context("failed to encode canonical direct message ack payload")
}

fn direct_message_frame_aad(
    dm_id: &str,
    message_id: &str,
    sender: &str,
    recipient: &str,
    created_at: i64,
) -> String {
    format!("kukuri:direct-message:frame:{dm_id}:{message_id}:{sender}:{recipient}:{created_at}")
}

fn direct_message_attachment_aad(message_id: &str, blob_id: &str) -> String {
    format!("kukuri:direct-message:attachment:{message_id}:{blob_id}")
}
