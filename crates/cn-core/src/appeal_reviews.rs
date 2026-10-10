//! 異議申し立ての運営者向け参照と原子的な審査操作（#680）。

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use uuid::Uuid;

use crate::operator_actions::OperatorAction;
use crate::safety_appeals::{
    RiskSignalCorrection, RiskSignalMetadataEdit, validate_optional_confidence,
    validate_optional_expires_at,
};
use crate::safety_events::{
    OperatorCorrectedRiskSignal, insert_operator_corrected_risk_signal, to_db_enum,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppealReviewReport {
    pub id: String,
    pub details: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppealReview {
    pub risk_signal_id: String,
    pub issuer_node_id: String,
    pub target: String,
    pub target_id: String,
    pub category: String,
    pub severity: String,
    pub basis: String,
    pub confidence: Option<u8>,
    pub visibility: String,
    pub expires_at: Option<String>,
    pub appeal_status: String,
    pub reports: Vec<AppealReviewReport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppealReviewVersion {
    pub appeal_status: String,
    pub category: String,
    pub severity: String,
    pub confidence: Option<u8>,
    pub visibility: String,
    pub expires_at: Option<String>,
    pub reports: Vec<(String, String)>,
}

impl AppealReview {
    pub fn version(&self) -> AppealReviewVersion {
        AppealReviewVersion {
            appeal_status: self.appeal_status.clone(),
            category: self.category.clone(),
            severity: self.severity.clone(),
            confidence: self.confidence,
            visibility: self.visibility.clone(),
            expires_at: self.expires_at.clone(),
            reports: self
                .reports
                .iter()
                .map(|report| (report.id.clone(), report.status.clone()))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppealReviewOperation {
    Accept {
        expected: AppealReviewVersion,
    },
    Reject {
        expected: AppealReviewVersion,
    },
    Edit {
        expected: AppealReviewVersion,
        edit: RiskSignalMetadataEdit,
    },
    Reissue {
        expected: AppealReviewVersion,
        correction: RiskSignalCorrection,
    },
}

pub async fn list_appeal_reviews(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<AppealReview>> {
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT rs.id
         FROM cn_safety.risk_signals rs
         WHERE COALESCE(rs.appeal_status, 'none') = 'disputed'
           AND rs.persisted_at > NOW() - cn_admin.retention_interval('risk_signal')
           AND EXISTS (
               SELECT 1 FROM cn_admin.reports report
               WHERE report.appeal_risk_signal_id = rs.id
                 AND report.created_at > NOW() - cn_admin.retention_interval('report')
           )
         ORDER BY rs.persisted_at DESC, rs.id DESC
         LIMIT $1 OFFSET $2",
    )
    .bind(limit.clamp(1, 200))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await?;
    let mut reviews = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(review) = get_appeal_review(pool, &id).await? {
            reviews.push(review);
        }
    }
    Ok(reviews)
}

pub async fn get_appeal_review(
    pool: &PgPool,
    risk_signal_id: &str,
) -> Result<Option<AppealReview>> {
    let row = sqlx::query(
        "SELECT id, issuer_node_id, target, target_id, category, severity, basis, confidence,
                visibility, expires_at, COALESCE(appeal_status, 'none') AS appeal_status
         FROM cn_safety.risk_signals
         WHERE id = $1 AND persisted_at > NOW() - cn_admin.retention_interval('risk_signal')",
    )
    .bind(risk_signal_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let reports = load_reports(pool, risk_signal_id).await?;
    Ok(Some(review_from_row(&row, reports)?))
}

pub async fn apply_appeal_review_action(
    pool: &PgPool,
    actor: &str,
    risk_signal_id: &str,
    operation: &AppealReviewOperation,
    operator_review_enabled: bool,
) -> Result<OperatorAction> {
    let actor = actor.trim();
    if actor.is_empty() {
        bail!("admin operation actor must not be empty");
    }
    if actor.len() > 320 {
        bail!("admin operation actor is too long");
    }
    if !operator_review_enabled {
        bail!("operator review is not enabled on this community node");
    }
    let risk_signal_id = risk_signal_id.trim();
    if risk_signal_id.is_empty() {
        bail!("risk signal id must not be empty");
    }

    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT id, issuer_node_id, target, target_id, category, severity, basis, confidence,
                visibility, expires_at, COALESCE(appeal_status, 'none') AS appeal_status,
                COALESCE(operator_origin_category, category) AS origin_category
         FROM cn_safety.risk_signals
         WHERE id = $1 AND persisted_at > NOW() - cn_admin.retention_interval('risk_signal')
         FOR UPDATE",
    )
    .bind(risk_signal_id)
    .fetch_optional(&mut *tx)
    .await?
    .with_context(|| format!("risk signal `{risk_signal_id}` was not found"))?;
    let report_rows = sqlx::query(
        "SELECT id, details, status, created_at
         FROM cn_admin.reports
         WHERE appeal_risk_signal_id = $1
           AND created_at > NOW() - cn_admin.retention_interval('report')
         ORDER BY created_at, id
         FOR UPDATE",
    )
    .bind(risk_signal_id)
    .fetch_all(&mut *tx)
    .await?;
    if report_rows.is_empty() {
        bail!("risk signal `{risk_signal_id}` has no linked appeal reports");
    }
    let current = review_from_row(
        &row,
        report_rows
            .iter()
            .map(report_from_row)
            .collect::<Result<Vec<_>>>()?,
    )?;
    let expected = match operation {
        AppealReviewOperation::Accept { expected }
        | AppealReviewOperation::Reject { expected }
        | AppealReviewOperation::Edit { expected, .. }
        | AppealReviewOperation::Reissue { expected, .. } => expected,
    };
    if &current.version() != expected {
        bail!("appeal review changed after preview; reload and confirm again");
    }
    if current.appeal_status != "disputed" {
        bail!("risk signal `{risk_signal_id}` is not disputed");
    }

    let before = audit_snapshot(&current);
    let (action, reissued_signal) = match operation {
        AppealReviewOperation::Accept { .. } => {
            sqlx::query(
                "UPDATE cn_safety.risk_signals SET appeal_status = 'cleared' WHERE id = $1",
            )
            .bind(risk_signal_id)
            .execute(&mut *tx)
            .await?;
            set_linked_report_status(&mut tx, risk_signal_id, "actioned").await?;
            ("appeal.accept", None)
        }
        AppealReviewOperation::Reject { .. } => {
            sqlx::query("UPDATE cn_safety.risk_signals SET appeal_status = 'none' WHERE id = $1")
                .bind(risk_signal_id)
                .execute(&mut *tx)
                .await?;
            set_linked_report_status(&mut tx, risk_signal_id, "dismissed").await?;
            ("appeal.reject", None)
        }
        AppealReviewOperation::Edit { edit, .. } => {
            validate_edit(edit)?;
            let category = edit
                .category
                .map(|value| to_db_enum(&value))
                .transpose()?
                .unwrap_or_else(|| current.category.clone());
            let severity = edit
                .severity
                .map(|value| to_db_enum(&value))
                .transpose()?
                .unwrap_or_else(|| current.severity.clone());
            let confidence = edit.confidence.or(current.confidence);
            let expires_at = edit
                .expires_at
                .clone()
                .or_else(|| current.expires_at.clone());
            // #1058: operator 確定の印を付け、訂正前の category を初回だけ記録する
            // （SET 右辺の category は更新前の値）。
            sqlx::query(
                "UPDATE cn_safety.risk_signals
                 SET category = $2, severity = $3, confidence = $4, expires_at = $5,
                     operator_adjusted_at = NOW(),
                     operator_origin_category = COALESCE(operator_origin_category, category)
                 WHERE id = $1",
            )
            .bind(risk_signal_id)
            .bind(category)
            .bind(severity)
            .bind(confidence.map(i16::from))
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
            set_linked_report_status(&mut tx, risk_signal_id, "reviewing").await?;
            ("appeal.edit_detection", None)
        }
        AppealReviewOperation::Reissue { correction, .. } => {
            validate_correction(correction)?;
            // #710(案A): 旧判定は失効させず、同一取引で認容(cleared)として終結させる。
            // 失効させると trust 供給層の失効除外で根拠一覧から消え、利用者が審査の終結を
            // 確認できない。cleared は失効させず配布に残り、受け手が寄与 0 で保持する
            // 既存契約(#685)にそのまま乗る(訂正版と旧判定の二重寄与は起きない)。
            sqlx::query(
                "UPDATE cn_safety.risk_signals SET appeal_status = 'cleared' WHERE id = $1",
            )
            .bind(risk_signal_id)
            .execute(&mut *tx)
            .await?;
            let category = enum_or_current(correction.category, &current.category)?;
            let severity = enum_or_current(correction.severity, &current.severity)?;
            let visibility = enum_or_current(correction.visibility, &current.visibility)?;
            let confidence = correction.confidence.or(current.confidence);
            let origin_category: String = row.try_get("origin_category")?;
            // #1058: 訂正版は scanner の集約経路を通さず、operator 確定の印を付けて挿入する。
            let reissued = insert_operator_corrected_risk_signal(
                &mut tx,
                &OperatorCorrectedRiskSignal {
                    issuer_node_id: &current.issuer_node_id,
                    target: &current.target,
                    target_id: &current.target_id,
                    category: &category,
                    severity: &severity,
                    basis: &current.basis,
                    visibility: &visibility,
                    confidence,
                    origin_category: &origin_category,
                },
            )
            .await?;
            let new_id = reissued.id;
            set_linked_report_status(&mut tx, risk_signal_id, "actioned").await?;
            (
                "appeal.reissue_correction",
                Some(json!({
                    "id": new_id,
                    "category": category,
                    "severity": severity,
                    "confidence": confidence,
                    "visibility": visibility,
                    "appeal_status": "none",
                })),
            )
        }
    };

    let after_row = sqlx::query(
        "SELECT id, issuer_node_id, target, target_id, category, severity, basis, confidence,
                visibility, expires_at, COALESCE(appeal_status, 'none') AS appeal_status
         FROM cn_safety.risk_signals WHERE id = $1",
    )
    .bind(risk_signal_id)
    .fetch_one(&mut *tx)
    .await?;
    let after_reports = sqlx::query(
        "SELECT id, details, status, created_at FROM cn_admin.reports
         WHERE appeal_risk_signal_id = $1
           AND created_at > NOW() - cn_admin.retention_interval('report')
         ORDER BY created_at, id",
    )
    .bind(risk_signal_id)
    .fetch_all(&mut *tx)
    .await?;
    let after_review = review_from_row(
        &after_row,
        after_reports
            .iter()
            .map(report_from_row)
            .collect::<Result<Vec<_>>>()?,
    )?;
    let mut after = audit_snapshot(&after_review);
    if let Some(reissued_signal) = reissued_signal {
        after["reissued_risk_signal"] = reissued_signal;
    }

    let action_id = Uuid::new_v4().to_string();
    let audit_row = sqlx::query(
        "INSERT INTO cn_admin.operator_actions
            (id, actor, action, target_kind, target_id, before_json, after_json)
         VALUES ($1, $2, $3, 'risk_signal_appeal', $4, $5, $6)
         RETURNING id, occurred_at, actor, action, target_kind, target_id, before_json, after_json",
    )
    .bind(&action_id)
    .bind(actor)
    .bind(action)
    .bind(risk_signal_id)
    .bind(before)
    .bind(after)
    .fetch_one(&mut *tx)
    .await?;
    let result = operator_action_from_row(&audit_row)?;
    tx.commit().await?;
    Ok(result)
}

async fn load_reports(pool: &PgPool, risk_signal_id: &str) -> Result<Vec<AppealReviewReport>> {
    let rows = sqlx::query(
        "SELECT id, details, status, created_at FROM cn_admin.reports
         WHERE appeal_risk_signal_id = $1
           AND created_at > NOW() - cn_admin.retention_interval('report')
         ORDER BY created_at, id",
    )
    .bind(risk_signal_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(report_from_row).collect()
}

fn review_from_row(row: &PgRow, reports: Vec<AppealReviewReport>) -> Result<AppealReview> {
    let confidence: Option<i16> = row.try_get("confidence")?;
    Ok(AppealReview {
        risk_signal_id: row.try_get("id")?,
        issuer_node_id: row.try_get("issuer_node_id")?,
        target: row.try_get("target")?,
        target_id: row.try_get("target_id")?,
        category: row.try_get("category")?,
        severity: row.try_get("severity")?,
        basis: row.try_get("basis")?,
        confidence: confidence.map(|value| value as u8),
        visibility: row.try_get("visibility")?,
        expires_at: row.try_get("expires_at")?,
        appeal_status: row.try_get("appeal_status")?,
        reports,
    })
}

fn report_from_row(row: &PgRow) -> Result<AppealReviewReport> {
    Ok(AppealReviewReport {
        id: row.try_get("id")?,
        details: row.try_get("details")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn set_linked_report_status(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    risk_signal_id: &str,
    status: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE cn_admin.reports SET status = $2
         WHERE appeal_risk_signal_id = $1
           AND created_at > NOW() - cn_admin.retention_interval('report')",
    )
    .bind(risk_signal_id)
    .bind(status)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn audit_snapshot(review: &AppealReview) -> Value {
    json!({
        "appeal_status": review.appeal_status,
        "category": review.category,
        "severity": review.severity,
        "confidence": review.confidence,
        "visibility": review.visibility,
        "expires_at": review.expires_at,
        "report_statuses": review.reports.iter().map(|report| {
            json!({ "id": report.id, "status": report.status })
        }).collect::<Vec<_>>(),
    })
}

fn validate_edit(edit: &RiskSignalMetadataEdit) -> Result<()> {
    if edit.category.is_none()
        && edit.severity.is_none()
        && edit.confidence.is_none()
        && edit.expires_at.is_none()
    {
        bail!("no detection metadata fields to edit");
    }
    validate_optional_confidence(edit.confidence)?;
    validate_optional_expires_at(edit.expires_at.as_deref())?;
    Ok(())
}

fn validate_correction(correction: &RiskSignalCorrection) -> Result<()> {
    if correction.category.is_none()
        && correction.severity.is_none()
        && correction.confidence.is_none()
        && correction.visibility.is_none()
    {
        bail!("no correction fields to reissue");
    }
    validate_optional_confidence(correction.confidence)?;
    Ok(())
}

fn enum_or_current<T: Serialize + Copy>(value: Option<T>, current: &str) -> Result<String> {
    value
        .map(|value| to_db_enum(&value))
        .transpose()
        .map(|value| value.unwrap_or_else(|| current.to_string()))
}

fn operator_action_from_row(row: &PgRow) -> Result<OperatorAction> {
    Ok(OperatorAction {
        id: row.try_get("id")?,
        occurred_at: row.try_get("occurred_at")?,
        actor: row.try_get("actor")?,
        action: row.try_get("action")?,
        target_kind: row.try_get("target_kind")?,
        target_id: row.try_get("target_id")?,
        before: row.try_get("before_json")?,
        after: row.try_get("after_json")?,
    })
}
