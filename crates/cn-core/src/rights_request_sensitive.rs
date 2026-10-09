//! 権利侵害申出の機微区分の分離と、読取り時の復元。

use anyhow::Result;
use chrono::{DateTime, Utc};
use kukuri_cn_protocol::{EvidenceReference, RightsRequestCreateRequest};
use serde_json::{Value, json};
use sqlx::postgres::PgPool;

use crate::rights_requests::RightsRequestRecord;
use crate::{LegalDataCipher, SensitiveDataCategory, load_sensitive_json};

pub(crate) fn split_sensitive_request(
    request: &RightsRequestCreateRequest,
) -> (
    RightsRequestCreateRequest,
    Value,
    Value,
    Vec<EvidenceReference>,
) {
    let mut stored = request.clone();
    let contact = json!({
        "requester_name": request.requester_name,
        "organization": request.organization,
        "address": request.address,
        "email": request.email,
        "phone": request.phone,
        "represented_rights_holder": request.represented_rights_holder,
    });
    let identity = request
        .authority_basis
        .as_ref()
        .map(|value| json!({"authority_basis": value}))
        .unwrap_or(Value::Null);
    let evidence = request.evidence_references.clone();
    stored.requester_name.clear();
    stored.organization = None;
    stored.address = None;
    stored.email.clear();
    stored.phone = None;
    stored.represented_rights_holder = None;
    stored.authority_basis = None;
    stored.evidence_references.clear();
    (stored, contact, identity, evidence)
}

pub(crate) async fn hydrate_sensitive_request(
    pool: &PgPool,
    cipher: &LegalDataCipher,
    record: &mut RightsRequestRecord,
    now: DateTime<Utc>,
) -> Result<()> {
    if let Some(contact) = load_sensitive_json::<Value>(
        pool,
        cipher,
        "rights_request",
        &record.id,
        SensitiveDataCategory::RightsRequestContact,
        now,
    )
    .await?
    {
        record.request.requester_name = json_string(&contact, "requester_name").unwrap_or_default();
        record.request.organization = json_string(&contact, "organization");
        record.request.address = json_string(&contact, "address");
        record.request.email = json_string(&contact, "email").unwrap_or_default();
        record.request.phone = json_string(&contact, "phone");
        record.request.represented_rights_holder =
            json_string(&contact, "represented_rights_holder");
    }
    if let Some(identity) = load_sensitive_json::<Value>(
        pool,
        cipher,
        "rights_request",
        &record.id,
        SensitiveDataCategory::RightsRequestIdentity,
        now,
    )
    .await?
    {
        record.request.authority_basis = json_string(&identity, "authority_basis");
    }
    record.request.evidence_references = load_sensitive_json::<Vec<EvidenceReference>>(
        pool,
        cipher,
        "rights_request",
        &record.id,
        SensitiveDataCategory::RightsRequestEvidence,
        now,
    )
    .await?
    .unwrap_or_default();
    Ok(())
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}
