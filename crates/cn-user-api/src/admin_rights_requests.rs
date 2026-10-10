//! IAP operator UI for rights-infringement requests (#760).

use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use kukuri_cn_core::{
    TransmissionPreventionCapability, action_rights_request, get_rights_request_with_sensitive,
    list_rights_requests_with_sensitive, transition_rights_request,
};
use kukuri_cn_protocol::RightsRequestStatus;
use serde::Deserialize;

use crate::admin::{AdminState, csrf_matches, escape_html, render_action_error};
use crate::admin_shell::render_admin_page;

#[derive(Debug, Deserialize)]
pub(crate) struct RightsRequestAdminForm {
    csrf_token: String,
    id: String,
    expected_version: i32,
    operation: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    public_message: String,
    #[serde(default = "default_delivery_status")]
    delivery_status: String,
    #[serde(default)]
    capabilities: String,
}

fn default_delivery_status() -> String {
    "status_surface".to_string()
}

pub(crate) async fn rights_requests_page(State(state): State<AdminState>) -> Html<String> {
    let requests = match state.runtime.legal_data_cipher.as_deref() {
        Some(cipher) => {
            list_rights_requests_with_sensitive(
                &state.runtime.pool,
                cipher,
                100,
                0,
                chrono::Utc::now(),
            )
            .await
        }
        None => Err(anyhow::anyhow!("legal data encryption is not configured")),
    };
    let main = match requests {
        Ok(requests) => {
            let rows = if requests.is_empty() {
                "<tr><td colspan=\"6\">権利侵害申出はありません。</td></tr>".to_string()
            } else {
                requests
                    .into_iter()
                    .map(|request| {
                        format!(
                            "<tr><td>{}</td><td><a href=\"/rights-requests/{}\"><code>{}</code></a></td><td><code>{}/{}</code></td><td>{:?}</td><td>{:?}</td><td>{}</td></tr>",
                            escape_html(&request.created_at.to_rfc3339()),
                            escape_html(&request.id),
                            escape_html(&request.id),
                            escape_html(&request.subject_kind),
                            escape_html(&request.subject_id),
                            request.scope_status,
                            request.status,
                            request.version,
                        )
                    })
                    .collect()
            };
            format!(
                "<section><p>新しい順に 100 件を表示します。この一覧には申出人情報を表示しません。</p><table><thead><tr><th>受信時刻</th><th>参照 ID</th><th>対象</th><th>scope</th><th>状態</th><th>版</th></tr></thead><tbody>{rows}</tbody></table></section>"
            )
        }
        Err(error) => format!(
            "<section><p>{}</p></section>",
            escape_html(&format!("{error:#}"))
        ),
    };
    Html(render_admin_page(
        "権利侵害申出",
        "<nav class=\"crumbs\"><a href=\"/\">コミュニティノード運営</a><span>›</span><span aria-current=\"page\">権利侵害申出</span></nav><h1>権利侵害申出</h1>",
        &main,
    ))
}

pub(crate) async fn rights_request_detail(
    State(state): State<AdminState>,
    Path(id): Path<String>,
) -> Response {
    let Some(cipher) = state.runtime.legal_data_cipher.as_deref() else {
        return render_action_error(StatusCode::SERVICE_UNAVAILABLE, "暗号鍵が未設定です。");
    };
    let request = match get_rights_request_with_sensitive(
        &state.runtime.pool,
        cipher,
        &id,
        chrono::Utc::now(),
    )
    .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return render_action_error(StatusCode::NOT_FOUND, "申出が見つかりません。"),
        Err(error) => {
            return render_action_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{error:#}"));
        }
    };
    let write = state.actor.as_ref().map_or_else(
        || "<p class=\"boundary\">参照専用です。変更には COMMUNITY_NODE_ADMIN_ACTOR が必要です。</p>".to_string(),
        |_| format!(
            r#"<form method="post" action="/rights-requests/actions/preview" class="edit-form">
<input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="id" value="{}"><input type="hidden" name="expected_version" value="{}">
<label>操作<select name="operation"><option value="transition">状態を変更</option><option value="action">送信防止を適用して actioned</option></select></label>
<label>変更後状態<select name="status"><option value="needs_information">needs_information</option><option value="reviewing">reviewing</option><option value="sender_contacting">sender_contacting</option><option value="declined">declined</option><option value="out_of_scope">out_of_scope</option></select></label>
<label>公開メッセージ<input name="public_message" maxlength="2000"></label><label>外部通知記録<input name="delivery_status" value="status_surface" maxlength="64"></label>
<label>措置 capability（action 時、comma 区切り）<input name="capabilities" placeholder="community_index,search,discovery,recommendation"></label><button type="submit">変更内容を確認</button></form>"#,
            escape_html(&state.csrf_token), escape_html(&request.id), request.version,
        ),
    );
    let request_data = escape_html(
        &serde_json::to_string_pretty(&request.request).unwrap_or_else(|_| "-".to_string()),
    );
    let header = format!(
        "<nav class=\"crumbs\"><a href=\"/\">コミュニティノード運営</a><span>›</span><a href=\"/rights-requests\">権利侵害申出</a><span>›</span><span aria-current=\"page\">{}</span></nav><h1>権利侵害申出の詳細</h1>",
        escape_html(&request.id)
    );
    let main = format!(
        "<section><dl class=\"facts\"><dt>参照 ID</dt><dd><code>{}</code></dd><dt>対象</dt><dd><code>{}/{}</code></dd><dt>scope</dt><dd>{:?}</dd><dt>状態</dt><dd>{:?}</dd><dt>版</dt><dd>{}</dd><dt>公開メッセージ</dt><dd>{}</dd></dl></section><section><h2>申出内容（運営者限定）</h2><pre><code>{}</code></pre></section><section><h2>操作</h2><p>変更は確認画面を経て、版競合を検査し、append-only event と監査記録へ残します。</p>{}</section>",
        escape_html(&request.id),
        escape_html(&request.subject_kind),
        escape_html(&request.subject_id),
        request.scope_status,
        request.status,
        request.version,
        escape_html(request.public_message.as_deref().unwrap_or("-")),
        request_data,
        write,
    );
    Html(render_admin_page("権利侵害申出の詳細", &header, &main)).into_response()
}

pub(crate) async fn preview_rights_request_action(
    State(state): State<AdminState>,
    Form(form): Form<RightsRequestAdminForm>,
) -> Response {
    let Some(actor) = state.actor.as_deref() else {
        return render_action_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "運営者 actor が未設定です。",
        );
    };
    if !csrf_matches(&state.csrf_token, &form.csrf_token) {
        return render_action_error(StatusCode::FORBIDDEN, "画面を読み直してください。");
    }
    let Some(cipher) = state.runtime.legal_data_cipher.as_deref() else {
        return render_action_error(StatusCode::SERVICE_UNAVAILABLE, "暗号鍵が未設定です。");
    };
    let current = match get_rights_request_with_sensitive(
        &state.runtime.pool,
        cipher,
        &form.id,
        chrono::Utc::now(),
    )
    .await
    {
        Ok(Some(value)) if value.version == form.expected_version => value,
        Ok(Some(_)) => {
            return render_action_error(
                StatusCode::CONFLICT,
                "申出が更新されています。詳細を読み直してください。",
            );
        }
        Ok(None) => return render_action_error(StatusCode::NOT_FOUND, "申出が見つかりません。"),
        Err(error) => return render_action_error(StatusCode::BAD_REQUEST, &format!("{error:#}")),
    };
    if validate_form(&form).is_err() {
        return render_action_error(
            StatusCode::BAD_REQUEST,
            "操作、状態、または capability が不正です。",
        );
    }
    let hidden = format!(
        r#"<input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="id" value="{}"><input type="hidden" name="expected_version" value="{}"><input type="hidden" name="operation" value="{}"><input type="hidden" name="status" value="{}"><input type="hidden" name="public_message" value="{}"><input type="hidden" name="delivery_status" value="{}"><input type="hidden" name="capabilities" value="{}">"#,
        escape_html(&form.csrf_token),
        escape_html(&form.id),
        form.expected_version,
        escape_html(&form.operation),
        escape_html(&form.status),
        escape_html(&form.public_message),
        escape_html(&form.delivery_status),
        escape_html(&form.capabilities),
    );
    let main = format!(
        "<section class=\"dialog\"><p>運営者: <code>{}</code></p><p>対象: <code>{}</code>（現在 {:?} / version {}）</p><p>操作: <strong>{}</strong></p><p>公開メッセージ: {}</p><form method=\"post\" action=\"/rights-requests/actions/apply\">{}<button type=\"submit\">この内容で確定</button> <a class=\"button secondary\" href=\"/rights-requests/{}\">戻る</a></form></section>",
        escape_html(actor),
        escape_html(&current.id),
        current.status,
        current.version,
        escape_html(&form.operation),
        escape_html(&form.public_message),
        hidden,
        escape_html(&current.id),
    );
    Html(render_admin_page(
        "権利侵害申出の変更確認",
        "<h1>変更内容を確認</h1>",
        &main,
    ))
    .into_response()
}

pub(crate) async fn apply_rights_request_action(
    State(state): State<AdminState>,
    Form(form): Form<RightsRequestAdminForm>,
) -> Response {
    let Some(actor) = state.actor.as_deref() else {
        return render_action_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "運営者 actor が未設定です。",
        );
    };
    if !csrf_matches(&state.csrf_token, &form.csrf_token) {
        return render_action_error(StatusCode::FORBIDDEN, "画面を読み直してください。");
    }
    let result = match form.operation.as_str() {
        "transition" => match parse_status(&form.status) {
            Ok(status) => {
                transition_rights_request(
                    &state.runtime.pool,
                    &form.id,
                    form.expected_version,
                    actor,
                    status,
                    nonempty(&form.public_message),
                    &form.delivery_status,
                    chrono::Utc::now(),
                )
                .await
            }
            Err(error) => Err(error),
        },
        "action" => match parse_capabilities(&form.capabilities) {
            Ok(capabilities) => action_rights_request(
                &state.runtime.pool,
                &form.id,
                form.expected_version,
                actor,
                capabilities,
                &form.public_message,
                chrono::Utc::now(),
            )
            .await
            .map(|result| result.request),
            Err(error) => Err(error),
        },
        _ => Err(anyhow::anyhow!("unsupported operation")),
    };
    match result {
        Ok(request) => Html(render_admin_page(
            "権利侵害申出を更新しました", "<h1>権利侵害申出を更新しました</h1>",
            &format!("<section><p>状態、event、監査記録を確定しました。</p><a class=\"button\" href=\"/rights-requests/{}\">詳細へ戻る</a></section>", escape_html(&request.id)),
        )).into_response(),
        Err(error) => render_action_error(StatusCode::CONFLICT, &format!("{error:#}")),
    }
}

fn validate_form(form: &RightsRequestAdminForm) -> anyhow::Result<()> {
    match form.operation.as_str() {
        "transition" => {
            parse_status(&form.status)?;
        }
        "action" => {
            parse_capabilities(&form.capabilities)?;
            if form.public_message.trim().is_empty() {
                anyhow::bail!("public message required");
            }
        }
        _ => anyhow::bail!("unsupported operation"),
    }
    Ok(())
}

fn parse_status(value: &str) -> anyhow::Result<RightsRequestStatus> {
    match value.trim() {
        "needs_information" => Ok(RightsRequestStatus::NeedsInformation),
        "reviewing" => Ok(RightsRequestStatus::Reviewing),
        "sender_contacting" => Ok(RightsRequestStatus::SenderContacting),
        "declined" => Ok(RightsRequestStatus::Declined),
        "out_of_scope" => Ok(RightsRequestStatus::OutOfScope),
        _ => anyhow::bail!("unsupported rights request status"),
    }
}

fn parse_capabilities(value: &str) -> anyhow::Result<Vec<TransmissionPreventionCapability>> {
    let values = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| match value {
            "community_index" => Ok(TransmissionPreventionCapability::CommunityIndex),
            "search" => Ok(TransmissionPreventionCapability::Search),
            "discovery" => Ok(TransmissionPreventionCapability::Discovery),
            "recommendation" => Ok(TransmissionPreventionCapability::Recommendation),
            "moderation" => Ok(TransmissionPreventionCapability::Moderation),
            "blob_cache" => Ok(TransmissionPreventionCapability::BlobCache),
            _ => anyhow::bail!("unsupported capability"),
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if values.is_empty() {
        anyhow::bail!("capabilities required");
    }
    Ok(values)
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_requires_explicit_supported_capabilities() {
        assert!(parse_capabilities("community_index,search").is_ok());
        assert!(parse_capabilities("").is_err());
        assert!(parse_capabilities("network_wide_delete").is_err());
    }
}
