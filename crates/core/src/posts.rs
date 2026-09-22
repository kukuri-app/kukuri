use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    AssetRef, BlobHash, ChannelId, EnvelopeId, KukuriEnvelope, KukuriKeys, Pubkey, TopicId,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadRef {
    InlineText {
        text: String,
    },
    BlobText {
        hash: BlobHash,
        mime: String,
        bytes: u64,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectVisibility {
    #[default]
    Public,
    Community,
    Room,
    Private,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectStatus {
    #[default]
    Active,
    Edited,
    Deleted,
    Tombstoned,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChannelRef {
    #[default]
    Public,
    PrivateChannel {
        channel_id: ChannelId,
    },
}

impl ChannelRef {
    pub fn channel_id(&self) -> Option<&ChannelId> {
        match self {
            Self::Public => None,
            Self::PrivateChannel { channel_id } => Some(channel_id),
        }
    }

    pub fn visibility(&self) -> ObjectVisibility {
        match self {
            Self::Public => ObjectVisibility::Public,
            Self::PrivateChannel { .. } => ObjectVisibility::Private,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
/// タイムライン・live・game の一覧の scope。1 つの channel だけを指す(#1280。複数の channel をまたぐ
/// `all_joined` は、許可されない channel の行を件数に比例して読み飛ばすので、API から閉じた)。
pub enum TimelineScope {
    #[default]
    Public,
    Channel {
        channel_id: ChannelId,
    },
}

/// 投稿者自己申告の成人向けラベル(#858、ADR 0046)。署名対象 content に含まれるため
/// 投稿者の署名で保護されるが、申告の真正性は検証できない。ラベルなしは安全を保証しない。
pub const ADULT_CONTENT_LABEL: &str = "adult";

pub fn has_adult_content_label(labels: &[String]) -> bool {
    labels.iter().any(|label| label == ADULT_CONTENT_LABEL)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriPostEnvelopeContentV1 {
    pub object_kind: String,
    pub topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub payload_ref: PayloadRef,
    #[serde(default)]
    pub attachments: Vec<AssetRef>,
    #[serde(default)]
    pub media_manifest_refs: Vec<String>,
    #[serde(default)]
    pub visibility: ObjectVisibility,
    pub reply_to: Option<EnvelopeId>,
    pub root_id: Option<EnvelopeId>,
    #[serde(default)]
    pub repost_of: Option<RepostSourceSnapshotV1>,
    /// 投稿者自己申告のラベル(既知値は `adult` のみ)。旧 envelope には無いため default。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_labels: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalReasonVisibility {
    Public,
    Private,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostWithdrawalReason {
    AuthorRequest,
    Correction,
    Privacy,
    Other,
}

/// 投稿本文とは別に同期する著者署名付き撤回事象の署名対象。
///
/// 撤回時刻は署名 envelope の `created_at` を正とし、非公開理由の説明本文は
/// public/private のいずれの replica にも載せない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriPostWithdrawalEnvelopeContentV1 {
    pub target_object_id: EnvelopeId,
    pub target_author: Pubkey,
    pub topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub generation: u64,
    #[serde(default)]
    pub replacement_object_id: Option<EnvelopeId>,
    pub reason_visibility: WithdrawalReasonVisibility,
    #[serde(default)]
    pub reason: Option<PostWithdrawalReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostWithdrawalV1 {
    pub withdrawal_envelope_id: EnvelopeId,
    pub target_object_id: EnvelopeId,
    pub target_author: Pubkey,
    pub topic_id: TopicId,
    pub channel_id: Option<ChannelId>,
    pub withdrawn_at: i64,
    pub generation: u64,
    pub replacement_object_id: Option<EnvelopeId>,
    pub reason_visibility: WithdrawalReasonVisibility,
    pub reason: Option<PostWithdrawalReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriPostObjectV1 {
    pub object_id: EnvelopeId,
    pub envelope_id: EnvelopeId,
    pub object_kind: String,
    pub topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub author: Pubkey,
    pub created_at: i64,
    pub updated_at: i64,
    pub payload_ref: PayloadRef,
    pub attachments: Vec<AssetRef>,
    pub media_manifest_refs: Vec<String>,
    pub visibility: ObjectVisibility,
    pub reply_to: Option<EnvelopeId>,
    pub root: Option<EnvelopeId>,
    #[serde(default)]
    pub repost_of: Option<RepostSourceSnapshotV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_labels: Vec<String>,
    pub status: ObjectStatus,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepostSourceSnapshotV1 {
    pub source_object_id: EnvelopeId,
    pub source_topic_id: TopicId,
    pub source_author_pubkey: Pubkey,
    pub source_object_kind: String,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<AssetRef>,
    #[serde(default)]
    pub reply_to_object_id: Option<EnvelopeId>,
    #[serde(default)]
    pub root_id: Option<EnvelopeId>,
    /// 引用/埋め込み表示でも元投稿のラベルを維持するため snapshot に含める(#858)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_labels: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRef {
    pub root: EnvelopeId,
    pub reply_to: Option<EnvelopeId>,
}

pub type CanonicalPostHeader = KukuriPostObjectV1;

/// 投稿の envelope が、著者の docs author の id を申告する tag の名前(ADR 0053 §2)。
pub const DOCS_AUTHOR_TAG: &str = "docs_author";

/// docs author の id の形(ed25519 の公開鍵 32 byte の、小文字の hex 64 桁)か。
pub fn is_docs_author_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn push_docs_author_tag(
    tags: &mut Vec<Vec<String>>,
    docs_author: Option<&str>,
) -> Result<()> {
    if let Some(docs_author) = docs_author {
        if !is_docs_author_id(docs_author) {
            bail!("docs author id must be 64 lowercase hex characters");
        }
        tags.push(vec![DOCS_AUTHOR_TAG.into(), docs_author.to_string()]);
    }
    Ok(())
}

impl KukuriEnvelope {
    /// 著者が申告した docs author の id(ADR 0053 §2)。tag は署名の対象なので、`verify()` に通った envelope では
    /// 「この `pubkey` の著者は、この docs author で書く」という著者自身の申告になる。tag が無い(旧 record)か、
    /// 値が docs author の id の形でなければ `None`。
    pub fn docs_author(&self) -> Option<&str> {
        self.tags
            .iter()
            .find(|tag| tag.first().map(String::as_str) == Some(DOCS_AUTHOR_TAG))
            .and_then(|tag| tag.get(1))
            .map(String::as_str)
            .filter(|value| is_docs_author_id(value))
    }

    pub fn topic_id(&self) -> Option<TopicId> {
        self.tags
            .iter()
            .find_map(|tag| match tag.first().map(String::as_str) {
                Some("topic" | "context") if tag.len() >= 2 => Some(TopicId::new(tag[1].clone())),
                _ => None,
            })
            .or_else(|| {
                self.post_content()
                    .ok()
                    .flatten()
                    .map(|content| content.topic_id)
            })
    }

    pub fn thread_ref(&self) -> Option<ThreadRef> {
        let root = self
            .tags
            .iter()
            .find(|tag| tag.first().map(String::as_str) == Some("root"))
            .and_then(|tag| tag.get(1).cloned())
            .filter(|value| !value.trim().is_empty())
            .map(EnvelopeId::from);
        let reply = self
            .tags
            .iter()
            .find(|tag| tag.first().map(String::as_str) == Some("reply_to"))
            .and_then(|tag| tag.get(1).cloned())
            .filter(|value| !value.trim().is_empty())
            .map(EnvelopeId::from);

        root.or_else(|| {
            self.post_content()
                .ok()
                .flatten()
                .and_then(|content| content.root_id.or(content.reply_to.clone()))
        })
        .map(|root| ThreadRef {
            root,
            reply_to: reply.or_else(|| {
                self.post_content()
                    .ok()
                    .flatten()
                    .and_then(|content| content.reply_to)
            }),
        })
    }

    pub fn post_content(&self) -> Result<Option<KukuriPostEnvelopeContentV1>> {
        if !matches!(self.kind.as_str(), "post" | "comment" | "repost") {
            return Ok(None);
        }
        serde_json::from_str(self.content.as_str())
            .map(Some)
            .context("failed to parse post envelope content")
    }

    pub fn post_withdrawal_content(&self) -> Result<Option<KukuriPostWithdrawalEnvelopeContentV1>> {
        if self.kind != "post_withdrawal" {
            return Ok(None);
        }
        serde_json::from_str(self.content.as_str())
            .map(Some)
            .context("failed to parse post withdrawal envelope content")
    }

    pub fn to_post_object(&self) -> Result<Option<KukuriPostObjectV1>> {
        let Some(content) = self.post_content()? else {
            return Ok(None);
        };
        Ok(Some(KukuriPostObjectV1 {
            object_id: self.id.clone(),
            envelope_id: self.id.clone(),
            object_kind: content.object_kind,
            topic_id: content.topic_id,
            channel_id: content.channel_id,
            author: self.pubkey.clone(),
            created_at: self.created_at,
            updated_at: self.created_at,
            payload_ref: content.payload_ref,
            attachments: content.attachments,
            media_manifest_refs: content.media_manifest_refs,
            visibility: content.visibility,
            reply_to: content.reply_to,
            root: content.root_id,
            repost_of: content.repost_of,
            content_labels: content.content_labels,
            status: ObjectStatus::Active,
            signature: self.sig.clone(),
        }))
    }
}

pub fn timeline_sort_key(created_at: i64, object_id: &EnvelopeId) -> String {
    format!("{created_at:020}-{}", object_id.as_str())
}

pub fn build_post_envelope(
    keys: &KukuriKeys,
    topic: &TopicId,
    body: &str,
    reply_to: Option<&KukuriEnvelope>,
) -> Result<KukuriEnvelope> {
    build_post_envelope_with_payload(
        keys,
        topic,
        PayloadRef::InlineText {
            text: body.to_string(),
        },
        Vec::new(),
        Vec::new(),
        reply_to,
        ObjectVisibility::Public,
    )
}

pub fn build_post_envelope_with_payload(
    keys: &KukuriKeys,
    topic: &TopicId,
    payload_ref: PayloadRef,
    attachments: Vec<AssetRef>,
    media_manifest_refs: Vec<String>,
    reply_to: Option<&KukuriEnvelope>,
    visibility: ObjectVisibility,
) -> Result<KukuriEnvelope> {
    build_post_envelope_with_payload_in_channel(
        keys,
        topic,
        payload_ref,
        attachments,
        media_manifest_refs,
        reply_to,
        visibility,
        None,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_post_envelope_with_payload_in_channel(
    keys: &KukuriKeys,
    topic: &TopicId,
    payload_ref: PayloadRef,
    attachments: Vec<AssetRef>,
    media_manifest_refs: Vec<String>,
    reply_to: Option<&KukuriEnvelope>,
    visibility: ObjectVisibility,
    channel_id: Option<&ChannelId>,
    content_labels: Vec<String>,
) -> Result<KukuriEnvelope> {
    build_post_envelope_with_docs_author(
        keys,
        topic,
        payload_ref,
        attachments,
        media_manifest_refs,
        reply_to,
        visibility,
        channel_id,
        content_labels,
        None,
    )
}

/// `docs_author` は、この投稿を docs へ書く docs author の id(ADR 0053 §2)。`Some` なら署名の対象の tag に入れる。
/// docs author を持たない docs へ書くときは `None`(tag なし = 旧 record と同じ扱い)。
#[allow(clippy::too_many_arguments)]
pub fn build_post_envelope_with_docs_author(
    keys: &KukuriKeys,
    topic: &TopicId,
    payload_ref: PayloadRef,
    attachments: Vec<AssetRef>,
    media_manifest_refs: Vec<String>,
    reply_to: Option<&KukuriEnvelope>,
    visibility: ObjectVisibility,
    channel_id: Option<&ChannelId>,
    content_labels: Vec<String>,
    docs_author: Option<&str>,
) -> Result<KukuriEnvelope> {
    let thread = reply_to
        .and_then(KukuriEnvelope::thread_ref)
        .unwrap_or_else(|| {
            reply_to
                .map(|parent| ThreadRef {
                    root: parent.id.clone(),
                    reply_to: Some(parent.id.clone()),
                })
                .unwrap_or(ThreadRef {
                    root: EnvelopeId::default(),
                    reply_to: None,
                })
        });
    let kind = if reply_to.is_some() {
        "comment"
    } else {
        "post"
    };
    let root_id = reply_to.map(|_| thread.root.clone());
    let reply_id = reply_to.map(|parent| parent.id.clone());
    let content = KukuriPostEnvelopeContentV1 {
        object_kind: kind.to_string(),
        topic_id: topic.clone(),
        channel_id: channel_id.cloned(),
        payload_ref,
        attachments,
        media_manifest_refs,
        visibility,
        reply_to: reply_id.clone(),
        root_id: root_id.clone(),
        repost_of: None,
        content_labels,
    };
    let mut tags = vec![
        vec!["topic".into(), topic.as_str().into()],
        vec!["object".into(), kind.into()],
    ];
    if let Some(root_id) = root_id {
        tags.push(vec!["root".into(), root_id.0]);
    }
    if let Some(reply_id) = reply_id {
        tags.push(vec!["reply_to".into(), reply_id.0]);
    }
    if let Some(channel_id) = channel_id {
        tags.push(vec!["channel".into(), channel_id.as_str().to_string()]);
    }
    push_docs_author_tag(&mut tags, docs_author)?;
    crate::sign_envelope_json(keys, kind, tags, &content)
}

pub fn build_repost_envelope(
    keys: &KukuriKeys,
    topic: &TopicId,
    repost_of: RepostSourceSnapshotV1,
    commentary: Option<&str>,
) -> Result<KukuriEnvelope> {
    build_repost_envelope_with_docs_author(keys, topic, repost_of, commentary, None)
}

/// `docs_author` の扱いは `build_post_envelope_with_docs_author` と同じ。
pub fn build_repost_envelope_with_docs_author(
    keys: &KukuriKeys,
    topic: &TopicId,
    repost_of: RepostSourceSnapshotV1,
    commentary: Option<&str>,
    docs_author: Option<&str>,
) -> Result<KukuriEnvelope> {
    if !matches!(repost_of.source_object_kind.as_str(), "post" | "comment") {
        bail!("repost source object kind must be post or comment");
    }
    let normalized_commentary = commentary
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let content = KukuriPostEnvelopeContentV1 {
        object_kind: "repost".into(),
        topic_id: topic.clone(),
        channel_id: None,
        payload_ref: PayloadRef::InlineText {
            text: normalized_commentary.clone().unwrap_or_default(),
        },
        attachments: Vec::new(),
        media_manifest_refs: Vec::new(),
        visibility: ObjectVisibility::Public,
        reply_to: None,
        root_id: None,
        // 引用元のラベルは snapshot 側が保持する。repost 自身のラベルは
        // 元投稿から引き継ぐ(引用表示のテキスト・メディア双方を覆う)。
        content_labels: repost_of.content_labels.clone(),
        repost_of: Some(repost_of.clone()),
    };
    let mut tags = vec![
        vec!["topic".into(), topic.as_str().into()],
        vec!["object".into(), "repost".into()],
        vec![
            "source_topic".into(),
            repost_of.source_topic_id.as_str().to_string(),
        ],
        vec![
            "source_object".into(),
            repost_of.source_object_id.as_str().to_string(),
        ],
        vec![
            "source_author".into(),
            repost_of.source_author_pubkey.as_str().to_string(),
        ],
    ];
    push_docs_author_tag(&mut tags, docs_author)?;
    crate::sign_envelope_json(keys, "repost", tags, &content)
}

#[allow(clippy::too_many_arguments)]
pub fn build_post_withdrawal_envelope(
    keys: &KukuriKeys,
    target: &KukuriEnvelope,
    generation: u64,
    replacement_object_id: Option<EnvelopeId>,
    reason_visibility: WithdrawalReasonVisibility,
    reason: Option<PostWithdrawalReason>,
) -> Result<KukuriEnvelope> {
    target.verify().context("invalid target post envelope")?;
    let target_content = target
        .post_content()?
        .ok_or_else(|| anyhow::anyhow!("post withdrawal target must be a post"))?;
    if keys.public_key() != target.pubkey {
        bail!("post withdrawal signer must be the original author");
    }
    validate_post_withdrawal_fields(
        &target.id,
        generation,
        replacement_object_id.as_ref(),
        reason_visibility,
        reason,
    )?;

    let content = KukuriPostWithdrawalEnvelopeContentV1 {
        target_object_id: target.id.clone(),
        target_author: target.pubkey.clone(),
        topic_id: target_content.topic_id.clone(),
        channel_id: target_content.channel_id.clone(),
        generation,
        replacement_object_id: replacement_object_id.clone(),
        reason_visibility,
        reason,
    };
    let mut tags = vec![
        vec!["topic".into(), target_content.topic_id.as_str().to_string()],
        vec!["object".into(), "post_withdrawal".into()],
        vec!["target".into(), target.id.as_str().to_string()],
        vec!["target_author".into(), target.pubkey.as_str().to_string()],
        vec!["generation".into(), generation.to_string()],
    ];
    if let Some(channel_id) = target_content.channel_id {
        tags.push(vec!["channel".into(), channel_id.as_str().to_string()]);
    }
    if let Some(replacement) = replacement_object_id {
        tags.push(vec!["replacement".into(), replacement.as_str().to_string()]);
    }
    crate::sign_envelope_json(keys, "post_withdrawal", tags, &content)
}

pub fn verify_post_withdrawal(
    withdrawal: &KukuriEnvelope,
    target: &KukuriEnvelope,
) -> Result<PostWithdrawalV1> {
    withdrawal
        .verify()
        .context("invalid post withdrawal envelope")?;
    target.verify().context("invalid target post envelope")?;
    let content = withdrawal
        .post_withdrawal_content()?
        .ok_or_else(|| anyhow::anyhow!("expected post withdrawal envelope"))?;
    let target_content = target
        .post_content()?
        .ok_or_else(|| anyhow::anyhow!("post withdrawal target must be a post"))?;
    if withdrawal.pubkey != target.pubkey || content.target_author != target.pubkey {
        bail!("post withdrawal signer must be the original author");
    }
    if content.target_object_id != target.id {
        bail!("post withdrawal target object does not match the original envelope");
    }
    if content.topic_id != target_content.topic_id
        || content.channel_id != target_content.channel_id
    {
        bail!("post withdrawal scope does not match the original post");
    }
    validate_post_withdrawal_fields(
        &content.target_object_id,
        content.generation,
        content.replacement_object_id.as_ref(),
        content.reason_visibility,
        content.reason,
    )?;
    Ok(PostWithdrawalV1 {
        withdrawal_envelope_id: withdrawal.id.clone(),
        target_object_id: content.target_object_id,
        target_author: content.target_author,
        topic_id: content.topic_id,
        channel_id: content.channel_id,
        withdrawn_at: withdrawal.created_at,
        generation: content.generation,
        replacement_object_id: content.replacement_object_id,
        reason_visibility: content.reason_visibility,
        reason: content.reason,
    })
}

fn validate_post_withdrawal_fields(
    target_object_id: &EnvelopeId,
    generation: u64,
    replacement_object_id: Option<&EnvelopeId>,
    reason_visibility: WithdrawalReasonVisibility,
    reason: Option<PostWithdrawalReason>,
) -> Result<()> {
    if generation == 0 {
        bail!("post withdrawal generation must be greater than zero");
    }
    if replacement_object_id == Some(target_object_id) {
        bail!("post withdrawal replacement must differ from the target");
    }
    match (reason_visibility, reason) {
        (WithdrawalReasonVisibility::Public, None) => {
            bail!("public post withdrawal requires a reason code")
        }
        (WithdrawalReasonVisibility::Private, Some(_)) => {
            bail!("private withdrawal reason must not be replicated")
        }
        _ => Ok(()),
    }
}
