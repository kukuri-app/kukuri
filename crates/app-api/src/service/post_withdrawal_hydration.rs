//! 著者による取り下げ(`withdrawals/<object id>/state`)を検証して projection へ反映する(ADR 0032、ADR 0052 §2)。
//!
//! 取り下げの行と伏せた行を書くのは `apply_verified_post_withdrawal` だけで、検証の後にある。

use super::*;

/// 反映済みの行を、取り下げ済みの投稿として伏せる。`VerifiedPost::withdrawn` から作る行と同じ形にする。
fn scrub_withdrawn_row(mut row: ObjectProjectionRow) -> ObjectProjectionRow {
    row.payload_ref = PayloadRef::InlineText {
        text: String::new(),
    };
    row.content = Some(String::new());
    row.attachments.clear();
    row.repost_of = None;
    row.source_blob_hash = None;
    row.derived_at = Utc::now().timestamp_millis();
    row
}

/// `withdrawals/<object id>/state` の record を 1 件反映した結果(#1239)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PostWithdrawalHydration {
    /// 検証できた取り下げを projection へ反映した。
    Applied,
    /// 対象の envelope がまだ手元に無く、検証できない。対象が届いたときに反映し直す。
    TargetMissing,
    /// 取り下げとして読めない、または署名・著者が対象と合わない。取り下げとして扱わない。
    Invalid,
}

impl PostWithdrawalHydration {
    pub(crate) fn applied(self) -> bool {
        self == Self::Applied
    }
}

/// 取り下げの record を、署名つきの取り下げの envelope として読む。docs は読まない。
///
/// 読めない record と署名が不正な record は warn を出して `None` を返す。
fn parse_post_withdrawal_record(
    replica: &ReplicaId,
    record: &DocRecord,
) -> Option<(KukuriEnvelope, EnvelopeId)> {
    let invalid = |reason: &str, error: &dyn std::fmt::Display| {
        warn_invalid_post_withdrawal(replica, record.key.as_str(), reason, error);
        None
    };
    let envelope: KukuriEnvelope = match serde_json::from_slice(&record.value) {
        Ok(envelope) => envelope,
        Err(error) => return invalid("the record is not an envelope", &error),
    };
    let content = match envelope.post_withdrawal_content() {
        Ok(Some(content)) => content,
        Ok(None) => {
            return invalid("the envelope is not a post withdrawal", &"kind mismatch");
        }
        Err(error) => return invalid("the withdrawal content is malformed", &error),
    };
    if let Err(error) = envelope.verify() {
        return invalid("the withdrawal signature is invalid", &error);
    }
    Some((envelope, content.target_object_id))
}

fn warn_invalid_post_withdrawal(
    replica: &ReplicaId,
    key: &str,
    reason: &str,
    error: &dyn std::fmt::Display,
) {
    warn!(
        replica = %replica.as_str(),
        key = %key,
        reason,
        error = %error,
        "ignored a post withdrawal record that cannot be verified"
    );
}

/// projection が保存できる取り下げか。
///
/// projection は `generation` を符号つき 64 bit で持つ。収まらない値は保存できず、書き込みの失敗として伝わると、
/// 署名の正しい取り下げを 1 件置くだけで、その object を含む範囲の取得や、投稿の反映を止められてしまう
/// (ADR 0052 §2)。保存できない取り下げは、取り下げとして扱わない。
fn storable_post_withdrawal(replica: &ReplicaId, key: &str, withdrawal: &PostWithdrawalV1) -> bool {
    match i64::try_from(withdrawal.generation) {
        Ok(_) => true,
        Err(error) => {
            warn_invalid_post_withdrawal(
                replica,
                key,
                "the withdrawal generation is out of range",
                &error,
            );
            false
        }
    }
}

/// 検証できた取り下げを projection へ反映する。取り下げの行と伏せた行を書くのは、ここだけ。
async fn apply_verified_post_withdrawal(
    projection_store: &dyn ProjectionStore,
    replica: &ReplicaId,
    withdrawal: PostWithdrawalV1,
    target_object_id: &EnvelopeId,
) -> Result<()> {
    projection_store
        .put_post_withdrawal(post_withdrawal_row(withdrawal, replica))
        .await?;

    // 反映済みの行を伏せる。行がまだ無ければ、投稿を反映する時点で伏せた行ができる
    // (`hydrate_object_in_topic` と全件走査は、取り下げを先に確認する)。docs の `state` の値は使わない(#1248)。
    if let Some(row) = projection_store
        .get_object_projection(target_object_id)
        .await?
    {
        projection_store
            .put_object_projection(scrub_withdrawn_row(row))
            .await?;
    }
    Ok(())
}

/// 取り下げの record を 1 件、検証して projection へ反映する。全件走査が record ごとに呼ぶ。
///
/// 読めない record と検証できない record は `Invalid` を返し、エラーにしない。public topic の replica は
/// 誰でも書けるので、読めない record を 1 件置くだけで、投稿の反映や topic 全体の操作を止められないようにする
/// (ADR 0052 §2)。docs と projection の読み書きの失敗はエラーとして返す。
///
/// key を指定して読む入口は、これを直接呼ばず `hydrate_post_withdrawal_for_object` を使う(#1250)。
pub(crate) async fn hydrate_post_withdrawal_from_record(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    replica: &ReplicaId,
    record: DocRecord,
    policy: DocFetchPolicy,
) -> Result<PostWithdrawalHydration> {
    let Some((envelope, target_object_id)) = parse_post_withdrawal_record(replica, &record) else {
        return Ok(PostWithdrawalHydration::Invalid);
    };
    // 同じ key には docs author ごとの record がありうる。先頭の 1 件だけを見ず、上限つきで対象を探す(#1248)。
    let target_records = docs_sync
        .query_replica_exact_bounded(
            replica,
            post_envelope_key(&target_object_id).as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    let withdrawal =
        match verify_withdrawal_against_records(&envelope, &target_object_id, &target_records) {
            WithdrawalTargetCheck::Verified(withdrawal) => *withdrawal,
            WithdrawalTargetCheck::Mismatch(error) => {
                warn_invalid_post_withdrawal(
                    replica,
                    record.key.as_str(),
                    "the withdrawal does not match the target",
                    &error,
                );
                return Ok(PostWithdrawalHydration::Invalid);
            }
            // 対象として読める envelope がまだ無い。対象が届いたときに反映し直す。
            WithdrawalTargetCheck::TargetMissing => {
                return Ok(PostWithdrawalHydration::TargetMissing);
            }
        };
    if !storable_post_withdrawal(replica, record.key.as_str(), &withdrawal) {
        return Ok(PostWithdrawalHydration::Invalid);
    }
    apply_verified_post_withdrawal(projection_store, replica, withdrawal, &target_object_id)
        .await?;
    Ok(PostWithdrawalHydration::Applied)
}

/// object id を 1 つ指定して、その投稿の取り下げを projection へ反映する(#1250)。replica は走査しない。
///
/// `withdrawals/<object id>/state` には docs author ごとの record がありうる。その replica に書ける誰もが同じ key へ
/// record を足せるので、先頭の 1 件だけを見ると、取り下げとして検証できない record を先に置くだけで、著者の正しい
/// 取り下げを無効にできてしまう。上限つきで複数の record を調べ、その object を対象とし、検証に通る最初の取り下げを
/// 反映する。読む docs の record は、取り下げの key の最大 `MAX_WITHDRAWAL_RECORDS_PER_OBJECT` 件と、候補があるとき
/// だけ読む対象の envelope の key の最大 `MAX_ENVELOPE_RECORDS_PER_OBJECT` 件。replica の大きさに依存しない。
///
/// record が 1 件も無ければ `None`。検証に通る取り下げが無ければ、対象が未着の候補があるときは `TargetMissing`、
/// それ以外は `Invalid`。docs と projection の読み書きの失敗はエラーとして返す。
pub(crate) async fn hydrate_post_withdrawal_for_object(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    replica: &ReplicaId,
    object_id: &EnvelopeId,
    policy: DocFetchPolicy,
) -> Result<Option<PostWithdrawalHydration>> {
    let key = post_withdrawal_key(object_id);
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            key.as_str(),
            MAX_WITHDRAWAL_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    if records.is_empty() {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    for record in records.iter().take(MAX_WITHDRAWAL_RECORDS_PER_OBJECT) {
        let Some((envelope, target_object_id)) = parse_post_withdrawal_record(replica, record)
        else {
            continue;
        };
        // 別の object の取り下げをこの key に置いても、この object の取り下げの確認を終わらせない。
        if target_object_id != *object_id {
            warn_invalid_post_withdrawal(
                replica,
                key.as_str(),
                "the withdrawal targets another object",
                &target_object_id.as_str(),
            );
            continue;
        }
        candidates.push(envelope);
    }
    if candidates.is_empty() {
        return Ok(Some(PostWithdrawalHydration::Invalid));
    }
    let target_records = docs_sync
        .query_replica_exact_bounded(
            replica,
            post_envelope_key(object_id).as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    let mut outcome = PostWithdrawalHydration::Invalid;
    for envelope in &candidates {
        match verify_withdrawal_against_records(envelope, object_id, &target_records) {
            WithdrawalTargetCheck::Verified(withdrawal) => {
                // 保存できない取り下げは飛ばして、残りの候補を調べる。
                if !storable_post_withdrawal(replica, key.as_str(), &withdrawal) {
                    continue;
                }
                apply_verified_post_withdrawal(projection_store, replica, *withdrawal, object_id)
                    .await?;
                return Ok(Some(PostWithdrawalHydration::Applied));
            }
            WithdrawalTargetCheck::Mismatch(error) => warn_invalid_post_withdrawal(
                replica,
                key.as_str(),
                "the withdrawal does not match the target",
                &error,
            ),
            WithdrawalTargetCheck::TargetMissing => {
                outcome = PostWithdrawalHydration::TargetMissing;
            }
        }
    }
    Ok(Some(outcome))
}

/// `withdrawals/<object id>/state` の key。
pub(crate) fn post_withdrawal_key(object_id: &EnvelopeId) -> String {
    stable_key("withdrawals", &format!("{}/state", object_id.as_str()))
}

/// `withdrawals/<object id>/state` の key から object id を取り出す。
pub(crate) fn object_id_from_post_withdrawal_key(key: &str) -> Option<EnvelopeId> {
    let object_id = key.strip_prefix("withdrawals/")?.strip_suffix("/state")?;
    (!object_id.is_empty() && !object_id.contains('/')).then(|| EnvelopeId::from(object_id))
}
