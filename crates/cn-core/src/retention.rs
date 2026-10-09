//! Community Node の区分別保持と期限削除。

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::postgres::PgPool;

use kukuri_cn_protocol::RightsRequestStatus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub report_days: u32,
    pub report_contact_days: u32,
    pub tester_feedback_days: u32,
    pub rights_request_active_days: u32,
    pub rights_request_resolved_days: u32,
    pub rights_request_rejected_days: u32,
    pub rights_request_contact_days: u32,
    pub rights_request_identity_days: u32,
    pub rights_request_evidence_days: u32,
    pub rights_request_history_days: u32,
    pub operator_audit_days: u32,
    pub moderation_event_days: u32,
    pub risk_signal_days: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            report_days: 180,
            report_contact_days: 90,
            tester_feedback_days: 180,
            rights_request_active_days: 730,
            rights_request_resolved_days: 365,
            rights_request_rejected_days: 180,
            rights_request_contact_days: 180,
            rights_request_identity_days: 180,
            rights_request_evidence_days: 180,
            rights_request_history_days: 365,
            operator_audit_days: 365,
            moderation_event_days: 180,
            risk_signal_days: 180,
        }
    }
}

impl RetentionPolicy {
    pub fn expiry(&self, at: DateTime<Utc>, days: u32) -> DateTime<Utc> {
        at + Duration::days(i64::from(days))
    }

    pub fn rights_request_days(&self, status: RightsRequestStatus) -> u32 {
        match status {
            RightsRequestStatus::Actioned => self.rights_request_resolved_days,
            RightsRequestStatus::Declined
            | RightsRequestStatus::OutOfScope
            | RightsRequestStatus::Withdrawn => self.rights_request_rejected_days,
            RightsRequestStatus::Received
            | RightsRequestStatus::NeedsInformation
            | RightsRequestStatus::Reviewing
            | RightsRequestStatus::SenderContacting => self.rights_request_active_days,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupCounts {
    pub sensitive_items: u64,
    pub rights_request_events: u64,
    pub reports: u64,
    pub tester_feedback: u64,
    pub rights_requests: u64,
    pub operator_actions: u64,
    pub moderation_events: u64,
    pub risk_signals: u64,
}

/// 現在の operator 設定を各行の絶対期限へ反映する。
///
/// 期限は immutable な作成時刻（rights request だけは最終状態遷移時刻）から再計算し、
/// 定期実行しても延長されない。append-only table は migration がこの transaction-local
/// opt-in に限って期限列だけの更新を許可する。
pub async fn apply_retention_policy(pool: &PgPool, policy: &RetentionPolicy) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('kukuri.retention_reconcile', 'on', true)")
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "UPDATE cn_admin.reports
         SET expires_at = created_at + make_interval(days => $1)",
    )
    .bind(policy.report_days as i32)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE cn_admin.tester_feedback
         SET expires_at = created_at + make_interval(days => $1)",
    )
    .bind(policy.tester_feedback_days as i32)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE cn_legal.sensitive_items
         SET expires_at = created_at + make_interval(days => CASE data_category
           WHEN 'report_contact' THEN $1
           WHEN 'rights_request_contact' THEN $2
           WHEN 'rights_request_identity' THEN $3
           WHEN 'rights_request_evidence' THEN $4
         END)",
    )
    .bind(policy.report_contact_days as i32)
    .bind(policy.rights_request_contact_days as i32)
    .bind(policy.rights_request_identity_days as i32)
    .bind(policy.rights_request_evidence_days as i32)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE cn_legal.rights_requests
         SET expires_at = updated_at + make_interval(days => CASE
           WHEN status = 'actioned' THEN $1
           WHEN status IN ('declined', 'out_of_scope', 'withdrawn') THEN $2
           ELSE $3
         END)",
    )
    .bind(policy.rights_request_resolved_days as i32)
    .bind(policy.rights_request_rejected_days as i32)
    .bind(policy.rights_request_active_days as i32)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE cn_legal.rights_request_events
         SET expires_at = occurred_at + make_interval(days => $1)",
    )
    .bind(policy.rights_request_history_days as i32)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE cn_admin.operator_actions
         SET expires_at = occurred_at + make_interval(days => $1)",
    )
    .bind(policy.operator_audit_days as i32)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE cn_safety.signed_moderation_events
         SET retention_expires_at = persisted_at + make_interval(days => $1)",
    )
    .bind(policy.moderation_event_days as i32)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE cn_safety.risk_signals
         SET retention_expires_at = persisted_at + make_interval(days => $1)",
    )
    .bind(policy.risk_signal_days as i32)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn cleanup_expired(pool: &PgPool, now: DateTime<Utc>) -> Result<CleanupCounts> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('kukuri.retention_cleanup', 'on', true)")
        .execute(&mut *tx)
        .await?;

    let sensitive_items = sqlx::query(
        "DELETE FROM cn_legal.sensitive_items item
         WHERE item.expires_at <= $1
           AND NOT EXISTS (
             SELECT 1 FROM cn_legal.legal_holds hold
             WHERE hold.target_kind = item.owner_kind
               AND hold.target_id = item.owner_id
               AND item.data_category = ANY(hold.data_categories)
           )",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let rights_request_events = sqlx::query(
        "DELETE FROM cn_legal.rights_request_events event
         WHERE event.expires_at <= $1
           AND NOT EXISTS (
             SELECT 1 FROM cn_legal.legal_holds hold
             WHERE hold.target_kind = 'rights_request'
               AND hold.target_id = event.request_id
               AND 'rights_request_history' = ANY(hold.data_categories)
           )",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let reports = sqlx::query(
        "DELETE FROM cn_admin.reports report
         WHERE report.expires_at <= $1
           AND NOT EXISTS (
             SELECT 1 FROM cn_legal.legal_holds hold
             WHERE hold.target_kind = 'report'
               AND hold.target_id = report.id
               AND 'report' = ANY(hold.data_categories)
           )",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let tester_feedback =
        sqlx::query("DELETE FROM cn_admin.tester_feedback WHERE expires_at <= $1")
            .bind(now)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    let rights_requests = sqlx::query(
        "DELETE FROM cn_legal.rights_requests request
         WHERE request.expires_at <= $1
           AND NOT EXISTS (
             SELECT 1 FROM cn_legal.legal_holds hold
             WHERE hold.target_kind = 'rights_request'
               AND hold.target_id = request.id
           )",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let operator_actions = sqlx::query(
        "DELETE FROM cn_admin.operator_actions action
         WHERE action.expires_at <= $1
           AND NOT EXISTS (
             SELECT 1 FROM cn_legal.legal_holds hold
             WHERE hold.target_kind = action.target_kind
               AND hold.target_id = action.target_id
               AND 'operator_audit' = ANY(hold.data_categories)
           )",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let moderation_events = sqlx::query(
        "DELETE FROM cn_safety.signed_moderation_events WHERE retention_expires_at <= $1",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let risk_signals = sqlx::query(
        "DELETE FROM cn_safety.risk_signals signal
         WHERE retention_expires_at <= $1
           AND NOT EXISTS (
             SELECT 1 FROM cn_admin.reports report
             WHERE report.appeal_risk_signal_id = signal.id
           )",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    tx.commit().await?;
    Ok(CleanupCounts {
        sensitive_items,
        rights_request_events,
        reports,
        tester_feedback,
        rights_requests,
        operator_actions,
        moderation_events,
        risk_signals,
    })
}

pub async fn retention_counts(pool: &PgPool) -> Result<CleanupCounts> {
    let row = sqlx::query(
        "SELECT
           (SELECT COUNT(*) FROM cn_legal.sensitive_items) AS sensitive_items,
           (SELECT COUNT(*) FROM cn_legal.rights_request_events) AS rights_request_events,
           (SELECT COUNT(*) FROM cn_admin.reports) AS reports,
           (SELECT COUNT(*) FROM cn_admin.tester_feedback) AS tester_feedback,
           (SELECT COUNT(*) FROM cn_legal.rights_requests) AS rights_requests,
           (SELECT COUNT(*) FROM cn_admin.operator_actions) AS operator_actions,
           (SELECT COUNT(*) FROM cn_safety.signed_moderation_events) AS moderation_events,
           (SELECT COUNT(*) FROM cn_safety.risk_signals) AS risk_signals",
    )
    .fetch_one(pool)
    .await?;
    Ok(CleanupCounts {
        sensitive_items: row.try_get::<i64, _>("sensitive_items")? as u64,
        rights_request_events: row.try_get::<i64, _>("rights_request_events")? as u64,
        reports: row.try_get::<i64, _>("reports")? as u64,
        tester_feedback: row.try_get::<i64, _>("tester_feedback")? as u64,
        rights_requests: row.try_get::<i64, _>("rights_requests")? as u64,
        operator_actions: row.try_get::<i64, _>("operator_actions")? as u64,
        moderation_events: row.try_get::<i64, _>("moderation_events")? as u64,
        risk_signals: row.try_get::<i64, _>("risk_signals")? as u64,
    })
}
