use crate::*;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, Uri, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kukuri_cn_protocol::{
    AUTH_REQUIRED_CODE, ApiErrorBody, INDEX_QUERY_NOT_ACTIVATED_CODE,
    INDEX_QUERY_NOT_CONFIGURED_CODE, IndexEntryView, IndexQueryParams, IndexQueryResponse,
    IndexScopeKind,
};
use kukuri_desktop_runtime::{CommunityNodeIndexQueryRequest, SetCommunityNodeConfigNode};
use tokio::sync::Mutex;

const ACCESS_TOKEN: &str = "harness-index-token";

#[derive(Clone)]
struct HarnessIndexState {
    base_url: String,
    object_override: Arc<Mutex<Option<(String, String)>>>,
    reports: Arc<Mutex<Vec<serde_json::Value>>>,
}

async fn auth_challenge() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "challenge": "harness-index-challenge",
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

async fn rendezvous_heartbeat() -> Json<serde_json::Value> {
    Json(serde_json::json!({"expires_in_seconds": 45, "topics": []}))
}

async fn bootstrap_nodes(State(state): State<HarnessIndexState>) -> Json<serde_json::Value> {
    let base_url = state.base_url;
    Json(serde_json::json!({
        "nodes": [{
            "base_url": base_url.clone(),
            "resolved_urls": {
                "public_base_url": base_url,
                "connectivity_urls": [],
                "seed_peers": []
            }
        }]
    }))
}

async fn index_query(
    State(state): State<HarnessIndexState>,
    uri: Uri,
    headers: HeaderMap,
    Query(params): Query<IndexQueryParams>,
) -> Response {
    let authenticated = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {ACCESS_TOKEN}"));
    if !authenticated {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorBody {
                code: AUTH_REQUIRED_CODE.to_string(),
                message: "authentication is required".to_string(),
            }),
        )
            .into_response();
    }
    if params.q.as_deref() == Some("unconfigured") {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                code: INDEX_QUERY_NOT_CONFIGURED_CODE.to_string(),
                message: "index query is not configured".to_string(),
            }),
        )
            .into_response();
    }
    if params.q.as_deref() == Some("inactive") {
        return (
            // 実サーバ(cn-user-api)は有効化失効を 404 で返す。契約を一致させる(#712)。
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                code: INDEX_QUERY_NOT_ACTIVATED_CODE.to_string(),
                message: "index query activation is not current".to_string(),
            }),
        )
            .into_response();
    }
    let operation = uri.path().rsplit('/').next().unwrap_or("search");
    let (scope_kind, scope_id) = match (params.scope_kind.as_deref(), params.scope_id) {
        (Some(kind), Some(scope_id)) => (
            IndexScopeKind::parse(kind).unwrap_or(IndexScopeKind::PublicTopic),
            scope_id,
        ),
        _ => (IndexScopeKind::PublicTopic, "cross-topic".to_string()),
    };
    let overridden = state.object_override.lock().await.clone();
    let (object_id, author_pubkey) =
        overridden.unwrap_or_else(|| (format!("{operation}-object"), "harness-author".to_string()));
    Json(IndexQueryResponse {
        entries: vec![IndexEntryView {
            source_replica_id: None,
            scope_kind,
            scope_id,
            object_id,
            author_pubkey,
            text: format!("{operation} preview\nderived-tag"),
            created_at: 42,
            content_advisories: Vec::new(),
        }],
    })
    .into_response()
}

async fn node_manifest(State(state): State<HarnessIndexState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "node_id": "harness-index-node",
        "node_name": "harness-index-node",
        "node_role": "community-node",
        "server_name": "harness-index-node",
        "manifest_version": "v1",
        "capability_scope": {"available_enabled": ["community_index", "report_endpoint"], "planned_enabled": []},
        "authority_scope": {"applies_to": ["this_node", "communities_indexed_by_this_node"], "does_not_apply_to": ["kukuri_network_as_a_whole"]},
        "p2p_boundary": {"identity_authority": false, "profile_canonical_store": false, "social_graph_canonical_store": false, "content_truth_source": false, "network_wide_authority": false},
        "abuse_contact": "abuse@harness.invalid",
        "report_endpoint": format!("{}/v1/report", state.base_url),
        "terms_url": "",
        "privacy_url": "",
        "moderation_policy_url": ""
    }))
}

async fn submit_report(
    State(state): State<HarnessIndexState>,
    Json(report): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    state.reports.lock().await.push(report);
    Json(serde_json::json!({"reference_id": "harness-report-1"}))
}

fn request(base_url: &str) -> CommunityNodeIndexQueryRequest {
    CommunityNodeIndexQueryRequest {
        base_url: base_url.to_string(),
        query: None,
        scope_kind: None,
        scope_id: None,
        topic_id: None,
        limit: Some(20),
    }
}

pub(crate) async fn run_community_node_index_query_client(
    root: &Path,
    scenario: &ScenarioSpec,
    artifacts_dir: &Path,
) -> Result<HarnessResult> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}", listener.local_addr()?);
    let index_state = HarnessIndexState {
        base_url: base_url.clone(),
        object_override: Arc::new(Mutex::new(None)),
        reports: Arc::new(Mutex::new(Vec::new())),
    };
    let router = Router::new()
        .route("/v1/auth/challenge", post(auth_challenge))
        .route("/v1/auth/verify", post(auth_verify))
        .route("/v1/consents/status", get(consent_status))
        .route("/v1/policies", get(public_policies))
        .route("/v1/bootstrap/heartbeat", post(heartbeat))
        .route("/v1/bootstrap/nodes", get(bootstrap_nodes))
        .route(
            "/v1/rendezvous/topics/heartbeat",
            post(rendezvous_heartbeat),
        )
        .route("/v1/index/search", get(index_query))
        .route("/v1/index/discovery", get(index_query))
        .route("/v1/index/recommendations", get(index_query))
        .route("/v1/node/manifest", get(node_manifest))
        .route("/v1/report", post(submit_report))
        .with_state(index_state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await });

    let runtime_dir = tempfile::Builder::new()
        .prefix("community-index-client-")
        .tempdir_in(artifacts_dir)?;
    let db_path = runtime_dir.path().join("runtime.db");
    let mut runtime = DesktopRuntime::new(&db_path).await?;
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
    if scenario.kind == ScenarioKind::CommunityNodeReportRouting {
        runtime
            .set_my_profile(SetMyProfileRequest {
                name: Some("harness-author".to_string()),
                display_name: Some("Harness Author".to_string()),
                about: Some("observed profile".to_string()),
                picture_upload: None,
                clear_picture: false,
            })
            .await?;
    }

    let overall_timeout = Duration::from_millis(scenario.timeouts.overall_ms);
    let run_result = timeout(overall_timeout, async {
        let mut steps = Vec::new();
        let mut created_object_id = None::<String>;
        for step in &scenario.steps {
            let started_at = Instant::now();
            match step {
                ScenarioStep::CreatePost { content } => {
                    let object_id = runtime
                        .create_post(CreatePostRequest {
                            topic: scenario.fixtures.topic.clone(),
                            content: content.clone(),
                            reply_to: None,
                            channel_ref: ChannelRef::Public,
                            attachments: if scenario.kind
                                == ScenarioKind::CommunityNodeReportRouting
                            {
                                vec![image_attachment_request(
                                    "observed.png",
                                    "image/png",
                                    b"community node report routing attachment",
                                )]
                            } else {
                                Vec::new()
                            },
                            content_labels: Vec::new(),
                        })
                        .await?;
                    *index_state.object_override.lock().await = Some((
                        object_id.clone(),
                        runtime.get_sync_status().await?.local_author_pubkey,
                    ));
                    created_object_id = Some(object_id);
                }
                ScenarioStep::RestartDesktop => {
                    runtime.shutdown().await;
                    runtime = DesktopRuntime::new(&db_path).await?;
                }
                ScenarioStep::SearchCommunityIndex {
                    query,
                    scope_kind,
                    scope_id,
                    expect_object_id,
                } => {
                    let response = runtime
                        .search_community_node_index(CommunityNodeIndexQueryRequest {
                            query: Some(query.clone()),
                            scope_kind: Some(IndexScopeKind::parse(scope_kind)?),
                            scope_id: Some(scope_id.clone()),
                            ..request(base_url.as_str())
                        })
                        .await?;
                    let expected = if expect_object_id == "created_post" {
                        created_object_id.as_deref()
                    } else {
                        Some(expect_object_id.as_str())
                    };
                    anyhow::ensure!(
                        response
                            .entries
                            .first()
                            .map(|entry| entry.object_id.as_str())
                            == expected,
                        "search result mismatch: {:?}",
                        response.entries
                    );
                }
                ScenarioStep::DiscoverCommunityIndex { expect_object_id } => {
                    let response = runtime
                        .discover_community_node_index(request(base_url.as_str()))
                        .await?;
                    anyhow::ensure!(
                        response
                            .entries
                            .first()
                            .map(|entry| entry.object_id.as_str())
                            == Some(expect_object_id.as_str()),
                        "discovery result mismatch: {:?}",
                        response.entries
                    );
                }
                ScenarioStep::RecommendCommunityIndex { expect_object_id } => {
                    let response = runtime
                        .recommend_community_node_index(request(base_url.as_str()))
                        .await?;
                    anyhow::ensure!(
                        response
                            .entries
                            .first()
                            .map(|entry| entry.object_id.as_str())
                            == Some(expect_object_id.as_str()),
                        "recommendation result mismatch: {:?}",
                        response.entries
                    );
                }
                ScenarioStep::AssertCommunityIndexError { query, code } => {
                    let error = runtime
                        .search_community_node_index(CommunityNodeIndexQueryRequest {
                            query: Some(query.clone()),
                            scope_kind: Some(IndexScopeKind::PublicTopic),
                            scope_id: Some(scenario.fixtures.topic.clone()),
                            ..request(base_url.as_str())
                        })
                        .await
                        .expect_err("index query should fail");
                    anyhow::ensure!(
                        error.code == *code,
                        "index error mismatch: expected {code}, got {}",
                        error.code
                    );
                }
                ScenarioStep::AssertNoReportProvenance => {
                    let object_id = created_object_id
                        .as_deref()
                        .context("created post missing")?;
                    let timeline = runtime
                        .list_timeline(ListTimelineRequest {
                            topic: scenario.fixtures.topic.clone(),
                            scope: TimelineScope::Public,
                            cursor: None,
                            limit: Some(20),
                        })
                        .await?;
                    let post = timeline
                        .items
                        .iter()
                        .find(|post| post.object_id == object_id)
                        .context("created post missing from timeline")?;
                    anyhow::ensure!(
                        post.provenance.is_none(),
                        "direct post unexpectedly has provenance"
                    );
                }
                ScenarioStep::AssertObservedReportRouting {
                    expect_capability,
                    expect_subject_kinds,
                } => {
                    let object_id = created_object_id
                        .as_deref()
                        .context("created post missing")?;
                    let timeline = runtime
                        .list_timeline(ListTimelineRequest {
                            topic: scenario.fixtures.topic.clone(),
                            scope: TimelineScope::Public,
                            cursor: None,
                            limit: Some(20),
                        })
                        .await?;
                    let post = timeline
                        .items
                        .iter()
                        .find(|post| post.object_id == object_id)
                        .context("created post missing from timeline")?;
                    let author = runtime
                        .get_author_social_view(AuthorRequest {
                            pubkey: post.author_pubkey.clone(),
                        })
                        .await?;
                    let expected_kinds = if expect_subject_kinds.is_empty() {
                        vec!["post".to_string()]
                    } else {
                        expect_subject_kinds.clone()
                    };
                    let report_subjects = expected_kinds
                        .iter()
                        .map(|kind| match kind.as_str() {
                            "post" => Ok((
                                "post".to_string(),
                                object_id.to_string(),
                                post.provenance
                                    .as_ref()
                                    .context("post provenance missing")?,
                            )),
                            "profile" => Ok((
                                "profile".to_string(),
                                post.author_pubkey.clone(),
                                author
                                    .provenance
                                    .as_ref()
                                    .context("profile provenance missing")?,
                            )),
                            "media" => {
                                let attachment =
                                    post.attachments.first().context("attachment missing")?;
                                Ok((
                                    "media".to_string(),
                                    attachment.hash.clone(),
                                    attachment
                                        .provenance
                                        .as_ref()
                                        .context("attachment provenance missing")?,
                                ))
                            }
                            other => anyhow::bail!("unsupported report subject kind: {other}"),
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let expected_reports = report_subjects
                        .iter()
                        .map(|(kind, id, _)| (kind.clone(), id.clone()))
                        .collect::<Vec<_>>();
                    let reports_before = index_state.reports.lock().await.len();
                    for (subject_kind, subject_id, provenance) in report_subjects {
                        if subject_kind == "media" {
                            anyhow::ensure!(
                                provenance.canonical_source == "blob",
                                "attachment canonical source mismatch"
                            );
                        }
                        let observation = provenance
                            .observed_via
                            .first()
                            .context("observed provenance missing")?;
                        anyhow::ensure!(
                            observation.capability == *expect_capability,
                            "capability mismatch"
                        );
                        let manifest = runtime
                            .fetch_community_node_manifest(CommunityNodeTargetRequest {
                                base_url: observation.node_base_url.clone(),
                            })
                            .await?
                            .manifest
                            .context("manifest missing")?;
                        // 画面側の通報先判定(apps/desktop/src/lib/api/reportRouting.ts、#702)を写す:
                        // 観測能力 community_index は、提供中能力 community_index と責任範囲語彙
                        // communities_indexed_by_this_node の両方を要求する。
                        let required_scope = "communities_indexed_by_this_node";
                        anyhow::ensure!(
                            !manifest.p2p_boundary.network_wide_authority
                                && !manifest.authority_scope.applies_to.is_empty()
                                && !manifest
                                    .authority_scope
                                    .does_not_apply_to
                                    .contains(&observation.capability)
                                && !manifest
                                    .authority_scope
                                    .does_not_apply_to
                                    .iter()
                                    .any(|entry| entry == required_scope)
                                && manifest
                                    .capability_scope
                                    .available_enabled
                                    .iter()
                                    .any(|entry| entry == "community_index")
                                && manifest
                                    .authority_scope
                                    .applies_to
                                    .iter()
                                    .any(|entry| entry == required_scope)
                                && !manifest.report_endpoint.trim().is_empty(),
                            "screen-facing report plan produced no endpoint candidate"
                        );
                        runtime
                            .submit_community_node_report(SubmitCommunityNodeReportRequest {
                                node_base_url: observation.node_base_url.clone(),
                                report_endpoint: manifest.report_endpoint,
                                subject_kind,
                                subject_id,
                                capability: observation.capability.clone(),
                                reason: "spam".to_string(),
                                details: None,
                                reporter_contact: None,
                                appeal: None,
                            })
                            .await?;
                    }
                    let reports = index_state.reports.lock().await;
                    anyhow::ensure!(
                        reports.len() == reports_before + expected_reports.len(),
                        "report count mismatch"
                    );
                    for (report, (expected_kind, expected_id)) in
                        reports[reports_before..].iter().zip(expected_reports)
                    {
                        anyhow::ensure!(
                            report
                                .get("subject_kind")
                                .and_then(serde_json::Value::as_str)
                                == Some(expected_kind.as_str()),
                            "report subject kind mismatch"
                        );
                        anyhow::ensure!(
                            report.get("subject_id").and_then(serde_json::Value::as_str)
                                == Some(expected_id.as_str()),
                            "report subject id mismatch"
                        );
                    }
                }
                other => anyhow::bail!(
                    "unsupported step for community index client scenario: {}",
                    step_name(other)
                ),
            }
            push_named_step(&mut steps, step_name(step), started_at);
        }
        Ok::<_, anyhow::Error>(steps)
    })
    .await
    .context("community index client scenario timed out")?;

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
