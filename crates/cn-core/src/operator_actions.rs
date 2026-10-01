//! IAP 内部 admin surface の runtime 操作と append-only audit。
//!
//! deployment config / credential は扱わず、Postgres が canonical source の操作だけを
//! mutation と audit の同一 transaction で適用する（ADR 0029）。

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use uuid::Uuid;

use crate::config::COMMUNITY_NODE_ADMISSION_SERVICE_NAME;
use crate::{AdmissionConfig, AdmissionMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorReportStatus {
    Received,
    Reviewing,
    Actioned,
    Dismissed,
}

impl OperatorReportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Reviewing => "reviewing",
            Self::Actioned => "actioned",
            Self::Dismissed => "dismissed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdminOperation {
    SetAdmissionMode {
        mode: AdmissionMode,
    },
    AddSupportedPublicTopic {
        topic_id: String,
    },
    RemoveSupportedPublicTopic {
        topic_id: String,
    },
    SetReportStatus {
        report_id: String,
        status: OperatorReportStatus,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorAction {
    pub id: String,
    pub occurred_at: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub before: Value,
    pub after: Value,
}

pub async fn apply_operator_action(
    pool: &PgPool,
    actor: &str,
    operation: &AdminOperation,
) -> Result<OperatorAction> {
    let actor = actor.trim();
    if actor.is_empty() {
        bail!("admin operation actor must not be empty");
    }
    if actor.len() > 320 {
        bail!("admin operation actor is too long");
    }
    validate_admin_operation(operation)?;

    let mut tx = pool.begin().await?;
    let (action, target_kind, target_id, before, after) = match operation {
        AdminOperation::SetAdmissionMode { mode } => {
            let current = sqlx::query_scalar::<_, Value>(
                "SELECT config_json FROM cn_admin.service_configs WHERE service_name = $1 FOR UPDATE",
            )
            .bind(COMMUNITY_NODE_ADMISSION_SERVICE_NAME)
            .fetch_optional(&mut *tx)
            .await?
            .map(serde_json::from_value::<AdmissionConfig>)
            .transpose()
            .context("failed to parse current admission config")?
            .unwrap_or_default();
            let next = AdmissionConfig { mode: *mode };
            sqlx::query(
                "INSERT INTO cn_admin.service_configs (service_name, version, config_json)
                 VALUES ($1, 1, $2)
                 ON CONFLICT (service_name) DO UPDATE
                 SET version = cn_admin.service_configs.version + 1,
                     config_json = EXCLUDED.config_json,
                     updated_at = NOW()",
            )
            .bind(COMMUNITY_NODE_ADMISSION_SERVICE_NAME)
            .bind(serde_json::to_value(&next)?)
            .execute(&mut *tx)
            .await?;
            (
                "admission.set_mode",
                "admission",
                COMMUNITY_NODE_ADMISSION_SERVICE_NAME.to_string(),
                json!({ "mode": current.mode.as_str() }),
                json!({ "mode": next.mode.as_str() }),
            )
        }
        AdminOperation::AddSupportedPublicTopic { topic_id } => {
            let topic_id = validate_public_topic_id(topic_id)?;
            let result = sqlx::query(
                "INSERT INTO cn_index.supported_topics (id, kind)
                 VALUES ($1, 'public_topic')
                 ON CONFLICT (kind, id) DO NOTHING",
            )
            .bind(topic_id)
            .execute(&mut *tx)
            .await?;
            let existed = result.rows_affected() == 0;
            (
                "supported_topic.add",
                "supported_public_topic",
                topic_id.to_string(),
                json!({ "present": existed }),
                json!({ "present": true }),
            )
        }
        AdminOperation::RemoveSupportedPublicTopic { topic_id } => {
            let topic_id = validate_public_topic_id(topic_id)?;
            let result = sqlx::query(
                "DELETE FROM cn_index.supported_topics
                 WHERE kind = 'public_topic' AND id = $1",
            )
            .bind(topic_id)
            .execute(&mut *tx)
            .await?;
            let existed = result.rows_affected() > 0;
            (
                "supported_topic.remove",
                "supported_public_topic",
                topic_id.to_string(),
                json!({ "present": existed }),
                json!({ "present": false }),
            )
        }
        AdminOperation::SetReportStatus { report_id, status } => {
            let report_id = report_id.trim();
            if report_id.is_empty() {
                bail!("report id must not be empty");
            }
            let current = sqlx::query_scalar::<_, String>(
                "SELECT status FROM cn_admin.reports
                 WHERE id = $1 AND expires_at > NOW() FOR UPDATE",
            )
            .bind(report_id)
            .fetch_optional(&mut *tx)
            .await?
            .with_context(|| format!("report `{report_id}` was not found"))?;
            sqlx::query("UPDATE cn_admin.reports SET status = $2 WHERE id = $1")
                .bind(report_id)
                .bind(status.as_str())
                .execute(&mut *tx)
                .await?;
            (
                "report.set_status",
                "report",
                report_id.to_string(),
                json!({ "status": current }),
                json!({ "status": status.as_str() }),
            )
        }
    };

    let id = Uuid::new_v4().to_string();
    let row = sqlx::query(
        "INSERT INTO cn_admin.operator_actions
            (id, actor, action, target_kind, target_id, before_json, after_json)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, occurred_at, actor, action, target_kind, target_id, before_json, after_json",
    )
    .bind(&id)
    .bind(actor)
    .bind(action)
    .bind(target_kind)
    .bind(&target_id)
    .bind(before)
    .bind(after)
    .fetch_one(&mut *tx)
    .await?;
    let result = operator_action_from_row(&row)?;
    tx.commit().await?;
    Ok(result)
}

pub fn validate_admin_operation(operation: &AdminOperation) -> Result<()> {
    match operation {
        AdminOperation::SetAdmissionMode { .. } => Ok(()),
        AdminOperation::AddSupportedPublicTopic { topic_id }
        | AdminOperation::RemoveSupportedPublicTopic { topic_id } => {
            validate_public_topic_id(topic_id).map(|_| ())
        }
        AdminOperation::SetReportStatus { report_id, .. } => {
            let report_id = report_id.trim();
            if report_id.is_empty() {
                bail!("report id must not be empty");
            }
            if report_id.len() > 128 {
                bail!("report id is too long");
            }
            Ok(())
        }
    }
}

pub async fn list_operator_actions(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<OperatorAction>> {
    let rows = sqlx::query(
        "SELECT id, occurred_at, actor, action, target_kind, target_id, before_json, after_json
         FROM cn_admin.operator_actions
         WHERE expires_at > NOW()
         ORDER BY occurred_at DESC, id DESC
         LIMIT $1 OFFSET $2",
    )
    .bind(limit.clamp(1, 200))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await?;
    rows.iter().map(operator_action_from_row).collect()
}

fn validate_public_topic_id(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() {
        bail!("supported topic id must not be empty");
    }
    if value.len() > 512 {
        bail!("supported topic id is too long");
    }
    if value.chars().any(char::is_control) {
        bail!("supported topic id must not contain control characters");
    }
    // 公開ではない topic（account 同期の hint 等）は対応 topic にしない（ADR 0061 §6）。
    if kukuri_core::wire::is_non_public_topic(value) {
        bail!("supported topic id is not a public topic");
    }
    Ok(value)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_topic_validation_matches_the_existing_opaque_scope_id_contract() {
        assert!(validate_public_topic_id("").is_err());
        assert!(validate_public_topic_id("demo\nother").is_err());
        assert_eq!(validate_public_topic_id(" demo ").unwrap(), "demo");
        assert_eq!(
            validate_public_topic_id(" kukuri:topic:demo ").unwrap(),
            "kukuri:topic:demo"
        );
        // 本人の端末間の account 同期の hint（ADR 0061 §6）は公開 topic ではない。
        assert!(validate_public_topic_id("kukuri:account:00ff").is_err());
        assert!(validate_public_topic_id("hint/kukuri:account:00ff").is_err());
    }

    #[test]
    fn operation_validation_rejects_invalid_preview_inputs() {
        assert!(
            validate_admin_operation(&AdminOperation::AddSupportedPublicTopic {
                topic_id: "\n".to_string(),
            })
            .is_err()
        );
        assert!(
            validate_admin_operation(&AdminOperation::SetReportStatus {
                report_id: " ".to_string(),
                status: OperatorReportStatus::Reviewing,
            })
            .is_err()
        );
    }
}
