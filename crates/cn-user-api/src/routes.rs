//! route 定義と起動(serve / tracing)。desktop 対向のパスは kukuri-cn-protocol の
//! 定数を参照する(パス変更が client / server 両側にコンパイルエラーとして届く)。

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kukuri_cn_operator::CommunityNodeManifest;
use kukuri_cn_protocol::{
    ADVISORY_LOOKUP_PATH, AUTH_CHALLENGE_PATH, AUTH_VERIFY_PATH, BOOTSTRAP_HEARTBEAT_PATH,
    BOOTSTRAP_NODES_PATH, CONSENTS_PATH, CONSENTS_STATUS_PATH, DOME_HOSTING_ACTIVATE_PATH,
    DOME_HOSTING_ASSIGNMENTS_PATH, DOME_HOSTING_LAYOUT_CANDIDATE_PATH, DOME_HOSTING_RELEASE_PATH,
    DOME_HOSTING_SESSION_INPUT_PATH, DOME_HOSTING_SESSION_WS_PATH,
    DOME_HOSTING_SNAPSHOT_RESYNC_PATH, DOME_HOSTING_STATUS_ROUTE, DOME_TRANSITION_ABORT_PATH,
    DOME_TRANSITION_COMMIT_PATH, DOME_TRANSITION_PREPARE_PATH, INDEX_DISCOVERY_PATH,
    INDEX_RECOMMENDATIONS_PATH, INDEX_SEARCH_PATH, INDEXING_REQUESTS_PATH, INDEXING_STATUS_PATH,
    NODE_MANIFEST_PATH, POLICIES_PATH, RELATION_NEIGHBORS_PATH, RELATION_OPTOUT_PATH,
    RELATION_USERS_ROUTE, REPORT_PATH, RIGHTS_REQUEST_CREATE_PATH, RIGHTS_REQUEST_FORM_PATH,
    RIGHTS_REQUEST_SCOPE_PATH, RIGHTS_REQUEST_STATUS_PATH, RIGHTS_REQUEST_WITHDRAW_PATH,
    TESTER_FEEDBACK_PATH, TOPIC_RENDEZVOUS_HEARTBEAT_PATH, TRUST_EVALUATIONS_PATH,
    TRUST_OBSERVATIONS_PATH, TRUST_USERS_ROUTE,
};
use serde_json::{Value, json};
use tower_http::trace::TraceLayer;

use crate::admin::admin_router;
use crate::config::{RateLimitConfig, UserApiConfig};
use crate::dome_hosting::{
    abort_dome_transition, activate_dome_hosting, assign_dome_hosting,
    capture_dome_layout_candidate, commit_dome_transition, dome_hosting_session_ws,
    dome_hosting_status, prepare_dome_transition, release_dome_hosting,
    resync_dome_hosting_snapshots, submit_dome_hosting_input,
};
use crate::handlers::advisories::lookup_content_advisories;
use crate::handlers::auth::{auth_challenge, auth_verify};
use crate::handlers::bootstrap::{
    bootstrap_heartbeat, bootstrap_nodes, topic_rendezvous_heartbeat,
};
use crate::handlers::consents::{
    accept_consents_handler, consent_status, public_policies, public_policy_revision,
    public_policy_revisions, public_policy_snapshot_revision,
};
use crate::handlers::indexing::{
    index_discovery, index_recommendations, index_search, indexing_status,
    revoke_own_indexing_request, submit_indexing_request,
};
use crate::handlers::reports::submit_report;
use crate::handlers::rights_requests::{
    rights_request_form, rights_request_scope, rights_request_status, rights_request_status_form,
    rights_request_status_form_submit, submit_rights_request, submit_rights_request_form,
    withdraw_rights_request_form_submit, withdraw_rights_request_handler,
};
use crate::handlers::tester_feedback::submit_tester_feedback;
use crate::handlers::transmission_prevention::transmission_prevention_status;
use crate::handlers::trust_observations::{revoke_trust_observations, submit_trust_observations};
use crate::handlers::trust_relation::{
    relation_neighbors, relation_optout_clear, relation_optout_get, relation_optout_set,
    relation_user_read, trust_evaluations, trust_pull, trust_user_read,
};
use crate::rate_limit::apply_rate_limit;
use crate::state::{ManifestState, UserApiState, build_runtime_state};
use kukuri_cn_core::{apply_retention_policy, cleanup_expired, cleanup_trust_observations};

pub fn app_router(state: UserApiState) -> Router {
    let manifest = manifest_routes(
        state.manifest.clone(),
        Arc::clone(&state.public_disclosures),
    );
    let api = Router::new()
        .route("/healthz", get(healthz))
        .route(AUTH_CHALLENGE_PATH, post(auth_challenge))
        .route(AUTH_VERIFY_PATH, post(auth_verify))
        .route(CONSENTS_STATUS_PATH, get(consent_status))
        .route(CONSENTS_PATH, post(accept_consents_handler))
        .route(POLICIES_PATH, get(public_policies))
        .route(
            "/v1/policies/{policy_slug}/revisions",
            get(public_policy_revisions),
        )
        .route(
            "/v1/policies/{policy_slug}/revisions/{policy_version}",
            get(public_policy_revision),
        )
        .route(
            "/v1/policies/{policy_slug}/snapshots/{policy_snapshot_revision}",
            get(public_policy_snapshot_revision),
        )
        .route(DOME_HOSTING_ASSIGNMENTS_PATH, post(assign_dome_hosting))
        .route(DOME_HOSTING_ACTIVATE_PATH, post(activate_dome_hosting))
        .route(DOME_HOSTING_RELEASE_PATH, post(release_dome_hosting))
        .route(DOME_HOSTING_STATUS_ROUTE, get(dome_hosting_status))
        .route(
            DOME_HOSTING_SESSION_INPUT_PATH,
            post(submit_dome_hosting_input),
        )
        .route(DOME_HOSTING_SESSION_WS_PATH, get(dome_hosting_session_ws))
        .route(DOME_TRANSITION_PREPARE_PATH, post(prepare_dome_transition))
        .route(DOME_TRANSITION_COMMIT_PATH, post(commit_dome_transition))
        .route(DOME_TRANSITION_ABORT_PATH, post(abort_dome_transition))
        .route(
            DOME_HOSTING_LAYOUT_CANDIDATE_PATH,
            post(capture_dome_layout_candidate),
        )
        .route(
            DOME_HOSTING_SNAPSHOT_RESYNC_PATH,
            post(resync_dome_hosting_snapshots),
        )
        .route(BOOTSTRAP_NODES_PATH, get(bootstrap_nodes))
        .route(BOOTSTRAP_HEARTBEAT_PATH, post(bootstrap_heartbeat))
        .route(
            TOPIC_RENDEZVOUS_HEARTBEAT_PATH,
            post(topic_rendezvous_heartbeat),
        )
        .route(REPORT_PATH, post(submit_report))
        .route(TESTER_FEEDBACK_PATH, post(submit_tester_feedback))
        .route(RIGHTS_REQUEST_SCOPE_PATH, get(rights_request_scope))
        .route(RIGHTS_REQUEST_CREATE_PATH, post(submit_rights_request))
        .route(RIGHTS_REQUEST_STATUS_PATH, post(rights_request_status))
        .route(
            RIGHTS_REQUEST_WITHDRAW_PATH,
            post(withdraw_rights_request_handler),
        )
        .route(
            RIGHTS_REQUEST_FORM_PATH,
            get(rights_request_form).post(submit_rights_request_form),
        )
        .route(
            "/rights-requests/status",
            get(rights_request_status_form).post(rights_request_status_form_submit),
        )
        .route(
            "/rights-requests/withdraw",
            post(withdraw_rights_request_form_submit),
        )
        .route(
            INDEXING_REQUESTS_PATH,
            post(submit_indexing_request).delete(revoke_own_indexing_request),
        )
        .route(INDEXING_STATUS_PATH, get(indexing_status))
        .route(INDEX_SEARCH_PATH, get(index_search))
        .route(INDEX_DISCOVERY_PATH, get(index_discovery))
        .route(INDEX_RECOMMENDATIONS_PATH, get(index_recommendations))
        .route(ADVISORY_LOOKUP_PATH, post(lookup_content_advisories))
        .route(
            "/v1/transmission-preventions/{subject_kind}/{subject_id}",
            get(transmission_prevention_status),
        )
        .route(TRUST_USERS_ROUTE, get(trust_user_read))
        .route(TRUST_EVALUATIONS_PATH, post(trust_evaluations))
        .route(
            TRUST_OBSERVATIONS_PATH,
            post(submit_trust_observations).delete(revoke_trust_observations),
        )
        .route("/v1/trust/pull/{pubkey}", get(trust_pull))
        .route(RELATION_USERS_ROUTE, get(relation_user_read))
        .route(RELATION_NEIGHBORS_PATH, get(relation_neighbors))
        .route(
            RELATION_OPTOUT_PATH,
            get(relation_optout_get)
                .put(relation_optout_set)
                .delete(relation_optout_clear),
        )
        .with_state(state);
    api.merge(manifest).layer(TraceLayer::new_for_http())
}

/// 公開 manifest endpoint。unauthenticated で取得できる。
///
/// `GET /.well-known/kukuri/community-node.json` と `GET /v1/node/manifest` の
/// 両方を同じ handler で提供する。manifest 単独でテスト・配信できるよう、DB を
/// 必要としない最小 state を持つ独立 router にしている。
pub fn manifest_routes(
    manifest: Option<Arc<CommunityNodeManifest>>,
    public_disclosures: Arc<BTreeMap<String, String>>,
) -> Router {
    Router::new()
        .route(
            "/.well-known/kukuri/community-node.json",
            get(node_manifest),
        )
        .route(NODE_MANIFEST_PATH, get(node_manifest))
        .route("/terms", get(terms))
        .route("/privacy", get(privacy))
        .route("/external-transmission", get(external_transmission))
        .route("/moderation-policy", get(moderation_policy))
        .route("/abuse-policy", get(abuse_policy))
        .route("/data-retention", get(data_retention))
        .route(
            "/rights-infringement-policy",
            get(rights_infringement_policy),
        )
        .with_state(ManifestState {
            manifest,
            public_disclosures,
        })
}

async fn terms(State(state): State<ManifestState>) -> Response {
    disclosure_response(&state, "terms.md")
}

async fn privacy(State(state): State<ManifestState>) -> Response {
    disclosure_response(&state, "privacy-policy.md")
}

async fn external_transmission(State(state): State<ManifestState>) -> Response {
    disclosure_response(&state, "external-transmission-notice.md")
}

async fn moderation_policy(State(state): State<ManifestState>) -> Response {
    disclosure_response(&state, "moderation-policy.md")
}

async fn abuse_policy(State(state): State<ManifestState>) -> Response {
    disclosure_response(&state, "abuse-policy.md")
}

async fn data_retention(State(state): State<ManifestState>) -> Response {
    disclosure_response(&state, "data-retention-policy.md")
}

async fn rights_infringement_policy(State(state): State<ManifestState>) -> Response {
    disclosure_response(&state, "rights-infringement-policy.md")
}

fn disclosure_response(state: &ManifestState, filename: &str) -> Response {
    let Some(content) = state.public_disclosures.get(filename) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "disclosure_not_configured",
                "message": "this community node does not publish this disclosure"
            })),
        )
            .into_response();
    };
    let mut response = content.clone().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}

/// public manifest を返す。設定されていなければ 404(client は別経路へ fallback)。
async fn node_manifest(State(state): State<ManifestState>) -> Response {
    match state.manifest {
        Some(manifest) => {
            let mut response = Json(manifest.as_ref()).into_response();
            // client が安全に cache できるようにする(private secret は含まれない)。
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=300"),
            );
            response
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "manifest_not_configured",
                "message": "this community node does not publish a manifest"
            })),
        )
            .into_response(),
    }
}

pub async fn run_from_env() -> Result<()> {
    init_tracing();

    let config = UserApiConfig::from_env()?;
    let bind_addr = config.bind_addr;
    let rate_limit = RateLimitConfig::from_env()?;
    let state = build_runtime_state(&config).await?;
    spawn_retention_cleanup(state.clone());
    let admin_bind_addr = std::env::var("COMMUNITY_NODE_ADMIN_BIND_ADDR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.parse::<SocketAddr>())
        .transpose()
        .context("invalid COMMUNITY_NODE_ADMIN_BIND_ADDR")?;
    let admin_app = admin_bind_addr.map(|_| admin_router(state.clone()));
    let app = apply_rate_limit(app_router(state), &rate_limit)?;
    if rate_limit.enabled {
        tracing::info!(
            per_second = rate_limit.per_second,
            burst = rate_limit.burst,
            trust_forwarded_for = rate_limit.trust_forwarded_for,
            "community-node user-api rate limit enabled"
        );
    }
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("failed to bind user api at {bind_addr}"))?;
    tracing::info!(bind_addr = %bind_addr, "community-node user-api listening");
    let user_server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    );
    if let (Some(admin_bind_addr), Some(admin_app)) = (admin_bind_addr, admin_app) {
        let admin_listener = tokio::net::TcpListener::bind(admin_bind_addr)
            .await
            .with_context(|| format!("failed to bind admin api at {admin_bind_addr}"))?;
        tracing::info!(bind_addr = %admin_bind_addr, "community-node IAP admin listening");
        tokio::select! {
            result = user_server => result?,
            result = axum::serve(admin_listener, admin_app) => result?,
        }
    } else {
        user_server.await?;
    }
    Ok(())
}

fn spawn_retention_cleanup(state: UserApiState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            let result = async {
                apply_retention_policy(&state.pool, &state.retention).await?;
                let now = chrono::Utc::now();
                let counts = cleanup_expired(&state.pool, now).await?;
                let trust_observations = cleanup_trust_observations(&state.pool, now).await?;
                tracing::info!(
                    trust_observations,
                    "community-node trust observation cleanup completed"
                );
                anyhow::Ok(counts)
            }
            .await;
            match result {
                Ok(counts) => tracing::info!(?counts, "community-node retention cleanup completed"),
                Err(error) => tracing::error!(%error, "community-node retention cleanup failed"),
            }
        }
    });
}

pub(crate) fn init_tracing() {
    kukuri_cn_runtime_support::init_tracing("info,kukuri_cn_user_api=debug");
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
