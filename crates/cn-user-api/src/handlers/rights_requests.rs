//! 公開の権利侵害申出 scope、受付、追跡、取下げ (#760)。

use axum::Json;
use axum::extract::{Form, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use kukuri_cn_core::{
    ApiError, ApiResult, get_public_rights_request_status, insert_rights_request,
    resolve_rights_request_scope, withdraw_rights_request,
};
use kukuri_cn_protocol::{
    EvidenceReference, EvidenceReferenceKind, RightsCategory, RightsRequestAccessRequest,
    RightsRequestCreateRequest, RightsRequestCreateResponse, RightsRequestScopeResponse,
    RightsRequesterKind,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::state::UserApiState;

const ACKNOWLEDGEMENT: &str = "この Community Node が実行できるのは、このノード自身の索引・検索・発見・推薦・moderation・cache に対する措置だけです。他のノード、第三者端末、投稿正本、Direct P2P、暗号化 relay packet、既に取得されたデータを削除・遮断することはできません。受付は権利侵害の認定や希望する措置を保証するものではありません。";

pub(crate) async fn rights_request_scope(
    State(state): State<UserApiState>,
) -> ApiResult<Json<RightsRequestScopeResponse>> {
    Ok(Json(scope_for_state(&state)?))
}

pub(crate) async fn submit_rights_request(
    State(state): State<UserApiState>,
    Json(request): Json<RightsRequestCreateRequest>,
) -> ApiResult<Json<RightsRequestCreateResponse>> {
    let response = create(&state, request).await?;
    Ok(Json(response))
}

pub(crate) async fn rights_request_status(
    State(state): State<UserApiState>,
    Json(access): Json<RightsRequestAccessRequest>,
) -> ApiResult<Json<kukuri_cn_protocol::RightsRequestStatusResponse>> {
    let status = get_public_rights_request_status(
        &state.pool,
        access.reference_id.trim(),
        access.tracking_secret.trim(),
    )
    .await
    .map_err(internal_error)?
    .ok_or_else(access_denied)?;
    Ok(Json(status))
}

pub(crate) async fn withdraw_rights_request_handler(
    State(state): State<UserApiState>,
    Json(access): Json<RightsRequestAccessRequest>,
) -> ApiResult<Json<kukuri_cn_protocol::RightsRequestStatusResponse>> {
    let status = withdraw_rights_request(
        &state.pool,
        access.reference_id.trim(),
        access.tracking_secret.trim(),
        chrono::Utc::now(),
    )
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::CONFLICT,
            "RIGHTS_REQUEST_NOT_WITHDRAWABLE",
            error.to_string(),
        )
    })?
    .ok_or_else(access_denied)?;
    Ok(Json(status))
}

pub(crate) async fn rights_request_form(State(state): State<UserApiState>) -> ApiResult<Response> {
    let scope = scope_for_state(&state)?;
    Ok(no_store_html(render_form(&scope)))
}

pub(crate) async fn rights_request_status_form() -> Response {
    no_store_html(
        "<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><title>権利侵害申出の状態確認</title><main><h1>権利侵害申出の状態確認</h1><form method=\"post\"><label>参照 ID<input name=\"reference_id\" required></label><label>追跡 secret<input type=\"password\" name=\"tracking_secret\" required></label><button type=\"submit\">状態を確認</button></form></main></html>".to_string(),
    )
}

#[derive(Deserialize)]
pub(crate) struct RightsRequestAccessForm {
    reference_id: String,
    tracking_secret: String,
}

pub(crate) async fn rights_request_status_form_submit(
    State(state): State<UserApiState>,
    Form(access): Form<RightsRequestAccessForm>,
) -> ApiResult<Response> {
    let status = get_public_rights_request_status(
        &state.pool,
        access.reference_id.trim(),
        access.tracking_secret.trim(),
    )
    .await
    .map_err(internal_error)?
    .ok_or_else(access_denied)?;
    let status_label = serde_json::to_string(&status.status)
        .unwrap_or_else(|_| "\"unknown\"".to_string())
        .trim_matches('"')
        .to_string();
    Ok(no_store_html(format!(
        "<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><title>権利侵害申出の状態</title><main><h1>権利侵害申出の状態</h1><dl><dt>参照 ID</dt><dd><code>{}</code></dd><dt>scope</dt><dd>{:?}</dd><dt>状態</dt><dd>{}</dd><dt>更新時刻</dt><dd>{}</dd><dt>公開メッセージ</dt><dd>{}</dd></dl><form method=\"post\" action=\"/rights-requests/withdraw\"><input type=\"hidden\" name=\"reference_id\" value=\"{}\"><input type=\"hidden\" name=\"tracking_secret\" value=\"{}\"><button type=\"submit\">申出を取り下げる</button></form></main></html>",
        escape_html(&status.reference_id),
        status.scope_status,
        escape_html(&status_label),
        escape_html(&status.updated_at),
        escape_html(status.public_message.as_deref().unwrap_or("-")),
        escape_html(&access.reference_id),
        escape_html(&access.tracking_secret),
    )))
}

pub(crate) async fn withdraw_rights_request_form_submit(
    State(state): State<UserApiState>,
    Form(access): Form<RightsRequestAccessForm>,
) -> ApiResult<Response> {
    let status = withdraw_rights_request(
        &state.pool,
        access.reference_id.trim(),
        access.tracking_secret.trim(),
        chrono::Utc::now(),
    )
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::CONFLICT,
            "RIGHTS_REQUEST_NOT_WITHDRAWABLE",
            error.to_string(),
        )
    })?
    .ok_or_else(access_denied)?;
    Ok(no_store_html(format!(
        "<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><title>申出を取り下げました</title><main><h1>申出を取り下げました</h1><p>参照 ID: <code>{}</code></p><p>状態: withdrawn</p></main></html>",
        escape_html(&status.reference_id),
    )))
}

#[derive(Deserialize)]
pub(crate) struct RightsRequestHtmlForm {
    scope_revision: String,
    #[serde(default)]
    scope_acknowledged: Option<String>,
    requester_kind: String,
    requester_name: String,
    #[serde(default)]
    organization: Option<String>,
    email: String,
    rights_category: String,
    rights_basis: String,
    subject_kind: String,
    subject_id: String,
    infringement_description: String,
    #[serde(default)]
    no_permission_statement: Option<String>,
    requested_capability: String,
    #[serde(default)]
    evidence_url: Option<String>,
}

pub(crate) async fn submit_rights_request_form(
    State(state): State<UserApiState>,
    Form(form): Form<RightsRequestHtmlForm>,
) -> ApiResult<Response> {
    let request = RightsRequestCreateRequest {
        scope_revision: form.scope_revision,
        scope_acknowledged: form.scope_acknowledged.is_some(),
        requester_kind: parse_requester_kind(&form.requester_kind)?,
        requester_name: form.requester_name,
        organization: normalize_optional(form.organization),
        address: None,
        email: form.email,
        phone: None,
        represented_rights_holder: None,
        authority_basis: None,
        rights_category: parse_rights_category(&form.rights_category)?,
        rights_basis: form.rights_basis,
        original_work_description: None,
        original_work_reference: None,
        subject_kind: form.subject_kind,
        subject_id: form.subject_id,
        subject_url: None,
        infringement_description: form.infringement_description,
        no_permission_statement: form.no_permission_statement.is_some(),
        evidence_references: normalize_optional(form.evidence_url)
            .map(|value| {
                vec![EvidenceReference {
                    kind: EvidenceReferenceKind::Url,
                    value,
                }]
            })
            .unwrap_or_default(),
        requested_capabilities: vec![form.requested_capability],
    };
    let response = create(&state, request).await?;
    Ok(no_store_html(format!(
        "<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><title>申出を受け付けました</title><main><h1>申出を受け付けました</h1><p>参照 ID: <code>{}</code></p><p>追跡 secret（再表示できません）: <code>{}</code></p><p>両方を安全な場所に保存してください。</p><p><a href=\"/rights-requests/status\">保存後に状態確認画面を開く</a></p></main></html>",
        escape_html(&response.reference_id),
        escape_html(&response.tracking_secret)
    )))
}

async fn create(
    state: &UserApiState,
    request: RightsRequestCreateRequest,
) -> Result<RightsRequestCreateResponse, ApiError> {
    let scope = scope_for_state(state)?;
    if !request.scope_acknowledged {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "RIGHTS_SCOPE_ACKNOWLEDGEMENT_REQUIRED",
            "対応可能な範囲を確認し、明示的に同意してから送信してください",
        ));
    }
    if request.scope_revision != scope.scope_revision {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "RIGHTS_SCOPE_REVISION_CHANGED",
            "対応可能な範囲が更新されました。最新の説明を確認し直してください",
        ));
    }
    let manifest = state.manifest.as_ref().expect("scope requires manifest");
    let scope_status = resolve_rights_request_scope(
        &state.pool,
        &request.subject_kind,
        &request.subject_id,
        &request.requested_capabilities,
        &manifest.capability_scope.available_enabled,
    )
    .await
    .map_err(internal_error)?;
    let cipher = state.legal_data_cipher.as_deref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "RIGHTS_REQUEST_ENCRYPTION_NOT_CONFIGURED",
            "rights request encryption is not configured",
        )
    })?;
    let created = insert_rights_request(
        &state.pool,
        &request,
        scope_status,
        cipher,
        chrono::Utc::now(),
    )
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_RIGHTS_REQUEST",
            error.to_string(),
        )
    })?;
    Ok(RightsRequestCreateResponse {
        reference_id: created.record.id,
        tracking_secret: created.tracking_secret,
        scope_status: created.record.scope_status,
        status: created.record.status,
        received_at: created.record.created_at.to_rfc3339(),
    })
}

fn scope_for_state(state: &UserApiState) -> Result<RightsRequestScopeResponse, ApiError> {
    let manifest = state
        .manifest
        .as_ref()
        .filter(|manifest| !manifest.rights_request_url.trim().is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "RIGHTS_REQUEST_NOT_CONFIGURED",
                "this community node does not accept rights-infringement requests",
            )
        })?;
    let mut available_actions = Vec::new();
    if manifest.capabilities.community_index {
        available_actions.extend([
            "community_index".to_string(),
            "search".to_string(),
            "discovery".to_string(),
            "recommendation".to_string(),
        ]);
    }
    if manifest.capabilities.moderation {
        available_actions.push("moderation".to_string());
    }
    if manifest.capabilities.blob_cache {
        available_actions.push("blob_cache".to_string());
    }
    let unavailable_actions = vec![
        "他の Community Node の索引・cache の削除".to_string(),
        "第三者端末または source peer のデータ削除".to_string(),
        "author-owned replica にある投稿正本の削除".to_string(),
        "Direct P2P の遮断".to_string(),
        "暗号化 relay packet の内容検査または遮断".to_string(),
        "既に取得されたデータの回収".to_string(),
    ];
    let revision_input = json!({
        "manifest_version": manifest.manifest_version,
        "available_actions": available_actions,
        "unavailable_actions": unavailable_actions,
        "initial_response_target_days": manifest.rights_request_initial_response_target_days,
        "acknowledgement": ACKNOWLEDGEMENT,
    });
    let scope_revision = hex::encode(Sha256::digest(
        serde_json::to_vec(&revision_input).expect("scope revision serializes"),
    ));
    Ok(RightsRequestScopeResponse {
        scope_revision,
        node_name: manifest.node_name.clone(),
        available_actions,
        unavailable_actions,
        initial_response_target_days: manifest.rights_request_initial_response_target_days,
        acknowledgement: ACKNOWLEDGEMENT.to_string(),
    })
}

fn render_form(scope: &RightsRequestScopeResponse) -> String {
    let actions = scope
        .available_actions
        .iter()
        .map(|action| {
            format!(
                "<option value=\"{}\">{}</option>",
                escape_html(action),
                escape_html(action)
            )
        })
        .collect::<String>();
    let unavailable = scope
        .unavailable_actions
        .iter()
        .map(|item| format!("<li>{}</li>", escape_html(item)))
        .collect::<String>();
    format!(
        "<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><title>権利侵害申出</title><style>body{{font-family:system-ui;max-width:52rem;margin:2rem auto;padding:0 1rem;line-height:1.6}}fieldset{{margin:1.5rem 0;padding:1rem}}label{{display:block;margin:.8rem 0}}input,select,textarea{{display:block;width:100%;box-sizing:border-box;padding:.5rem}}.scope{{border:2px solid #8a5b00;background:#fff8df;padding:1rem}}button{{padding:.7rem 1.2rem}}</style><main><h1>{} への権利侵害申出</h1><section class=\"scope\"><h2>送信前に対応可能な範囲を確認してください</h2><p>{}</p><h3>このノードでは対応できないこと</h3><ul>{}</ul><p>初回応答は {} 日以内を運用目標とします。法定期限ではなく、措置や回答時期を保証しません。</p></section><form method=\"post\"><input type=\"hidden\" name=\"scope_revision\" value=\"{}\"><fieldset><legend>対応範囲への同意</legend><label><input type=\"checkbox\" name=\"scope_acknowledged\" value=\"yes\" required>上記の可能・不可能な対応範囲を理解しました</label></fieldset><fieldset><legend>申出人</legend><label>区分<select name=\"requester_kind\"><option value=\"rights_holder\">権利者本人</option><option value=\"representative\">代理人</option><option value=\"rights_management_organization\">権利管理団体</option></select></label><label>氏名<input name=\"requester_name\" required maxlength=\"320\"></label><label>組織名（任意）<input name=\"organization\" maxlength=\"320\"></label><label>メール<input type=\"email\" name=\"email\" required maxlength=\"320\"></label></fieldset><fieldset><legend>権利と対象</legend><label>権利<select name=\"rights_category\"><option value=\"copyright\">著作権</option><option value=\"privacy\">プライバシー</option><option value=\"personality_rights\">人格権</option><option value=\"trademark\">商標権</option><option value=\"other_rights\">その他</option></select></label><label>権利の根拠<textarea name=\"rights_basis\" required maxlength=\"4000\"></textarea></label><label>対象種別<select name=\"subject_kind\"><option value=\"post\">投稿</option><option value=\"blob\">添付 blob</option></select></label><label>対象 ID<input name=\"subject_id\" required maxlength=\"512\"></label><label>侵害態様<textarea name=\"infringement_description\" required maxlength=\"8000\"></textarea></label><label>希望する node-local 措置<select name=\"requested_capability\">{}</select></label><label>証拠 URL（任意、ファイルは送信できません）<input type=\"url\" name=\"evidence_url\" maxlength=\"2048\"></label><label><input type=\"checkbox\" name=\"no_permission_statement\" value=\"yes\" required>対象の利用を許諾していないことを確認します</label></fieldset><button type=\"submit\">この Community Node に申出を送信</button></form></main></html>",
        escape_html(&scope.node_name),
        escape_html(&scope.acknowledgement),
        unavailable,
        scope.initial_response_target_days,
        escape_html(&scope.scope_revision),
        actions,
    )
}

fn parse_requester_kind(value: &str) -> Result<RightsRequesterKind, ApiError> {
    match value {
        "rights_holder" => Ok(RightsRequesterKind::RightsHolder),
        "representative" => Ok(RightsRequesterKind::Representative),
        "rights_management_organization" => Ok(RightsRequesterKind::RightsManagementOrganization),
        _ => Err(invalid_form()),
    }
}

fn parse_rights_category(value: &str) -> Result<RightsCategory, ApiError> {
    match value {
        "copyright" => Ok(RightsCategory::Copyright),
        "privacy" => Ok(RightsCategory::Privacy),
        "personality_rights" => Ok(RightsCategory::PersonalityRights),
        "trademark" => Ok(RightsCategory::Trademark),
        "other_rights" => Ok(RightsCategory::OtherRights),
        _ => Err(invalid_form()),
    }
}

fn invalid_form() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "INVALID_RIGHTS_REQUEST",
        "申出フォームの選択値が不正です",
    )
}

fn access_denied() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "RIGHTS_REQUEST_NOT_FOUND",
        "参照 ID または追跡 secret を確認してください",
    )
}

fn internal_error(error: anyhow::Error) -> ApiError {
    tracing::warn!(error = %format!("{error:#}"), "rights request operation failed");
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "RIGHTS_REQUEST_UNAVAILABLE",
        "権利侵害申出を処理できませんでした",
    )
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn no_store_html(body: String) -> Response {
    let mut response = Html(body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_places_scope_and_acknowledgement_before_request_fields() {
        let html = render_form(&RightsRequestScopeResponse {
            scope_revision: "revision".to_string(),
            node_name: "Node".to_string(),
            available_actions: vec!["community_index".to_string()],
            unavailable_actions: vec!["third party deletion".to_string()],
            initial_response_target_days: 7,
            acknowledgement: ACKNOWLEDGEMENT.to_string(),
        });
        let scope = html.find("対応可能な範囲").unwrap();
        let acknowledgement = html.find("scope_acknowledged").unwrap();
        let requester = html.find("requester_name").unwrap();
        assert!(scope < acknowledgement && acknowledgement < requester);
        assert!(html.contains("Direct P2P"));
        assert!(html.contains("法定期限ではなく"));
    }

    #[test]
    fn html_escapes_manifest_controlled_node_name() {
        let html = render_form(&RightsRequestScopeResponse {
            scope_revision: "revision".to_string(),
            node_name: "<script>alert(1)</script>".to_string(),
            available_actions: vec![],
            unavailable_actions: vec![],
            initial_response_target_days: 7,
            acknowledgement: ACKNOWLEDGEMENT.to_string(),
        });
        assert!(!html.contains("<script>"));
    }
}
