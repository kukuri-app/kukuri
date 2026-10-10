//! テスターフィードバック受付(#802 / ADR 0039)。

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use kukuri_cn_core::{
    ApiError, ApiResult, NewTesterFeedback, insert_tester_feedback, require_bearer_identity,
    require_consents,
};
use kukuri_cn_protocol::{
    CommunityNodeTesterFeedbackRequest, CommunityNodeTesterFeedbackResponse,
    INVALID_TESTER_FEEDBACK_CODE, TESTER_FEEDBACK_MAX_CHARS, TESTER_FEEDBACK_NOT_CONFIGURED_CODE,
};

use crate::errors::{SupportEndpointError, SupportEndpointOperation, support_endpoint_error};
use crate::state::UserApiState;

/// client version / OS は client が自動付与する短い識別子。自由記述と違い長文を許す理由が
/// ないため、上限を小さく取る(Unicode コードポイント数)。
const TESTER_FEEDBACK_META_MAX_CHARS: usize = 200;

/// テスターフィードバックを受信して保存する(#802)。
///
/// 受付可否は `tester_feedback` capability の opt-in で判断する(無効なら 404 fail-closed)。
/// 認証済み(bearer)+ consent 済み user のみ送信できるが、送信者の identity は
/// レポート record に保存しない(ADR 0039)。
///
/// 3 つの自由記述は必須で、各 `TESTER_FEEDBACK_MAX_CHARS`(2000)コードポイント以内。
/// 文字数は byte 長ではなく Unicode コードポイント数で判定する(要件が「2000 字」のため)。
pub(crate) async fn submit_tester_feedback(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<CommunityNodeTesterFeedbackRequest>,
) -> ApiResult<Json<CommunityNodeTesterFeedbackResponse>> {
    let feedback_enabled = state
        .manifest
        .as_ref()
        .map(|manifest| manifest.capabilities.tester_feedback)
        .unwrap_or(false);
    if !feedback_enabled {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            TESTER_FEEDBACK_NOT_CONFIGURED_CODE,
            "this community node does not accept tester feedback",
        ));
    }

    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;

    let what_attempted = request.what_attempted.trim();
    let what_happened = request.what_happened.trim();
    let what_seemed_wrong = request.what_seemed_wrong.trim();
    if what_attempted.is_empty() || what_happened.is_empty() || what_seemed_wrong.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            INVALID_TESTER_FEEDBACK_CODE,
            "what_attempted, what_happened and what_seemed_wrong are required",
        ));
    }
    for text in [what_attempted, what_happened, what_seemed_wrong] {
        if text.chars().count() > TESTER_FEEDBACK_MAX_CHARS {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                INVALID_TESTER_FEEDBACK_CODE,
                format!("each field must be at most {TESTER_FEEDBACK_MAX_CHARS} characters"),
            ));
        }
    }
    let client_version = request.client_version.trim();
    let os = request.os.trim();
    for value in [client_version, os] {
        if value.chars().count() > TESTER_FEEDBACK_META_MAX_CHARS {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                INVALID_TESTER_FEEDBACK_CODE,
                format!(
                    "client_version and os must be at most {TESTER_FEEDBACK_META_MAX_CHARS} characters"
                ),
            ));
        }
    }

    let stored = insert_tester_feedback(
        &state.pool,
        &NewTesterFeedback {
            what_attempted: what_attempted.to_string(),
            what_happened: what_happened.to_string(),
            what_seemed_wrong: what_seemed_wrong.to_string(),
            client_version: client_version.to_string(),
            os: os.to_string(),
        },
    )
    .await
    .map_err(|source| {
        SupportEndpointError::new(SupportEndpointOperation::StoreTesterFeedback, source)
    })
    .map_err(support_endpoint_error)?;
    Ok(Json(CommunityNodeTesterFeedbackResponse {
        reference_id: Some(stored.id),
    }))
}
