//! risk signal の appeal 遷移と operator レビュー（#420 / ADR 0028 §2.3 / §2.8）。
//!
//! 誤検知の是正は **risk signal 側**で行う。署名済み moderation event は編集しない
//! （編集すると署名が壊れる。event は不変の監査記録として残る）。
//!
//! - **appeal 遷移**: `None → Disputed`（申し立て）、`Disputed → Cleared`（operator 認容）、
//!   `Disputed → None`（operator 棄却）。それ以外の遷移は拒否する。
//! - **Cleared の伝播**: `Cleared` の signal は失効させず配布クエリ
//!   （`list_distributable_risk_signals`）に**残す**。受け手は appeal_status を見て
//!   trust 寄与から除外する（対象ごとの集計は `Cleared` を寄与 0 として数える。ADR 0026 §10）。
//! - **operator レビュー**: 検知メタデータ（severity / confidence / category / expires_at）の
//!   直接編集。node-local advisory の是正であり、user の canonical state は変更しない。
//!   operator config（`safety.moderation.operator_review`）で明示的に有効化された場合のみ
//!   実行できる（API 層で強制）。
//! - **訂正 signal の再発行**: 訂正版を新規 persist し、旧 signal に `expires_at` を刻んで
//!   失効させる（ADR 0028 §2.8 の第 3 の是正手段）。

use anyhow::{Context, Result, bail};
use chrono::DateTime;
use sqlx::postgres::PgPool;

use kukuri_cn_safety::{AppealStatus, SafetyCategory, Severity, Visibility};

use crate::safety_events::{
    OperatorCorrectedRiskSignal, StoredRiskSignal, get_risk_signal,
    insert_operator_corrected_risk_signal, to_db_enum,
};

/// appeal 状態を遷移させる（遷移ガード付き）。
///
/// 許可される遷移:
/// - `None → Disputed`（user / client からの申し立て受理）
/// - `Disputed → Cleared`（operator が誤検知と認容。trust 寄与が戻る）
/// - `Disputed → None`（operator が棄却。確定寄与を維持）
///
/// 同一状態への遷移は冪等に成功として扱う。それ以外（例: `Cleared → Disputed`、
/// `None → Cleared` の飛び越し）は Err。
pub async fn update_risk_signal_appeal_status(
    pool: &PgPool,
    id: &str,
    to: AppealStatus,
) -> Result<StoredRiskSignal> {
    let stored = get_risk_signal(pool, id)
        .await?
        .with_context(|| format!("risk signal `{id}` not found"))?;
    let from = stored.signal.appeal_status.unwrap_or_default();
    if from == to {
        return Ok(stored);
    }
    let allowed = matches!(
        (from, to),
        (AppealStatus::None, AppealStatus::Disputed)
            | (AppealStatus::Disputed, AppealStatus::Cleared)
            | (AppealStatus::Disputed, AppealStatus::None)
    );
    if !allowed {
        bail!(
            "invalid appeal transition for risk signal `{id}`: {} -> {}",
            to_db_enum(&from)?,
            to_db_enum(&to)?
        );
    }
    sqlx::query("UPDATE cn_safety.risk_signals SET appeal_status = $2 WHERE id = $1")
        .bind(id)
        .bind(to_db_enum(&to)?)
        .execute(pool)
        .await?;
    get_risk_signal(pool, id)
        .await?
        .context("updated risk signal disappeared")
}

/// user / client からの申し立てを受理して `Disputed` にする（report 受付経路用）。
///
/// 既に `Disputed` なら冪等に成功。既に `Cleared`（解決済み）なら Err。
pub async fn dispute_risk_signal(pool: &PgPool, id: &str) -> Result<StoredRiskSignal> {
    let stored = get_risk_signal(pool, id)
        .await?
        .with_context(|| format!("risk signal `{id}` not found"))?;
    match stored.signal.appeal_status.unwrap_or_default() {
        AppealStatus::Disputed => Ok(stored),
        AppealStatus::Cleared => bail!("risk signal `{id}` is already cleared"),
        AppealStatus::None => {
            update_risk_signal_appeal_status(pool, id, AppealStatus::Disputed).await
        }
    }
}

/// operator レビュー入力の共通検証: confidence は 0-100 のみ受理する（#700）。
///
/// 審査（調整・訂正再発行）・個別編集・`cn-cli` の全経路で共有し、範囲外の値が
/// どの経路からも保存されないようにする（多層防御）。
pub fn validate_optional_confidence(confidence: Option<u8>) -> Result<()> {
    if let Some(confidence) = confidence
        && confidence > 100
    {
        bail!("confidence must be between 0 and 100 (got {confidence})");
    }
    Ok(())
}

/// operator レビュー入力の共通検証: expires_at は RFC 3339 のみ受理する（#700）。
///
/// 過去時刻は単独失効の正規手段なので受理する。不正な形式を保存すると対象ごとの集計
/// （読めない失効時刻は期限切れとして扱う）や配布クエリの時刻変換と食い違うため、
/// 保存前にここで拒否する。
pub fn validate_optional_expires_at(expires_at: Option<&str>) -> Result<()> {
    if let Some(expires_at) = expires_at {
        DateTime::parse_from_rfc3339(expires_at)
            .with_context(|| format!("invalid expires_at `{expires_at}` (expected RFC3339)"))?;
    }
    Ok(())
}

/// operator レビューによる検知メタデータの編集内容。`None` のフィールドは変更しない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RiskSignalMetadataEdit {
    pub category: Option<SafetyCategory>,
    pub severity: Option<Severity>,
    /// confidence の上書き（0-100）。
    pub confidence: Option<u8>,
    /// 失効時刻（RFC3339）の設定。
    pub expires_at: Option<String>,
}

impl RiskSignalMetadataEdit {
    fn is_empty(&self) -> bool {
        self.category.is_none()
            && self.severity.is_none()
            && self.confidence.is_none()
            && self.expires_at.is_none()
    }
}

/// operator レビュー: 検知メタデータを直接編集する（ADR 0028 §2.3）。
///
/// `operator_review_enabled`（operator config `safety.moderation.operator_review`）が false の
/// 場合は Err（optional 機能の明示的有効化を API 層で強制する）。編集は node-local advisory の
/// 是正であり、user の canonical state を変更しない。
pub async fn edit_risk_signal_detection_metadata(
    pool: &PgPool,
    id: &str,
    edit: &RiskSignalMetadataEdit,
    operator_review_enabled: bool,
) -> Result<StoredRiskSignal> {
    if !operator_review_enabled {
        bail!(
            "operator review is not enabled on this node \
             (set safety.moderation.operator_review / COMMUNITY_NODE_SAFETY_OPERATOR_REVIEW)"
        );
    }
    if edit.is_empty() {
        bail!("no detection metadata fields to edit");
    }
    validate_optional_confidence(edit.confidence)?;
    validate_optional_expires_at(edit.expires_at.as_deref())?;
    let stored = get_risk_signal(pool, id)
        .await?
        .with_context(|| format!("risk signal `{id}` not found"))?;
    let category = edit.category.unwrap_or(stored.signal.category);
    let severity = edit.severity.unwrap_or(stored.signal.severity);
    let confidence = edit.confidence.or(stored.signal.confidence);
    let expires_at = edit
        .expires_at
        .clone()
        .or_else(|| stored.signal.expires_at.clone());
    // #1058: operator 確定の印を付け、訂正前の category を初回だけ記録する
    // （SET 右辺の category は更新前の値）。
    sqlx::query(
        "UPDATE cn_safety.risk_signals
         SET category = $2, severity = $3, confidence = $4, expires_at = $5,
             operator_adjusted_at = NOW(),
             operator_origin_category = COALESCE(operator_origin_category, category)
         WHERE id = $1",
    )
    .bind(id)
    .bind(to_db_enum(&category)?)
    .bind(to_db_enum(&severity)?)
    .bind(confidence.map(i16::from))
    .bind(expires_at.as_deref())
    .execute(pool)
    .await?;
    get_risk_signal(pool, id)
        .await?
        .context("edited risk signal disappeared")
}

/// 訂正 signal の再発行に適用する内容。`None` のフィールドは旧 signal の値を引き継ぐ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RiskSignalCorrection {
    pub category: Option<SafetyCategory>,
    pub severity: Option<Severity>,
    pub confidence: Option<u8>,
    pub visibility: Option<Visibility>,
}

/// 訂正 signal を再発行する（ADR 0028 §2.8）。
///
/// 旧 signal に `expires_at = now_rfc3339` を刻んで失効させ（配布クエリから除外され、
/// trust 供給層でも失効除外される）、訂正内容を適用した新 signal を同じ issuer で
/// 同一取引に挿入して返す。新 signal は operator 確定の印を持ち、再 scan で上書きされない
/// （#1058）。
pub async fn reissue_corrected_risk_signal(
    pool: &PgPool,
    id: &str,
    correction: &RiskSignalCorrection,
    now_rfc3339: &str,
    operator_review_enabled: bool,
) -> Result<StoredRiskSignal> {
    if !operator_review_enabled {
        bail!(
            "operator review is not enabled on this node \
             (set safety.moderation.operator_review / COMMUNITY_NODE_SAFETY_OPERATOR_REVIEW)"
        );
    }
    validate_optional_confidence(correction.confidence)?;
    let stored = get_risk_signal(pool, id)
        .await?
        .with_context(|| format!("risk signal `{id}` not found"))?;

    let signal = &stored.signal;
    let origin_category = stored.operator_origin_category.unwrap_or(signal.category);
    let mut tx = pool.begin().await?;
    // 旧 signal を失効させる（訂正の対にならない単独失効は edit_… の expires_at 編集で行う）。
    sqlx::query("UPDATE cn_safety.risk_signals SET expires_at = $2 WHERE id = $1")
        .bind(id)
        .bind(now_rfc3339)
        .execute(&mut *tx)
        .await?;
    // #1058: 訂正版は scanner の集約経路を通さず、operator 確定の印を付けて挿入する。
    // 集約経路を通すと、訂正済み行の再発行が印による抑止に掛かって新しい行を作れない。
    let reissued = insert_operator_corrected_risk_signal(
        &mut tx,
        &OperatorCorrectedRiskSignal {
            issuer_node_id: &stored.issuer_node_id,
            target: &to_db_enum(&signal.target)?,
            target_id: &signal.target_id,
            category: &to_db_enum(&correction.category.unwrap_or(signal.category))?,
            severity: &to_db_enum(&correction.severity.unwrap_or(signal.severity))?,
            basis: &to_db_enum(&signal.basis)?,
            visibility: &to_db_enum(&correction.visibility.unwrap_or(signal.visibility))?,
            confidence: correction.confidence.or(signal.confidence),
            origin_category: &to_db_enum(&origin_category)?,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(reissued)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- #700: operator レビュー入力の共通検証（保存前検証の contract） ---

    #[test]
    fn confidence_within_0_to_100_is_accepted() {
        assert!(validate_optional_confidence(None).is_ok());
        assert!(validate_optional_confidence(Some(0)).is_ok());
        assert!(validate_optional_confidence(Some(100)).is_ok());
    }

    #[test]
    fn confidence_above_100_is_rejected() {
        for confidence in [101, 255] {
            let error = validate_optional_confidence(Some(confidence)).unwrap_err();
            assert!(error.to_string().contains("between 0 and 100"), "{error}");
        }
    }

    #[test]
    fn expires_at_accepts_rfc3339_including_offsets_and_past() {
        assert!(validate_optional_expires_at(None).is_ok());
        assert!(validate_optional_expires_at(Some("2026-07-30T09:00:00Z")).is_ok());
        // 時差付き表記も RFC 3339 として妥当。
        assert!(validate_optional_expires_at(Some("2026-07-30T18:00:00+09:00")).is_ok());
        // 過去時刻は単独失効の正規手段なので受理する。
        assert!(validate_optional_expires_at(Some("2000-01-01T00:00:00Z")).is_ok());
    }

    #[test]
    fn expires_at_rejects_non_rfc3339() {
        for invalid in ["not-a-timestamp", "", "2026-07-30", "2026/07/30 09:00"] {
            let error = validate_optional_expires_at(Some(invalid)).unwrap_err();
            assert!(error.to_string().contains("RFC3339"), "{invalid}: {error}");
        }
    }
}
