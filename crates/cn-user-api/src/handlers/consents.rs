//! 同意(consents)。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use kukuri_cn_core::{
    ApiError, ApiResult, accept_consents, get_consent_status, get_policy_revision,
    get_policy_snapshot_revision, list_policies_for_language, list_policy_revisions,
    require_bearer_pubkey,
};
use kukuri_cn_protocol::{
    AcceptConsentsRequest, CommunityNodeConsentStatus, CommunityNodePoliciesResponse,
    CommunityNodePolicyDocument,
};
use serde::Deserialize;

use crate::errors::{AccountLifecycleError, AccountLifecycleOperation, account_lifecycle_error};
use crate::state::UserApiState;

/// 認証不要の公開 policy カタログ(#857)。同意提示に必要な文書一覧・本文・版だけを
/// 返し、ユーザー固有情報を含まない。同意記録の読み書き(status / accept)は
/// 引き続き bearer 認証を要求する。
pub(crate) async fn public_policies(
    State(state): State<UserApiState>,
    Query(query): Query<PolicyLanguageQuery>,
) -> ApiResult<Json<CommunityNodePoliciesResponse>> {
    let policies = list_policies_for_language(&state.pool, query.language.as_deref())
        .await
        .map_err(|error| {
            ApiError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                error.to_string(),
            )
        })?;
    if policies.is_empty() {
        return Err(ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            "POLICIES_NOT_CONFIGURED",
            "this community node does not publish a versioned policy catalog",
        ));
    }
    let policy_snapshot_revision = policies
        .iter()
        .find_map(|policy| policy.policy_snapshot_revision.clone());
    Ok(Json(CommunityNodePoliciesResponse {
        policies: with_policy_kinds(&state, policies),
        policy_snapshot_revision,
    }))
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PolicyLanguageQuery {
    language: Option<String>,
}

/// #1192: slug は operator が自由に決めるため、文書の役割は operator config の
/// `kind` から付与する。現在の config に無い slug(退役 revision 等)では付けない。
fn with_policy_kind(
    state: &UserApiState,
    mut policy: CommunityNodePolicyDocument,
) -> CommunityNodePolicyDocument {
    policy.policy_kind = state.policy_kinds.get(&policy.policy_slug).cloned();
    policy
}

fn with_policy_kinds(
    state: &UserApiState,
    policies: Vec<CommunityNodePolicyDocument>,
) -> Vec<CommunityNodePolicyDocument> {
    policies
        .into_iter()
        .map(|policy| with_policy_kind(state, policy))
        .collect()
}

pub(crate) async fn public_policy_revisions(
    State(state): State<UserApiState>,
    Path(policy_slug): Path<String>,
) -> ApiResult<Json<CommunityNodePoliciesResponse>> {
    let policies = list_policy_revisions(&state.pool, &policy_slug)
        .await
        .map_err(|error| {
            ApiError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                error.to_string(),
            )
        })?;
    if policies.is_empty() {
        return Err(ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            "POLICY_NOT_FOUND",
            "the requested policy does not exist",
        ));
    }
    Ok(Json(CommunityNodePoliciesResponse {
        policy_snapshot_revision: policies
            .first()
            .and_then(|policy| policy.policy_snapshot_revision.clone()),
        policies: with_policy_kinds(&state, policies),
    }))
}

pub(crate) async fn public_policy_revision(
    State(state): State<UserApiState>,
    Path((policy_slug, policy_version)): Path<(String, i32)>,
    Query(query): Query<PolicyLanguageQuery>,
) -> ApiResult<Json<CommunityNodePolicyDocument>> {
    let policy = get_policy_revision(
        &state.pool,
        &policy_slug,
        policy_version,
        query.language.as_deref(),
    )
    .await
    .map_err(|error| {
        ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            error.to_string(),
        )
    })?
    .ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            "POLICY_REVISION_NOT_FOUND",
            "the requested policy revision does not exist",
        )
    })?;
    Ok(Json(with_policy_kind(&state, policy)))
}

pub(crate) async fn public_policy_snapshot_revision(
    State(state): State<UserApiState>,
    Path((policy_slug, policy_snapshot_revision)): Path<(String, String)>,
    Query(query): Query<PolicyLanguageQuery>,
) -> ApiResult<Json<CommunityNodePolicyDocument>> {
    let policy = get_policy_snapshot_revision(
        &state.pool,
        &policy_slug,
        &policy_snapshot_revision,
        query.language.as_deref(),
    )
    .await
    .map_err(|error| {
        ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            error.to_string(),
        )
    })?
    .ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            "POLICY_REVISION_NOT_FOUND",
            "the requested policy snapshot revision does not exist",
        )
    })?;
    Ok(Json(with_policy_kind(&state, policy)))
}

pub(crate) async fn consent_status(
    State(state): State<UserApiState>,
    headers: HeaderMap,
) -> ApiResult<Json<CommunityNodeConsentStatus>> {
    let pubkey = require_bearer_pubkey(&state.pool, &state.jwt_config, &headers).await?;
    let status = get_consent_status(&state.pool, pubkey.as_str())
        .await
        .map_err(|source| {
            AccountLifecycleError::infrastructure(
                AccountLifecycleOperation::GetConsentStatus,
                source,
            )
        })
        .map_err(account_lifecycle_error)?;
    Ok(Json(status))
}

pub(crate) async fn accept_consents_handler(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<AcceptConsentsRequest>,
) -> ApiResult<Json<CommunityNodeConsentStatus>> {
    let pubkey = require_bearer_pubkey(&state.pool, &state.jwt_config, &headers).await?;
    let status = accept_consents(
        &state.pool,
        pubkey.as_str(),
        &request.policy_slugs,
        request.policy_snapshot_revision.as_deref(),
    )
    .await
    .map_err(|source| {
        if source.to_string().contains("policy snapshot changed") {
            ApiError::new(
                axum::http::StatusCode::CONFLICT,
                "POLICY_SNAPSHOT_CHANGED",
                "the policy catalog changed; reload it before accepting",
            )
        } else {
            account_lifecycle_error(AccountLifecycleError::infrastructure(
                AccountLifecycleOperation::AcceptConsents,
                source,
            ))
        }
    })?;
    Ok(Json(status))
}
