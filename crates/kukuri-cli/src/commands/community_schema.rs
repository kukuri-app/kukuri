use super::{
    community_views,
    schema::{array, nullable, object},
};
use serde_json::{Value, json};

pub(super) fn string() -> Value {
    json!({"type": "string"})
}
pub(super) fn strings() -> Value {
    array(string())
}
pub(super) fn integer() -> Value {
    json!({"type": "integer"})
}
pub(super) fn number() -> Value {
    json!({"type": "number"})
}
pub(super) fn boolean() -> Value {
    json!({"type": "boolean"})
}
pub(super) fn scope() -> Value {
    json!({"enum": ["public_topic", "private_channel"]})
}

pub(super) fn input(name: &str) -> Value {
    match name {
        "get_community_node_config"
        | "get_community_node_statuses"
        | "clear_community_node_config" => object(json!({}), &[]),
        "set_community_node_config" => object(
            // #1056: content_advisory_enabled は任意。未指定は保存済みの採用設定を維持する。
            json!({"nodes": array(object(json!({"base_url": string(), "content_advisory_enabled": nullable(boolean())}), &["base_url"]))}),
            &["nodes"],
        ),
        "fetch_community_node_policies" => object(
            json!({"base_url": string(), "language": nullable(string())}),
            &["base_url"],
        ),
        "accept_community_node_consents" => object(
            json!({"base_url": string(), "language": string(),
            "documents": array(object(json!({"policy_slug": string(), "policy_version": integer(), "policy_snapshot_revision": nullable(string())}), &["policy_slug", "policy_version"]))}),
            &["base_url", "documents", "language"],
        ),
        "submit_community_node_report" => object(
            json!({"node_base_url": string(), "report_endpoint": string(), "subject_kind": string(), "subject_id": string(),
            "capability": string(), "reason": string(), "details": nullable(string()), "reporter_contact": nullable(string()),
            "appeal": nullable(object(json!({"risk_signal_id": string()}), &["risk_signal_id"]))}),
            &[
                "node_base_url",
                "report_endpoint",
                "subject_kind",
                "subject_id",
                "capability",
                "reason",
            ],
        ),
        "submit_community_node_tester_feedback" => object(
            json!({"base_url": string(), "what_attempted": string(), "what_happened": string(), "what_seemed_wrong": string()}),
            &[
                "base_url",
                "what_attempted",
                "what_happened",
                "what_seemed_wrong",
            ],
        ),
        "submit_community_node_indexing_request" | "revoke_community_node_indexing_request" => {
            object(
                json!({"base_url": string(), "scope_kind": scope(), "topic_id": string(), "channel_id": nullable(string()), "confirm_private_channel_secret_disclosure": boolean()}),
                &["base_url", "scope_kind", "topic_id"],
            )
        }
        "read_community_node_indexing_status" => object(
            json!({"base_url": string(), "scope_kind": {"enum": ["public_topic", "private_channel", null]}, "topic_id": nullable(string()), "channel_id": nullable(string()), "confirm_private_channel_secret_disclosure": boolean()}),
            &["base_url"],
        ),
        "search_community_node_index"
        | "discover_community_node_index"
        | "recommend_community_node_index" => object(
            json!({"base_url": string(), "query": nullable(string()),
            "scope_kind": {"enum": ["public_topic", "private_channel", null]}, "scope_id": nullable(string()), "topic_id": nullable(string()), "limit": nullable(integer())}),
            &["base_url"],
        ),
        "lookup_community_node_content_advisories" => object(
            json!({"post_ids": strings(), "blob_hashes": strings()}),
            &["post_ids", "blob_hashes"],
        ),
        "read_community_node_trust_user" | "read_community_node_relation_user" => object(
            json!({"base_url": string(), "target_pubkey": string()}),
            &["base_url", "target_pubkey"],
        ),
        "list_community_node_relation_neighbors" => object(
            json!({"base_url": string(), "limit": nullable(integer())}),
            &["base_url"],
        ),
        "set_community_node_invite_code" => {
            let mut input = object(json!({"base_url": string()}), &["base_url"]);
            input["description"] =
                json!("招待コードは専用frameのUTF-8で渡す。空のframeは保存済みコードを解除する。");
            input
        }
        "enable_community_node_observation_sharing" => object(
            json!({"base_url": string(), "policy_version": integer(), "policy_snapshot_revision": nullable(string()), "language": string(), "include_existing": boolean()}),
            &["base_url", "policy_version"],
        ),
        "evaluate_author_trust_gates" => {
            object(json!({"author_pubkeys": strings()}), &["author_pubkeys"])
        }
        "set_author_trust_display_exception" => object(
            json!({"author_pubkey": string(), "always_visible": boolean()}),
            &["author_pubkey", "always_visible"],
        ),
        "list_author_trust_display_exceptions" => object(json!({}), &[]),
        "get_community_node_observation_sharing"
        | "disable_community_node_observation_sharing"
        | "authenticate_community_node"
        | "clear_community_node_token"
        | "withdraw_community_node_consents"
        | "refresh_community_node_metadata"
        | "fetch_community_node_manifest"
        | "get_community_node_relation_optout"
        | "set_community_node_relation_optout"
        | "clear_community_node_relation_optout" => {
            object(json!({"base_url": string()}), &["base_url"])
        }
        _ => unreachable!("Community Node input schema"),
    }
}

pub(super) fn output(name: &str) -> Value {
    use community_views::view;
    match name {
        "get_community_node_config" | "set_community_node_config" => view(
            json!({"nodes": array(view(json!({"base_url": string(), "resolved_urls": nullable(community_views::resolved_urls()), "content_advisory_enabled": boolean()}), &[])),
            "trust_node_priority": strings()}),
            &[],
        ),
        "get_community_node_statuses" => array(community_views::status()),
        "authenticate_community_node"
        | "set_community_node_invite_code"
        | "clear_community_node_token"
        | "accept_community_node_consents"
        | "withdraw_community_node_consents"
        | "refresh_community_node_metadata" => community_views::status(),
        "clear_community_node_config" => json!({"type": "null"}),
        "evaluate_author_trust_gates" => view(
            json!({"gates": array(community_views::author_trust_gate())}),
            &[],
        ),
        "set_author_trust_display_exception" => community_views::author_trust_gate(),
        "list_author_trust_display_exceptions" => strings(),
        "get_community_node_observation_sharing"
        | "enable_community_node_observation_sharing"
        | "disable_community_node_observation_sharing" => view(
            json!({"base_url": string(), "offered": boolean(), "policy": nullable(community_views::policy_document()),
                "enabled": boolean(), "needs_reconsent": boolean(), "revocation_pending": boolean(), "pending_count": integer()}),
            &[],
        ),
        "fetch_community_node_policies" => community_views::policies(),
        "fetch_community_node_manifest" => view(
            json!({"status": {"enum": ["ok", "absent"]}, "manifest": nullable(community_views::manifest())}),
            &[],
        ),
        "submit_community_node_report" => view(
            json!({"status": {"const": "submitted"}, "reference_id": nullable(string()), "disputed_risk_signal_id": nullable(string())}),
            &[],
        ),
        "submit_community_node_tester_feedback" => object(json!({"reference_id": string()}), &[]),
        "submit_community_node_indexing_request" => view(
            json!({"request_id": string(), "status": {"enum": ["pending", "approved", "rejected"]}}),
            &[],
        ),
        "revoke_community_node_indexing_request" => json!({"type": "null"}),
        "read_community_node_indexing_status" => view(
            json!({
                "requests": array(view(json!({"request_id": string(), "scope_kind": scope(), "target_id": string(), "status": {"enum": ["pending", "approved", "rejected"]}, "created_at": integer(), "decided_at": nullable(integer())}), &[])),
                "target": nullable(view(json!({"scope_kind": scope(), "scope_id": string(), "supported": boolean()}), &[]))
            }),
            &[],
        ),
        "search_community_node_index"
        | "discover_community_node_index"
        | "recommend_community_node_index" => view(
            json!({"entries": array(view(json!({"scope_kind": scope(), "scope_id": string(), "object_id": string(), "author_pubkey": string(), "text": string(), "created_at": integer(),
                "content_advisories": array(community_views::content_advisory())}), &[]))}),
            &[],
        ),
        "lookup_community_node_content_advisories" => view(
            json!({"nodes": array(view(json!({"base_url": string(), "node_id": nullable(string()),
                "advisories": array(community_views::content_advisory()),
                "error": nullable(view(json!({"code": string(), "message": string(), "status": nullable(integer())}), &[]))}), &[]))}),
            &[],
        ),
        "read_community_node_trust_user" => community_views::trust(),
        "read_community_node_relation_user" => view(
            json!({"viewer_pubkey": string(), "target_pubkey": string(), "score": number(), "basis": array(view(json!({"feature": string(), "value": number(), "weight": number(), "contribution": number()}), &[]))}),
            &[],
        ),
        "list_community_node_relation_neighbors" => view(
            json!({"viewer_pubkey": string(), "neighbors": strings()}),
            &[],
        ),
        "get_community_node_relation_optout"
        | "set_community_node_relation_optout"
        | "clear_community_node_relation_optout" => view(
            json!({"pubkey": string(), "opted_out": boolean(), "opted_out_at": nullable(string()), "min_proximity": number()}),
            &[],
        ),
        _ => unreachable!("Community Node output schema"),
    }
}
