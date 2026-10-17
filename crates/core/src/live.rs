use anyhow::Result;
use secp256k1::schnorr::Signature;
use secp256k1::{Keypair, XOnlyPublicKey};
use serde::{Deserialize, Serialize};

use crate::{ChannelId, EnvelopeId, ManifestBlobRef, Pubkey, TopicId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveSignalKind {
    SessionStarted,
    SessionEnded,
    RoomActivity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum LiveSessionStatus {
    Scheduled,
    Live,
    Paused,
    Ended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSessionManifestBlobV1 {
    pub session_id: String,
    /// Owner-signed monotonic revision. The first persisted state is 1.
    pub revision: i64,
    pub topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub owner_pubkey: Pubkey,
    pub title: String,
    pub description: String,
    pub status: LiveSessionStatus,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// 映像・音声を送る配信者の端末の endpoint id(ADR 0067 §8)。無い session は media を持たない。
    /// 旧版は知らない項目として読み飛ばす。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_endpoint_id: Option<String>,
    /// 映像・音声の frame の配信者の署名を確かめる鍵(x-only 公開鍵の hex。ADR 0067 §8)。転送する視聴者が中身を
    /// 変えたら、視聴者はこの鍵で見分ける。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_signing_key: Option<String>,
    /// 開始 post の id(#1839)。この Live への返信・リアクションはこの post に付く。開始 post の無い旧版の manifest には無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_post_id: Option<EnvelopeId>,
}

/// post が参照する Live(#1839)。Live の id と、取得・認可に使う元の場所(作成した topic・限定なら channel)。
/// 参照は Live の権限を変えない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRefV1 {
    pub session_id: String,
    pub topic_id: TopicId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
}

impl LiveRefV1 {
    /// この場所(topic・channel)の post が参照してよい Live か。限定 channel の Live は、その channel の post からだけ参照できる
    /// (非公開投稿を repost できないのと同じ。公開の場所に限定 channel の id と Live の id を載せない)。公開 channel(#1840)
    /// で始めた Live は公開の Live なので、どこからでも参照できる。
    pub fn allowed_in(&self, topic_id: &TopicId, channel_id: Option<&ChannelId>) -> bool {
        match &self.channel_id {
            None => true,
            Some(channel) if channel.is_public_channel() => true,
            Some(channel) => self.topic_id == *topic_id && Some(channel) == channel_id,
        }
    }
}

/// live の配信の frame の署名鍵(owner の鍵から session ごとに導く。`KukuriKeys::derive_live_media_signer`)。
#[derive(Clone)]
pub struct LiveMediaSigner(pub(crate) Keypair);

/// frame の payload の先頭に置く署名の長さ(Schnorr)。
const LIVE_MEDIA_SIGNATURE_LEN: usize = 64;

impl LiveMediaSigner {
    /// manifest に載せる公開鍵(x-only の hex)。
    pub fn public_key_hex(&self) -> String {
        self.0.x_only_public_key().0.to_string()
    }

    /// track・timestamp・payload に署名し、署名を先頭に付けた payload を返す。
    pub fn sign(&self, track: &str, timestamp_us: u64, payload: &[u8]) -> Vec<u8> {
        let signature = self
            .0
            .sign_schnorr(&live_media_digest(track, timestamp_us, payload));
        let mut signed = Vec::with_capacity(LIVE_MEDIA_SIGNATURE_LEN + payload.len());
        signed.extend_from_slice(&signature.to_byte_array());
        signed.extend_from_slice(payload);
        signed
    }
}

/// 配信者の署名を確かめる鍵(manifest の `media_signing_key`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveMediaVerifier(XOnlyPublicKey);

impl LiveMediaVerifier {
    pub fn from_hex(key: &str) -> Option<Self> {
        key.parse().ok().map(Self)
    }

    /// 配信者の署名を確かめ、署名を除いた payload を返す。確かめられなければ `None`。
    pub fn verify<'a>(&self, track: &str, timestamp_us: u64, signed: &'a [u8]) -> Option<&'a [u8]> {
        let (signature, payload) = signed.split_at_checked(LIVE_MEDIA_SIGNATURE_LEN)?;
        Signature::from_byte_array(signature.try_into().ok()?)
            .verify(&live_media_digest(track, timestamp_us, payload), &self.0)
            .ok()?;
        Some(payload)
    }
}

fn live_media_digest(track: &str, timestamp_us: u64, payload: &[u8]) -> [u8; 32] {
    let mut message = Vec::with_capacity(32 + track.len() + payload.len());
    message.extend_from_slice(b"kukuri live media frame v1");
    message.extend_from_slice(&(track.len() as u64).to_le_bytes());
    message.extend_from_slice(track.as_bytes());
    message.extend_from_slice(&timestamp_us.to_le_bytes());
    message.extend_from_slice(payload);
    crate::crypto::sha256_digest(&message)
}

/// 配信の映像・音声の broadcast の path(ADR 0067 §9)。
pub fn live_media_broadcast_path(session_id: &str) -> String {
    format!("live/{session_id}")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSessionStateDocV1 {
    pub session_id: String,
    pub topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub owner_pubkey: Pubkey,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: LiveSessionStatus,
    pub current_manifest: ManifestBlobRef,
    pub last_envelope_id: EnvelopeId,
}

pub fn build_live_session_envelope<T: Serialize>(
    keys: &crate::KukuriKeys,
    topic: &TopicId,
    session_id: &str,
    content: &T,
) -> Result<crate::KukuriEnvelope> {
    crate::sign_envelope_json(
        keys,
        "live-session",
        vec![
            vec!["topic".into(), topic.as_str().into()],
            vec!["object".into(), "live-session".into()],
            vec!["session_id".into(), session_id.to_string()],
        ],
        content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KukuriKeys;

    // #1034 AC-2b: 配信者の署名は track・timestamp・payload に束縛し、session ごとの鍵で確かめる。
    #[test]
    fn live_media_frames_verify_only_with_the_session_key() {
        let keys = KukuriKeys::generate();
        let signer = keys.derive_live_media_signer("live-1").expect("signer");
        assert_eq!(
            signer.public_key_hex(),
            keys.derive_live_media_signer("live-1")
                .expect("signer")
                .public_key_hex(),
            "the same account and session derive the same key"
        );
        let key = LiveMediaVerifier::from_hex(&signer.public_key_hex()).expect("key");
        let signed = signer.sign("video", 7, b"frame");
        assert_eq!(key.verify("video", 7, &signed), Some(&b"frame"[..]));
        // 中身・track・timestamp を変えた frame と、別の session の鍵では確かめられない。
        let mut tampered = signed.clone();
        *tampered.last_mut().expect("payload") ^= 1;
        assert_eq!(key.verify("video", 7, &tampered), None);
        assert_eq!(key.verify("audio", 7, &signed), None);
        assert_eq!(key.verify("video", 8, &signed), None);
        assert_eq!(key.verify("video", 7, b"short"), None);
        let other = keys.derive_live_media_signer("live-2").expect("signer");
        let other_key = LiveMediaVerifier::from_hex(&other.public_key_hex()).expect("key");
        assert_eq!(other_key.verify("video", 7, &signed), None);
    }
}
