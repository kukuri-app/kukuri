use super::*;
use kukuri_cn_protocol::{AdvisoryLookupRequest, AdvisoryLookupResponse};

use crate::community_node::CommunityNodeContentAdvisoryLookupRequest;

// #1056: タイムライン向け content advisory 一括照会の client(ADR 0046 §6.1 / §6.3)。
// 外部送信の門(採用設定・同意・token・発行元確認)と、送信内容・採用範囲を固定する。

/// (Authorization ヘッダ, 本文)。
pub(crate) type RecordedLookup = (Option<String>, AdvisoryLookupRequest);

pub(crate) async fn mock_advisory_lookup(
    State(state): State<MockIndexQueryState>,
    headers: HeaderMap,
    Json(request): Json<AdvisoryLookupRequest>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    state
        .advisory_lookups
        .lock()
        .await
        .push((authorization.clone(), request));
    let expected = format!("Bearer {}", state.expected_token.lock().await.clone());
    let unauthorized = authorization.as_deref() != Some(expected.as_str())
        || state
            .unauthorized_remaining
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok();
    if unauthorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorBody {
                code: "AUTH_REQUIRED".to_string(),
                message: "community node authentication is required".to_string(),
            }),
        )
            .into_response();
    }
    Json(AdvisoryLookupResponse {
        advisories: state.advisory_lookup_response.lock().await.clone(),
    })
    .into_response()
}

fn post_advisory(issuer_node_id: &str, post_id: &str) -> ContentAdvisory {
    ContentAdvisory {
        subject_kind: AdvisorySubjectKind::PostId,
        subject_id: post_id.to_string(),
        category: SafetyCategory::Objectionable,
        label: "sensitive".to_string(),
        ..blob_advisory(issuer_node_id, "")
    }
}

fn visible_request() -> CommunityNodeContentAdvisoryLookupRequest {
    CommunityNodeContentAdvisoryLookupRequest {
        post_ids: vec!["post-visible".to_string(), " post-visible ".to_string()],
        blob_hashes: vec![ADVISORY_BLOB_HASH.to_string()],
    }
}

// AC-3 / INVAR-1: 採用 ON の設定済み node へ、可視の post id と blob hash だけを送る。
// 自 node 発行の advisory は採用され、blob 対象は取得ゲートへ登録される。
#[tokio::test]
async fn lookup_sends_only_visible_ids_to_enabled_nodes() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, _managed, state, server, _dir) = index_runtime(None).await;
    *state.advisory_lookup_response.lock().await = vec![
        blob_advisory(INDEX_NODE_ID, ADVISORY_BLOB_HASH),
        post_advisory(INDEX_NODE_ID, "post-visible"),
    ];

    let result = runtime
        .lookup_community_node_content_advisories(visible_request())
        .await
        .expect("lookup");

    let lookups = state.advisory_lookups.lock().await.clone();
    assert_eq!(lookups.len(), 1);
    let (authorization, body) = &lookups[0];
    assert_eq!(authorization.as_deref(), Some("Bearer index-token"));
    assert_eq!(
        serde_json::to_value(body).expect("json"),
        serde_json::json!({
            "post_ids": ["post-visible"],
            "blob_hashes": [ADVISORY_BLOB_HASH],
        }),
        "only visible identifiers may be sent"
    );
    assert_eq!(result.nodes.len(), 1);
    let node = &result.nodes[0];
    assert_eq!(node.base_url, base_url);
    assert_eq!(node.node_id.as_deref(), Some(INDEX_NODE_ID));
    assert_eq!(node.advisories.len(), 2);
    assert!(node.error.is_none());
    assert!(
        runtime
            .app_service
            .is_advisory_media_hash(ADVISORY_BLOB_HASH)
            .await
    );

    // 発行元は cache され、2 回目の照会で manifest を引き直さない。
    runtime
        .lookup_community_node_content_advisories(visible_request())
        .await
        .expect("second lookup");
    assert_eq!(state.manifest_hits.load(Ordering::SeqCst), 1);

    server.abort();
}

// TR-7 / INV-2a: 採用 OFF の node へは送信しない(HTTP 0 回)。
#[tokio::test]
async fn lookup_skips_nodes_with_content_advisory_disabled() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, _base_url, _managed, state, server, _dir) = index_runtime(None).await;
    runtime.community_node_config.lock().await.nodes[0].content_advisory_enabled = false;

    let result = runtime
        .lookup_community_node_content_advisories(visible_request())
        .await
        .expect("lookup");

    assert!(result.nodes.is_empty());
    assert!(state.advisory_lookups.lock().await.is_empty());
    assert_eq!(state.manifest_hits.load(Ordering::SeqCst), 0);

    server.abort();
}

// TR-3: node 未設定なら照会しない。
#[tokio::test]
async fn lookup_without_configured_nodes_makes_no_request() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, _base_url, _managed, state, server, _dir) = index_runtime(None).await;
    runtime.community_node_config.lock().await.nodes.clear();

    let result = runtime
        .lookup_community_node_content_advisories(visible_request())
        .await
        .expect("lookup");

    assert!(result.nodes.is_empty());
    assert!(state.advisory_lookups.lock().await.is_empty());

    server.abort();
}

// TR-8 / INV-2b: 同意が未成立なら HTTP 前に止める。
#[tokio::test]
async fn lookup_stops_before_http_when_consent_is_pending() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, _base_url, managed, state, server, _dir) = index_runtime(None).await;
    managed.consent_accepted.store(false, Ordering::SeqCst);
    managed
        .simulate_pending_update
        .store(true, Ordering::SeqCst);

    let result = runtime
        .lookup_community_node_content_advisories(visible_request())
        .await
        .expect("lookup");

    assert_eq!(result.nodes.len(), 1);
    assert_eq!(
        result.nodes[0]
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("CONSENT_REQUIRED")
    );
    assert!(result.nodes[0].advisories.is_empty());
    assert!(state.advisory_lookups.lock().await.is_empty());

    server.abort();
}

// 発行元を確認できない node へは識別子を送らない(fail-closed)。
#[tokio::test]
async fn lookup_stops_before_http_when_manifest_is_unavailable() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, _base_url, _managed, state, server, _dir) = index_runtime(None).await;
    *state.manifest_node_id.lock().await = None;

    let result = runtime
        .lookup_community_node_content_advisories(visible_request())
        .await
        .expect("lookup");

    assert_eq!(
        result.nodes[0]
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("COMMUNITY_NODE_MANIFEST_UNAVAILABLE")
    );
    assert!(state.advisory_lookups.lock().await.is_empty());

    server.abort();
}

// TR-9 / INV-2d: 他 issuer・未要求 subject・未知ラベルの advisory は採用せず、ゲートにも登録しない。
#[tokio::test]
async fn lookup_drops_unrequested_subjects_and_mismatched_issuer() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, _base_url, _managed, state, server, _dir) = index_runtime(None).await;
    let other_hash = "4444444444444444444444444444444444444444444444444444444444444444";
    let other_issuer = "3333333333333333333333333333333333333333333333333333333333333333";
    let mut unknown_label = post_advisory(INDEX_NODE_ID, "post-visible");
    unknown_label.label = "violent".to_string();
    *state.advisory_lookup_response.lock().await = vec![
        blob_advisory(other_issuer, ADVISORY_BLOB_HASH),
        blob_advisory(INDEX_NODE_ID, other_hash),
        post_advisory(INDEX_NODE_ID, "post-not-requested"),
        unknown_label,
    ];

    let result = runtime
        .lookup_community_node_content_advisories(visible_request())
        .await
        .expect("lookup");

    assert!(result.nodes[0].advisories.is_empty());
    assert!(
        !runtime
            .app_service
            .is_advisory_media_hash(ADVISORY_BLOB_HASH)
            .await
    );
    assert!(!runtime.app_service.is_advisory_media_hash(other_hash).await);

    server.abort();
}

// 401 は 1 回だけ再認証して再送する。
#[tokio::test]
async fn lookup_reauthenticates_once_after_unauthorized() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, _base_url, _managed, state, server, _dir) = index_runtime(None).await;
    state.unauthorized_remaining.store(1, Ordering::SeqCst);
    *state.advisory_lookup_response.lock().await =
        vec![blob_advisory(INDEX_NODE_ID, ADVISORY_BLOB_HASH)];

    let result = runtime
        .lookup_community_node_content_advisories(visible_request())
        .await
        .expect("lookup");

    assert_eq!(state.advisory_lookups.lock().await.len(), 2);
    assert_eq!(result.nodes[0].advisories.len(), 1);
    assert!(result.nodes[0].error.is_none());

    server.abort();
}
