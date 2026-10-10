use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use kukuri_cn_core::{
    RetentionPolicy, cleanup_expired, configure_case_retention, initialize_database,
};
use sqlx::PgPool;

use crate::RetentionAction;

pub(super) async fn run(pool: &PgPool, action: RetentionAction) -> Result<()> {
    initialize_database(pool).await?;
    match action {
        RetentionAction::Sweep { now } => {
            let now = now
                .as_deref()
                .map(DateTime::parse_from_rfc3339)
                .transpose()
                .context("--now must be RFC3339")?
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);
            configure_case_retention(pool, &retention_policy()?).await?;
            let counts = cleanup_expired(pool, now).await?;
            println!("{}", serde_json::to_string(&counts)?);
        }
    }
    Ok(())
}

fn retention_policy() -> Result<RetentionPolicy> {
    let path = std::env::var_os("COMMUNITY_NODE_OPERATOR_CONFIG")
        .context("COMMUNITY_NODE_OPERATOR_CONFIG is required for retention operations")?;
    let yaml = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read operator config `{}`",
            path.to_string_lossy()
        )
    })?;
    let resolved = kukuri_cn_operator::load_and_validate(&yaml)?;
    let value = &resolved.raw.retention;
    Ok(RetentionPolicy {
        report_days: value.report_days,
        report_contact_days: value.report_contact_days,
        tester_feedback_days: value.tester_feedback_days,
        rights_request_active_days: value.rights_request_active_days,
        rights_request_resolved_days: value.rights_request_resolved_days,
        rights_request_rejected_days: value.rights_request_rejected_days,
        rights_request_contact_days: value.rights_request_contact_days,
        rights_request_identity_days: value.rights_request_identity_days,
        rights_request_evidence_days: value.rights_request_evidence_days,
        rights_request_history_days: value.rights_request_history_days,
        operator_audit_days: value.operator_audit_days,
        moderation_event_days: value.moderation_event_days,
        risk_signal_days: value.risk_signal_days,
    })
}
