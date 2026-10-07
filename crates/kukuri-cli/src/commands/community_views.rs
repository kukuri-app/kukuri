use super::{
    community_schema::{boolean, integer, number, string, strings},
    schema::{array, nullable, object},
};
use serde_json::{Value, json};

/// serdeで省略されるフィールドだけをrequiredから除く。
pub(super) fn view(properties: Value, optional: &[&str]) -> Value {
    let required = properties
        .as_object()
        .expect("view properties")
        .keys()
        .filter(|key| !optional.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

/// #1056 / ADR 0028 §8.6: content advisory(confidence は無ければ省略される)。
pub(super) fn content_advisory() -> Value {
    view(
        json!({"issuer_node_id": string(), "subject_kind": {"enum": ["post_id", "blob_cid"]}, "subject_id": string(),
        "category": string(), "label": string(), "confidence": integer(), "signal_id": string(), "basis": string()}),
        &["confidence"],
    )
}

pub(super) fn resolved_urls() -> Value {
    view(
        json!({"public_base_url": string(), "connectivity_urls": strings(),
        "seed_peers": array(object(json!({"endpoint_id": string(), "addr_hint": string()}), &["endpoint_id"])),
        "public_blob_search": boolean()}),
        &[],
    )
}

pub(super) fn status() -> Value {
    view(
        json!({"base_url": string(), "auth_state": view(json!({"authenticated": boolean(), "expires_at": nullable(integer())}), &[]),
        "consent_state": nullable(view(json!({"all_required_accepted": boolean(), "policy_snapshot_revision": string(),
            "items": array(view(json!({"policy_slug": string(), "policy_version": integer(), "title": string(), "body": string(), "required": boolean(),
                "accepted_at": nullable(integer()), "previously_accepted_version": nullable(integer()), "effective_date": string(), "language": string(), "policy_snapshot_revision": string()}),
                &["effective_date", "language", "policy_snapshot_revision"]))}), &["policy_snapshot_revision"])),
        "local_consent": view(json!({"records": array(view(json!({"policy_slug": string(), "policy_version": integer(), "policy_snapshot_revision": nullable(string()),
            "accepted_at": integer(), "language": string(), "app_version": string()}), &[])), "withdrawn_at": nullable(integer())}), &[]),
        "consent_update_pending": boolean(), "resolved_urls": nullable(resolved_urls()), "last_error": nullable(string()), "invite_code_saved": boolean(),
        "admission_rejection": nullable(view(json!({"code": {"enum": ["INVITE_REQUIRED", "INVITE_INVALID", "INVITE_EXPIRED", "INVITE_EXHAUSTED", "INVITE_REVOKED", "NOT_ALLOWLISTED", "BANNED"]}, "message": string()}), &[])),
        "session_phase": {"enum": ["idle", "connecting", "authenticating", "accepting", "refreshing", "ready", "retrying", "awaiting_admission"]},
        "retry_after": nullable(integer()), "restart_required": boolean()}),
        &[],
    )
}

pub(super) fn policies() -> Value {
    view(
        json!({"policies": array(policy_document()), "policy_snapshot_revision": string()}),
        &["policy_snapshot_revision"],
    )
}

/// #1061: 著者 1 人分の表示判断。
pub(super) fn author_trust_gate() -> Value {
    view(
        json!({"author_pubkey": string(), "hidden": boolean(), "node_base_url": nullable(string()),
        "reasons": array(json!({"type": "string", "enum": ["risk_signals", "related_users_block_or_mute"]})),
        "expires_at": nullable(string()), "always_visible": boolean()}),
        &[],
    )
}

/// 公開 policy カタログの文書 1 件。
pub(super) fn policy_document() -> Value {
    view(
        json!({"policy_slug": string(), "policy_version": integer(), "title": string(), "body_markdown": string(), "required": boolean(),
        "effective_date": string(), "language": string(), "policy_snapshot_revision": string(), "authoritative_language": string(), "reference_translation": boolean(),
        "translation_revision": integer(), "translation_of_version": integer(), "fallback": boolean(), "requested_language": string(),
        "material_change": boolean(), "requires_reconsent": boolean(), "is_current": boolean(), "publication_status": string(), "published_at": string(), "retired_at": string(),
        "previous_policy_version": integer(), "previous_policy_snapshot_revision": string(), "next_policy_version": integer(), "next_policy_snapshot_revision": string(),
        "policy_kind": string()}),
        &[
            "effective_date",
            "language",
            "policy_snapshot_revision",
            "authoritative_language",
            "translation_revision",
            "translation_of_version",
            "requested_language",
            "publication_status",
            "published_at",
            "retired_at",
            "previous_policy_version",
            "previous_policy_snapshot_revision",
            "next_policy_version",
            "next_policy_snapshot_revision",
            "policy_kind",
        ],
    )
}

pub(super) fn manifest() -> Value {
    view(
        json!({"node_id": string(), "node_name": string(), "node_role": string(), "server_name": string(), "manifest_version": string(),
        "capability_scope": view(json!({"available_enabled": strings(), "planned_enabled": strings()}), &[]),
        "authority_scope": view(json!({"applies_to": strings(), "does_not_apply_to": strings()}), &[]),
        "p2p_boundary": view(json!({"identity_authority": boolean(), "profile_canonical_store": boolean(), "social_graph_canonical_store": boolean(), "content_truth_source": boolean(), "network_wide_authority": boolean()}), &[]),
        "abuse_contact": string(), "report_endpoint": string(), "rights_request_url": string(), "rights_request_policy_url": string(), "rights_request_initial_response_target_days": integer(),
        "terms_url": string(), "privacy_url": string(), "external_transmission_url": string(), "moderation_policy_url": string(), "abuse_policy_url": string(), "data_retention_url": string(),
        "legal_documents": array(view(json!({"slug": string(), "version": integer(), "effective_date": string(), "language": string(), "required": boolean(), "url": string()}), &[]))}),
        &["legal_documents"],
    )
}

pub(super) fn trust() -> Value {
    let basis = view(
        json!({"signal_id": string(), "issuer_node_id": string(), "target": string(), "target_id": string(), "component": string(), "category": string(),
        "severity": string(), "basis": string(), "confidence": nullable(integer()), "visibility": string(), "appeal_status": string(), "expires_at": nullable(string()),
        "raw_contribution": number(), "decay_factor": number(), "relation_weight": number(), "contribution": number()}),
        &[],
    );
    // #1061: `trust` は CN が合算した S。`evaluation` は旧 node の応答では欠落する。
    let evaluation = view(
        json!({"policy_version": string(), "trust_version": string(), "relation_version": string(), "computed_at": string(), "expires_at": string(),
        "hide_recommended": boolean(), "reasons": array(json!({"type": "string", "enum": ["risk_signals", "related_users_block_or_mute"]}))}),
        &[],
    );
    view(
        json!({"viewer_pubkey": string(), "target_id": string(), "absolute": number(), "relative": number(), "trust": number(), "w_abs_applied": number(), "computed_at": string(), "basis": array(basis),
        "evaluation": evaluation}),
        &["evaluation"],
    )
}
