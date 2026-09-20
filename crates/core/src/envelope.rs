use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use secp256k1::schnorr::Signature;
use secp256k1::{SECP256K1, XOnlyPublicKey};
use serde::{Deserialize, Serialize};

use crate::crypto::sha256_digest;
use crate::{BlobHash, DirectMessageAckV1, EnvelopeId, Pubkey, TopicId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HintObjectRef {
    pub object_id: String,
    pub object_kind: String,
    /// その object を docs へ書いた docs author の id(ADR 0053 §2)。署名の無い手がかりで、読む record を選ぶことだけに使う。
    /// 旧 client の hint には無い。旧 client は、この field を無視して読む。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_author: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GossipHint {
    TopicObjectsChanged {
        topic_id: TopicId,
        objects: Vec<HintObjectRef>,
    },
    ThreadUpdated {
        root_id: EnvelopeId,
        object_ids: Vec<EnvelopeId>,
    },
    ProfileUpdated {
        author: Pubkey,
    },
    Presence {
        topic_id: TopicId,
        author: Pubkey,
        ttl_ms: u32,
    },
    Typing {
        topic_id: TopicId,
        root_id: Option<EnvelopeId>,
        author: Pubkey,
        ttl_ms: u32,
    },
    SessionChanged {
        topic_id: TopicId,
        session_id: String,
        object_kind: String,
    },
    LivePresence {
        topic_id: TopicId,
        session_id: String,
        author: Pubkey,
        ttl_ms: u32,
    },
    MetaverseRoomEvent {
        topic_id: TopicId,
        room_id: String,
        event: Box<KukuriEnvelope>,
    },
    DomeHostHeartbeat {
        topic_id: TopicId,
        instance_id: String,
        heartbeat: Box<crate::SignedDomeHostHeartbeatV1>,
    },
    DirectMessageFrame {
        topic_id: TopicId,
        dm_id: String,
        message_id: String,
        frame_hash: BlobHash,
    },
    DirectMessageAck {
        topic_id: TopicId,
        ack: DirectMessageAckV1,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriAuthEnvelopeContentV1 {
    pub scope: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriEnvelope {
    pub id: EnvelopeId,
    pub pubkey: Pubkey,
    pub created_at: i64,
    pub kind: String,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

impl KukuriEnvelope {
    pub fn verify(&self) -> Result<()> {
        let canonical = canonical_envelope_payload(
            self.pubkey.as_str(),
            self.created_at,
            self.kind.as_str(),
            &self.tags,
            self.content.as_str(),
        )?;
        let digest = sha256_digest(canonical.as_bytes());
        let computed_id = hex::encode(digest);
        if computed_id != self.id.0 {
            bail!("envelope id mismatch");
        }
        let signature = Signature::from_str(self.sig.as_str()).context("invalid envelope sig")?;
        let public_key =
            XOnlyPublicKey::from_str(self.pubkey.as_str()).context("invalid envelope pubkey")?;
        SECP256K1
            .verify_schnorr(&signature, &digest, &public_key)
            .context("envelope signature verification failed")?;
        Ok(())
    }
}

pub fn sign_envelope_json<T: Serialize>(
    keys: &crate::KukuriKeys,
    kind: impl Into<String>,
    tags: Vec<Vec<String>>,
    content: &T,
) -> Result<KukuriEnvelope> {
    let content = serde_json::to_string(content).context("failed to encode envelope content")?;
    sign_envelope(keys, kind, tags, content)
}

/// 作成時刻を指定して署名する。作成時刻が「今」でない envelope を作るとき(時系列の並びを固定する test など)に使う。
/// `created_at` は署名者の申告値で、受け取る側は信頼できる時刻として扱わない。
pub fn sign_envelope_json_at<T: Serialize>(
    keys: &crate::KukuriKeys,
    kind: impl Into<String>,
    tags: Vec<Vec<String>>,
    content: &T,
    created_at: i64,
) -> Result<KukuriEnvelope> {
    let content = serde_json::to_string(content).context("failed to encode envelope content")?;
    sign_envelope_at(keys, kind, tags, content, created_at)
}

pub(crate) fn sign_envelope(
    keys: &crate::KukuriKeys,
    kind: impl Into<String>,
    tags: Vec<Vec<String>>,
    content: String,
) -> Result<KukuriEnvelope> {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_secs() as i64;
    sign_envelope_at(keys, kind, tags, content, created_at)
}

pub(crate) fn sign_envelope_at(
    keys: &crate::KukuriKeys,
    kind: impl Into<String>,
    tags: Vec<Vec<String>>,
    content: String,
    created_at: i64,
) -> Result<KukuriEnvelope> {
    let kind = kind.into();
    let pubkey = keys.public_key_hex();
    let canonical = canonical_envelope_payload(
        pubkey.as_str(),
        created_at,
        kind.as_str(),
        &tags,
        content.as_str(),
    )?;
    let digest = sha256_digest(canonical.as_bytes());
    let id = hex::encode(digest);
    let sig = keys.sign_schnorr(&digest).to_string();
    Ok(KukuriEnvelope {
        id: EnvelopeId(id),
        pubkey: Pubkey(pubkey),
        created_at,
        kind,
        tags,
        content,
        sig,
    })
}

pub(crate) fn canonical_envelope_payload(
    pubkey: &str,
    created_at: i64,
    kind: &str,
    tags: &[Vec<String>],
    content: &str,
) -> Result<String> {
    serde_json::to_string(&serde_json::json!([
        0, pubkey, created_at, kind, tags, content
    ]))
    .context("failed to encode canonical envelope payload")
}
