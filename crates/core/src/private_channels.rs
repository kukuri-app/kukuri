use anyhow::{Context, Result, anyhow, bail};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use secp256k1::rand::{RngCore, rng};
use serde::{Deserialize, Serialize};

use crate::crypto::{
    derive_hkdf_key, now_timestamp_millis, pairwise_shared_secret, validate_pubkey,
};
use crate::{ChannelId, KukuriEnvelope, KukuriKeys, Pubkey, TopicId};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChannelAudienceKind {
    #[default]
    InviteOnly,
    FriendOnly,
    FriendPlus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChannelSharingState {
    #[default]
    Open,
    Frozen,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePrivateChannelInput {
    pub topic_id: TopicId,
    pub label: String,
    #[serde(default)]
    pub audience_kind: ChannelAudienceKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateChannelJoinMode {
    OwnerSeed,
    InviteToken,
    FriendOnlyGrant,
    FriendPlusShare,
    RotationRedeem,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateChannelMetadataDocV1 {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub label: String,
    pub creator_pubkey: Pubkey,
    pub created_at: i64,
    #[serde(default)]
    pub audience_kind: ChannelAudienceKind,
    #[serde(default)]
    pub owner_pubkey: Pubkey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateChannelPolicyDocV1 {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub audience_kind: ChannelAudienceKind,
    pub owner_pubkey: Pubkey,
    pub epoch_id: String,
    pub sharing_state: ChannelSharingState,
    pub rotated_at: Option<i64>,
    #[serde(default)]
    pub previous_epoch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_dome_instance_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateChannelParticipantDocV1 {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub epoch_id: String,
    pub participant_pubkey: Pubkey,
    pub joined_at: i64,
    pub is_owner: bool,
    #[serde(default)]
    pub join_mode: Option<PrivateChannelJoinMode>,
    #[serde(default)]
    pub sponsor_pubkey: Option<Pubkey>,
    #[serde(default)]
    pub share_token_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriPrivateChannelInviteEnvelopeContentV1 {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub channel_label: String,
    pub owner_pubkey: Pubkey,
    pub epoch_id: String,
    pub namespace_secret_hex: String,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateChannelInviteTokenV1 {
    pub envelope: KukuriEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct PrivateChannelInvitePreview {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub channel_label: String,
    pub inviter_pubkey: Pubkey,
    pub owner_pubkey: Pubkey,
    pub epoch_id: String,
    pub expires_at: Option<i64>,
    pub namespace_secret_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriFriendOnlyGrantEnvelopeContentV1 {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub channel_label: String,
    pub owner_pubkey: Pubkey,
    pub epoch_id: String,
    pub namespace_secret_hex: String,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendOnlyGrantTokenV1 {
    pub envelope: KukuriEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct FriendOnlyGrantPreview {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub channel_label: String,
    pub owner_pubkey: Pubkey,
    pub epoch_id: String,
    pub expires_at: Option<i64>,
    pub namespace_secret_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriFriendPlusShareEnvelopeContentV1 {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub channel_label: String,
    pub owner_pubkey: Pubkey,
    pub sponsor_pubkey: Pubkey,
    pub epoch_id: String,
    pub namespace_secret_hex: String,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendPlusShareTokenV1 {
    pub envelope: KukuriEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct FriendPlusSharePreview {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub channel_label: String,
    pub owner_pubkey: Pubkey,
    pub sponsor_pubkey: Pubkey,
    pub epoch_id: String,
    pub expires_at: Option<i64>,
    pub namespace_secret_hex: String,
    pub share_token_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateChannelEpochHandoffGrantPayloadV1 {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub owner_pubkey: Pubkey,
    pub recipient_pubkey: Pubkey,
    pub old_epoch_id: String,
    pub new_epoch_id: String,
    pub new_namespace_secret_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateChannelEpochHandoffGrantDocV1 {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    pub owner_pubkey: Pubkey,
    pub recipient_pubkey: Pubkey,
    pub old_epoch_id: String,
    pub new_epoch_id: String,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
}

pub struct PrivateChannelInviteTokenParams<'a> {
    pub topic: &'a TopicId,
    pub channel_id: &'a ChannelId,
    pub channel_label: &'a str,
    pub owner_pubkey: &'a Pubkey,
    pub epoch_id: &'a str,
    pub namespace_secret_hex: &'a str,
    pub expires_at: Option<i64>,
}

pub fn build_private_channel_invite_token(
    keys: &KukuriKeys,
    params: PrivateChannelInviteTokenParams<'_>,
) -> Result<String> {
    let token = PrivateChannelInviteTokenV1 {
        envelope: crate::sign_envelope_json(
            keys,
            "channel-invite",
            vec![
                vec!["topic".into(), params.topic.as_str().to_string()],
                vec!["object".into(), "channel-invite".into()],
                vec!["channel".into(), params.channel_id.as_str().to_string()],
            ],
            &KukuriPrivateChannelInviteEnvelopeContentV1 {
                channel_id: params.channel_id.clone(),
                topic_id: params.topic.clone(),
                channel_label: params.channel_label.trim().to_string(),
                owner_pubkey: params.owner_pubkey.clone(),
                epoch_id: params.epoch_id.trim().to_string(),
                namespace_secret_hex: params.namespace_secret_hex.trim().to_string(),
                expires_at: params.expires_at,
            },
        )?,
    };
    serde_json::to_string(&token).context("failed to encode private channel invite token")
}

pub fn build_friend_only_grant_token(
    keys: &KukuriKeys,
    topic: &TopicId,
    channel_id: &ChannelId,
    channel_label: &str,
    epoch_id: &str,
    namespace_secret_hex: &str,
    expires_at: Option<i64>,
) -> Result<String> {
    let owner_pubkey = keys.public_key();
    let token = FriendOnlyGrantTokenV1 {
        envelope: crate::sign_envelope_json(
            keys,
            "channel-friend-grant",
            vec![
                vec!["topic".into(), topic.as_str().to_string()],
                vec!["object".into(), "channel-friend-grant".into()],
                vec!["channel".into(), channel_id.as_str().to_string()],
                vec!["epoch".into(), epoch_id.trim().to_string()],
            ],
            &KukuriFriendOnlyGrantEnvelopeContentV1 {
                channel_id: channel_id.clone(),
                topic_id: topic.clone(),
                channel_label: channel_label.trim().to_string(),
                owner_pubkey,
                epoch_id: epoch_id.trim().to_string(),
                namespace_secret_hex: namespace_secret_hex.trim().to_string(),
                expires_at,
            },
        )?,
    };
    serde_json::to_string(&token).context("failed to encode friend-only grant token")
}

#[allow(clippy::too_many_arguments)]
pub fn build_friend_plus_share_token(
    keys: &KukuriKeys,
    topic: &TopicId,
    channel_id: &ChannelId,
    channel_label: &str,
    owner_pubkey: &Pubkey,
    epoch_id: &str,
    namespace_secret_hex: &str,
    expires_at: Option<i64>,
) -> Result<String> {
    let sponsor_pubkey = keys.public_key();
    let token = FriendPlusShareTokenV1 {
        envelope: crate::sign_envelope_json(
            keys,
            "channel-share",
            vec![
                vec!["topic".into(), topic.as_str().to_string()],
                vec!["object".into(), "channel-share".into()],
                vec!["channel".into(), channel_id.as_str().to_string()],
                vec!["epoch".into(), epoch_id.trim().to_string()],
                vec!["owner".into(), owner_pubkey.as_str().to_string()],
            ],
            &KukuriFriendPlusShareEnvelopeContentV1 {
                channel_id: channel_id.clone(),
                topic_id: topic.clone(),
                channel_label: channel_label.trim().to_string(),
                owner_pubkey: owner_pubkey.clone(),
                sponsor_pubkey,
                epoch_id: epoch_id.trim().to_string(),
                namespace_secret_hex: namespace_secret_hex.trim().to_string(),
                expires_at,
            },
        )?,
    };
    serde_json::to_string(&token).context("failed to encode friend-plus share token")
}

pub fn parse_private_channel_invite_token(token: &str) -> Result<PrivateChannelInvitePreview> {
    let token: PrivateChannelInviteTokenV1 =
        serde_json::from_str(token).context("failed to parse private channel invite token")?;
    token.envelope.verify()?;
    if token.envelope.kind != "channel-invite" {
        bail!("invite envelope kind must be channel-invite");
    }
    let content: KukuriPrivateChannelInviteEnvelopeContentV1 =
        serde_json::from_str(token.envelope.content.as_str())
            .context("failed to decode private channel invite content")?;
    if content.channel_label.trim().is_empty() {
        bail!("channel invite label is required");
    }
    validate_pubkey(content.owner_pubkey.as_str()).context("invalid channel invite owner")?;
    if content.epoch_id.trim().is_empty() {
        bail!("channel invite epoch id is required");
    }
    let secret_bytes =
        hex::decode(content.namespace_secret_hex.trim()).context("invalid invite secret hex")?;
    if secret_bytes.len() != 32 {
        bail!("invite secret must be 32 bytes");
    }
    if let Some(expires_at) = content.expires_at
        && expires_at < now_timestamp_millis()?
    {
        bail!("invite has expired");
    }
    Ok(PrivateChannelInvitePreview {
        channel_id: content.channel_id,
        topic_id: content.topic_id,
        channel_label: content.channel_label,
        inviter_pubkey: token.envelope.pubkey,
        owner_pubkey: content.owner_pubkey,
        epoch_id: content.epoch_id,
        expires_at: content.expires_at,
        namespace_secret_hex: content.namespace_secret_hex,
    })
}

pub fn build_private_channel_policy_envelope(
    keys: &KukuriKeys,
    doc: &PrivateChannelPolicyDocV1,
) -> Result<KukuriEnvelope> {
    if keys.public_key() != doc.owner_pubkey {
        bail!("channel policy owner pubkey must match signer");
    }
    let created_at = now_timestamp_millis()?;
    let encoded = serde_json::to_string(doc).context("failed to encode channel policy doc")?;
    crate::sign_envelope_at(
        keys,
        "channel-policy",
        vec![
            vec!["topic".into(), doc.topic_id.as_str().to_string()],
            vec!["channel".into(), doc.channel_id.as_str().to_string()],
            vec!["epoch".into(), doc.epoch_id.clone()],
            vec!["object".into(), "channel-policy".into()],
        ],
        encoded,
        created_at,
    )
}

pub fn parse_private_channel_policy(
    envelope: &KukuriEnvelope,
) -> Result<Option<PrivateChannelPolicyDocV1>> {
    if envelope.kind != "channel-policy" {
        return Ok(None);
    }
    let doc: PrivateChannelPolicyDocV1 =
        serde_json::from_str(&envelope.content).context("failed to parse channel policy")?;
    validate_pubkey(doc.owner_pubkey.as_str()).context("invalid channel policy owner pubkey")?;
    if envelope.pubkey != doc.owner_pubkey {
        bail!("channel policy owner pubkey must match envelope signer");
    }
    if doc.epoch_id.trim().is_empty() {
        bail!("channel policy epoch id is required");
    }
    if doc
        .previous_epoch_id
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        bail!("channel policy previous epoch id must not be empty");
    }
    Ok(Some(doc))
}

pub fn build_private_channel_participant_envelope(
    keys: &KukuriKeys,
    doc: &PrivateChannelParticipantDocV1,
) -> Result<KukuriEnvelope> {
    if keys.public_key() != doc.participant_pubkey {
        bail!("channel participant pubkey must match signer");
    }
    let created_at = now_timestamp_millis()?;
    let encoded = serde_json::to_string(doc).context("failed to encode channel participant doc")?;
    crate::sign_envelope_at(
        keys,
        "channel-participant",
        vec![
            vec!["topic".into(), doc.topic_id.as_str().to_string()],
            vec!["channel".into(), doc.channel_id.as_str().to_string()],
            vec!["epoch".into(), doc.epoch_id.clone()],
            vec![
                "participant".into(),
                doc.participant_pubkey.as_str().to_string(),
            ],
            vec!["object".into(), "channel-participant".into()],
        ],
        encoded,
        created_at,
    )
}

pub fn parse_private_channel_participant(
    envelope: &KukuriEnvelope,
) -> Result<Option<PrivateChannelParticipantDocV1>> {
    if envelope.kind != "channel-participant" {
        return Ok(None);
    }
    let doc: PrivateChannelParticipantDocV1 =
        serde_json::from_str(&envelope.content).context("failed to parse channel participant")?;
    validate_pubkey(doc.participant_pubkey.as_str())
        .context("invalid channel participant pubkey")?;
    if envelope.pubkey != doc.participant_pubkey {
        bail!("channel participant pubkey must match envelope signer");
    }
    if doc.epoch_id.trim().is_empty() {
        bail!("channel participant epoch id is required");
    }
    if let Some(sponsor_pubkey) = doc.sponsor_pubkey.as_ref() {
        validate_pubkey(sponsor_pubkey.as_str())
            .context("invalid channel participant sponsor pubkey")?;
    }
    if doc
        .share_token_id
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        bail!("channel participant share token id must not be empty");
    }
    Ok(Some(doc))
}

pub fn parse_friend_only_grant_token(token: &str) -> Result<FriendOnlyGrantPreview> {
    let token: FriendOnlyGrantTokenV1 =
        serde_json::from_str(token).context("failed to parse friend-only grant token")?;
    token.envelope.verify()?;
    if token.envelope.kind != "channel-friend-grant" {
        bail!("grant envelope kind must be channel-friend-grant");
    }
    let content: KukuriFriendOnlyGrantEnvelopeContentV1 =
        serde_json::from_str(token.envelope.content.as_str())
            .context("failed to decode friend-only grant content")?;
    validate_pubkey(content.owner_pubkey.as_str()).context("invalid friend-only grant owner")?;
    if token.envelope.pubkey != content.owner_pubkey {
        bail!("friend-only grant owner pubkey must match envelope signer");
    }
    if content.channel_label.trim().is_empty() {
        bail!("friend-only grant label is required");
    }
    if content.epoch_id.trim().is_empty() {
        bail!("friend-only grant epoch id is required");
    }
    let secret_bytes =
        hex::decode(content.namespace_secret_hex.trim()).context("invalid grant secret hex")?;
    if secret_bytes.len() != 32 {
        bail!("grant secret must be 32 bytes");
    }
    if let Some(expires_at) = content.expires_at
        && expires_at < now_timestamp_millis()?
    {
        bail!("friend-only grant has expired");
    }
    Ok(FriendOnlyGrantPreview {
        channel_id: content.channel_id,
        topic_id: content.topic_id,
        channel_label: content.channel_label,
        owner_pubkey: content.owner_pubkey,
        epoch_id: content.epoch_id,
        expires_at: content.expires_at,
        namespace_secret_hex: content.namespace_secret_hex,
    })
}

pub fn parse_friend_plus_share_token(token: &str) -> Result<FriendPlusSharePreview> {
    let token: FriendPlusShareTokenV1 =
        serde_json::from_str(token).context("failed to parse friend-plus share token")?;
    token.envelope.verify()?;
    if token.envelope.kind != "channel-share" {
        bail!("share envelope kind must be channel-share");
    }
    let content: KukuriFriendPlusShareEnvelopeContentV1 =
        serde_json::from_str(token.envelope.content.as_str())
            .context("failed to decode friend-plus share content")?;
    validate_pubkey(content.owner_pubkey.as_str()).context("invalid friend-plus share owner")?;
    validate_pubkey(content.sponsor_pubkey.as_str())
        .context("invalid friend-plus share sponsor")?;
    if token.envelope.pubkey != content.sponsor_pubkey {
        bail!("friend-plus share sponsor pubkey must match envelope signer");
    }
    if content.channel_label.trim().is_empty() {
        bail!("friend-plus share label is required");
    }
    if content.epoch_id.trim().is_empty() {
        bail!("friend-plus share epoch id is required");
    }
    validate_private_channel_secret_hex(
        content.namespace_secret_hex.as_str(),
        "friend-plus share secret",
    )?;
    if let Some(expires_at) = content.expires_at
        && expires_at < now_timestamp_millis()?
    {
        bail!("friend-plus share has expired");
    }
    Ok(FriendPlusSharePreview {
        channel_id: content.channel_id,
        topic_id: content.topic_id,
        channel_label: content.channel_label,
        owner_pubkey: content.owner_pubkey,
        sponsor_pubkey: content.sponsor_pubkey,
        epoch_id: content.epoch_id,
        expires_at: content.expires_at,
        namespace_secret_hex: content.namespace_secret_hex,
        share_token_id: token.envelope.id.as_str().to_string(),
    })
}

pub fn encrypt_private_channel_epoch_handoff_grant(
    owner_keys: &KukuriKeys,
    payload: &PrivateChannelEpochHandoffGrantPayloadV1,
) -> Result<PrivateChannelEpochHandoffGrantDocV1> {
    if owner_keys.public_key() != payload.owner_pubkey {
        bail!("channel epoch handoff grant owner pubkey must match signer");
    }
    validate_pubkey(payload.recipient_pubkey.as_str())
        .context("invalid channel epoch handoff grant recipient pubkey")?;
    if payload.old_epoch_id.trim().is_empty() {
        bail!("channel epoch handoff grant old epoch id is required");
    }
    if payload.new_epoch_id.trim().is_empty() {
        bail!("channel epoch handoff grant new epoch id is required");
    }
    validate_private_channel_secret_hex(
        payload.new_namespace_secret_hex.as_str(),
        "channel epoch handoff grant secret",
    )?;
    let plaintext = serde_json::to_vec(payload)
        .context("failed to encode channel epoch handoff grant payload")?;
    let mut nonce = [0u8; 24];
    rng().fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new_from_slice(
        derive_epoch_handoff_grant_key(owner_keys, &payload.recipient_pubkey, payload)?.as_slice(),
    )
    .context("failed to initialize epoch handoff grant cipher")?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext.as_slice(),
                aad: epoch_handoff_grant_aad(payload).as_bytes(),
            },
        )
        .map_err(|_| anyhow!("failed to encrypt channel epoch handoff grant"))?;
    Ok(PrivateChannelEpochHandoffGrantDocV1 {
        channel_id: payload.channel_id.clone(),
        topic_id: payload.topic_id.clone(),
        owner_pubkey: payload.owner_pubkey.clone(),
        recipient_pubkey: payload.recipient_pubkey.clone(),
        old_epoch_id: payload.old_epoch_id.clone(),
        new_epoch_id: payload.new_epoch_id.clone(),
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(ciphertext),
    })
}

pub fn decrypt_private_channel_epoch_handoff_grant(
    local_keys: &KukuriKeys,
    doc: &PrivateChannelEpochHandoffGrantDocV1,
) -> Result<PrivateChannelEpochHandoffGrantPayloadV1> {
    if local_keys.public_key() != doc.recipient_pubkey {
        bail!("channel epoch handoff grant recipient pubkey must match decrypting author");
    }
    let nonce =
        hex::decode(doc.nonce_hex.trim()).context("invalid channel epoch handoff grant nonce")?;
    if nonce.len() != 24 {
        bail!("channel epoch handoff grant nonce must be 24 bytes");
    }
    let ciphertext = hex::decode(doc.ciphertext_hex.trim())
        .context("invalid channel epoch handoff grant ciphertext")?;
    let payload_stub = PrivateChannelEpochHandoffGrantPayloadV1 {
        channel_id: doc.channel_id.clone(),
        topic_id: doc.topic_id.clone(),
        owner_pubkey: doc.owner_pubkey.clone(),
        recipient_pubkey: doc.recipient_pubkey.clone(),
        old_epoch_id: doc.old_epoch_id.clone(),
        new_epoch_id: doc.new_epoch_id.clone(),
        new_namespace_secret_hex: String::new(),
    };
    let cipher = XChaCha20Poly1305::new_from_slice(
        derive_epoch_handoff_grant_key(local_keys, &doc.owner_pubkey, &payload_stub)?.as_slice(),
    )
    .context("failed to initialize epoch handoff grant cipher")?;
    let plaintext = cipher
        .decrypt(
            <&XNonce>::try_from(nonce.as_slice()).expect("nonce length checked"),
            Payload {
                msg: ciphertext.as_slice(),
                aad: epoch_handoff_grant_aad(&payload_stub).as_bytes(),
            },
        )
        .map_err(|_| anyhow!("failed to decrypt channel epoch handoff grant"))?;
    let payload: PrivateChannelEpochHandoffGrantPayloadV1 = serde_json::from_slice(&plaintext)
        .context("failed to decode channel epoch handoff grant payload")?;
    if payload.channel_id != doc.channel_id || payload.topic_id != doc.topic_id {
        bail!("channel epoch handoff grant payload does not match doc identity");
    }
    if payload.owner_pubkey != doc.owner_pubkey || payload.recipient_pubkey != doc.recipient_pubkey
    {
        bail!("channel epoch handoff grant payload does not match doc recipients");
    }
    if payload.old_epoch_id != doc.old_epoch_id || payload.new_epoch_id != doc.new_epoch_id {
        bail!("channel epoch handoff grant payload does not match doc epochs");
    }
    validate_private_channel_secret_hex(
        payload.new_namespace_secret_hex.as_str(),
        "channel epoch handoff grant secret",
    )?;
    Ok(payload)
}

pub fn build_private_channel_epoch_handoff_grant_envelope(
    owner_keys: &KukuriKeys,
    doc: &PrivateChannelEpochHandoffGrantDocV1,
) -> Result<KukuriEnvelope> {
    if owner_keys.public_key() != doc.owner_pubkey {
        bail!("channel epoch handoff grant owner pubkey must match signer");
    }
    let created_at = now_timestamp_millis()?;
    let encoded =
        serde_json::to_string(doc).context("failed to encode channel epoch handoff grant doc")?;
    crate::sign_envelope_at(
        owner_keys,
        "channel-rotation-grant",
        vec![
            vec!["topic".into(), doc.topic_id.as_str().to_string()],
            vec!["channel".into(), doc.channel_id.as_str().to_string()],
            vec!["epoch".into(), doc.old_epoch_id.clone()],
            vec![
                "recipient".into(),
                doc.recipient_pubkey.as_str().to_string(),
            ],
            vec!["object".into(), "channel-rotation-grant".into()],
        ],
        encoded,
        created_at,
    )
}

pub fn parse_private_channel_epoch_handoff_grant(
    envelope: &KukuriEnvelope,
) -> Result<Option<PrivateChannelEpochHandoffGrantDocV1>> {
    if envelope.kind != "channel-rotation-grant" {
        return Ok(None);
    }
    let doc: PrivateChannelEpochHandoffGrantDocV1 = serde_json::from_str(&envelope.content)
        .context("failed to parse channel epoch handoff grant")?;
    validate_pubkey(doc.owner_pubkey.as_str())
        .context("invalid channel epoch handoff grant owner")?;
    validate_pubkey(doc.recipient_pubkey.as_str())
        .context("invalid channel epoch handoff grant recipient")?;
    if envelope.pubkey != doc.owner_pubkey {
        bail!("channel epoch handoff grant owner pubkey must match envelope signer");
    }
    if doc.old_epoch_id.trim().is_empty() {
        bail!("channel epoch handoff grant old epoch id is required");
    }
    if doc.new_epoch_id.trim().is_empty() {
        bail!("channel epoch handoff grant new epoch id is required");
    }
    if doc.old_epoch_id == doc.new_epoch_id {
        bail!("channel epoch handoff grant must rotate to a new epoch");
    }
    let nonce =
        hex::decode(doc.nonce_hex.trim()).context("invalid channel epoch handoff grant nonce")?;
    if nonce.len() != 24 {
        bail!("channel epoch handoff grant nonce must be 24 bytes");
    }
    let _ = hex::decode(doc.ciphertext_hex.trim())
        .context("invalid channel epoch handoff grant ciphertext")?;
    Ok(Some(doc))
}

fn validate_private_channel_secret_hex(value: &str, label: &str) -> Result<()> {
    let secret_bytes = hex::decode(value.trim()).with_context(|| format!("invalid {label} hex"))?;
    if secret_bytes.len() != 32 {
        bail!("{label} must be 32 bytes");
    }
    Ok(())
}

fn epoch_handoff_grant_aad(payload: &PrivateChannelEpochHandoffGrantPayloadV1) -> String {
    format!(
        "kukuri:rotation-grant:{}:{}:{}:{}:{}",
        payload.channel_id.as_str(),
        payload.topic_id.as_str(),
        payload.owner_pubkey.as_str(),
        payload.recipient_pubkey.as_str(),
        payload.new_epoch_id
    )
}

fn derive_epoch_handoff_grant_key(
    local_keys: &KukuriKeys,
    remote_pubkey: &Pubkey,
    payload: &PrivateChannelEpochHandoffGrantPayloadV1,
) -> Result<[u8; 32]> {
    let shared = pairwise_shared_secret(local_keys, remote_pubkey)?;
    derive_hkdf_key(
        b"kukuri/private-channel/rotation-grant",
        shared.to_secret_bytes().as_slice(),
        epoch_handoff_grant_aad(payload).as_bytes(),
        "channel epoch handoff grant key",
    )
}

/// 世代の鍵の行の封の導出の context（ADR 0061 §9）。変えると保存した行を開けなくなるので、変更してはならない。
const KEY_ROW_CONTEXT: &str = "kukuri.app 2026-10-02 private channel key rows v1";
const KEY_ROW_AAD_DOMAIN: &[u8] = b"kukuri private channel key row v1\0";

/// private channel の世代の秘密を、端末の行へ置くための封（ADR 0061 §9）。アカウント鍵から導出するので、
/// 保護の強さはアカウント鍵の置き場と同じ。秘密を含むので `Debug` へ中身を出さない。
#[derive(Clone)]
pub struct PrivateChannelKeyRowSeal {
    account: Pubkey,
    key: [u8; 32],
}

impl std::fmt::Debug for PrivateChannelKeyRowSeal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateChannelKeyRowSeal")
            .finish_non_exhaustive()
    }
}

impl KukuriKeys {
    pub fn derive_private_channel_key_row_seal(&self) -> PrivateChannelKeyRowSeal {
        PrivateChannelKeyRowSeal {
            account: Pubkey::from(self.public_key_hex()),
            key: blake3::derive_key(KEY_ROW_CONTEXT, &self.secret_bytes()),
        }
    }
}

impl PrivateChannelKeyRowSeal {
    /// 世代の秘密（32 byte の hex）を封じる。nonce（24 byte）と暗号文を続けたもの。
    pub fn seal(&self, channel_id: &str, epoch_id: &str, secret_hex: &str) -> Result<Vec<u8>> {
        validate_private_channel_secret_hex(secret_hex, "private channel epoch secret")?;
        let secret = hex::decode(secret_hex.trim())?;
        let mut nonce = [0u8; 24];
        rng().fill_bytes(&mut nonce);
        let aad = self.aad(channel_id, epoch_id);
        let ciphertext = self
            .cipher()?
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: secret.as_slice(),
                    aad: aad.as_slice(),
                },
            )
            .map_err(|_| anyhow!("failed to seal private channel key row"))?;
        Ok([nonce.as_slice(), ciphertext.as_slice()].concat())
    }

    /// 封を開いて、世代の秘密の hex を返す。別の行・別の account へ移した封と改ざんは失敗にする。
    pub fn open(&self, channel_id: &str, epoch_id: &str, sealed: &[u8]) -> Result<String> {
        let (nonce, ciphertext) = sealed
            .split_at_checked(24)
            .ok_or_else(|| anyhow!("private channel key row is too short"))?;
        let aad = self.aad(channel_id, epoch_id);
        let secret = self
            .cipher()?
            .decrypt(
                &XNonce::from(<[u8; 24]>::try_from(nonce)?),
                Payload {
                    msg: ciphertext,
                    aad: aad.as_slice(),
                },
            )
            .map_err(|_| anyhow!("private channel key row failed authentication"))?;
        let secret = hex::encode(secret);
        validate_private_channel_secret_hex(&secret, "private channel epoch secret")?;
        Ok(secret)
    }

    fn aad(&self, channel_id: &str, epoch_id: &str) -> Vec<u8> {
        let mut aad = KEY_ROW_AAD_DOMAIN.to_vec();
        for part in [self.account.as_str(), channel_id, epoch_id] {
            aad.extend_from_slice(part.as_bytes());
            aad.push(0);
        }
        aad
    }

    fn cipher(&self) -> Result<XChaCha20Poly1305> {
        XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|_| anyhow!("failed to initialize private channel key row cipher"))
    }
}
