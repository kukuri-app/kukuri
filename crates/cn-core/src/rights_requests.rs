//! Durable, accountless rights-infringement requests (#760).

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use kukuri_cn_protocol::{
    RightsCategory, RightsRequestCreateRequest, RightsRequestScopeStatus, RightsRequestStatus,
    RightsRequestStatusResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::rights_request_sensitive::{hydrate_sensitive_request, split_sensitive_request};
use crate::transmission_preventions::apply_transmission_prevention_in_tx;
use crate::{
    LegalDataCipher, NewTransmissionPrevention, SensitiveDataCategory, TransmissionPreventionBasis,
    TransmissionPreventionCapability, TransmissionPreventionMutation, upsert_sensitive_json_in_tx,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RightsRequestRecord {
    pub id: String,
    #[serde(skip_serializing)]
    pub tracking_secret_hash: String,
    pub scope_revision: String,
    pub scope_status: RightsRequestScopeStatus,
    pub status: RightsRequestStatus,
    pub subject_kind: String,
    pub subject_id: String,
    pub requested_capabilities: Vec<String>,
    pub request: RightsRequestCreateRequest,
    pub version: i32,
    pub public_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RightsRequestEvent {
    pub id: String,
    pub request_id: String,
    pub actor: String,
    pub action: String,
    pub from_status: Option<RightsRequestStatus>,
    pub to_status: RightsRequestStatus,
    pub public_message: Option<String>,
    pub delivery_status: String,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedRightsRequest {
    pub record: RightsRequestRecord,
    /// 一度だけ呼び出し元へ返す。DB には hash だけを保存する。
    pub tracking_secret: String,
}

#[derive(Clone, Debug)]
pub struct RightsRequestActionResult {
    pub request: RightsRequestRecord,
    pub prevention: TransmissionPreventionMutation,
}

pub async fn resolve_rights_request_scope(
    pool: &PgPool,
    subject_kind: &str,
    subject_id: &str,
    requested_capabilities: &[String],
    available_capabilities: &[String],
) -> Result<RightsRequestScopeStatus> {
    let subject_kind = subject_kind.trim();
    let subject_id = subject_id.trim();
    if !matches!(subject_kind, "post" | "blob")
        || subject_id.is_empty()
        || requested_capabilities.is_empty()
    {
        return Ok(RightsRequestScopeStatus::OutOfScope);
    }

    let supports = |requested: &str| match requested {
        "community_index" | "search" | "discovery" | "recommendation" => available_capabilities
            .iter()
            .any(|value| value == "community_index"),
        "moderation" => available_capabilities
            .iter()
            .any(|value| value == "moderation"),
        "blob_cache" => available_capabilities
            .iter()
            .any(|value| value == "blob_cache"),
        _ => false,
    };
    if requested_capabilities
        .iter()
        .any(|value| !supports(value.as_str()))
    {
        return Ok(RightsRequestScopeStatus::OutOfScope);
    }

    let needs_index = requested_capabilities.iter().any(|value| {
        matches!(
            value.as_str(),
            "community_index" | "search" | "discovery" | "recommendation"
        )
    });
    let index_verified = !needs_index
        || (subject_kind == "post"
            && sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM cn_index.index_entries WHERE object_id = $1)",
            )
            .bind(subject_id)
            .fetch_one(pool)
            .await?);

    let needs_moderation = requested_capabilities
        .iter()
        .any(|value| value == "moderation");
    let moderation_verified = !needs_moderation
        || sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cn_safety.scan_verdicts
                WHERE subject_kind = $1 AND subject_id = $2
                UNION ALL
                SELECT 1 FROM cn_safety.risk_signals
                WHERE target = $3 AND target_id = $2
                  AND persisted_at > NOW() - cn_admin.retention_interval('risk_signal')
            )",
        )
        .bind(subject_kind)
        .bind(subject_id)
        .bind(if subject_kind == "post" {
            "post_id"
        } else {
            "blob_cid"
        })
        .fetch_one(pool)
        .await?;

    // 現行 blob cache は削除・再取得拒否 backend の readiness 契約だけで、対象単位の
    // durable inventory を持たない。client の申告だけで verified にはしない。
    let blob_cache_verified = !requested_capabilities
        .iter()
        .any(|value| value == "blob_cache");

    Ok(
        if index_verified && moderation_verified && blob_cache_verified {
            RightsRequestScopeStatus::VerifiedScope
        } else {
            RightsRequestScopeStatus::UnverifiedScope
        },
    )
}

pub async fn insert_rights_request(
    pool: &PgPool,
    request: &RightsRequestCreateRequest,
    scope_status: RightsRequestScopeStatus,
    cipher: &LegalDataCipher,
    now: DateTime<Utc>,
) -> Result<CreatedRightsRequest> {
    validate_request(request)?;
    let id = Uuid::new_v4().to_string();
    let tracking_secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let tracking_secret_hash = tracking_secret_hash(&tracking_secret);
    let status = initial_status(scope_status);
    let (stored_request, contact, identity, evidence) = split_sensitive_request(request);
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "INSERT INTO cn_legal.rights_requests
            (id, tracking_secret_hash, scope_revision, scope_status, status, subject_kind,
             subject_id, requested_capabilities, request_data, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
         RETURNING *",
    )
    .bind(&id)
    .bind(&tracking_secret_hash)
    .bind(request.scope_revision.trim())
    .bind(scope_status_as_str(scope_status))
    .bind(status_as_str(status))
    .bind(request.subject_kind.trim())
    .bind(request.subject_id.trim())
    .bind(&request.requested_capabilities)
    .bind(serde_json::to_value(&stored_request)?)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    upsert_sensitive_json_in_tx(
        &mut tx,
        cipher,
        "rights_request",
        &id,
        SensitiveDataCategory::RightsRequestContact,
        &contact,
    )
    .await?;
    if identity != serde_json::Value::Null {
        upsert_sensitive_json_in_tx(
            &mut tx,
            cipher,
            "rights_request",
            &id,
            SensitiveDataCategory::RightsRequestIdentity,
            &identity,
        )
        .await?;
    }
    if !evidence.is_empty() {
        upsert_sensitive_json_in_tx(
            &mut tx,
            cipher,
            "rights_request",
            &id,
            SensitiveDataCategory::RightsRequestEvidence,
            &evidence,
        )
        .await?;
    }
    append_event(
        &mut tx,
        RightsRequestEventInput {
            request_id: &id,
            actor: "requester",
            action: "rights_request.create",
            from_status: None,
            to_status: status,
            public_message: None,
            delivery_status: "status_surface",
            occurred_at: now,
        },
    )
    .await?;
    let mut record = record_from_row(&row)?;
    record.request = request.clone();
    tx.commit().await?;
    Ok(CreatedRightsRequest {
        record,
        tracking_secret,
    })
}

pub async fn get_rights_request(pool: &PgPool, id: &str) -> Result<Option<RightsRequestRecord>> {
    let row = sqlx::query(
        "SELECT * FROM cn_legal.rights_requests
         WHERE id = $1
           AND updated_at > NOW() - cn_admin.retention_interval(
               cn_legal.rights_request_retention(status))",
    )
    .bind(id.trim())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(record_from_row).transpose()
}

pub async fn get_rights_request_with_sensitive(
    pool: &PgPool,
    cipher: &LegalDataCipher,
    id: &str,
    now: DateTime<Utc>,
) -> Result<Option<RightsRequestRecord>> {
    let Some(mut record) = get_rights_request(pool, id).await? else {
        return Ok(None);
    };
    hydrate_sensitive_request(pool, cipher, &mut record, now).await?;
    Ok(Some(record))
}

pub async fn list_rights_requests(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<RightsRequestRecord>> {
    let rows = sqlx::query(
        "SELECT * FROM cn_legal.rights_requests
         WHERE updated_at > NOW() - cn_admin.retention_interval(
             cn_legal.rights_request_retention(status))
         ORDER BY created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit.clamp(1, 200))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await?;
    rows.iter().map(record_from_row).collect()
}

pub async fn list_rights_requests_with_sensitive(
    pool: &PgPool,
    cipher: &LegalDataCipher,
    limit: i64,
    offset: i64,
    now: DateTime<Utc>,
) -> Result<Vec<RightsRequestRecord>> {
    let mut records = list_rights_requests(pool, limit, offset).await?;
    for record in &mut records {
        hydrate_sensitive_request(pool, cipher, record, now).await?;
    }
    Ok(records)
}

pub async fn get_public_rights_request_status(
    pool: &PgPool,
    reference_id: &str,
    tracking_secret: &str,
) -> Result<Option<RightsRequestStatusResponse>> {
    let row = sqlx::query(
        "SELECT * FROM cn_legal.rights_requests
         WHERE id = $1 AND tracking_secret_hash = $2
           AND updated_at > NOW() - cn_admin.retention_interval(
               cn_legal.rights_request_retention(status))",
    )
    .bind(reference_id.trim())
    .bind(tracking_secret_hash(tracking_secret.trim()))
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let record = record_from_row(&row)?;
    Ok(Some(RightsRequestStatusResponse {
        reference_id: record.id,
        scope_status: record.scope_status,
        status: record.status,
        updated_at: record.updated_at.to_rfc3339(),
        public_message: record.public_message,
    }))
}

#[allow(clippy::too_many_arguments)]
pub async fn transition_rights_request(
    pool: &PgPool,
    id: &str,
    expected_version: i32,
    actor: &str,
    to_status: RightsRequestStatus,
    public_message: Option<&str>,
    delivery_status: &str,
    now: DateTime<Utc>,
) -> Result<RightsRequestRecord> {
    validate_actor(actor)?;
    validate_delivery_status(delivery_status)?;
    let mut tx = pool.begin().await?;
    let current_row = sqlx::query(
        "SELECT * FROM cn_legal.rights_requests
         WHERE id = $1
           AND updated_at > NOW() - cn_admin.retention_interval(
               cn_legal.rights_request_retention(status))
         FOR UPDATE",
    )
    .bind(id.trim())
    .fetch_optional(&mut *tx)
    .await?
    .context("rights request was not found")?;
    let current = record_from_row(&current_row)?;
    if current.version != expected_version {
        bail!(
            "rights request version conflict: expected {expected_version}, current {}",
            current.version
        );
    }
    if !transition_allowed(current.status, to_status) {
        bail!(
            "invalid rights request transition: {} -> {}",
            status_as_str(current.status),
            status_as_str(to_status)
        );
    }
    let public_message = normalize_optional(public_message, 2_000)?;
    let row = sqlx::query(
        "UPDATE cn_legal.rights_requests
         SET status = $2, public_message = $3, version = version + 1, updated_at = $4
         WHERE id = $1 RETURNING *",
    )
    .bind(id.trim())
    .bind(status_as_str(to_status))
    .bind(public_message.as_deref())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    append_event(
        &mut tx,
        RightsRequestEventInput {
            request_id: id,
            actor,
            action: "rights_request.transition",
            from_status: Some(current.status),
            to_status,
            public_message: public_message.as_deref(),
            delivery_status,
            occurred_at: now,
        },
    )
    .await?;
    append_operator_audit(
        &mut tx,
        actor,
        id,
        status_as_str(current.status),
        status_as_str(to_status),
        now,
    )
    .await?;
    let updated = record_from_row(&row)?;
    tx.commit().await?;
    Ok(updated)
}

/// 申出を node-local 送信防止へ結び、措置・申出 event・audit を同じ transaction で確定する。
#[allow(clippy::too_many_arguments)]
pub async fn action_rights_request(
    pool: &PgPool,
    id: &str,
    expected_version: i32,
    actor: &str,
    capabilities: Vec<TransmissionPreventionCapability>,
    public_message: &str,
    now: DateTime<Utc>,
) -> Result<RightsRequestActionResult> {
    validate_actor(actor)?;
    let public_message = normalize_optional(Some(public_message), 2_000)?
        .context("public_message must not be empty")?;
    let mut tx = pool.begin().await?;
    let current_row = sqlx::query(
        "SELECT * FROM cn_legal.rights_requests
         WHERE id = $1
           AND updated_at > NOW() - cn_admin.retention_interval(
               cn_legal.rights_request_retention(status))
         FOR UPDATE",
    )
    .bind(id.trim())
    .fetch_optional(&mut *tx)
    .await?
    .context("rights request was not found")?;
    let current = record_from_row(&current_row)?;
    if current.version != expected_version {
        bail!(
            "rights request version conflict: expected {expected_version}, current {}",
            current.version
        );
    }
    if !matches!(
        current.status,
        RightsRequestStatus::Reviewing | RightsRequestStatus::SenderContacting
    ) {
        bail!("rights request must be reviewing before it can be actioned");
    }
    if capabilities.is_empty()
        || capabilities.iter().any(|capability| {
            !requested_capability_allows(&current.request.requested_capabilities, *capability)
        })
    {
        bail!("action capabilities must stay within the request's verified node-local scope");
    }
    let prevention = apply_transmission_prevention_in_tx(
        &mut tx,
        actor,
        &NewTransmissionPrevention {
            subject_kind: current.subject_kind.clone(),
            subject_id: current.subject_id.clone(),
            basis: basis_for_category(current.request.rights_category),
            capabilities,
            expires_at: None,
            related_report_id: Some(current.id.clone()),
        },
    )
    .await?;
    let row = sqlx::query(
        "UPDATE cn_legal.rights_requests
         SET status = 'actioned', public_message = $2, version = version + 1, updated_at = $3
         WHERE id = $1 RETURNING *",
    )
    .bind(id.trim())
    .bind(&public_message)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    append_event(
        &mut tx,
        RightsRequestEventInput {
            request_id: id,
            actor,
            action: "rights_request.action",
            from_status: Some(current.status),
            to_status: RightsRequestStatus::Actioned,
            public_message: Some(&public_message),
            delivery_status: "status_surface",
            occurred_at: now,
        },
    )
    .await?;
    append_operator_audit(
        &mut tx,
        actor,
        id,
        status_as_str(current.status),
        "actioned",
        now,
    )
    .await?;
    let request = record_from_row(&row)?;
    tx.commit().await?;
    Ok(RightsRequestActionResult {
        request,
        prevention,
    })
}

pub async fn withdraw_rights_request(
    pool: &PgPool,
    reference_id: &str,
    tracking_secret: &str,
    now: DateTime<Utc>,
) -> Result<Option<RightsRequestStatusResponse>> {
    let hash = tracking_secret_hash(tracking_secret.trim());
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT * FROM cn_legal.rights_requests
         WHERE id = $1 AND tracking_secret_hash = $2
           AND updated_at > NOW() - cn_admin.retention_interval(
               cn_legal.rights_request_retention(status))
         FOR UPDATE",
    )
    .bind(reference_id.trim())
    .bind(hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let current = record_from_row(&row)?;
    if current.status == RightsRequestStatus::Withdrawn {
        tx.commit().await?;
        return Ok(Some(public_status(&current)));
    }
    if !transition_allowed(current.status, RightsRequestStatus::Withdrawn) {
        bail!("completed rights request cannot be withdrawn");
    }
    let row = sqlx::query(
        "UPDATE cn_legal.rights_requests
         SET status = 'withdrawn', public_message = '申出人が申出を取り下げました',
             version = version + 1, updated_at = $2
         WHERE id = $1 RETURNING *",
    )
    .bind(reference_id.trim())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    append_event(
        &mut tx,
        RightsRequestEventInput {
            request_id: reference_id,
            actor: "requester",
            action: "rights_request.withdraw",
            from_status: Some(current.status),
            to_status: RightsRequestStatus::Withdrawn,
            public_message: Some("申出人が申出を取り下げました"),
            delivery_status: "status_surface",
            occurred_at: now,
        },
    )
    .await?;
    append_operator_audit(
        &mut tx,
        "requester",
        reference_id,
        status_as_str(current.status),
        "withdrawn",
        now,
    )
    .await?;
    let updated = record_from_row(&row)?;
    tx.commit().await?;
    Ok(Some(public_status(&updated)))
}

fn validate_request(request: &RightsRequestCreateRequest) -> Result<()> {
    for (name, value, max) in [
        ("scope_revision", request.scope_revision.as_str(), 128),
        ("requester_name", request.requester_name.as_str(), 320),
        ("email", request.email.as_str(), 320),
        ("rights_basis", request.rights_basis.as_str(), 4_000),
        ("subject_kind", request.subject_kind.as_str(), 64),
        ("subject_id", request.subject_id.as_str(), 512),
        (
            "infringement_description",
            request.infringement_description.as_str(),
            8_000,
        ),
    ] {
        if value.trim().is_empty() || value.len() > max {
            bail!("{name} must be present and at most {max} characters");
        }
    }
    if !request.scope_acknowledged {
        bail!("scope_acknowledged must be true");
    }
    if !request.no_permission_statement {
        bail!("no_permission_statement must be true");
    }
    if !request.email.contains('@') {
        bail!("email must be valid");
    }
    if request.requested_capabilities.is_empty() || request.requested_capabilities.len() > 6 {
        bail!("requested_capabilities must contain 1 to 6 entries");
    }
    if request.evidence_references.len() > 20
        || request
            .evidence_references
            .iter()
            .any(|reference| reference.value.trim().is_empty() || reference.value.len() > 2_048)
    {
        bail!("evidence_references contain an invalid value");
    }
    Ok(())
}

fn initial_status(scope: RightsRequestScopeStatus) -> RightsRequestStatus {
    match scope {
        RightsRequestScopeStatus::VerifiedScope => RightsRequestStatus::Received,
        RightsRequestScopeStatus::UnverifiedScope => RightsRequestStatus::NeedsInformation,
        RightsRequestScopeStatus::OutOfScope => RightsRequestStatus::OutOfScope,
    }
}

fn basis_for_category(category: RightsCategory) -> TransmissionPreventionBasis {
    match category {
        RightsCategory::Copyright => TransmissionPreventionBasis::Copyright,
        RightsCategory::Privacy => TransmissionPreventionBasis::Privacy,
        RightsCategory::PersonalityRights => TransmissionPreventionBasis::PersonalityRights,
        RightsCategory::Trademark => TransmissionPreventionBasis::Trademark,
        RightsCategory::OtherRights => TransmissionPreventionBasis::OtherRights,
    }
}

fn requested_capability_allows(
    requested: &[String],
    capability: TransmissionPreventionCapability,
) -> bool {
    let exact = capability.as_str();
    requested.iter().any(|value| {
        value == exact
            || (value == "community_index"
                && matches!(
                    capability,
                    TransmissionPreventionCapability::Search
                        | TransmissionPreventionCapability::Discovery
                        | TransmissionPreventionCapability::Recommendation
                ))
    })
}

fn transition_allowed(from: RightsRequestStatus, to: RightsRequestStatus) -> bool {
    use RightsRequestStatus as S;
    match from {
        S::Received => matches!(
            to,
            S::NeedsInformation | S::Reviewing | S::Declined | S::OutOfScope | S::Withdrawn
        ),
        S::NeedsInformation => matches!(
            to,
            S::Reviewing | S::Declined | S::OutOfScope | S::Withdrawn
        ),
        S::Reviewing => matches!(
            to,
            S::NeedsInformation
                | S::SenderContacting
                | S::Actioned
                | S::Declined
                | S::OutOfScope
                | S::Withdrawn
        ),
        S::SenderContacting => matches!(
            to,
            S::Reviewing | S::Actioned | S::Declined | S::OutOfScope | S::Withdrawn
        ),
        S::Actioned | S::Declined | S::OutOfScope | S::Withdrawn => false,
    }
}

fn tracking_secret_hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn public_status(record: &RightsRequestRecord) -> RightsRequestStatusResponse {
    RightsRequestStatusResponse {
        reference_id: record.id.clone(),
        scope_status: record.scope_status,
        status: record.status,
        updated_at: record.updated_at.to_rfc3339(),
        public_message: record.public_message.clone(),
    }
}

struct RightsRequestEventInput<'a> {
    request_id: &'a str,
    actor: &'a str,
    action: &'a str,
    from_status: Option<RightsRequestStatus>,
    to_status: RightsRequestStatus,
    public_message: Option<&'a str>,
    delivery_status: &'a str,
    occurred_at: DateTime<Utc>,
}

async fn append_event(
    tx: &mut Transaction<'_, Postgres>,
    event: RightsRequestEventInput<'_>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_legal.rights_request_events
            (id, request_id, actor, action, from_status, to_status, public_message,
             delivery_status, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(event.request_id.trim())
    .bind(event.actor.trim())
    .bind(event.action)
    .bind(event.from_status.map(status_as_str))
    .bind(status_as_str(event.to_status))
    .bind(event.public_message)
    .bind(event.delivery_status)
    .bind(event.occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn append_operator_audit(
    tx: &mut Transaction<'_, Postgres>,
    actor: &str,
    request_id: &str,
    from_status: &str,
    to_status: &str,
    occurred_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_admin.operator_actions
            (id, actor, action, target_kind, target_id, before_json, after_json, occurred_at)
         VALUES ($1, $2, 'rights_request.transition', 'rights_request', $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(actor.trim())
    .bind(request_id.trim())
    .bind(json!({ "status": from_status }))
    .bind(json!({ "status": to_status }))
    .bind(occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn record_from_row(row: &PgRow) -> Result<RightsRequestRecord> {
    Ok(RightsRequestRecord {
        id: row.try_get("id")?,
        tracking_secret_hash: row.try_get("tracking_secret_hash")?,
        scope_revision: row.try_get("scope_revision")?,
        scope_status: parse_scope_status(&row.try_get::<String, _>("scope_status")?)?,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        subject_kind: row.try_get("subject_kind")?,
        subject_id: row.try_get("subject_id")?,
        requested_capabilities: row.try_get("requested_capabilities")?,
        request: serde_json::from_value(row.try_get("request_data")?)?,
        version: row.try_get("version")?,
        public_message: row.try_get("public_message")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn scope_status_as_str(status: RightsRequestScopeStatus) -> &'static str {
    match status {
        RightsRequestScopeStatus::VerifiedScope => "verified_scope",
        RightsRequestScopeStatus::UnverifiedScope => "unverified_scope",
        RightsRequestScopeStatus::OutOfScope => "out_of_scope",
    }
}

fn parse_scope_status(value: &str) -> Result<RightsRequestScopeStatus> {
    match value {
        "verified_scope" => Ok(RightsRequestScopeStatus::VerifiedScope),
        "unverified_scope" => Ok(RightsRequestScopeStatus::UnverifiedScope),
        "out_of_scope" => Ok(RightsRequestScopeStatus::OutOfScope),
        _ => bail!("unknown rights request scope status `{value}`"),
    }
}

fn status_as_str(status: RightsRequestStatus) -> &'static str {
    match status {
        RightsRequestStatus::Received => "received",
        RightsRequestStatus::NeedsInformation => "needs_information",
        RightsRequestStatus::Reviewing => "reviewing",
        RightsRequestStatus::SenderContacting => "sender_contacting",
        RightsRequestStatus::Actioned => "actioned",
        RightsRequestStatus::Declined => "declined",
        RightsRequestStatus::OutOfScope => "out_of_scope",
        RightsRequestStatus::Withdrawn => "withdrawn",
    }
}

fn parse_status(value: &str) -> Result<RightsRequestStatus> {
    match value {
        "received" => Ok(RightsRequestStatus::Received),
        "needs_information" => Ok(RightsRequestStatus::NeedsInformation),
        "reviewing" => Ok(RightsRequestStatus::Reviewing),
        "sender_contacting" => Ok(RightsRequestStatus::SenderContacting),
        "actioned" => Ok(RightsRequestStatus::Actioned),
        "declined" => Ok(RightsRequestStatus::Declined),
        "out_of_scope" => Ok(RightsRequestStatus::OutOfScope),
        "withdrawn" => Ok(RightsRequestStatus::Withdrawn),
        _ => bail!("unknown rights request status `{value}`"),
    }
}

fn validate_actor(actor: &str) -> Result<()> {
    if actor.trim().is_empty() || actor.len() > 320 {
        bail!("actor must be present and at most 320 characters");
    }
    Ok(())
}

fn validate_delivery_status(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 64 {
        bail!("delivery_status must be present and at most 64 characters");
    }
    Ok(())
}

fn normalize_optional(value: Option<&str>, max: usize) -> Result<Option<String>> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.len() > max {
                bail!("value must be at most {max} characters");
            }
            Ok(value.to_string())
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_status_never_overstates_unverified_scope() {
        assert_eq!(
            initial_status(RightsRequestScopeStatus::VerifiedScope),
            RightsRequestStatus::Received
        );
        assert_eq!(
            initial_status(RightsRequestScopeStatus::UnverifiedScope),
            RightsRequestStatus::NeedsInformation
        );
        assert_eq!(
            initial_status(RightsRequestScopeStatus::OutOfScope),
            RightsRequestStatus::OutOfScope
        );
    }

    #[test]
    fn completed_requests_are_terminal() {
        for status in [
            RightsRequestStatus::Actioned,
            RightsRequestStatus::Declined,
            RightsRequestStatus::OutOfScope,
            RightsRequestStatus::Withdrawn,
        ] {
            assert!(!transition_allowed(status, RightsRequestStatus::Reviewing));
        }
    }

    #[test]
    fn tracking_secret_hash_is_stable_and_not_plaintext() {
        let hash = tracking_secret_hash("secret");
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, tracking_secret_hash("secret"));
        assert_ne!(hash, "secret");
    }
}
