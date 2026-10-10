use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub struct MockNode {
    pub base_url: String,
    pub policy_version: AtomicUsize,
    pub reject_index_once: AtomicBool,
    pub index_hits: AtomicUsize,
    pub verify_hits: AtomicUsize,
}

pub const TOKEN: &str = "cli-token-secret-sentinel";

pub async fn mock_node(
    axum::extract::State(node): axum::extract::State<Arc<MockNode>>,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::{Json, http::StatusCode, response::IntoResponse};
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        + 3600;
    let version = node.policy_version.load(Ordering::SeqCst);
    match uri.path() {
        "/v1/policies" => {
            assert!(
                headers.get("authorization").is_none(),
                "公開policyへtokenを送らない"
            );
            return Json(json!({"policies": [{"policy_slug": "builder-preview", "policy_version": version,
                "title": "テスト規約", "body_markdown": "テスト文書の本文", "required": true, "language": "ja", "is_current": true, "policy_kind": "terms"}]})).into_response();
        }
        "/v1/node/manifest" => return StatusCode::NOT_FOUND.into_response(),
        "/v1/auth/challenge" => {
            return Json(json!({"challenge": "cli-challenge", "expires_at": expires}))
                .into_response();
        }
        "/v1/auth/verify" => {
            node.verify_hits.fetch_add(1, Ordering::SeqCst);
            return Json(json!({"access_token": TOKEN, "token_type": "Bearer", "expires_at": expires, "pubkey": "f".repeat(64)})).into_response();
        }
        _ => {}
    }
    let authorized = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {TOKEN}"));
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"code": "AUTH_REQUIRED", "message": "認証が必要です"})),
        )
            .into_response();
    }
    match uri.path() {
        "/v1/consents" | "/v1/consents/status" => Json(json!({"all_required_accepted": true, "items": [{"policy_slug": "builder-preview", "policy_version": version,
            "title": "テスト規約", "required": true, "accepted_at": expires - 3600, "previously_accepted_version": version}]})).into_response(),
        "/v1/bootstrap/heartbeat" => Json(json!({"expires_at": expires})).into_response(),
        "/v1/bootstrap/nodes" => Json(json!({"nodes": [{"base_url": node.base_url, "resolved_urls": {"public_base_url": node.base_url, "connectivity_urls": [], "seed_peers": []}}]})).into_response(),
        "/v1/rendezvous/topics/heartbeat" => Json(json!({"expires_in_seconds": 45, "topics": []})).into_response(),
        "/v1/index/search" => {
            node.index_hits.fetch_add(1, Ordering::SeqCst);
            if node.reject_index_once.swap(false, Ordering::SeqCst) {
                (StatusCode::UNAUTHORIZED, Json(json!({"code": "AUTH_REQUIRED", "message": TOKEN}))).into_response()
            } else {
                Json(json!({"entries": []})).into_response()
            }
        }
        // #1702: 受け取った cursor の続きを示す cursor を返す（受け渡しの確認用）。
        path if path.starts_with("/v1/trust/users/") => Json(json!({"viewer_pubkey": "f".repeat(64),
            "target_id": path.trim_start_matches("/v1/trust/users/"), "absolute": 0.0, "relative": 0.0, "trust": 0.0, "w_abs_applied": 0.5,
            "computed_at": "2026-10-09T00:00:00Z", "basis": [],
            "basis_next_cursor": uri.query().and_then(|query| query.strip_prefix("cursor=")).map(|cursor| format!("{cursor}-next"))})).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
