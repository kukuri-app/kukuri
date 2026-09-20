//! docs から反映する reaction の検証(#1252)。
//!
//! `reactions/<target>/<reaction id>/state` の doc は署名を持たない。反映する値は同じ reaction の署名つき envelope
//! (`reactions/<target>/<reaction id>/envelope`)だけから作り、読んだ replica が受け入れる topic / channel と照らす。
//! 検証に通った reaction だけが `VerifiedReaction` になり、projection の行は `VerifiedReaction` からしか作れない。

use super::*;

/// reaction を読んだ key(`reactions/<target>/<reaction id>/...`)が指す対象と id。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ReactionKey {
    pub(crate) target_object_id: EnvelopeId,
    pub(crate) reaction_id: EnvelopeId,
}

impl ReactionKey {
    /// `state` と `envelope` のどちらの key からも取り出す。custom reaction の asset(`reactions/assets/...`)は対象外。
    pub(crate) fn from_doc_key(key: &str) -> Option<Self> {
        let rest = key.strip_prefix("reactions/")?;
        let rest = rest
            .strip_suffix("/state")
            .or_else(|| rest.strip_suffix("/envelope"))?;
        let (target, reaction_id) = rest.split_once('/')?;
        (!target.is_empty() && !reaction_id.is_empty() && !reaction_id.contains('/')).then(|| {
            Self {
                target_object_id: EnvelopeId::from(target),
                reaction_id: EnvelopeId::from(reaction_id),
            }
        })
    }

    pub(crate) fn envelope_key(&self) -> String {
        stable_key(
            "reactions",
            &format!(
                "{}/{}/envelope",
                self.target_object_id.as_str(),
                self.reaction_id.as_str()
            ),
        )
    }
}

/// 検証に通らなかった理由。I/O の失敗は含まない(呼び出し側へ `Err` で返す)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReactionRejection {
    /// reaction を置く replica ではない、または replica と購読の topic が合わない。
    UnsupportedReplica,
    /// record が envelope として読めない。
    NotAnEnvelope,
    /// id または署名が envelope の内容と合わない。
    InvalidSignature,
    /// reaction の envelope ではない、または content が規則に合わない。
    NotAReaction,
    /// envelope の対象・reaction id が key と違う、または reaction id が replica・対象・署名者・key から決まる値と違う。
    IdentityMismatch,
    /// envelope が申告する topic・channel が、読んだ replica と合わない。
    ScopeMismatch,
}

impl ReactionRejection {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedReplica => "the replica does not hold reactions for this topic",
            Self::NotAnEnvelope => "the record is not an envelope",
            Self::InvalidSignature => "the envelope id or signature is invalid",
            Self::NotAReaction => "the envelope is not a valid reaction",
            Self::IdentityMismatch => "the reaction identity does not match its key and signer",
            Self::ScopeMismatch => "the reaction does not belong to the replica it was read from",
        }
    }
}

/// 署名つき envelope と、読んだ replica の scope に照らして確かめた reaction。
///
/// 値はすべて envelope(`parse_reaction`)から作る。docs の `state` の record の値は使わない。
#[derive(Clone, Debug)]
pub(crate) struct VerifiedReaction {
    doc: ReactionDocV1,
    replica: ReplicaId,
}

impl VerifiedReaction {
    /// envelope を検証する。docs は読まない。
    pub(crate) fn verify(
        envelope: &KukuriEnvelope,
        key: &ReactionKey,
        replica: &ReplicaId,
        scope: &ReplicaPostScope,
    ) -> std::result::Result<Self, ReactionRejection> {
        if envelope.verify().is_err() {
            return Err(ReactionRejection::InvalidSignature);
        }
        let Ok(Some(doc)) = parse_reaction(envelope) else {
            return Err(ReactionRejection::NotAReaction);
        };
        // reaction id は replica・対象・署名者・key から決まる。別の replica や別の著者の reaction を、この key へ置けない。
        let expected_id = deterministic_reaction_id(
            replica,
            &doc.target_object_id,
            &doc.author_pubkey,
            doc.normalized_reaction_key.as_str(),
        );
        if doc.target_object_id != key.target_object_id
            || doc.reaction_id != key.reaction_id
            || doc.reaction_id != expected_id
        {
            return Err(ReactionRejection::IdentityMismatch);
        }
        if !scope.accepts(&doc.target_topic_id, doc.channel_id.as_ref()) {
            return Err(ReactionRejection::ScopeMismatch);
        }
        Ok(Self {
            doc,
            replica: replica.clone(),
        })
    }

    /// 自分で署名した reaction を、書き込む replica に照らして確かめる。docs は読まない。
    pub(crate) fn verify_local(
        envelope: &KukuriEnvelope,
        replica: &ReplicaId,
    ) -> std::result::Result<Self, ReactionRejection> {
        let Ok(Some(doc)) = parse_reaction(envelope) else {
            return Err(ReactionRejection::NotAReaction);
        };
        let scope = ReplicaPostScope::for_replica(replica, doc.target_topic_id.as_str())
            .ok_or(ReactionRejection::UnsupportedReplica)?;
        let key = ReactionKey {
            target_object_id: doc.target_object_id.clone(),
            reaction_id: doc.reaction_id.clone(),
        };
        Self::verify(envelope, &key, replica, &scope)
    }

    pub(crate) fn doc(&self) -> &ReactionDocV1 {
        &self.doc
    }

    pub(crate) fn replica(&self) -> &ReplicaId {
        &self.replica
    }
}

/// 同じ reaction の envelope の record(複数ありうる)から、検証に通るもののうち最も新しい 1 件を選ぶ。docs は読まない。
///
/// 通るものが無ければ、最後に見た record の理由を返す。record が 1 件も無ければ `Ok(None)`。
pub(crate) fn select_verified_reaction<'a>(
    records: impl IntoIterator<Item = &'a DocRecord>,
    key: &ReactionKey,
    replica: &ReplicaId,
    scope: &ReplicaPostScope,
) -> std::result::Result<Option<VerifiedReaction>, ReactionRejection> {
    let mut rejection = None;
    let mut newest: Option<VerifiedReaction> = None;
    for record in records.into_iter().take(MAX_ENVELOPE_RECORDS_PER_OBJECT) {
        let verified = serde_json::from_slice::<KukuriEnvelope>(&record.value)
            .map_err(|_| ReactionRejection::NotAnEnvelope)
            .and_then(|envelope| VerifiedReaction::verify(&envelope, key, replica, scope));
        match verified {
            Ok(reaction) => {
                if newest
                    .as_ref()
                    .is_none_or(|current| reaction.doc.updated_at > current.doc.updated_at)
                {
                    newest = Some(reaction);
                }
            }
            Err(reason) => rejection = Some(reason),
        }
    }
    match (newest, rejection) {
        (Some(reaction), _) => Ok(Some(reaction)),
        (None, Some(reason)) => Err(reason),
        (None, None) => Ok(None),
    }
}

/// reaction を 1 件、署名つき envelope から読んで検証する。
///
/// 読む docs の record は、その reaction の `envelope` の key の最大 `MAX_ENVELOPE_RECORDS_PER_OBJECT` 件だけ。
/// 検証に通る envelope が無ければ `Ok(None)`(warn)。docs の読み出しの失敗は `Err`。
pub(crate) async fn load_verified_reaction(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    subscription_topic_id: &str,
    key: &ReactionKey,
    policy: DocFetchPolicy,
) -> Result<Option<VerifiedReaction>> {
    let Some(scope) = ReplicaPostScope::for_replica(replica, subscription_topic_id) else {
        warn_rejected_reaction(replica, key, ReactionRejection::UnsupportedReplica);
        return Ok(None);
    };
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            key.envelope_key().as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    match select_verified_reaction(records.iter(), key, replica, &scope) {
        Ok(reaction) => Ok(reaction),
        Err(reason) => {
            warn_rejected_reaction(replica, key, reason);
            Ok(None)
        }
    }
}

pub(crate) fn warn_rejected_reaction(
    replica: &ReplicaId,
    key: &ReactionKey,
    reason: ReactionRejection,
) {
    warn!(
        replica = %replica.as_str(),
        target_object_id = %key.target_object_id.as_str(),
        reaction_id = %key.reaction_id.as_str(),
        reason = reason.as_str(),
        "ignored a reaction that cannot be verified"
    );
}
