use crate::*;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kukuri_cn_protocol::{
    AUTH_REQUIRED_CODE, ApiErrorBody, AppealStatus, Basis, CommunityNodeReportRequest,
    CommunityNodeReportResponse, IndexEntryView, IndexQueryResponse, IndexScopeKind, Proximity,
    ProximityBasisEntry, RELATION_NOT_FOUND_CODE, RelationNeighborsResponse,
    RelationOptoutResponse, RelationReadResponse, RiskSignalTarget, SafetyCategory, Severity,
    TRUST_READ_NOT_ACTIVATED_CODE, TRUST_READ_NOT_CONFIGURED_CODE, TrustBasisEntry,
    TrustComponentKind, TrustReadView, TrustUserReadResponse, Visibility,
};
use kukuri_desktop_runtime::{
    CommunityNodeIndexQueryRequest, CommunityNodeRelationNeighborsRequest,
    CommunityNodeTargetRequest, CommunityNodeUserAdvisoryRequest, SetCommunityNodeConfigNode,
    SubmitCommunityNodeReportRequest,
};

const ACCESS_TOKEN: &str = "harness-trust-relation-token";

/// 異議申し立て対象の固定のリスク判定識別子。
const APPEAL_SIGNAL_ID: &str = "signal-1";

#[derive(Clone)]
struct ServerState {
    base_url: String,
    opted_out: Arc<AtomicBool>,
    /// signal-1 の異議申し立て状態(none → disputed → cleared)。#685 の契約を写す。
    appeal_status: Arc<Mutex<AppealStatus>>,
}

fn authenticated(headers: &HeaderMap) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {ACCESS_TOKEN}"))
}

fn require_auth(headers: &HeaderMap) -> Option<Response> {
    (!authenticated(headers)).then(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorBody {
                code: AUTH_REQUIRED_CODE.to_string(),
                message: "authentication is required".to_string(),
            }),
        )
            .into_response()
    })
}

async fn auth_challenge() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "challenge": "harness-trust-relation-challenge",
        "expires_at": 4_102_444_800_i64
    }))
}

async fn auth_verify() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "access_token": ACCESS_TOKEN,
        "token_type": "Bearer",
        "expires_at": 4_102_444_800_i64,
        "pubkey": "harness-user"
    }))
}

async fn consent_status() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "all_required_accepted": true, "items": [] }))
}

// #857: 認証(JWT 発行)前の公開 policy カタログ。client はこれに同意してから認証する。
async fn public_policies() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "policies": [{
            "policy_slug": "harness-basic",
            "policy_version": 1,
            "title": "Harness Basic",
            "body_markdown": "Harness policy body.",
            "required": true
        }]
    }))
}

async fn heartbeat() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "expires_at": 4_102_444_800_i64 }))
}

async fn bootstrap_nodes(State(state): State<ServerState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "nodes": [{
            "base_url": state.base_url.clone(),
            "resolved_urls": {
                "public_base_url": state.base_url,
                "connectivity_urls": [],
                "seed_peers": []
            }
        }]
    }))
}

fn api_error(code: &str, message: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody {
            code: code.to_string(),
            message: message.to_string(),
        }),
    )
        .into_response()
}

async fn trust_user(
    State(state): State<ServerState>,
    AxumPath(target): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = require_auth(&headers) {
        return response;
    }
    if target.starts_with('c') {
        return api_error(
            TRUST_READ_NOT_CONFIGURED_CODE,
            "trust read is not configured",
        );
    }
    if target.starts_with('d') {
        return api_error(TRUST_READ_NOT_ACTIVATED_CODE, "trust read is not activated");
    }
    // #685 の契約: cleared は根拠に残したまま寄与を 0 にする。none / disputed は寄与を維持する。
    let appeal_status = *state.appeal_status.lock().expect("appeal status lock");
    let contribution = if appeal_status == AppealStatus::Cleared {
        0.0
    } else {
        0.55
    };
    Json(TrustUserReadResponse {
        viewer_pubkey: "harness-user".to_string(),
        view: TrustReadView {
            target_id: target.clone(),
            absolute: 0.5,
            relative: 0.6,
            trust: 0.55,
            w_abs_applied: 0.5,
            computed_at: "2026-08-14T00:00:00Z".to_string(),
            evaluation: None,
            basis: vec![TrustBasisEntry {
                signal_id: APPEAL_SIGNAL_ID.to_string(),
                issuer_node_id: "harness-issuer-node".to_string(),
                target: RiskSignalTarget::UserPubkey,
                target_id: target,
                component: TrustComponentKind::Relative,
                category: SafetyCategory::Spam,
                severity: Severity::Low,
                basis: Basis::ProviderVerdict,
                confidence: Some(75),
                visibility: Visibility::SubscribedNodes,
                appeal_status,
                expires_at: None,
                operator_adjusted_at: None,
                raw_contribution: contribution,
                decay_factor: 1.0,
                relation_weight: 1.0,
                contribution,
            }],
        },
    })
    .into_response()
}

/// 匿名の異議申し立てを受理して signal-1 を disputed にする(#669 / #685 の契約を写す)。
async fn report(
    State(state): State<ServerState>,
    Json(request): Json<CommunityNodeReportRequest>,
) -> Response {
    let Some(appeal) = request.appeal.as_ref() else {
        return api_error(
            "REPORT_NOT_CONFIGURED",
            "only appeals are accepted by this stub",
        );
    };
    if request.reporter_contact.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                code: "INVALID_APPEAL".to_string(),
                message: "appeals must stay anonymous".to_string(),
            }),
        )
            .into_response();
    }
    let mut status = state.appeal_status.lock().expect("appeal status lock");
    if appeal.risk_signal_id != APPEAL_SIGNAL_ID || *status == AppealStatus::Cleared {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                code: "INVALID_APPEAL".to_string(),
                message: "unknown or already resolved risk signal".to_string(),
            }),
        )
            .into_response();
    }
    *status = AppealStatus::Disputed;
    Json(CommunityNodeReportResponse {
        reference_id: Some("harness-appeal-1".to_string()),
        disputed_risk_signal_id: Some(appeal.risk_signal_id.clone()),
    })
    .into_response()
}

/// 距離利用停止の結線確認用の索引検索。opt-out 中は境界外の投稿が抑制され 0 件になる(#665)。
async fn index_search(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    if let Some(response) = require_auth(&headers) {
        return response;
    }
    let entries = if state.opted_out.load(Ordering::SeqCst) {
        Vec::new()
    } else {
        vec![IndexEntryView {
            source_replica_id: None,
            scope_kind: IndexScopeKind::PublicTopic,
            scope_id: "trust-relation".to_string(),
            object_id: "distant-post-1".to_string(),
            author_pubkey: "f".repeat(64),
            text: "distant community post".to_string(),
            created_at: 42,
            content_advisories: Vec::new(),
        }]
    };
    Json(IndexQueryResponse { entries }).into_response()
}

async fn relation_user(AxumPath(target): AxumPath<String>, headers: HeaderMap) -> Response {
    if let Some(response) = require_auth(&headers) {
        return response;
    }
    if target.starts_with('e') {
        return api_error(RELATION_NOT_FOUND_CODE, "relation is unavailable");
    }
    Json(RelationReadResponse {
        viewer_pubkey: "harness-user".to_string(),
        target_pubkey: target,
        proximity: Proximity {
            score: 0.42,
            basis: vec![ProximityBasisEntry {
                feature: "shared_topics".to_string(),
                value: 1.0,
                weight: 0.42,
                contribution: 0.42,
            }],
        },
    })
    .into_response()
}

async fn relation_neighbors(
    headers: HeaderMap,
    Query(_query): Query<std::collections::BTreeMap<String, String>>,
) -> Response {
    if let Some(response) = require_auth(&headers) {
        return response;
    }
    Json(RelationNeighborsResponse {
        viewer_pubkey: "harness-user".to_string(),
        neighbors: vec!["b".repeat(64)],
    })
    .into_response()
}

fn optout_value(state: &ServerState) -> RelationOptoutResponse {
    let enabled = state.opted_out.load(Ordering::SeqCst);
    RelationOptoutResponse {
        pubkey: "harness-user".to_string(),
        opted_out: enabled,
        opted_out_at: enabled.then(|| "2026-08-14T00:00:00Z".to_string()),
        min_proximity: 0.25,
    }
}

async fn get_optout(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    if let Some(response) = require_auth(&headers) {
        return response;
    }
    Json(optout_value(&state)).into_response()
}

async fn set_optout(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    if let Some(response) = require_auth(&headers) {
        return response;
    }
    state.opted_out.store(true, Ordering::SeqCst);
    Json(optout_value(&state)).into_response()
}

async fn clear_optout(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    if let Some(response) = require_auth(&headers) {
        return response;
    }
    state.opted_out.store(false, Ordering::SeqCst);
    Json(optout_value(&state)).into_response()
}

pub(crate) async fn run_community_node_trust_relation_client(
    root: &Path,
    scenario: &ScenarioSpec,
    artifacts_dir: &Path,
) -> Result<HarnessResult> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}", listener.local_addr()?);
    let state = ServerState {
        base_url: base_url.clone(),
        opted_out: Arc::new(AtomicBool::new(false)),
        appeal_status: Arc::new(Mutex::new(AppealStatus::None)),
    };
    let router = Router::new()
        .route("/v1/auth/challenge", post(auth_challenge))
        .route("/v1/auth/verify", post(auth_verify))
        .route("/v1/consents/status", get(consent_status))
        .route("/v1/policies", get(public_policies))
        .route("/v1/bootstrap/heartbeat", post(heartbeat))
        .route("/v1/bootstrap/nodes", get(bootstrap_nodes))
        .route("/v1/trust/users/{target}", get(trust_user))
        .route("/v1/relation/users/{target}", get(relation_user))
        .route("/v1/relation/neighbors", get(relation_neighbors))
        .route(
            "/v1/relation/optout",
            get(get_optout).put(set_optout).delete(clear_optout),
        )
        .route("/v1/report", post(report))
        .route("/v1/index/search", get(index_search))
        .with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await });

    let runtime_dir = tempfile::Builder::new()
        .prefix("trust-relation-client-")
        .tempdir_in(artifacts_dir)?;
    let db_path = runtime_dir.path().join("runtime.db");
    let runtime = DesktopRuntime::new(&db_path).await?;
    runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            trust_node_priority: None,
            nodes: vec![SetCommunityNodeConfigNode {
                content_advisory_enabled: None,
                base_url: base_url.clone(),
            }],
        })
        .await?;
    // #857: 同意成立(公開カタログ提示 → ローカル記録)後にのみ認証・接続が始まる。
    let catalog = runtime
        .fetch_community_node_policies(kukuri_desktop_runtime::FetchCommunityNodePoliciesRequest {
            base_url: base_url.clone(),
            language: Some("ja".to_string()),
        })
        .await?;
    runtime
        .accept_community_node_consents(
            AcceptCommunityNodeConsentsRequest {
                base_url: base_url.clone(),
                documents: catalog
                    .policies
                    .iter()
                    .map(
                        |policy| kukuri_desktop_runtime::CommunityNodeConsentDocumentRef {
                            policy_slug: policy.policy_slug.clone(),
                            policy_version: policy.policy_version,
                            policy_snapshot_revision: policy.policy_snapshot_revision.clone(),
                        },
                    )
                    .collect(),
                language: "ja".to_string(),
            },
            "harness",
        )
        .await?;
    runtime
        .authenticate_community_node(CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await?;

    let run_result = timeout(Duration::from_millis(scenario.timeouts.overall_ms), async {
        let mut steps = Vec::new();
        for step in &scenario.steps {
            let started_at = Instant::now();
            match step {
                ScenarioStep::ReadCommunityTrust {
                    target_pubkey,
                    expect_trust_millis,
                } => {
                    let value = runtime
                        .read_community_node_trust_user(CommunityNodeUserAdvisoryRequest {
                            base_url: base_url.clone(),
                            target_pubkey: target_pubkey.clone(),
                        })
                        .await?;
                    anyhow::ensure!(
                        (value.view.trust * 1000.0).round() as i64 == *expect_trust_millis,
                        "trust mismatch"
                    );
                }
                ScenarioStep::ReadCommunityRelation {
                    target_pubkey,
                    expect_score_millis,
                } => {
                    let value = runtime
                        .read_community_node_relation_user(CommunityNodeUserAdvisoryRequest {
                            base_url: base_url.clone(),
                            target_pubkey: target_pubkey.clone(),
                        })
                        .await?;
                    anyhow::ensure!(
                        (value.proximity.score * 1000.0).round() as i64 == *expect_score_millis,
                        "relation mismatch"
                    );
                }
                ScenarioStep::AssertCommunityRelationNeighbor { pubkey } => {
                    let value = runtime
                        .list_community_node_relation_neighbors(
                            CommunityNodeRelationNeighborsRequest {
                                base_url: base_url.clone(),
                                limit: Some(20),
                            },
                        )
                        .await?;
                    anyhow::ensure!(value.neighbors.contains(pubkey), "neighbor missing");
                }
                ScenarioStep::AssertRelationOptout {
                    operation,
                    expect_enabled,
                } => {
                    let request = CommunityNodeTargetRequest {
                        base_url: base_url.clone(),
                    };
                    let value = match operation.as_str() {
                        "get" => runtime.get_community_node_relation_optout(request).await?,
                        "set" => runtime.set_community_node_relation_optout(request).await?,
                        "clear" => {
                            runtime
                                .clear_community_node_relation_optout(request)
                                .await?
                        }
                        _ => anyhow::bail!("unsupported optout operation: {operation}"),
                    };
                    anyhow::ensure!(value.opted_out == *expect_enabled, "optout mismatch");
                }
                ScenarioStep::AssertTrustRelationError {
                    endpoint,
                    target_pubkey,
                    code,
                } => {
                    let request = CommunityNodeUserAdvisoryRequest {
                        base_url: base_url.clone(),
                        target_pubkey: target_pubkey.clone(),
                    };
                    let error = if endpoint == "trust" {
                        runtime
                            .read_community_node_trust_user(request)
                            .await
                            .expect_err("trust request should fail")
                    } else {
                        runtime
                            .read_community_node_relation_user(request)
                            .await
                            .expect_err("relation request should fail")
                    };
                    anyhow::ensure!(
                        error.code == *code,
                        "error code mismatch: expected {code}, got {}",
                        error.code
                    );
                }
                ScenarioStep::SubmitCommunityAppeal {
                    target_pubkey,
                    risk_signal_id,
                } => {
                    // 匿名の異議申し立て(#669)。受付先は #703 の制約どおり構成済みノードの
                    // 同一オリジンの受付先を使う。
                    let submitted = runtime
                        .submit_community_node_report(SubmitCommunityNodeReportRequest {
                            node_base_url: base_url.clone(),
                            report_endpoint: format!("{base_url}/v1/report"),
                            subject_kind: "profile".to_string(),
                            subject_id: target_pubkey.clone(),
                            capability: "trust_signal".to_string(),
                            reason: "other".to_string(),
                            details: None,
                            reporter_contact: None,
                            appeal: Some(kukuri_cn_protocol::CommunityNodeReportAppeal {
                                risk_signal_id: risk_signal_id.clone(),
                            }),
                        })
                        .await?;
                    anyhow::ensure!(
                        submitted.disputed_risk_signal_id.as_deref()
                            == Some(risk_signal_id.as_str()),
                        "disputed risk signal mismatch"
                    );
                }
                ScenarioStep::AssertTrustBasisAppeal {
                    target_pubkey,
                    signal_id,
                    expect_status,
                    expect_contribution_zero,
                } => {
                    // 再取得で係争中 / 解消済みの状態と寄与(#685 の契約)を確認する。
                    let value = runtime
                        .read_community_node_trust_user(CommunityNodeUserAdvisoryRequest {
                            base_url: base_url.clone(),
                            target_pubkey: target_pubkey.clone(),
                        })
                        .await?;
                    let entry = value
                        .view
                        .basis
                        .iter()
                        .find(|entry| entry.signal_id == *signal_id)
                        .context("appealed basis entry missing")?;
                    let status = match entry.appeal_status {
                        AppealStatus::None => "none",
                        AppealStatus::Disputed => "disputed",
                        AppealStatus::Cleared => "cleared",
                    };
                    anyhow::ensure!(
                        status == expect_status,
                        "appeal status mismatch: expected {expect_status}, got {status}"
                    );
                    let contribution_zero = entry.contribution == 0.0;
                    anyhow::ensure!(
                        contribution_zero == *expect_contribution_zero,
                        "contribution mismatch: expected zero={expect_contribution_zero}, got {}",
                        entry.contribution
                    );
                }
                ScenarioStep::ResolveCommunityAppeal => {
                    // 運営者の認容をスタブ上で確定させる。実際の審査の原子性・寄与反映は
                    // サーバ側結合試験(post_appeal_acceptance_is_visible_after_trust_refetch)で
                    // 固定済みのため、ここでは審査後のクライアント表示だけを確認する。
                    let mut status = state.appeal_status.lock().expect("appeal status lock");
                    anyhow::ensure!(
                        *status == AppealStatus::Disputed,
                        "only a disputed appeal can be resolved"
                    );
                    *status = AppealStatus::Cleared;
                }
                ScenarioStep::AssertCommunityIndexEntryCount {
                    query,
                    scope_kind,
                    scope_id,
                    expect_entry_count,
                } => {
                    // 距離利用停止の結線確認(#665): 設定前後で索引応答が変わり、解除後に戻る。
                    let response = runtime
                        .search_community_node_index(CommunityNodeIndexQueryRequest {
                            base_url: base_url.clone(),
                            query: Some(query.clone()),
                            scope_kind: Some(match scope_kind.as_str() {
                                "public_topic" => IndexScopeKind::PublicTopic,
                                "private_channel" => IndexScopeKind::PrivateChannel,
                                other => anyhow::bail!("unsupported scope kind: {other}"),
                            }),
                            scope_id: Some(scope_id.clone()),
                            topic_id: None,
                            limit: Some(20),
                        })
                        .await?;
                    anyhow::ensure!(
                        response.entries.len() == *expect_entry_count,
                        "index entry count mismatch: expected {expect_entry_count}, got {}",
                        response.entries.len()
                    );
                }
                other => anyhow::bail!(
                    "unsupported trust relation client step: {}",
                    step_name(other)
                ),
            }
            push_named_step(&mut steps, step_name(step), started_at);
        }
        Ok::<_, anyhow::Error>(steps)
    })
    .await
    .context("community trust relation client scenario timed out")?;

    runtime.shutdown().await;
    server.abort();
    let result = HarnessResult {
        status: HarnessStatus::Pass,
        scenario: scenario.name.clone(),
        steps: run_result?,
        artifacts: vec![db_path.display().to_string()],
        metrics_snapshot: None,
    };
    write_result_artifact(root, artifacts_dir, &result)?;
    Ok(result)
}
