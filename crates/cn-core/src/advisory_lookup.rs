//! タイムライン向け content advisory 一括照会の読み口(#1056 / ADR 0046 §6.3 / ADR 0028 §8.12)。
//!
//! 真実源は `cn_safety.risk_signals`(ラベル付き allow で生成される Low の advisory-only signal)。
//! verdict 行の `advisory_labels` ではなく signal を読むことで、異議申し立ての `Cleared` と
//! `expires_at` の失効を自然に除外する。SELECT のみで、index scope・verdict・signal を変更しない。

use anyhow::Result;
use kukuri_cn_safety::{
    AdvisorySubjectKind, AppealStatus, Basis, ContentAdvisory, RiskSignalTarget,
};
use sqlx::PgPool;

use crate::safety_events::{RISK_SIGNAL_COLUMNS, risk_signal_from_row};

/// `issuer_node_id` が発行した、指定 subject の有効な content advisory を返す。
///
/// - 対象は `post_id` / `blob_cid` の nsfw / objectionable signal のみ(critical・spam 等と
///   user / peer 対象は返さない)。
/// - `appeal_status = cleared`、`expires_at` 失効済み、retention 切れは除外する。
/// - basis は `classifier_score` のものだけを返す(confirmed へ昇格しない)。
/// - (target, target_id, category) ごとに最新 1 件へ畳む。
pub async fn list_content_advisories_for_subjects(
    pool: &PgPool,
    issuer_node_id: &str,
    post_ids: &[String],
    blob_hashes: &[String],
    now_rfc3339: &str,
) -> Result<Vec<ContentAdvisory>> {
    if post_ids.is_empty() && blob_hashes.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT DISTINCT ON (target, target_id, category) {RISK_SIGNAL_COLUMNS}
         FROM cn_safety.risk_signals
         WHERE issuer_node_id = $1
           AND category IN ('nsfw', 'objectionable')
           AND basis = 'classifier_score'
           AND ((target = 'post_id' AND target_id = ANY($2))
             OR (target = 'blob_cid' AND target_id = ANY($3)))
           AND (appeal_status IS NULL OR appeal_status <> 'cleared')
           AND (expires_at IS NULL OR expires_at::timestamptz > $4::timestamptz)
           AND persisted_at > NOW() - cn_admin.retention_interval('risk_signal')
         ORDER BY target, target_id, category, persisted_at DESC, id"
    )))
    .bind(issuer_node_id)
    .bind(post_ids)
    .bind(blob_hashes)
    .bind(now_rfc3339)
    .fetch_all(pool)
    .await?;

    let mut advisories = Vec::new();
    for row in &rows {
        let stored = risk_signal_from_row(row)?;
        let signal = &stored.signal;
        // SQL 側の条件と同じ規則を型でも確認する(列の値域が将来増えても誤って返さない)。
        if signal.basis != Basis::ClassifierScore
            || signal.appeal_status == Some(AppealStatus::Cleared)
        {
            continue;
        }
        let Some(label) = signal.category.advisory_display_label() else {
            continue;
        };
        let subject_kind = match signal.target {
            RiskSignalTarget::PostId => AdvisorySubjectKind::PostId,
            RiskSignalTarget::BlobCid => AdvisorySubjectKind::BlobCid,
            RiskSignalTarget::UserPubkey | RiskSignalTarget::PeerNode => continue,
        };
        advisories.push(ContentAdvisory {
            issuer_node_id: stored.issuer_node_id.clone(),
            subject_kind,
            subject_id: signal.target_id.clone(),
            category: signal.category,
            label: label.to_string(),
            confidence: signal.confidence,
            signal_id: stored.id.clone(),
            basis: signal.basis,
        });
    }
    Ok(advisories)
}
