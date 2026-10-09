//! 案件・データ区分を限定した legal hold と allowlist export。

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::legal_data::decrypt_row;
use crate::{LegalDataCipher, RetentionPolicy, SensitiveDataCategory};

const REPORT_CATEGORIES: &[&str] = &["report", "report_contact", "operator_audit"];
const RIGHTS_REQUEST_CATEGORIES: &[&str] = &[
    "rights_request",
    "rights_request_contact",
    "rights_request_identity",
    "rights_request_evidence",
    "rights_request_history",
    "operator_audit",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegalHold {
    pub id: String,
    pub target_kind: String,
    pub target_id: String,
    pub data_categories: Vec<String>,
    pub basis: String,
    pub release_condition: String,
    pub started_by: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegalHoldExport {
    pub hold_id: String,
    pub exported_at: DateTime<Utc>,
    pub target_kind: String,
    pub target_id: String,
    pub data_categories: Vec<String>,
    pub data: Value,
}

#[allow(clippy::too_many_arguments)]
pub async fn start_legal_hold(
    pool: &PgPool,
    target_kind: &str,
    target_id: &str,
    data_categories: &[String],
    basis: &str,
    release_condition: &str,
    actor: &str,
    retention: &RetentionPolicy,
    now: DateTime<Utc>,
) -> Result<LegalHold> {
    validate_hold_input(
        target_kind,
        target_id,
        data_categories,
        basis,
        release_condition,
        actor,
    )?;
    let mut tx = pool.begin().await?;
    ensure_target_exists(&mut tx, target_kind, target_id).await?;
    let id = Uuid::new_v4().to_string();
    let row = sqlx::query(
        "INSERT INTO cn_legal.legal_holds
            (id, target_kind, target_id, data_categories, basis, release_condition,
             started_by, started_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING *",
    )
    .bind(&id)
    .bind(target_kind)
    .bind(target_id.trim())
    .bind(data_categories)
    .bind(basis.trim())
    .bind(release_condition.trim())
    .bind(actor.trim())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    append_hold_audit(
        &mut tx,
        "legal_hold.start",
        target_kind,
        target_id,
        actor,
        json!({}),
        json!({"hold_id": id, "data_categories": data_categories}),
        now,
        retention.expiry(now, retention.operator_audit_days),
    )
    .await?;
    let hold = hold_from_row(&row)?;
    tx.commit().await?;
    Ok(hold)
}

pub async fn release_legal_hold(
    pool: &PgPool,
    hold_id: &str,
    actor: &str,
    retention: &RetentionPolicy,
    now: DateTime<Utc>,
) -> Result<LegalHold> {
    validate_text("hold id", hold_id, 128)?;
    validate_text("actor", actor, 320)?;
    // 解除で行を消す。解除の時刻・actor は operator audit に残す（#1706）。
    let mut tx = pool.begin().await?;
    let row = sqlx::query("DELETE FROM cn_legal.legal_holds WHERE id = $1 RETURNING *")
        .bind(hold_id.trim())
        .fetch_optional(&mut *tx)
        .await?
        .context("legal hold was not found")?;
    let hold = hold_from_row(&row)?;
    append_hold_audit(
        &mut tx,
        "legal_hold.release",
        &hold.target_kind,
        &hold.target_id,
        actor,
        json!({"hold_id": hold.id, "active": true}),
        json!({"hold_id": hold.id, "active": false}),
        now,
        retention.expiry(now, retention.operator_audit_days),
    )
    .await?;
    tx.commit().await?;
    Ok(hold)
}

pub async fn export_legal_hold(
    pool: &PgPool,
    cipher: &LegalDataCipher,
    hold_id: &str,
    actor: &str,
    retention: &RetentionPolicy,
    now: DateTime<Utc>,
) -> Result<LegalHoldExport> {
    validate_text("actor", actor, 320)?;
    let row = sqlx::query("SELECT * FROM cn_legal.legal_holds WHERE id = $1")
        .bind(hold_id.trim())
        .fetch_optional(pool)
        .await?
        .context("legal hold was not found")?;
    let hold = hold_from_row(&row)?;
    let mut data = Map::new();
    for category in &hold.data_categories {
        if let Some(value) = export_category(pool, cipher, &hold, category).await? {
            data.insert(category.clone(), value);
        }
    }
    let mut tx = pool.begin().await?;
    append_hold_audit(
        &mut tx,
        "legal_hold.export",
        &hold.target_kind,
        &hold.target_id,
        actor,
        json!({}),
        json!({"hold_id": hold.id, "data_categories": hold.data_categories}),
        now,
        retention.expiry(now, retention.operator_audit_days),
    )
    .await?;
    tx.commit().await?;
    Ok(LegalHoldExport {
        hold_id: hold.id,
        exported_at: now,
        target_kind: hold.target_kind,
        target_id: hold.target_id,
        data_categories: hold.data_categories,
        data: Value::Object(data),
    })
}

async fn export_category(
    pool: &PgPool,
    cipher: &LegalDataCipher,
    hold: &LegalHold,
    category: &str,
) -> Result<Option<Value>> {
    match category {
        "report" => Ok(sqlx::query(
            "SELECT id, subject_kind, subject_id, capability, reason, details, status,
                    appeal_risk_signal_id, created_at, expires_at
             FROM cn_admin.reports WHERE id = $1",
        )
        .bind(&hold.target_id)
        .fetch_optional(pool)
        .await?
        .map(|row| {
            json!({
                "id": row.get::<String, _>("id"),
                "subject_kind": row.get::<String, _>("subject_kind"),
                "subject_id": row.get::<String, _>("subject_id"),
                "capability": row.get::<String, _>("capability"),
                "reason": row.get::<String, _>("reason"),
                "details": row.get::<Option<String>, _>("details"),
                "status": row.get::<String, _>("status"),
                "appeal_risk_signal_id": row.get::<Option<String>, _>("appeal_risk_signal_id"),
                "created_at": row.get::<DateTime<Utc>, _>("created_at"),
                "expires_at": row.get::<DateTime<Utc>, _>("expires_at"),
            })
        })),
        "rights_request" => Ok(sqlx::query(
            "SELECT id, scope_revision, scope_status, status, subject_kind, subject_id,
                    requested_capabilities, request_data, version, public_message,
                    created_at, updated_at, expires_at
             FROM cn_legal.rights_requests WHERE id = $1",
        )
        .bind(&hold.target_id)
        .fetch_optional(pool)
        .await?
        .map(|row| {
            json!({
                "id": row.get::<String, _>("id"),
                "scope_revision": row.get::<String, _>("scope_revision"),
                "scope_status": row.get::<String, _>("scope_status"),
                "status": row.get::<String, _>("status"),
                "subject_kind": row.get::<String, _>("subject_kind"),
                "subject_id": row.get::<String, _>("subject_id"),
                "requested_capabilities": row.get::<Vec<String>, _>("requested_capabilities"),
                "request": row.get::<Value, _>("request_data"),
                "version": row.get::<i32, _>("version"),
                "public_message": row.get::<Option<String>, _>("public_message"),
                "created_at": row.get::<DateTime<Utc>, _>("created_at"),
                "updated_at": row.get::<DateTime<Utc>, _>("updated_at"),
                "expires_at": row.get::<DateTime<Utc>, _>("expires_at"),
            })
        })),
        "report_contact" => {
            export_sensitive(pool, cipher, hold, SensitiveDataCategory::ReportContact).await
        }
        "rights_request_contact" => {
            export_sensitive(
                pool,
                cipher,
                hold,
                SensitiveDataCategory::RightsRequestContact,
            )
            .await
        }
        "rights_request_identity" => {
            export_sensitive(
                pool,
                cipher,
                hold,
                SensitiveDataCategory::RightsRequestIdentity,
            )
            .await
        }
        "rights_request_evidence" => {
            export_sensitive(
                pool,
                cipher,
                hold,
                SensitiveDataCategory::RightsRequestEvidence,
            )
            .await
        }
        "rights_request_history" => {
            let rows = sqlx::query(
                "SELECT id, actor, action, from_status, to_status, public_message,
                        delivery_status, occurred_at
                 FROM cn_legal.rights_request_events WHERE request_id = $1
                 ORDER BY occurred_at, id",
            )
            .bind(&hold.target_id)
            .fetch_all(pool)
            .await?;
            Ok(Some(Value::Array(
                rows.into_iter()
                    .map(|row| {
                        json!({
                            "id": row.get::<String, _>("id"),
                            "actor": row.get::<String, _>("actor"),
                            "action": row.get::<String, _>("action"),
                            "from_status": row.get::<Option<String>, _>("from_status"),
                            "to_status": row.get::<String, _>("to_status"),
                            "public_message": row.get::<Option<String>, _>("public_message"),
                            "delivery_status": row.get::<String, _>("delivery_status"),
                            "occurred_at": row.get::<DateTime<Utc>, _>("occurred_at"),
                        })
                    })
                    .collect(),
            )))
        }
        "operator_audit" => {
            let rows = sqlx::query(
                "SELECT id, occurred_at, actor, action, target_kind, target_id,
                        before_json, after_json
                 FROM cn_admin.operator_actions
                 WHERE target_kind = $1 AND target_id = $2
                 ORDER BY occurred_at, id",
            )
            .bind(&hold.target_kind)
            .bind(&hold.target_id)
            .fetch_all(pool)
            .await?;
            Ok(Some(Value::Array(
                rows.into_iter()
                    .map(|row| {
                        json!({
                            "id": row.get::<String, _>("id"),
                            "occurred_at": row.get::<DateTime<Utc>, _>("occurred_at"),
                            "actor": row.get::<String, _>("actor"),
                            "action": row.get::<String, _>("action"),
                            "target_kind": row.get::<String, _>("target_kind"),
                            "target_id": row.get::<String, _>("target_id"),
                            "before": row.get::<Value, _>("before_json"),
                            "after": row.get::<Value, _>("after_json"),
                        })
                    })
                    .collect(),
            )))
        }
        _ => bail!("unsupported legal hold category `{category}`"),
    }
}

async fn export_sensitive(
    pool: &PgPool,
    cipher: &LegalDataCipher,
    hold: &LegalHold,
    category: SensitiveDataCategory,
) -> Result<Option<Value>> {
    let row = sqlx::query(
        "SELECT nonce, ciphertext FROM cn_legal.sensitive_items
         WHERE owner_kind = $1 AND owner_id = $2 AND data_category = $3",
    )
    .bind(&hold.target_kind)
    .bind(&hold.target_id)
    .bind(category.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref()
        .map(|row| decrypt_row(cipher, &hold.target_kind, &hold.target_id, category, row))
        .transpose()
}

fn validate_hold_input(
    target_kind: &str,
    target_id: &str,
    categories: &[String],
    basis: &str,
    release_condition: &str,
    actor: &str,
) -> Result<()> {
    validate_text("target id", target_id, 512)?;
    validate_text("basis", basis, 2_000)?;
    validate_text("release condition", release_condition, 2_000)?;
    validate_text("actor", actor, 320)?;
    let allowed = match target_kind {
        "report" => REPORT_CATEGORIES,
        "rights_request" => RIGHTS_REQUEST_CATEGORIES,
        _ => bail!("legal hold target kind must be report or rights_request"),
    };
    let unique = categories.iter().collect::<BTreeSet<_>>();
    if categories.is_empty()
        || categories.len() > allowed.len()
        || unique.len() != categories.len()
        || categories
            .iter()
            .any(|value| !allowed.contains(&value.as_str()))
    {
        bail!("legal hold contains an invalid data category");
    }
    Ok(())
}

fn validate_text(name: &str, value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        bail!("{name} must be present, bounded, and contain no control characters");
    }
    Ok(())
}

async fn ensure_target_exists(
    tx: &mut Transaction<'_, Postgres>,
    target_kind: &str,
    target_id: &str,
) -> Result<()> {
    let exists = match target_kind {
        "report" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cn_admin.reports WHERE id = $1)",
        ),
        "rights_request" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cn_legal.rights_requests WHERE id = $1)",
        ),
        _ => unreachable!("validated target kind"),
    }
    .bind(target_id.trim())
    .fetch_one(&mut **tx)
    .await?;
    if !exists {
        bail!("legal hold target was not found");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn append_hold_audit(
    tx: &mut Transaction<'_, Postgres>,
    action: &str,
    target_kind: &str,
    target_id: &str,
    actor: &str,
    before: Value,
    after: Value,
    occurred_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_admin.operator_actions
            (id, actor, action, target_kind, target_id, before_json, after_json,
             occurred_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(actor.trim())
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(before)
    .bind(after)
    .bind(occurred_at)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn hold_from_row(row: &PgRow) -> Result<LegalHold> {
    Ok(LegalHold {
        id: row.try_get("id")?,
        target_kind: row.try_get("target_kind")?,
        target_id: row.try_get("target_id")?,
        data_categories: row.try_get("data_categories")?,
        basis: row.try_get("basis")?,
        release_condition: row.try_get("release_condition")?,
        started_by: row.try_get("started_by")?,
        started_at: row.try_get("started_at")?,
    })
}
