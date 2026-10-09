//! community node の通報受信ストレージ（#370）。
//!
//! node は自分の authority scope 内の対象に対する通報のみ受理する（中央通報窓口ではない）。
//! 通報先の解決は client（#310）が provenance + manifest authority scope で行い、node 側は
//! 「report endpoint capability を有効化したかどうか」で受付可否を判断する。ここでは受理した
//! 通報の保存・参照のみを担う。
//!
//! reporter の identity / social graph は node-independent であり保持しない。明示的に入力された
//! 連絡先（任意）のみ保存する。

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use uuid::Uuid;

use crate::{
    LegalDataCipher, SensitiveDataCategory, load_sensitive_json, upsert_sensitive_json_in_tx,
};

/// 受信直後の通報状態。
pub const COMMUNITY_NODE_REPORT_STATUS_RECEIVED: &str = "received";

/// 保存済みの通報レコード。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityNodeReport {
    pub id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub capability: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporter_contact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appeal_risk_signal_id: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// 新規通報の入力。reporter identity / social graph は受け取らない。
#[derive(Clone, Debug, Default)]
pub struct NewCommunityNodeReport {
    pub subject_kind: String,
    pub subject_id: String,
    pub capability: String,
    pub reason: String,
    pub details: Option<String>,
    pub reporter_contact: Option<String>,
    pub appeal_risk_signal_id: Option<String>,
}

/// 受信した通報を保存し、受付参照 ID を含むレコードを返す。保持の起算点は保存した時刻。
pub async fn insert_community_node_report(
    pool: &PgPool,
    input: &NewCommunityNodeReport,
    cipher: Option<&LegalDataCipher>,
) -> Result<CommunityNodeReport> {
    if input.reporter_contact.is_some() && cipher.is_none() {
        bail!("legal data encryption key is required for reporter contact");
    }
    let id = Uuid::new_v4().to_string();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "INSERT INTO cn_admin.reports
            (id, subject_kind, subject_id, capability, reason, details, reporter_contact,
             appeal_risk_signal_id, status)
         VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8)
         RETURNING id, subject_kind, subject_id, capability, reason, details, reporter_contact,
                   appeal_risk_signal_id, status, created_at",
    )
    .bind(&id)
    .bind(&input.subject_kind)
    .bind(&input.subject_id)
    .bind(&input.capability)
    .bind(&input.reason)
    .bind(&input.details)
    .bind(&input.appeal_risk_signal_id)
    .bind(COMMUNITY_NODE_REPORT_STATUS_RECEIVED)
    .fetch_one(&mut *tx)
    .await?;
    if let (Some(cipher), Some(contact)) = (cipher, input.reporter_contact.as_ref()) {
        upsert_sensitive_json_in_tx(
            &mut tx,
            cipher,
            "report",
            &id,
            SensitiveDataCategory::ReportContact,
            contact,
        )
        .await?;
    }
    let report = report_from_row(&row)?;
    tx.commit().await?;
    Ok(report)
}

/// この node が発行したリスク判定への異議申し立てを、状態遷移と通報保存を一体で受理する。
pub async fn insert_community_node_appeal(
    pool: &PgPool,
    issuer_node_id: &str,
    risk_signal_id: &str,
    input: &NewCommunityNodeReport,
) -> Result<CommunityNodeReport> {
    let issuer_node_id = issuer_node_id.trim();
    if issuer_node_id.is_empty() {
        bail!("community node issuer id must not be empty");
    }
    let risk_signal_id = risk_signal_id.trim();
    if risk_signal_id.is_empty() {
        bail!("appeal risk signal id must not be empty");
    }

    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT issuer_node_id, target, target_id, COALESCE(appeal_status, 'none') AS appeal_status
         FROM cn_safety.risk_signals
         WHERE id = $1 AND persisted_at > NOW() - cn_admin.retention_interval('risk_signal')
         FOR UPDATE",
    )
    .bind(risk_signal_id)
    .fetch_optional(&mut *tx)
    .await?
    .with_context(|| format!("risk signal `{risk_signal_id}` was not found"))?;
    let stored_issuer: String = row.try_get("issuer_node_id")?;
    if stored_issuer != issuer_node_id {
        bail!("risk signal `{risk_signal_id}` was not issued by this community node");
    }
    let stored_target: String = row.try_get("target")?;
    let stored_target_id: String = row.try_get("target_id")?;
    if !appeal_subject_kind_matches(stored_target.as_str(), input.subject_kind.as_str())
        || stored_target_id != input.subject_id
    {
        bail!("appeal subject does not match risk signal `{risk_signal_id}`");
    }
    let appeal_status: String = row.try_get("appeal_status")?;
    match appeal_status.as_str() {
        "none" => {
            sqlx::query(
                "UPDATE cn_safety.risk_signals SET appeal_status = 'disputed' WHERE id = $1",
            )
            .bind(risk_signal_id)
            .execute(&mut *tx)
            .await?;
        }
        "disputed" => {}
        "cleared" => bail!("risk signal `{risk_signal_id}` is already cleared"),
        other => bail!("risk signal `{risk_signal_id}` has invalid appeal status `{other}`"),
    }

    let id = Uuid::new_v4().to_string();
    let row = sqlx::query(
        "INSERT INTO cn_admin.reports
            (id, subject_kind, subject_id, capability, reason, details, reporter_contact,
             appeal_risk_signal_id, status)
         VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8)
         RETURNING id, subject_kind, subject_id, capability, reason, details, reporter_contact,
                   appeal_risk_signal_id, status, created_at",
    )
    .bind(&id)
    .bind(&input.subject_kind)
    .bind(&input.subject_id)
    .bind(&input.capability)
    .bind(&input.reason)
    .bind(&input.details)
    .bind(risk_signal_id)
    .bind(COMMUNITY_NODE_REPORT_STATUS_RECEIVED)
    .fetch_one(&mut *tx)
    .await?;
    let report = report_from_row(&row)?;
    tx.commit().await?;
    Ok(report)
}

fn appeal_subject_kind_matches(target: &str, subject_kind: &str) -> bool {
    matches!(
        (target, subject_kind.trim()),
        ("user_pubkey", "profile")
            | ("peer_node", "peer_node")
            | ("post_id", "post")
            | ("blob_cid", "media")
    )
}

/// 受信した通報を新着順で取得する（運営者の確認用）。
pub async fn list_community_node_reports(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<CommunityNodeReport>> {
    let rows = sqlx::query(
        "SELECT id, subject_kind, subject_id, capability, reason, details, reporter_contact,
                appeal_risk_signal_id, status, created_at
         FROM cn_admin.reports
         WHERE created_at > NOW() - cn_admin.retention_interval('report')
         ORDER BY created_at DESC
         LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(report_from_row).collect()
}

/// 単一の通報を ID で取得する。
pub async fn get_community_node_report(
    pool: &PgPool,
    id: &str,
) -> Result<Option<CommunityNodeReport>> {
    let row = sqlx::query(
        "SELECT id, subject_kind, subject_id, capability, reason, details, reporter_contact,
                appeal_risk_signal_id, status, created_at
         FROM cn_admin.reports
         WHERE id = $1 AND created_at > NOW() - cn_admin.retention_interval('report')",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(report_from_row).transpose()
}

pub async fn get_community_node_report_with_contact(
    pool: &PgPool,
    cipher: &LegalDataCipher,
    id: &str,
    now: DateTime<Utc>,
) -> Result<Option<CommunityNodeReport>> {
    let Some(mut report) = get_community_node_report(pool, id).await? else {
        return Ok(None);
    };
    report.reporter_contact = load_sensitive_json(
        pool,
        cipher,
        "report",
        &report.id,
        SensitiveDataCategory::ReportContact,
        now,
    )
    .await?;
    Ok(Some(report))
}

pub async fn seal_legacy_report_contacts(pool: &PgPool, cipher: &LegalDataCipher) -> Result<u64> {
    let rows = sqlx::query(
        "SELECT id, reporter_contact FROM cn_admin.reports
         WHERE reporter_contact IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    let mut sealed = 0;
    for row in rows {
        let id: String = row.try_get("id")?;
        let contact: String = row.try_get("reporter_contact")?;
        let mut tx = pool.begin().await?;
        upsert_sensitive_json_in_tx(
            &mut tx,
            cipher,
            "report",
            &id,
            SensitiveDataCategory::ReportContact,
            &contact,
        )
        .await?;
        sqlx::query("UPDATE cn_admin.reports SET reporter_contact = NULL WHERE id = $1")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        sealed += 1;
    }
    Ok(sealed)
}

fn report_from_row(row: &PgRow) -> Result<CommunityNodeReport> {
    Ok(CommunityNodeReport {
        id: row.try_get("id")?,
        subject_kind: row.try_get("subject_kind")?,
        subject_id: row.try_get("subject_id")?,
        capability: row.try_get("capability")?,
        reason: row.try_get("reason")?,
        details: row.try_get("details")?,
        reporter_contact: row.try_get("reporter_contact")?,
        appeal_risk_signal_id: row.try_get("appeal_risk_signal_id")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
    })
}
