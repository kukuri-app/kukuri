//! Durable, node-local legal transmission-prevention decisions (#761).

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{IndexScopeKind, OperatorAction};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransmissionPreventionBasis {
    Copyright,
    Privacy,
    PersonalityRights,
    Trademark,
    OtherRights,
}

impl TransmissionPreventionBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Copyright => "copyright",
            Self::Privacy => "privacy",
            Self::PersonalityRights => "personality_rights",
            Self::Trademark => "trademark",
            Self::OtherRights => "other_rights",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "copyright" => Ok(Self::Copyright),
            "privacy" => Ok(Self::Privacy),
            "personality_rights" => Ok(Self::PersonalityRights),
            "trademark" => Ok(Self::Trademark),
            "other_rights" => Ok(Self::OtherRights),
            _ => bail!("unknown transmission-prevention basis `{value}`"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransmissionPreventionCapability {
    CommunityIndex,
    Search,
    Discovery,
    Recommendation,
    Moderation,
    BlobCache,
}

impl TransmissionPreventionCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommunityIndex => "community_index",
            Self::Search => "search",
            Self::Discovery => "discovery",
            Self::Recommendation => "recommendation",
            Self::Moderation => "moderation",
            Self::BlobCache => "blob_cache",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "community_index" => Ok(Self::CommunityIndex),
            "search" => Ok(Self::Search),
            "discovery" => Ok(Self::Discovery),
            "recommendation" => Ok(Self::Recommendation),
            "moderation" => Ok(Self::Moderation),
            "blob_cache" => Ok(Self::BlobCache),
            _ => bail!("unknown transmission-prevention capability `{value}`"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewTransmissionPrevention {
    pub subject_kind: String,
    pub subject_id: String,
    pub basis: TransmissionPreventionBasis,
    pub capabilities: Vec<TransmissionPreventionCapability>,
    pub expires_at: Option<DateTime<Utc>>,
    pub related_report_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransmissionPrevention {
    pub id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub basis: TransmissionPreventionBasis,
    pub capabilities: Vec<TransmissionPreventionCapability>,
    pub decided_by: String,
    pub decided_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub related_report_id: Option<String>,
    pub released_at: Option<DateTime<Utc>>,
    pub released_by: Option<String>,
    pub release_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransmissionPreventionMutation {
    pub decision: TransmissionPrevention,
    pub audit: OperatorAction,
    pub removed_index_scopes: Vec<(IndexScopeKind, String)>,
}

pub async fn apply_transmission_prevention(
    pool: &PgPool,
    actor: &str,
    input: &NewTransmissionPrevention,
) -> Result<TransmissionPreventionMutation> {
    validate_actor(actor)?;
    validate_input(input)?;
    let mut tx = pool.begin().await?;
    let mutation = apply_transmission_prevention_in_tx(&mut tx, actor, input).await?;
    tx.commit().await?;
    Ok(mutation)
}

pub(crate) async fn apply_transmission_prevention_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor: &str,
    input: &NewTransmissionPrevention,
) -> Result<TransmissionPreventionMutation> {
    validate_actor(actor)?;
    validate_input(input)?;
    lock_subject(tx, &input.subject_id).await?;
    if let Some(current) = active_in_tx(tx, &input.subject_kind, &input.subject_id).await? {
        let before = audit_snapshot(&current);
        let expired = sqlx::query(
            "UPDATE cn_legal.transmission_preventions
             SET released_at = expires_at, released_by = 'system:expiry',
                 release_reason = 'expired'
             WHERE id = $1 AND expires_at IS NOT NULL AND expires_at <= NOW()
             RETURNING *",
        )
        .bind(&current.id)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(row) = expired {
            let expired_decision = from_row(&row)?;
            append_audit(
                tx,
                "system:expiry",
                "transmission_prevention.expire",
                &current.subject_kind,
                &current.subject_id,
                before,
                audit_snapshot(&expired_decision),
            )
            .await?;
        } else {
            bail!("an active transmission-prevention decision already exists for this subject");
        }
    }
    let capabilities = input
        .capabilities
        .iter()
        .map(|value| value.as_str())
        .collect::<Vec<_>>();
    let row = sqlx::query(
        "INSERT INTO cn_legal.transmission_preventions
            (id, subject_kind, subject_id, basis_category, capabilities, decided_by,
             expires_at, related_report_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(input.subject_kind.trim())
    .bind(input.subject_id.trim())
    .bind(input.basis.as_str())
    .bind(&capabilities)
    .bind(actor.trim())
    .bind(input.expires_at)
    .bind(input.related_report_id.as_deref())
    .fetch_one(&mut **tx)
    .await?;
    let decision = from_row(&row)?;
    let mut removed_index_scopes = Vec::new();
    if input.subject_kind == "post" && controls_index_surfaces(&input.capabilities) {
        // ArcadeDB の写しは消し待ちに入れ、indexer の巡回が消す（#1698）。
        for row in sqlx::query(
            "WITH removed AS (
                 DELETE FROM cn_index.index_entries WHERE object_id = $1
                 RETURNING scope_kind, scope_id, object_id
             ), queued AS (
                 INSERT INTO cn_index.projection_evictions (scope_kind, scope_id, object_id)
                 SELECT scope_kind, scope_id, object_id FROM removed ON CONFLICT DO NOTHING
             )
             SELECT scope_kind, scope_id FROM removed",
        )
        .bind(input.subject_id.trim())
        .fetch_all(&mut **tx)
        .await?
        {
            removed_index_scopes.push((
                IndexScopeKind::parse(&row.try_get::<String, _>("scope_kind")?)?,
                row.try_get("scope_id")?,
            ));
        }
    }
    let audit = append_audit(
        tx,
        actor,
        "transmission_prevention.apply",
        &input.subject_kind,
        &input.subject_id,
        json!({ "active": false }),
        audit_snapshot(&decision),
    )
    .await?;
    Ok(TransmissionPreventionMutation {
        decision,
        audit,
        removed_index_scopes,
    })
}

pub async fn release_transmission_prevention(
    pool: &PgPool,
    actor: &str,
    subject_kind: &str,
    subject_id: &str,
    reason: &str,
) -> Result<TransmissionPreventionMutation> {
    validate_actor(actor)?;
    if reason.trim().is_empty() {
        bail!("release reason must not be empty");
    }
    let mut tx = pool.begin().await?;
    lock_subject(&mut tx, subject_id).await?;
    let current = active_in_tx(&mut tx, subject_kind, subject_id)
        .await?
        .context("active transmission-prevention decision was not found")?;
    let before = audit_snapshot(&current);
    let row = sqlx::query(
        "UPDATE cn_legal.transmission_preventions
         SET released_at = NOW(), released_by = $2, release_reason = $3
         WHERE id = $1 RETURNING *",
    )
    .bind(&current.id)
    .bind(actor.trim())
    .bind(reason.trim())
    .fetch_one(&mut *tx)
    .await?;
    let decision = from_row(&row)?;
    let audit = append_audit(
        &mut tx,
        actor,
        "transmission_prevention.release",
        subject_kind,
        subject_id,
        before,
        audit_snapshot(&decision),
    )
    .await?;
    tx.commit().await?;
    Ok(TransmissionPreventionMutation {
        decision,
        audit,
        removed_index_scopes: Vec::new(),
    })
}

pub async fn get_active_transmission_prevention(
    pool: &PgPool,
    subject_kind: &str,
    subject_id: &str,
) -> Result<Option<TransmissionPrevention>> {
    let row = sqlx::query(
        "SELECT * FROM cn_legal.transmission_preventions
         WHERE subject_kind = $1 AND subject_id = $2 AND released_at IS NULL
           AND (expires_at IS NULL OR expires_at > NOW())
         ORDER BY decided_at DESC LIMIT 1",
    )
    .bind(subject_kind)
    .bind(subject_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(from_row).transpose()
}

pub async fn is_transmission_prevented(
    pool: &PgPool,
    subject_kind: &str,
    subject_id: &str,
    capability: TransmissionPreventionCapability,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cn_legal.transmission_preventions
         WHERE subject_kind = $1 AND subject_id = $2 AND released_at IS NULL
           AND (expires_at IS NULL OR expires_at > NOW()) AND $3 = ANY(capabilities))",
    )
    .bind(subject_kind)
    .bind(subject_id)
    .bind(capability.as_str())
    .fetch_one(pool)
    .await?)
}

pub async fn is_transmission_prevented_for_any(
    pool: &PgPool,
    subject_kind: &str,
    subject_id: &str,
    capabilities: &[TransmissionPreventionCapability],
) -> Result<bool> {
    let capabilities = capabilities
        .iter()
        .map(|capability| capability.as_str())
        .collect::<Vec<_>>();
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cn_legal.transmission_preventions
         WHERE subject_kind = $1 AND subject_id = $2 AND released_at IS NULL
           AND (expires_at IS NULL OR expires_at > NOW()) AND capabilities && $3)",
    )
    .bind(subject_kind)
    .bind(subject_id)
    .bind(capabilities)
    .fetch_one(pool)
    .await?)
}

async fn active_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    subject_kind: &str,
    subject_id: &str,
) -> Result<Option<TransmissionPrevention>> {
    let row = sqlx::query(
        "SELECT * FROM cn_legal.transmission_preventions
         WHERE subject_kind = $1 AND subject_id = $2 AND released_at IS NULL
         ORDER BY decided_at DESC LIMIT 1 FOR UPDATE",
    )
    .bind(subject_kind.trim())
    .bind(subject_id.trim())
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(from_row).transpose()
}

async fn lock_subject(tx: &mut Transaction<'_, Postgres>, subject_id: &str) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 761))")
        .bind(subject_id.trim())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn append_audit(
    tx: &mut Transaction<'_, Postgres>,
    actor: &str,
    action: &str,
    target_kind: &str,
    target_id: &str,
    before: serde_json::Value,
    after: serde_json::Value,
) -> Result<OperatorAction> {
    let row = sqlx::query(
        "INSERT INTO cn_admin.operator_actions
            (id, actor, action, target_kind, target_id, before_json, after_json)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, occurred_at, actor, action, target_kind, target_id, before_json, after_json",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(actor.trim())
    .bind(action)
    .bind(target_kind.trim())
    .bind(target_id.trim())
    .bind(before)
    .bind(after)
    .fetch_one(&mut **tx)
    .await?;
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

fn validate_actor(actor: &str) -> Result<()> {
    if actor.trim().is_empty() || actor.len() > 320 {
        bail!("actor must be present and at most 320 characters");
    }
    Ok(())
}

fn validate_input(input: &NewTransmissionPrevention) -> Result<()> {
    if !matches!(input.subject_kind.trim(), "post" | "blob") {
        bail!("subject_kind must be post or blob");
    }
    if input.subject_id.trim().is_empty() || input.subject_id.len() > 512 {
        bail!("subject_id must be present and at most 512 characters");
    }
    if input.capabilities.is_empty() {
        bail!("capabilities must not be empty");
    }
    Ok(())
}

fn controls_index_surfaces(capabilities: &[TransmissionPreventionCapability]) -> bool {
    capabilities.iter().any(|value| {
        matches!(
            value,
            TransmissionPreventionCapability::CommunityIndex
                | TransmissionPreventionCapability::Search
                | TransmissionPreventionCapability::Discovery
                | TransmissionPreventionCapability::Recommendation
        )
    })
}

fn from_row(row: &PgRow) -> Result<TransmissionPrevention> {
    let capabilities = row
        .try_get::<Vec<String>, _>("capabilities")?
        .iter()
        .map(|value| TransmissionPreventionCapability::parse(value))
        .collect::<Result<Vec<_>>>()?;
    Ok(TransmissionPrevention {
        id: row.try_get("id")?,
        subject_kind: row.try_get("subject_kind")?,
        subject_id: row.try_get("subject_id")?,
        basis: TransmissionPreventionBasis::parse(&row.try_get::<String, _>("basis_category")?)?,
        capabilities,
        decided_by: row.try_get("decided_by")?,
        decided_at: row.try_get("decided_at")?,
        expires_at: row.try_get("expires_at")?,
        related_report_id: row.try_get("related_report_id")?,
        released_at: row.try_get("released_at")?,
        released_by: row.try_get("released_by")?,
        release_reason: row.try_get("release_reason")?,
    })
}

fn audit_snapshot(value: &TransmissionPrevention) -> serde_json::Value {
    json!({
        "active": value.released_at.is_none(),
        "basis": value.basis.as_str(),
        "capabilities": value.capabilities.iter().map(|item| item.as_str()).collect::<Vec<_>>(),
        "expires_at": value.expires_at,
        "related_report_id": value.related_report_id,
        "released_at": value.released_at,
        "release_reason": value.release_reason,
    })
}
