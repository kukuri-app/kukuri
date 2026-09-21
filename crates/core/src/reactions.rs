use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::crypto::{now_timestamp_millis, sha256_digest, validate_pubkey};
use crate::{
    BlobHash, ChannelId, EnvelopeId, KukuriEnvelope, KukuriKeys, ObjectStatus, Pubkey, ReplicaId,
    TopicId,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactionKeyKind {
    Emoji,
    CustomAsset,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomReactionAssetSnapshotV1 {
    pub asset_id: String,
    pub owner_pubkey: Pubkey,
    pub blob_hash: BlobHash,
    #[serde(default)]
    pub search_key: String,
    pub mime: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReactionKeyV1 {
    Emoji {
        emoji: String,
    },
    CustomAsset {
        asset_id: String,
        snapshot: CustomReactionAssetSnapshotV1,
    },
}

impl ReactionKeyV1 {
    pub fn normalized_key(&self) -> Result<String> {
        match self {
            Self::Emoji { emoji } => {
                let emoji = normalize_reaction_emoji(emoji)
                    .ok_or_else(|| anyhow!("reaction emoji must not be empty"))?;
                Ok(format!("emoji:{emoji}"))
            }
            Self::CustomAsset { asset_id, snapshot } => {
                let asset_id = asset_id.trim();
                if asset_id.is_empty() {
                    bail!("custom reaction asset id must not be empty");
                }
                if snapshot.asset_id.trim() != asset_id {
                    bail!("custom reaction asset snapshot id must match reaction key");
                }
                Ok(format!("custom_asset:{asset_id}"))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriReactionEnvelopeContentV1 {
    pub reaction_id: EnvelopeId,
    pub target_topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub target_object_id: EnvelopeId,
    pub reaction_key_kind: ReactionKeyKind,
    pub normalized_reaction_key: String,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub custom_asset_id: Option<String>,
    #[serde(default)]
    pub custom_asset_snapshot: Option<CustomReactionAssetSnapshotV1>,
    #[serde(default)]
    pub status: ObjectStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionDocV1 {
    pub reaction_id: EnvelopeId,
    pub target_topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub target_object_id: EnvelopeId,
    pub author_pubkey: Pubkey,
    pub created_at: i64,
    pub updated_at: i64,
    pub reaction_key_kind: ReactionKeyKind,
    pub normalized_reaction_key: String,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub custom_asset_id: Option<String>,
    #[serde(default)]
    pub custom_asset_snapshot: Option<CustomReactionAssetSnapshotV1>,
    pub status: ObjectStatus,
    pub envelope_id: EnvelopeId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriCustomReactionAssetEnvelopeContentV1 {
    pub author_pubkey: Pubkey,
    pub blob_hash: BlobHash,
    #[serde(default)]
    pub search_key: String,
    pub mime: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomReactionAssetDocV1 {
    pub asset_id: String,
    pub author_pubkey: Pubkey,
    pub blob_hash: BlobHash,
    #[serde(default)]
    pub search_key: String,
    pub mime: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub envelope_id: EnvelopeId,
}

impl KukuriEnvelope {
    pub(crate) fn reaction_content(&self) -> Result<Option<KukuriReactionEnvelopeContentV1>> {
        if self.kind != "reaction" {
            return Ok(None);
        }
        serde_json::from_str(self.content.as_str())
            .map(Some)
            .context("failed to parse reaction envelope content")
    }

    pub(crate) fn to_reaction_doc(&self) -> Result<Option<ReactionDocV1>> {
        let Some(content) = self.reaction_content()? else {
            return Ok(None);
        };
        Ok(Some(ReactionDocV1 {
            reaction_id: content.reaction_id,
            target_topic_id: content.target_topic_id,
            channel_id: content.channel_id,
            target_object_id: content.target_object_id,
            author_pubkey: self.pubkey.clone(),
            created_at: self.created_at,
            updated_at: self.created_at,
            reaction_key_kind: content.reaction_key_kind,
            normalized_reaction_key: content.normalized_reaction_key,
            emoji: content.emoji,
            custom_asset_id: content.custom_asset_id,
            custom_asset_snapshot: content.custom_asset_snapshot,
            status: content.status,
            envelope_id: self.id.clone(),
        }))
    }

    pub(crate) fn custom_reaction_asset_content(
        &self,
    ) -> Result<Option<KukuriCustomReactionAssetEnvelopeContentV1>> {
        if self.kind != "custom-reaction-asset" {
            return Ok(None);
        }
        serde_json::from_str(self.content.as_str())
            .map(Some)
            .context("failed to parse custom reaction asset envelope content")
    }

    pub(crate) fn to_custom_reaction_asset_doc(&self) -> Result<Option<CustomReactionAssetDocV1>> {
        let Some(content) = self.custom_reaction_asset_content()? else {
            return Ok(None);
        };
        Ok(Some(CustomReactionAssetDocV1 {
            asset_id: self.id.as_str().to_string(),
            author_pubkey: self.pubkey.clone(),
            blob_hash: content.blob_hash,
            search_key: content.search_key,
            mime: content.mime,
            bytes: content.bytes,
            width: content.width,
            height: content.height,
            created_at: self.created_at,
            updated_at: self.created_at,
            envelope_id: self.id.clone(),
        }))
    }
}

pub(crate) fn normalize_reaction_emoji(value: &str) -> Option<String> {
    let normalized = value.trim();
    (!normalized.is_empty()).then(|| normalized.to_string())
}

pub fn deterministic_reaction_id(
    source_replica_id: &ReplicaId,
    target_object_id: &EnvelopeId,
    author_pubkey: &Pubkey,
    normalized_reaction_key: &str,
) -> EnvelopeId {
    EnvelopeId(hex::encode(sha256_digest(
        format!(
            "kukuri:reaction:{}:{}:{}:{}",
            source_replica_id.as_str(),
            target_object_id.as_str(),
            author_pubkey.as_str(),
            normalized_reaction_key.trim()
        )
        .as_bytes(),
    )))
}

pub fn build_reaction_envelope(
    keys: &KukuriKeys,
    target_topic_id: &TopicId,
    channel_id: Option<&ChannelId>,
    target_object_id: &EnvelopeId,
    reaction_key: ReactionKeyV1,
    reaction_id: &EnvelopeId,
    status: ObjectStatus,
) -> Result<KukuriEnvelope> {
    if !matches!(status, ObjectStatus::Active | ObjectStatus::Deleted) {
        bail!("reaction status must be active or deleted");
    }
    let author_pubkey = keys.public_key();
    let normalized_reaction_key = reaction_key.normalized_key()?;
    let (reaction_key_kind, emoji, custom_asset_id, custom_asset_snapshot) = match reaction_key {
        ReactionKeyV1::Emoji { emoji } => (
            ReactionKeyKind::Emoji,
            Some(
                normalize_reaction_emoji(&emoji)
                    .ok_or_else(|| anyhow!("reaction emoji must not be empty"))?,
            ),
            None,
            None,
        ),
        ReactionKeyV1::CustomAsset { asset_id, snapshot } => {
            if snapshot.owner_pubkey.as_str().trim().is_empty() {
                bail!("custom reaction snapshot owner pubkey must not be empty");
            }
            if snapshot.mime.trim().is_empty() {
                bail!("custom reaction snapshot mime must not be empty");
            }
            (
                ReactionKeyKind::CustomAsset,
                None,
                Some(asset_id),
                Some(snapshot),
            )
        }
    };
    let created_at = now_timestamp_millis()?;
    crate::sign_envelope_at(
        keys,
        "reaction",
        vec![
            vec!["topic".into(), target_topic_id.as_str().into()],
            vec!["object".into(), "reaction".into()],
            vec![
                "target_object".into(),
                target_object_id.as_str().to_string(),
            ],
            vec!["reaction_id".into(), reaction_id.as_str().to_string()],
            vec!["reaction_key".into(), normalized_reaction_key.clone()],
            vec!["author".into(), author_pubkey.as_str().to_string()],
        ]
        .into_iter()
        .chain(
            channel_id
                .into_iter()
                .map(|channel_id| vec!["channel".into(), channel_id.as_str().to_string()]),
        )
        .collect(),
        serde_json::to_string(&KukuriReactionEnvelopeContentV1 {
            reaction_id: reaction_id.clone(),
            target_topic_id: target_topic_id.clone(),
            channel_id: channel_id.cloned(),
            target_object_id: target_object_id.clone(),
            reaction_key_kind,
            normalized_reaction_key,
            emoji,
            custom_asset_id,
            custom_asset_snapshot,
            status,
        })?,
        created_at,
    )
}

pub fn build_custom_reaction_asset_envelope(
    keys: &KukuriKeys,
    blob_hash: BlobHash,
    search_key: String,
    mime: String,
    bytes: u64,
    width: u32,
    height: u32,
) -> Result<KukuriEnvelope> {
    build_custom_reaction_asset_envelope_with_docs_author(
        keys, blob_hash, search_key, mime, bytes, width, height, None,
    )
}

/// custom reaction の asset の envelope に、著者の docs author の id の tag を入れる(ADR 0053 §2、#1239)。
#[allow(clippy::too_many_arguments)]
pub fn build_custom_reaction_asset_envelope_with_docs_author(
    keys: &KukuriKeys,
    blob_hash: BlobHash,
    search_key: String,
    mime: String,
    bytes: u64,
    width: u32,
    height: u32,
    docs_author: Option<&str>,
) -> Result<KukuriEnvelope> {
    let author_pubkey = keys.public_key();
    if mime.trim().is_empty() {
        bail!("custom reaction asset mime must not be empty");
    }
    let search_key = search_key.trim();
    if search_key.is_empty() {
        bail!("custom reaction asset search key must not be empty");
    }
    if width == 0 || height == 0 {
        bail!("custom reaction asset dimensions must be non-zero");
    }
    let created_at = now_timestamp_millis()?;
    let mut tags = vec![
        vec!["author".into(), author_pubkey.as_str().to_string()],
        vec!["object".into(), "custom-reaction-asset".into()],
        vec!["blob_hash".into(), blob_hash.as_str().to_string()],
    ];
    crate::posts::push_docs_author_tag(&mut tags, docs_author)?;
    crate::sign_envelope_at(
        keys,
        "custom-reaction-asset",
        tags,
        serde_json::to_string(&KukuriCustomReactionAssetEnvelopeContentV1 {
            author_pubkey,
            blob_hash,
            search_key: search_key.to_string(),
            mime,
            bytes,
            width,
            height,
        })?,
        created_at,
    )
}

pub fn parse_reaction(envelope: &KukuriEnvelope) -> Result<Option<ReactionDocV1>> {
    if envelope.kind != "reaction" {
        return Ok(None);
    }
    let reaction = envelope
        .to_reaction_doc()?
        .ok_or_else(|| anyhow!("failed to parse reaction doc"))?;
    validate_pubkey(reaction.author_pubkey.as_str()).context("invalid reaction author pubkey")?;
    match reaction.reaction_key_kind {
        ReactionKeyKind::Emoji => {
            let emoji = reaction
                .emoji
                .as_deref()
                .and_then(normalize_reaction_emoji)
                .ok_or_else(|| anyhow!("reaction emoji must not be empty"))?;
            if reaction.normalized_reaction_key != format!("emoji:{emoji}") {
                bail!("reaction normalized key does not match emoji value");
            }
        }
        ReactionKeyKind::CustomAsset => {
            let asset_id = reaction
                .custom_asset_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("reaction custom asset id must not be empty"))?;
            let snapshot = reaction
                .custom_asset_snapshot
                .as_ref()
                .ok_or_else(|| anyhow!("reaction custom asset snapshot is missing"))?;
            if snapshot.asset_id != asset_id {
                bail!("reaction custom asset snapshot id must match custom asset id");
            }
            if reaction.normalized_reaction_key != format!("custom_asset:{asset_id}") {
                bail!("reaction normalized key does not match custom asset value");
            }
        }
    }
    if !matches!(
        reaction.status,
        ObjectStatus::Active | ObjectStatus::Deleted
    ) {
        bail!("reaction status must be active or deleted");
    }
    Ok(Some(reaction))
}

pub fn parse_custom_reaction_asset(
    envelope: &KukuriEnvelope,
) -> Result<Option<CustomReactionAssetDocV1>> {
    if envelope.kind != "custom-reaction-asset" {
        return Ok(None);
    }
    let asset = envelope
        .to_custom_reaction_asset_doc()?
        .ok_or_else(|| anyhow!("failed to parse custom reaction asset doc"))?;
    validate_pubkey(asset.author_pubkey.as_str())
        .context("invalid custom reaction asset author pubkey")?;
    if asset.author_pubkey != envelope.pubkey {
        bail!("custom reaction asset author pubkey must match envelope signer");
    }
    if asset.mime.trim().is_empty() {
        bail!("custom reaction asset mime must not be empty");
    }
    if asset.width == 0 || asset.height == 0 {
        bail!("custom reaction asset dimensions must be non-zero");
    }
    Ok(Some(asset))
}
