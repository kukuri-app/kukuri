//! Community Node の区分別保持と期限削除。
//!
//! 期限は行に持たず、起算点の列と保持区分ごとの日数（`cn_admin.retention_days`）で判定する
//! （ADR 0034 §1）。読取りは「起算点 > 基準時刻 − `cn_admin.retention_interval(区分)`」で期限切れを
//! 除く。日数は起動時と `cn-cli retention sweep` が書くだけなので、変えると既存の行にもすぐ効き、
//! 行は書き直さない。期限削除は 1 取引に保持区分ごと [`RETENTION_CLEANUP_BATCH`] 件までの小分けで進める。

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPool;

/// 期限削除が 1 取引で保持区分ごとに消す行の上限。
pub const RETENTION_CLEANUP_BATCH: i64 = 128;

/// 上限まで消せた間だけ次の取引へ進む（案件と観測の保持処理で共用）。
macro_rules! cleanup_batches {
    ($batch:expr) => {
        while $batch {}
    };
}
pub(crate) use cleanup_batches;

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

/// operator config の保持日数を、保持区分ごとの日数の行へ書く。書く行の数は区分の数で決まる。
pub async fn configure_case_retention(pool: &PgPool, policy: &RetentionPolicy) -> Result<()> {
    let (categories, days): (Vec<&str>, Vec<i32>) = [
        ("report", policy.report_days),
        ("report_contact", policy.report_contact_days),
        ("tester_feedback", policy.tester_feedback_days),
        ("rights_request_active", policy.rights_request_active_days),
        (
            "rights_request_resolved",
            policy.rights_request_resolved_days,
        ),
        (
            "rights_request_rejected",
            policy.rights_request_rejected_days,
        ),
        ("rights_request_contact", policy.rights_request_contact_days),
        (
            "rights_request_identity",
            policy.rights_request_identity_days,
        ),
        (
            "rights_request_evidence",
            policy.rights_request_evidence_days,
        ),
        ("rights_request_history", policy.rights_request_history_days),
        ("operator_audit", policy.operator_audit_days),
        ("moderation_event", policy.moderation_event_days),
        ("risk_signal", policy.risk_signal_days),
    ]
    .into_iter()
    .map(|(category, days)| (category, days as i32))
    .unzip();
    sqlx::query(
        "UPDATE cn_admin.retention_days SET days = given.days
         FROM UNNEST($1::TEXT[], $2::INTEGER[]) AS given (category, days)
         WHERE retention_days.category = given.category",
    )
    .bind(&categories)
    .bind(&days)
    .execute(pool)
    .await?;
    Ok(())
}

const EXPIRED_SENSITIVE_ITEMS: &str = "DELETE FROM cn_legal.sensitive_items WHERE ctid = ANY(ARRAY(
     SELECT item.ctid FROM cn_legal.sensitive_items item
     WHERE item.data_category = $2
       AND item.created_at <= $1 - cn_admin.retention_interval($2)
       AND NOT EXISTS (
         SELECT 1 FROM cn_legal.legal_holds hold
         WHERE hold.target_kind = item.owner_kind
           AND hold.target_id = item.owner_id
           AND item.data_category = ANY(hold.data_categories)
       )
     ORDER BY item.created_at LIMIT $3))";

const EXPIRED_RIGHTS_REQUESTS: &str = "DELETE FROM cn_legal.rights_requests WHERE ctid = ANY(ARRAY(
     SELECT request.ctid FROM cn_legal.rights_requests request
     WHERE cn_legal.rights_request_retention(request.status) = $2
       AND request.updated_at <= $1 - cn_admin.retention_interval($2)
       AND NOT EXISTS (
         SELECT 1 FROM cn_legal.legal_holds hold
         WHERE hold.target_kind = 'rights_request'
           AND hold.target_id = request.id
       )
     ORDER BY request.updated_at LIMIT $3))";

/// 表ごとの期限削除の文と、その表の保持区分（子から親の順）。`$1` は基準時刻、`$2` は保持区分、`$3` は
/// 消す行の上限で、どの文も起算点の索引の範囲から期限切れの行を古い順に上限まで選び、その行の位置
/// （`ctid`）で消す（主キーの索引を引かない）。
pub const EXPIRED_DELETES: [(&[&str], &str); 8] = [
    (
        &[
            "report_contact",
            "rights_request_contact",
            "rights_request_identity",
            "rights_request_evidence",
        ],
        EXPIRED_SENSITIVE_ITEMS,
    ),
    (
        &["rights_request_history"],
        "DELETE FROM cn_legal.rights_request_events WHERE ctid = ANY(ARRAY(
         SELECT event.ctid FROM cn_legal.rights_request_events event
         WHERE event.occurred_at <= $1 - cn_admin.retention_interval($2)
           AND NOT EXISTS (
             SELECT 1 FROM cn_legal.legal_holds hold
             WHERE hold.target_kind = 'rights_request'
               AND hold.target_id = event.request_id
               AND 'rights_request_history' = ANY(hold.data_categories)
           )
         ORDER BY event.occurred_at LIMIT $3))",
    ),
    (
        &["report"],
        "DELETE FROM cn_admin.reports WHERE ctid = ANY(ARRAY(
         SELECT report.ctid FROM cn_admin.reports report
         WHERE report.created_at <= $1 - cn_admin.retention_interval($2)
           AND NOT EXISTS (
             SELECT 1 FROM cn_legal.legal_holds hold
             WHERE hold.target_kind = 'report'
               AND hold.target_id = report.id
               AND 'report' = ANY(hold.data_categories)
           )
         ORDER BY report.created_at LIMIT $3))",
    ),
    (
        &["tester_feedback"],
        "DELETE FROM cn_admin.tester_feedback WHERE ctid = ANY(ARRAY(
         SELECT ctid FROM cn_admin.tester_feedback
         WHERE created_at <= $1 - cn_admin.retention_interval($2)
         ORDER BY created_at LIMIT $3))",
    ),
    (
        &[
            "rights_request_active",
            "rights_request_resolved",
            "rights_request_rejected",
        ],
        EXPIRED_RIGHTS_REQUESTS,
    ),
    (
        &["operator_audit"],
        "DELETE FROM cn_admin.operator_actions WHERE ctid = ANY(ARRAY(
         SELECT action.ctid FROM cn_admin.operator_actions action
         WHERE action.occurred_at <= $1 - cn_admin.retention_interval($2)
           AND NOT EXISTS (
             SELECT 1 FROM cn_legal.legal_holds hold
             WHERE hold.target_kind = action.target_kind
               AND hold.target_id = action.target_id
               AND 'operator_audit' = ANY(hold.data_categories)
           )
         ORDER BY action.occurred_at LIMIT $3))",
    ),
    (
        &["moderation_event"],
        "DELETE FROM cn_safety.signed_moderation_events WHERE ctid = ANY(ARRAY(
         SELECT ctid FROM cn_safety.signed_moderation_events
         WHERE persisted_at <= $1 - cn_admin.retention_interval($2)
         ORDER BY persisted_at LIMIT $3))",
    ),
    (
        &["risk_signal"],
        "DELETE FROM cn_safety.risk_signals WHERE ctid = ANY(ARRAY(
         SELECT signal.ctid FROM cn_safety.risk_signals signal
         WHERE signal.persisted_at <= $1 - cn_admin.retention_interval($2)
           AND NOT EXISTS (
             SELECT 1 FROM cn_admin.reports report
             WHERE report.appeal_risk_signal_id = signal.id
           )
         ORDER BY signal.persisted_at LIMIT $3))",
    ),
];

/// 期限切れ（hold の外）を、1 取引で保持区分ごとに `limit` 件まで子から親の順に消し、`counts` に足す。
/// 上限まで消せた区分があれば（残りがあるかもしれないので）`true` を返す。
pub async fn delete_expired_batch(
    pool: &PgPool,
    now: DateTime<Utc>,
    limit: i64,
    counts: &mut CleanupCounts,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('kukuri.retention_cleanup', 'on', true)")
        .execute(&mut *tx)
        .await?;
    let tables = [
        &mut counts.sensitive_items,
        &mut counts.rights_request_events,
        &mut counts.reports,
        &mut counts.tester_feedback,
        &mut counts.rights_requests,
        &mut counts.operator_actions,
        &mut counts.moderation_events,
        &mut counts.risk_signals,
    ];
    let mut more = false;
    for (count, (categories, sql)) in tables.into_iter().zip(EXPIRED_DELETES) {
        for category in categories {
            let deleted = sqlx::query(sql)
                .bind(now)
                .bind(category)
                .bind(limit)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            more |= deleted == limit as u64;
            *count += deleted;
        }
    }
    tx.commit().await?;
    Ok(more)
}

/// 期限切れ（hold の外）を、上限まで消せた区分がある間、1 取引に保持区分ごと
/// [`RETENTION_CLEANUP_BATCH`] 件ずつ消す。消した件数の合計を返す。
pub async fn cleanup_expired(pool: &PgPool, now: DateTime<Utc>) -> Result<CleanupCounts> {
    let mut counts = CleanupCounts::default();
    cleanup_batches!(delete_expired_batch(pool, now, RETENTION_CLEANUP_BATCH, &mut counts).await?);
    Ok(counts)
}
