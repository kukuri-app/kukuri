//! 通報受信(#370)+ moderation advisory への異議申し立て受付(#420 / ADR 0028 §2.8)。

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use kukuri_cn_core::{
    ApiError, ApiResult, NewCommunityNodeReport, insert_community_node_appeal,
    insert_community_node_report,
};
use kukuri_cn_protocol::{CommunityNodeReportRequest, CommunityNodeReportResponse};

use crate::errors::{SupportEndpointError, SupportEndpointOperation, support_endpoint_error};
use crate::state::UserApiState;

/// 通報受信リクエスト(#370)。client(#310)が provenance + manifest authority scope で
/// 通報先を解決し、この node の report endpoint へ POST する。
///
/// `appeal` を伴う通報は、この node が発行した moderation advisory(risk signal)への
/// 異議申し立て(#420)。issuer node への申し立て導線は専用 endpoint を新設せず、
/// report routing が既に候補化している report endpoint を再利用する。
/// 通報を受信して保存する(#370)。unauthenticated で受け付ける(匿名通報を許す)。
///
/// 受付可否は「この node が report_endpoint capability を有効化しているか」で判断する。これが
/// node の authority scope への opt-in であり、中央通報窓口を作らない。通報先の解決自体は client
/// (#310)が provenance + manifest authority scope で行っているため、ここへ届く時点で対象は
/// この node が関与した範囲に絞られている。reporter の identity / social graph は保持しない。
pub(crate) async fn submit_report(
    State(state): State<UserApiState>,
    Json(request): Json<CommunityNodeReportRequest>,
) -> ApiResult<Json<CommunityNodeReportResponse>> {
    // report endpoint capability が無効な node は通報を受け付けない。
    let report_enabled = state
        .manifest
        .as_ref()
        .map(|manifest| !manifest.report_endpoint.trim().is_empty())
        .unwrap_or(false);
    if !report_enabled {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "REPORT_NOT_CONFIGURED",
            "this community node does not accept reports",
        ));
    }

    let subject_kind = request.subject_kind.trim();
    let subject_id = request.subject_id.trim();
    let capability = request.capability.trim();
    let reason = request.reason.trim();
    if subject_kind.is_empty()
        || subject_id.is_empty()
        || capability.is_empty()
        || reason.is_empty()
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_REPORT",
            "subject_kind, subject_id, capability and reason are required",
        ));
    }
    if reason == "rights_infringement" {
        let message = state
            .manifest
            .as_ref()
            .map(|manifest| manifest.rights_request_url.trim())
            .filter(|url| !url.is_empty())
            .map(|url| format!("権利侵害申出は一般通報では受け付けません。対応範囲を確認して {url} から送信してください"))
            .unwrap_or_else(|| {
                "権利侵害申出は一般通報では受け付けません。この node は専用受付を公開していません"
                    .to_string()
            });
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "RIGHTS_REQUEST_REQUIRES_DEDICATED_INTAKE",
            message,
        ));
    }

    let mut report = NewCommunityNodeReport {
        subject_kind: subject_kind.to_string(),
        subject_id: subject_id.to_string(),
        capability: capability.to_string(),
        reason: reason.to_string(),
        details: normalize_optional(request.details),
        reporter_contact: normalize_optional(request.reporter_contact),
        appeal_risk_signal_id: None,
    };
    let (stored, disputed_risk_signal_id) = match request.appeal.as_ref() {
        Some(appeal) => {
            let risk_signal_id = appeal.risk_signal_id.trim();
            let issuer_node_id = state
                .manifest
                .as_ref()
                .map(|manifest| manifest.node_id.trim())
                .filter(|value| !value.is_empty());
            let Some(issuer_node_id) = issuer_node_id else {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "INVALID_APPEAL",
                    "this community node does not publish an issuer node id",
                ));
            };
            if risk_signal_id.is_empty() {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "INVALID_APPEAL",
                    "appeal.risk_signal_id is required",
                ));
            }
            report.reporter_contact = None;
            report.appeal_risk_signal_id = Some(risk_signal_id.to_string());
            let stored =
                insert_community_node_appeal(&state.pool, issuer_node_id, risk_signal_id, &report)
                    .await
                    .map_err(|_| {
                        ApiError::new(
                            StatusCode::BAD_REQUEST,
                            "INVALID_APPEAL",
                            "the referenced moderation advisory cannot be disputed",
                        )
                    })?;
            (stored, Some(risk_signal_id.to_string()))
        }
        None => {
            let cipher = state.legal_data_cipher.as_deref().ok_or_else(|| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "REPORT_ENCRYPTION_NOT_CONFIGURED",
                    "report intake encryption is not configured",
                )
            })?;
            let stored = insert_community_node_report(&state.pool, &report, Some(cipher))
                .await
                .map_err(|source| {
                    SupportEndpointError::new(SupportEndpointOperation::StoreReport, source)
                })
                .map_err(support_endpoint_error)?;
            (stored, None)
        }
    };
    Ok(Json(CommunityNodeReportResponse {
        reference_id: Some(stored.id),
        disputed_risk_signal_id,
    }))
}

/// 任意の文字列入力を正規化する。空白のみ / 空文字は None にする。
fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}
