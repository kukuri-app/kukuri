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

/// カスタムリアクションの ID（#1232 D1）。画像の hash と検索名（前後の空白を除く）だけから決まり、作成者・作成時刻・
/// 署名を含まない。同じ画像＋検索名は、誰が作っても・取り込んでも同じ ID になり、同じリアクションとして数えられる。
/// 署名つき envelope の ID を使う旧い asset ID のリアクションは、そのままの key で別に数える（D2）。
pub fn custom_reaction_id(blob_hash: &str, search_key: &str) -> String {
    hex::encode(sha256_digest(
        format!(
            "kukuri:custom-reaction:v1:{}:{}",
            blob_hash.trim(),
            search_key.trim()
        )
        .as_bytes(),
    ))
}

/// セットに入れられる件数の上限（#1232 D3）。
const CUSTOM_REACTION_SET_MAX_ITEMS: usize = 100;
/// セットの名前の文字数の上限。
const CUSTOM_REACTION_SET_MAX_NAME_CHARS: usize = 64;
/// セットの blob の大きさの上限（取得もこの大きさで打ち切る）。
pub const CUSTOM_REACTION_SET_MAX_BYTES: u64 = 64 * 1024;
pub const CUSTOM_REACTION_SET_MIME: &str = "application/json";
/// 投稿・DM の本文でセットを指す文字列の頭（`kukuri:reaction-set:<セットの blob hash>`）。
const CUSTOM_REACTION_SET_LINK_PREFIX: &str = "kukuri:reaction-set:";

/// セットの 1 件（#1232 D3）。リアクションの識別は `custom_reaction_id(blob_hash, search_key)`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomReactionSetItemV1 {
    /// 最初に作った人（識別には使わない）。
    pub owner_pubkey: Pubkey,
    pub blob_hash: BlobHash,
    pub search_key: String,
    pub mime: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

/// カスタムリアクションのセット（#1232 D3）。署名のない公開 blob（JSON）で、その blob の hash がセットの識別になる。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomReactionSetV1 {
    pub name: String,
    pub items: Vec<CustomReactionSetItemV1>,
}

impl CustomReactionSetV1 {
    /// 名前（前後の空白を除いて 1〜64 文字）、1〜100 件、同じリアクションの重複が無いこと、各件の欄を確かめる。
    pub fn validate(&self) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > CUSTOM_REACTION_SET_MAX_NAME_CHARS {
            bail!("custom reaction set name must have 1 to 64 characters");
        }
        if self.items.is_empty() || self.items.len() > CUSTOM_REACTION_SET_MAX_ITEMS {
            bail!("custom reaction set must have 1 to 100 reactions");
        }
        let mut seen = std::collections::BTreeSet::new();
        for item in &self.items {
            validate_pubkey(item.owner_pubkey.as_str())
                .context("invalid custom reaction set owner pubkey")?;
            let hash = item.blob_hash.as_str();
            if !is_blob_hash(hash) {
                bail!("custom reaction set blob hash must be 64 lowercase hex characters");
            }
            if item.search_key.trim().is_empty()
                || !item.mime.starts_with("image/")
                || item.bytes == 0
                || item.width == 0
                || item.height == 0
            {
                bail!("custom reaction set item is incomplete");
            }
            if !seen.insert(custom_reaction_id(hash, &item.search_key)) {
                bail!("custom reaction set must not repeat a reaction");
            }
        }
        Ok(())
    }

    /// 確かめてから blob の bytes にする。
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 > CUSTOM_REACTION_SET_MAX_BYTES {
            bail!("custom reaction set is too large");
        }
        Ok(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > CUSTOM_REACTION_SET_MAX_BYTES {
            bail!("custom reaction set is too large");
        }
        let set: Self =
            serde_json::from_slice(bytes).context("failed to parse custom reaction set")?;
        set.validate()?;
        Ok(set)
    }
}

/// 本文の中の `kukuri:reaction-set:<64 桁の 16 進>` が指すセットの hash（出てきた順、重複なし）。
pub fn custom_reaction_set_hashes_in_text(text: &str) -> Vec<String> {
    let mut hashes = Vec::new();
    for (index, _) in text.match_indices(CUSTOM_REACTION_SET_LINK_PREFIX) {
        let rest = &text[index + CUSTOM_REACTION_SET_LINK_PREFIX.len()..];
        let Some(hash) = rest.get(..64) else {
            continue;
        };
        let ends = !rest[64..]
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_alphanumeric());
        if is_blob_hash(hash) && ends && !hashes.iter().any(|known| known == hash) {
            hashes.push(hash.to_string());
        }
    }
    hashes
}

/// blob の hash の表記（BLAKE3 の 64 桁の小文字の 16 進）。
fn is_blob_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
