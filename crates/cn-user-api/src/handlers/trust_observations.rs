//! ブロック / ミュート観測の受付と取消(ADR 0026 §8.3、#1061)。
//!
//! 観測は observer 本人が署名した envelope だけを受け付け、bearer identity と署名者の一致、
//! 必須同意、任意文書 `trust_observation_sharing` への同意をすべて確認してから保存する。
//! 検証に失敗した要求は 1 件も保存しない。

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use kukuri_cn_core::{
    ApiError, ApiResult, StoreTrustObservationsOutcome, TRUST_OBSERVATION_MAX_CLOCK_SKEW_SECONDS,
    TrustObservationSharingStatus, require_bearer_identity, revoke_trust_observation_sharing,
    store_trust_observations,
};
use kukuri_cn_protocol::{
    INVALID_TRUST_OBSERVATION_CODE, TRUST_OBSERVATION_SHARING_CONSENT_REQUIRED_CODE,
    TRUST_OBSERVATION_SHARING_NOT_OFFERED_CODE, TRUST_OBSERVATIONS_MAX_ENVELOPES,
    TRUST_READ_NOT_ACTIVATED_CODE, TRUST_READ_NOT_CONFIGURED_CODE, TrustObservationsRevokeResponse,
    TrustObservationsSubmitRequest, TrustObservationsSubmitResponse,
};
use kukuri_core::parse_trust_observation;

use crate::errors::{TrustRelationError, TrustRelationOperation, trust_relation_error};
use crate::handlers::trust_relation::require_trust_read;
use crate::state::UserApiState;

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        INVALID_TRUST_OBSERVATION_CODE,
        message,
    )
}

fn sharing_rejection(status: TrustObservationSharingStatus) -> ApiError {
    match status {
        TrustObservationSharingStatus::NotOffered => ApiError::new(
            StatusCode::NOT_FOUND,
            TRUST_OBSERVATION_SHARING_NOT_OFFERED_CODE,
            "this community node does not accept block / mute observations",
        ),
        TrustObservationSharingStatus::NotAccepted | TrustObservationSharingStatus::Active => {
            ApiError::new(
                StatusCode::FORBIDDEN,
                TRUST_OBSERVATION_SHARING_CONSENT_REQUIRED_CODE,
                "observation sharing consent has not been accepted",
            )
        }
    }
}

/// 観測の提供(`POST /v1/trust/observations`)。
pub(crate) async fn submit_trust_observations(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<TrustObservationsSubmitRequest>,
) -> ApiResult<Json<TrustObservationsSubmitResponse>> {
    let (_, observer) = require_trust_read(&state, &headers).await?;
    if request.envelopes.is_empty() || request.envelopes.len() > TRUST_OBSERVATIONS_MAX_ENVELOPES {
        return Err(invalid(format!(
            "envelopes must contain 1 to {TRUST_OBSERVATIONS_MAX_ENVELOPES} observations"
        )));
    }
    let now = chrono::Utc::now();
    let max_observed_at = (now
        + chrono::Duration::seconds(TRUST_OBSERVATION_MAX_CLOCK_SKEW_SECONDS))
    .timestamp_millis();
    let mut observations = Vec::with_capacity(request.envelopes.len());
    for envelope in &request.envelopes {
        let observation = parse_trust_observation(envelope)
            .map_err(|error| invalid(format!("invalid observation envelope: {error}")))?
            .ok_or_else(|| invalid("envelope kind is not a block / mute observation"))?;
        if observation.observer_pubkey.as_str() != observer {
            return Err(invalid(
                "observation signer must match the authenticated identity",
            ));
        }
        if observation.observed_at > max_observed_at || observation.observed_at < 0 {
            return Err(invalid("observation timestamp is out of range"));
        }
        observations.push(observation);
    }
    let outcome = store_trust_observations(&state.pool, observer.as_str(), &observations, now)
        .await
        .map_err(|source| {
            // 署名者・時刻の検証は上で済ませているので、ここでの失敗は保存側の不整合として扱う。
            TrustRelationError::trust_read(TrustRelationOperation::StoreObservations, source)
        })
        .map_err(trust_relation_error)?;
    match outcome {
        StoreTrustObservationsOutcome::Rejected(status) => Err(sharing_rejection(status)),
        StoreTrustObservationsOutcome::Stored { stored, ignored } => {
            Ok(Json(TrustObservationsSubmitResponse { stored, ignored }))
        }
    }
}

/// 自分の観測の全削除と観測提供の同意の取消(`DELETE /v1/trust/observations`)。
///
/// 必須同意・任意文書の同意状態によらず、本人認証だけで実行できる(取消は常に受け付ける)。
pub(crate) async fn revoke_trust_observations(
    State(state): State<UserApiState>,
    headers: HeaderMap,
) -> ApiResult<Json<TrustObservationsRevokeResponse>> {
    if state.trust_read.is_none() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            TRUST_READ_NOT_CONFIGURED_CODE,
            "this community node does not provide trust / relation reads",
        ));
    }
    if !state.readiness_activation_is_valid().await {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            TRUST_READ_NOT_ACTIVATED_CODE,
            "this community node trust activation is not current",
        ));
    }
    let observer = require_bearer_identity(&state.pool, &state.jwt_config, &headers)
        .await?
        .pubkey;
    let deleted = revoke_trust_observation_sharing(&state.pool, observer.as_str())
        .await
        .map_err(|source| {
            TrustRelationError::trust_read(TrustRelationOperation::RevokeObservations, source)
        })
        .map_err(trust_relation_error)?;
    Ok(Json(TrustObservationsRevokeResponse { deleted }))
}
