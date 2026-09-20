//! docs から反映する投稿の検証(#1248)。
//!
//! public topic の replica は topic id を知る誰もが書ける。`objects/<object id>/state` の header は署名を
//! 持たないので、反映する値は同じ object の署名つき envelope(`objects/<object id>/envelope`)だけから作り、
//! 読んだ replica が受け入れる topic / channel と照らす。検証に通った投稿だけが `VerifiedPost` になり、
//! projection の行・通知・repost の snapshot は `VerifiedPost` からしか作れない。

use super::*;
use kukuri_docs_sync::{PostReplicaKind, post_replica_kind};

/// 1 つの object の `envelope` の key について調べる record 数の上限。
///
/// 同じ key には docs author ごとの entry がありうる。先頭の 1 件だけを見ると、不正な entry を 1 件置くだけで
/// 正しい投稿を隠せる。上限を超える数の不正な entry を積まれた投稿は反映できない(best effort の範囲)。
pub(crate) const MAX_ENVELOPE_RECORDS_PER_OBJECT: usize = 8;

/// 1 つの object の `withdrawals/<object id>/state` の key で調べる record 数の上限(#1250)。
///
/// 同じ key には docs author ごとの record がありうる。検証に通らない record が先に並んでいても、この件数までは
/// 後ろの record を調べる。上限を超える数の不正な record を積まれた取り下げは反映できない(best effort)。
pub(crate) const MAX_WITHDRAWAL_RECORDS_PER_OBJECT: usize = 8;

/// 投稿を読んだ replica が受け入れる topic と channel。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReplicaPostScope {
    topic_id: String,
    channel_id: Option<String>,
}

impl ReplicaPostScope {
    /// `subscription_topic_id` は、その replica を読む文脈の topic。private channel の replica id は topic を
    /// 含まないので、参加状態が持つ topic を呼び出し側が渡す。投稿を置く replica でなければ `None`。
    pub(crate) fn for_replica(replica: &ReplicaId, subscription_topic_id: &str) -> Option<Self> {
        match post_replica_kind(replica)? {
            PostReplicaKind::PublicTopic { topic_id } => (topic_id == subscription_topic_id)
                .then_some(Self {
                    topic_id,
                    channel_id: None,
                }),
            PostReplicaKind::PrivateChannel { channel_id } => Some(Self {
                topic_id: subscription_topic_id.to_string(),
                channel_id: Some(channel_id),
            }),
        }
    }
}

/// 検証に通らなかった理由。I/O の失敗は含まない(呼び出し側へ `Err` で返す)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PostRejection {
    /// 投稿を置く replica ではない、または replica と購読の topic が合わない。
    UnsupportedReplica,
    /// record が envelope として読めない。
    NotAnEnvelope,
    /// id または署名が envelope の内容と合わない。
    InvalidSignature,
    /// envelope の id が、key の object id と違う。
    ObjectIdMismatch,
    /// 投稿(post・comment・repost)の envelope ではない、または content が読めない。
    NotAPost,
    /// envelope が申告する topic・channel・公開範囲が、読んだ replica と合わない。
    ScopeMismatch,
}

impl PostRejection {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedReplica => "the replica does not hold posts for this topic",
            Self::NotAnEnvelope => "the record is not an envelope",
            Self::InvalidSignature => "the envelope id or signature is invalid",
            Self::ObjectIdMismatch => "the envelope id does not match the object id",
            Self::NotAPost => "the envelope is not a post",
            Self::ScopeMismatch => "the post does not belong to the replica it was read from",
        }
    }
}

/// 署名つき envelope と、読んだ replica の scope に照らして確かめた投稿。
///
/// 値はすべて envelope(`to_post_object()`)から作る。docs の `state` の record の値は使わない。
#[derive(Clone, Debug)]
pub(crate) struct VerifiedPost {
    header: CanonicalPostHeader,
    replica: ReplicaId,
}

impl VerifiedPost {
    /// envelope を検証する。docs は読まない。
    pub(crate) fn verify(
        envelope: KukuriEnvelope,
        object_id: &EnvelopeId,
        replica: &ReplicaId,
        scope: &ReplicaPostScope,
    ) -> std::result::Result<Self, PostRejection> {
        if envelope.verify().is_err() {
            return Err(PostRejection::InvalidSignature);
        }
        if envelope.id != *object_id {
            return Err(PostRejection::ObjectIdMismatch);
        }
        let Ok(Some(header)) = envelope.to_post_object() else {
            return Err(PostRejection::NotAPost);
        };
        let channel_matches =
            header.channel_id.as_ref().map(ChannelId::as_str) == scope.channel_id.as_deref();
        let visibility_matches =
            scope.channel_id.is_some() || header.visibility == ObjectVisibility::Public;
        if header.topic_id.as_str() != scope.topic_id || !channel_matches || !visibility_matches {
            return Err(PostRejection::ScopeMismatch);
        }
        Ok(Self {
            header,
            replica: replica.clone(),
        })
    }

    /// 自分で署名した投稿を、書き込む replica に照らして確かめる。docs は読まない。
    pub(crate) fn verify_local(
        envelope: KukuriEnvelope,
        replica: &ReplicaId,
    ) -> std::result::Result<Self, PostRejection> {
        let topic_id = envelope
            .to_post_object()
            .ok()
            .flatten()
            .map(|header| header.topic_id)
            .ok_or(PostRejection::NotAPost)?;
        let scope = ReplicaPostScope::for_replica(replica, topic_id.as_str())
            .ok_or(PostRejection::UnsupportedReplica)?;
        let object_id = envelope.id.clone();
        Self::verify(envelope, &object_id, replica, &scope)
    }

    pub(crate) fn header(&self) -> &CanonicalPostHeader {
        &self.header
    }

    pub(crate) fn replica(&self) -> &ReplicaId {
        &self.replica
    }

    /// 取り下げ済みの投稿として、本文・添付・repost 元を落とす。
    pub(crate) fn withdrawn(mut self) -> Self {
        self.header.payload_ref = PayloadRef::InlineText {
            text: String::new(),
        };
        self.header.attachments.clear();
        self.header.media_manifest_refs.clear();
        self.header.repost_of = None;
        self
    }
}

/// `objects/<object id>/state` と `objects/<object id>/envelope` の key から object id を取り出す。
pub(crate) fn object_id_from_post_key(key: &str) -> Option<EnvelopeId> {
    let rest = key.strip_prefix("objects/")?;
    let object_id = rest
        .strip_suffix("/state")
        .or_else(|| rest.strip_suffix("/envelope"))?;
    (!object_id.is_empty() && !object_id.contains('/')).then(|| EnvelopeId::from(object_id))
}

pub(crate) fn post_envelope_key(object_id: &EnvelopeId) -> String {
    stable_key("objects", &format!("{}/envelope", object_id.as_str()))
}

/// 同じ object の envelope の record(複数ありうる)から、検証に通る最初の 1 件を選ぶ。docs は読まない。
///
/// 通るものが無ければ、最後に見た record の理由を返す。record が 1 件も無ければ `Ok(None)`。
pub(crate) fn select_verified_post<'a>(
    records: impl IntoIterator<Item = &'a DocRecord>,
    object_id: &EnvelopeId,
    replica: &ReplicaId,
    scope: &ReplicaPostScope,
) -> std::result::Result<Option<VerifiedPost>, PostRejection> {
    let mut rejection = None;
    for record in records.into_iter().take(MAX_ENVELOPE_RECORDS_PER_OBJECT) {
        let verified = serde_json::from_slice::<KukuriEnvelope>(&record.value)
            .map_err(|_| PostRejection::NotAnEnvelope)
            .and_then(|envelope| VerifiedPost::verify(envelope, object_id, replica, scope));
        match verified {
            Ok(post) => return Ok(Some(post)),
            Err(reason) => rejection = Some(reason),
        }
    }
    rejection.map_or(Ok(None), Err)
}

/// 投稿を 1 件、署名つき envelope から読んで検証する。
///
/// 読む docs の record は、その object の `envelope` の key の最大 `MAX_ENVELOPE_RECORDS_PER_OBJECT` 件だけで、
/// replica の総 entry 数に依存しない。検証に通る envelope が無ければ `Ok(None)`(不正な record は warn を出して
/// 飛ばす)。docs の読み出しの失敗は `Err`。
pub(crate) async fn load_verified_post(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    subscription_topic_id: &str,
    object_id: &EnvelopeId,
    policy: DocFetchPolicy,
) -> Result<Option<VerifiedPost>> {
    let Some(scope) = ReplicaPostScope::for_replica(replica, subscription_topic_id) else {
        warn_rejected_post(replica, object_id, PostRejection::UnsupportedReplica);
        return Ok(None);
    };
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            post_envelope_key(object_id).as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    match select_verified_post(records.iter(), object_id, replica, &scope) {
        Ok(post) => Ok(post),
        Err(reason) => {
            warn_rejected_post(replica, object_id, reason);
            Ok(None)
        }
    }
}

/// 取り下げを、対象の envelope の record(複数ありうる)に照らした結果。
pub(crate) enum WithdrawalTargetCheck {
    Verified(Box<PostWithdrawalV1>),
    /// 対象の envelope は正しいが、取り下げがそれと合わない(署名者が違うなど)。
    Mismatch(anyhow::Error),
    /// 対象として読める envelope がまだ無い。
    TargetMissing,
}

/// 同じ key の record のうち、取り下げが検証に通る対象を探す。先頭の 1 件だけを見ない(#1248)。
pub(crate) fn verify_withdrawal_against_records(
    withdrawal: &KukuriEnvelope,
    target_object_id: &EnvelopeId,
    target_records: &[DocRecord],
) -> WithdrawalTargetCheck {
    let mut mismatch = None;
    for record in target_records.iter().take(MAX_ENVELOPE_RECORDS_PER_OBJECT) {
        let Ok(target) = serde_json::from_slice::<KukuriEnvelope>(&record.value) else {
            continue;
        };
        match verify_post_withdrawal(withdrawal, &target) {
            Ok(verified) => return WithdrawalTargetCheck::Verified(Box::new(verified)),
            // 対象の envelope が正しいのに取り下げが合わないときだけ、取り下げを不正とする。
            Err(error) if target.verify().is_ok() && target.id == *target_object_id => {
                mismatch = Some(error);
            }
            Err(_) => {}
        }
    }
    mismatch.map_or(
        WithdrawalTargetCheck::TargetMissing,
        WithdrawalTargetCheck::Mismatch,
    )
}

pub(crate) fn warn_rejected_post(
    replica: &ReplicaId,
    object_id: &EnvelopeId,
    reason: PostRejection,
) {
    warn!(
        replica = %replica.as_str(),
        object_id = %object_id.as_str(),
        reason = reason.as_str(),
        "ignored a post that cannot be verified"
    );
}
