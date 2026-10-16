//! IAP TCP forwarding 経由でのみ公開する operator dashboard。
//!
//! public user API と route / listener を分離する。browser write は ADR 0029 に従い、
//! runtime DB 操作だけを preview / CSRF / deployment actor / append-only audit 付きで扱う。

use anyhow::{Context, bail};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use kukuri_cn_core::{
    AdminOperation, AdmissionMode, AppealReview, AppealReviewOperation, AppealReviewVersion,
    MalwareSubject, OperatorReportStatus, RiskSignalCorrection, RiskSignalMetadataEdit,
    apply_appeal_review_action, apply_operator_action, get_appeal_review,
    latest_readiness_activation, list_appeal_reviews, list_community_node_reports,
    list_operator_actions, list_supported_topics, load_admission_config, parse_bool_env,
    reported_subjects_with_malware_decisions, validate_admin_operation,
    validate_optional_confidence, validate_optional_expires_at,
};
use serde::Deserialize;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::admin_action_render::{render_action_error_page, render_action_success, render_preview};
use crate::admin_appeal_render::{render_appeal_preview, render_appeal_reviews};
use crate::admin_malware::{malware_restrictions_page, malware_subject, render_restrict_form};
use crate::admin_rights_requests::{
    apply_rights_request_action, preview_rights_request_action, rights_request_detail,
    rights_requests_page,
};
use crate::admin_shell::render_admin_page;
use crate::admin_tester_feedback::tester_feedback_page;
use crate::state::UserApiState;

pub(crate) fn admin_router(state: UserApiState) -> Router {
    let actor = std::env::var("COMMUNITY_NODE_ADMIN_ACTOR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string());
    let csrf_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let operator_review_enabled =
        parse_bool_env("COMMUNITY_NODE_SAFETY_OPERATOR_REVIEW", false).unwrap_or_else(|error| {
            tracing::warn!(error = %format!("{error:#}"), "運営者確認の有効化設定を解釈できないため無効化します");
            false
        });
    let state = AdminState {
        runtime: state,
        actor,
        csrf_token,
        operator_review_enabled,
    };
    Router::new()
        .route("/", get(dashboard))
        .route("/healthz", get(|| async { "ok" }))
        .route("/actions/preview", post(preview_action))
        .route("/actions/apply", post(apply_action))
        .route("/rights-requests", get(rights_requests_page))
        .route("/tester-feedback", get(tester_feedback_page))
        .route("/malware-restrictions", get(malware_restrictions_page))
        .route("/rights-requests/{id}", get(rights_request_detail))
        .route(
            "/rights-requests/actions/preview",
            post(preview_rights_request_action),
        )
        .route(
            "/rights-requests/actions/apply",
            post(apply_rights_request_action),
        )
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

#[derive(Clone)]
pub(crate) struct AdminState {
    pub(crate) runtime: UserApiState,
    pub(crate) actor: Option<String>,
    pub(crate) csrf_token: String,
    operator_review_enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct AdminActionForm {
    pub(crate) csrf_token: String,
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) target_id: String,
    #[serde(default)]
    pub(crate) value: String,
    #[serde(default)]
    pub(crate) expected_state: String,
    #[serde(default)]
    pub(crate) category: String,
    #[serde(default)]
    pub(crate) severity: String,
    #[serde(default)]
    pub(crate) confidence: String,
    #[serde(default)]
    pub(crate) expires_at: String,
    #[serde(default)]
    pub(crate) visibility: String,
}

async fn dashboard(State(state): State<AdminState>) -> Html<String> {
    match load_dashboard(&state).await {
        Ok(view) => Html(render_dashboard(&view)),
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "admin dashboard data load failed");
            Html(render_error())
        }
    }
}

async fn preview_action(
    State(state): State<AdminState>,
    Form(form): Form<AdminActionForm>,
) -> Response {
    let actor = match require_admin_actor(state.actor.as_deref()) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    if !csrf_matches(state.csrf_token.as_str(), form.csrf_token.as_str()) {
        return render_action_error(
            StatusCode::FORBIDDEN,
            "画面の確認情報が無効または期限切れです。画面を読み直してください。",
        );
    }
    if form.action.starts_with("appeal.") {
        return preview_appeal_action(&state, actor, &form).await;
    }
    match operation_from_form(&form) {
        Ok(operation) => Html(render_preview(
            state.csrf_token.as_str(),
            actor,
            &form,
            &operation,
        ))
        .into_response(),
        Err(error) => render_action_error(StatusCode::BAD_REQUEST, format!("{error:#}").as_str()),
    }
}

async fn apply_action(
    State(state): State<AdminState>,
    Form(form): Form<AdminActionForm>,
) -> Response {
    let actor = match require_admin_actor(state.actor.as_deref()) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    if !csrf_matches(state.csrf_token.as_str(), form.csrf_token.as_str()) {
        return render_action_error(
            StatusCode::FORBIDDEN,
            "画面の確認情報が無効または期限切れです。画面を読み直してください。",
        );
    }
    if form.action.starts_with("appeal.") {
        return apply_appeal_action(&state, actor, &form).await;
    }
    let operation = match operation_from_form(&form) {
        Ok(operation) => operation,
        Err(error) => {
            return render_action_error(StatusCode::BAD_REQUEST, format!("{error:#}").as_str());
        }
    };
    match apply_operator_action(&state.runtime.pool, actor, &operation).await {
        Ok(action) => {
            tracing::info!(
                action_id = %action.id,
                actor = %action.actor,
                action = %action.action,
                target_kind = %action.target_kind,
                target_id = %action.target_id,
                "admin operation applied"
            );
            Html(render_action_success(&action)).into_response()
        }
        Err(error) => {
            tracing::warn!(
                actor,
                error = %format!("{error:#}"),
                "admin operation rejected"
            );
            render_action_error(StatusCode::BAD_REQUEST, format!("{error:#}").as_str())
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AdminActorMissing;

impl AdminActorMissing {
    fn into_response(self) -> Response {
        render_action_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "COMMUNITY_NODE_ADMIN_ACTOR が未設定のため、変更操作は無効です。",
        )
    }
}

fn require_admin_actor(actor: Option<&str>) -> Result<&str, AdminActorMissing> {
    actor.ok_or(AdminActorMissing)
}

async fn preview_appeal_action(
    state: &AdminState,
    actor: &str,
    form: &AdminActionForm,
) -> Response {
    if !state.operator_review_enabled {
        return render_action_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "このノードでは異議申し立ての運営者確認が無効です。",
        );
    }
    let review = match get_appeal_review(&state.runtime.pool, form.target_id.trim()).await {
        Ok(Some(review)) if review.appeal_status == "disputed" => review,
        Ok(Some(_)) => {
            return render_action_error(
                StatusCode::BAD_REQUEST,
                "この異議申し立てはすでに審査対象ではありません。",
            );
        }
        Ok(None) => {
            return render_action_error(
                StatusCode::NOT_FOUND,
                "審査対象のリスク判定が見つかりません。",
            );
        }
        Err(error) => {
            return render_action_error(StatusCode::BAD_REQUEST, format!("{error:#}").as_str());
        }
    };
    // 表示値と適用値の出所を一致させるため、解析済み操作をそのまま描画へ渡す(#701)。
    let operation = match appeal_operation_from_form(form, review.version()) {
        Ok(operation) => operation,
        Err(error) => {
            return render_action_error(StatusCode::BAD_REQUEST, format!("{error:#}").as_str());
        }
    };
    Html(render_appeal_preview(
        state.csrf_token.as_str(),
        actor,
        &operation,
        &review,
    ))
    .into_response()
}

async fn apply_appeal_action(state: &AdminState, actor: &str, form: &AdminActionForm) -> Response {
    let expected = match serde_json::from_str::<AppealReviewVersion>(&form.expected_state) {
        Ok(expected) => expected,
        Err(_) => {
            return render_action_error(
                StatusCode::BAD_REQUEST,
                "確認時の状態が欠けています。画面を読み直してもう一度確認してください。",
            );
        }
    };
    let operation = match appeal_operation_from_form(form, expected) {
        Ok(operation) => operation,
        Err(error) => {
            return render_action_error(StatusCode::BAD_REQUEST, format!("{error:#}").as_str());
        }
    };
    match apply_appeal_review_action(
        &state.runtime.pool,
        actor,
        form.target_id.trim(),
        &operation,
        state.operator_review_enabled,
    )
    .await
    {
        Ok(action) => Html(render_action_success(&action)).into_response(),
        Err(error) => {
            tracing::warn!(actor, error = %format!("{error:#}"), "異議申し立て審査を拒否しました");
            render_action_error(StatusCode::BAD_REQUEST, format!("{error:#}").as_str())
        }
    }
}

fn appeal_operation_from_form(
    form: &AdminActionForm,
    expected: AppealReviewVersion,
) -> anyhow::Result<AppealReviewOperation> {
    match form.action.as_str() {
        "appeal.accept" => Ok(AppealReviewOperation::Accept { expected }),
        "appeal.reject" => Ok(AppealReviewOperation::Reject { expected }),
        "appeal.edit" => Ok(AppealReviewOperation::Edit {
            expected,
            edit: RiskSignalMetadataEdit {
                category: parse_optional_enum("category", &form.category)?,
                severity: parse_optional_enum("severity", &form.severity)?,
                confidence: parse_optional_confidence(&form.confidence)?,
                expires_at: parse_optional_expires_at(&form.expires_at)?,
            },
        }),
        "appeal.reissue" => Ok(AppealReviewOperation::Reissue {
            expected,
            correction: RiskSignalCorrection {
                category: parse_optional_enum("category", &form.category)?,
                severity: parse_optional_enum("severity", &form.severity)?,
                confidence: parse_optional_confidence(&form.confidence)?,
                visibility: parse_optional_enum("visibility", &form.visibility)?,
            },
        }),
        _ => bail!("unsupported appeal review operation"),
    }
}

fn parse_optional_enum<T: serde::de::DeserializeOwned>(
    field: &str,
    value: &str,
) -> anyhow::Result<Option<T>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .with_context(|| format!("unsupported {field}"))
        .map(Some)
}

fn parse_optional_confidence(value: &str) -> anyhow::Result<Option<u8>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let Ok(confidence) = value.parse::<u8>() else {
        bail!("確信度は 0 から 100 の整数で入力してください。");
    };
    if validate_optional_confidence(Some(confidence)).is_err() {
        bail!("確信度は 0 から 100 の整数で入力してください。");
    }
    Ok(Some(confidence))
}

fn parse_optional_expires_at(value: &str) -> anyhow::Result<Option<String>> {
    let Some(value) = optional_text(value) else {
        return Ok(None);
    };
    if validate_optional_expires_at(Some(value.as_str())).is_err() {
        bail!(
            "有効期限は RFC 3339 形式（例: 2026-08-20T09:00:00+09:00）で入力してください。過去時刻は失効として受理されます。"
        );
    }
    Ok(Some(value))
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[derive(Debug)]
struct DashboardView {
    admission_mode: String,
    readiness: String,
    topics: Vec<(String, String, String)>,
    reports: Vec<ReportView>,
    appeals: Vec<AppealReview>,
    audit: Vec<AuditView>,
    gcp_project: Option<String>,
    actor: Option<String>,
    csrf_token: String,
    operator_review_enabled: bool,
}

#[derive(Debug)]
struct ReportView {
    created_at: String,
    id: String,
    subject: String,
    capability: String,
    reason: String,
    status: String,
    /// malware の通報の対象（投稿・形式の正しい添付の hash）。制限の操作を出す。
    restrictable: Option<MalwareSubject>,
    /// 制限による対応済み（対象に malware の判断がある）。状態を変えられない。
    restricted: bool,
}

#[derive(Debug)]
struct AuditView {
    occurred_at: String,
    id: String,
    actor: String,
    action: String,
    target: String,
    change: String,
}

async fn load_dashboard(state: &AdminState) -> anyhow::Result<DashboardView> {
    let admission = load_admission_config(&state.runtime.pool).await?;
    let readiness = latest_readiness_activation(&state.runtime.pool)
        .await?
        .map(|activation| {
            format!(
                "{} · profile={} · {}",
                activation.activated_at.to_rfc3339(),
                activation.profile,
                if activation.revoked {
                    "revoked"
                } else {
                    "active"
                }
            )
        })
        .unwrap_or_else(|| "no activation recorded".to_string());
    let topics = list_supported_topics(&state.runtime.pool)
        .await?
        .into_iter()
        .map(|topic| {
            (
                topic.kind.as_str().to_string(),
                topic.id,
                topic.created_at.to_rfc3339(),
            )
        })
        .collect();
    let reports = list_community_node_reports(&state.runtime.pool, 50, 0).await?;
    let malware_subjects = reports
        .iter()
        .filter(|report| report.reason == "malware")
        .map(|report| (report.subject_kind.clone(), report.subject_id.clone()))
        .collect::<Vec<_>>();
    let decided =
        reported_subjects_with_malware_decisions(&state.runtime.pool, &malware_subjects).await?;
    let reports = reports
        .into_iter()
        .map(|report| ReportView {
            restricted: report.status == "actioned"
                && report.reason == "malware"
                && decided.contains(&(report.subject_kind.clone(), report.subject_id.clone())),
            restrictable: MalwareSubject::from_report_kind(&report.subject_kind).filter(
                |subject| {
                    report.reason == "malware" && subject.validate_id(&report.subject_id).is_ok()
                },
            ),
            created_at: report.created_at.to_rfc3339(),
            id: report.id,
            subject: format!("{}/{}", report.subject_kind, report.subject_id),
            capability: report.capability,
            reason: report.reason,
            status: report.status,
        })
        .collect();
    let appeals = list_appeal_reviews(&state.runtime.pool, 50, 0).await?;
    let audit = list_operator_actions(&state.runtime.pool, 50, 0)
        .await?
        .into_iter()
        .map(|entry| AuditView {
            occurred_at: entry.occurred_at.to_rfc3339(),
            id: entry.id,
            actor: entry.actor,
            action: entry.action,
            target: format!("{}/{}", entry.target_kind, entry.target_id),
            change: format!("{} -> {}", entry.before, entry.after),
        })
        .collect();

    Ok(DashboardView {
        admission_mode: admission.mode.as_str().to_string(),
        readiness,
        topics,
        reports,
        appeals,
        audit,
        gcp_project: std::env::var("COMMUNITY_NODE_GCP_PROJECT")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        actor: state.actor.clone(),
        csrf_token: state.csrf_token.clone(),
        operator_review_enabled: state.operator_review_enabled,
    })
}

fn render_dashboard(view: &DashboardView) -> String {
    let write_enabled = view.actor.is_some();
    let admission_control = view.actor.as_ref().map_or_else(
        || "<p class=\"boundary\">現在は参照専用です。変更するには、確認済みの配備手順で <code>COMMUNITY_NODE_ADMIN_ACTOR</code> を設定してください。</p>".to_string(),
        |actor| {
            format!(
                r#"<form method="post" action="/actions/preview" class="inline-form">
<input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="action" value="admission.set_mode">
<label>変更後の受け入れ方式 <select name="value">{}</select></label><button type="submit">変更内容を確認</button>
</form><p class="meta">操作記録に残る運営者: <code>{}</code></p>"#,
                escape_html(&view.csrf_token),
                admission_options(&view.admission_mode),
                escape_html(actor),
            )
        },
    );
    let topics = if view.topics.is_empty() {
        table_empty("対応トピックは設定されていません。", 4)
    } else {
        view.topics
            .iter()
            .map(|(kind, id, created_at)| {
                let action = if write_enabled && kind == "public_topic" {
                    format!(
                        r#"<form method="post" action="/actions/preview"><input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="action" value="supported_topic.remove"><input type="hidden" name="target_id" value="{}"><button class="secondary" type="submit">削除内容を確認</button></form>"#,
                        escape_html(&view.csrf_token),
                        escape_html(id),
                    )
                } else {
                    "<span class=\"meta\">参照専用</span>".to_string()
                };
                format!(
                    "<tr><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                    escape_html(kind),
                    escape_html(id),
                    escape_html(created_at),
                    action,
                )
            })
            .collect()
    };
    let add_topic = if write_enabled {
        format!(
            r#"<form method="post" action="/actions/preview" class="inline-form"><input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="action" value="supported_topic.add"><label>公開トピック識別子 <input name="target_id" required placeholder="demo"></label><button type="submit">追加内容を確認</button></form>"#,
            escape_html(&view.csrf_token),
        )
    } else {
        String::new()
    };
    let reports = if view.reports.is_empty() {
        table_empty("受信済みの通報はありません。", 7)
    } else {
        view.reports
            .iter()
            .map(|report| {
                // malware の通報の対応済みは制限の操作だけが作り、戻さない（#1062）。
                let malware = report.reason == "malware";
                let action = if !write_enabled {
                    escape_html(&report.status)
                } else if report.restricted {
                    "<a href=\"/malware-restrictions\">制限で対応済み（malware の制限を参照）</a>"
                        .to_string()
                } else {
                    let restrict = report.restrictable.map_or_else(String::new, |subject| {
                        render_restrict_form(&view.csrf_token, &report.id, subject)
                    });
                    format!(
                        r#"<form method="post" action="/actions/preview" class="compact-form"><input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="action" value="report.set_status"><input type="hidden" name="target_id" value="{}"><select aria-label="変更後の状態" name="value">{}</select><button class="secondary" type="submit">変更内容を確認</button></form>{restrict}"#,
                        escape_html(&view.csrf_token),
                        escape_html(&report.id),
                        report_status_options(&report.status, malware),
                    )
                };
                format!(
                    "<tr><td>{}</td><td><code>{}</code></td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape_html(&report.created_at),
                    escape_html(&report.id),
                    escape_html(&report.subject),
                    escape_html(&report.capability),
                    escape_html(&report.reason),
                    escape_html(&report.status),
                    action,
                )
            })
            .collect()
    };
    let appeals = render_appeal_reviews(
        &view.appeals,
        view.actor.is_some() && view.operator_review_enabled,
        &view.csrf_token,
    );
    let audit = if view.audit.is_empty() {
        table_empty("画面から行った操作記録はありません。", 6)
    } else {
        view.audit
            .iter()
            .map(|entry| {
                format!(
                    "<tr><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td><code>{}</code></td><td><code>{}</code></td></tr>",
                    escape_html(&entry.occurred_at),
                    escape_html(&entry.id),
                    escape_html(&entry.actor),
                    escape_html(&entry.action),
                    escape_html(&entry.target),
                    escape_html(&entry.change),
                )
            })
            .collect()
    };
    let logs_link = view.gcp_project.as_ref().map_or_else(
        || "<p><code>COMMUNITY_NODE_GCP_PROJECT</code> を設定するとログへのリンクを表示します。</p>".to_string(),
        |project| format!(
            "<p><a href=\"https://console.cloud.google.com/logs/query?project={}\" target=\"_blank\" rel=\"noreferrer\">運用ログを開く</a></p>",
            escape_html(project)
        ),
    );

    let main = format!(
        r#"<section><h2>稼働状態と受け入れ方式</h2><div class="metrics"><div class="metric"><strong>利用者向け接続先</strong><br>稼働中</div><div class="metric"><strong>受け入れ方式</strong><br>{}</div><div class="metric"><strong>直近の準備確認</strong><br>{}</div></div>{}</section>
<section><h2>対応トピック</h2><p>この画面で変更できるのは公開トピックだけです。非公開チャンネルの権限は変更できません。</p>{}<table><thead><tr><th>種類</th><th>識別子</th><th>作成時刻</th><th>操作</th></tr></thead><tbody>{}</tbody></table></section>
<section><h2>権利侵害申出</h2><p>専用 schema の申出、scope 判定、追跡状態を確認します。申出人情報は一覧・監査記録へ表示しません。</p><a class="button secondary" href="/rights-requests">権利侵害申出を確認</a></section>
<section><h2>テスターフィードバック</h2><p>テスターの利用経験レポートを確認します。送信者の識別情報は保存していません。</p><a class="button secondary" href="/tester-feedback">テスターフィードバックを確認</a></section>
<section><h2>最近の通報</h2><p>新しい順に 50 件を表示します。補足説明と連絡先は、この一覧と操作記録には表示しません。</p><table><thead><tr><th>受信時刻</th><th>識別子</th><th>対象</th><th>機能</th><th>理由</th><th>状態</th><th>操作</th></tr></thead><tbody>{}</tbody></table></section>
<section><h2>malware の制限</h2><p>malware の通報から、このノードの索引・検索・発見・推薦で返さなくした投稿と添付の hash の判断を確認・解除します。</p><a class="button secondary" href="/malware-restrictions">malware の制限を確認</a></section>
<section><h2>異議申し立ての審査</h2>{}</section>
<section><h2>運営操作の記録</h2><p>画面から行った直近 50 件の操作です。データベースで追記専用に保護されています。</p><table><thead><tr><th>実行時刻</th><th>識別子</th><th>運営者</th><th>操作</th><th>対象</th><th>変更</th></tr></thead><tbody>{}</tbody></table></section>
<section><h2>運用ログ</h2><p>実行時ログは既存のログ基盤に残します。この画面が秘密情報や生ログを複製することはありません。</p>{}<p><code>sudo journalctl -u kukuri-readiness.service -n 100 --no-pager</code></p></section>
<section><h2>配備との責任境界</h2><p class="boundary">プロバイダーや言語モデルの認証情報、機能の有効化、配備版、非公開チャンネルの秘密、招待符号、許可一覧、禁止一覧はこの画面から変更できません。確認済みの <code>operator-config.yaml</code>、<code>Terraform</code>、秘密管理、<code>cn-cli readiness</code> を使ってください。画面からの変更は、内容確認と明示的な確定を必須にします。</p></section>"#,
        escape_html(&view.admission_mode),
        escape_html(&view.readiness),
        admission_control,
        add_topic,
        topics,
        reports,
        appeals,
        audit,
        logs_link,
    );
    render_admin_page(
        "kukuri コミュニティノード運営",
        "<h1>コミュニティノード運営</h1><p>運営者専用の画面です。接続は <code>GCP IAP</code> の転送と権限管理で制限されます。</p>",
        &main,
    )
}

fn table_empty(message: &str, colspan: usize) -> String {
    format!(
        "<tr><td colspan=\"{colspan}\">{}</td></tr>",
        escape_html(message)
    )
}

fn admission_options(current: &str) -> String {
    ["open", "invite", "whitelist"]
        .into_iter()
        .map(|value| option(value, value, value == current))
        .collect()
}

fn report_status_options(current: &str, malware: bool) -> String {
    ["received", "reviewing", "actioned", "dismissed"]
        .into_iter()
        .filter(|value| !malware || *value != "actioned")
        .map(|value| option(value, value, value == current))
        .collect()
}

pub(crate) fn option(value: &str, label: &str, selected: bool) -> String {
    format!(
        "<option value=\"{}\"{}>{}</option>",
        escape_html(value),
        if selected { " selected" } else { "" },
        escape_html(label)
    )
}

fn operation_from_form(form: &AdminActionForm) -> anyhow::Result<AdminOperation> {
    let operation = match form.action.as_str() {
        "admission.set_mode" => {
            let mode = match form.value.trim() {
                "open" => AdmissionMode::Open,
                "invite" => AdmissionMode::Invite,
                "whitelist" => AdmissionMode::Whitelist,
                _ => bail!("unsupported admission mode"),
            };
            AdminOperation::SetAdmissionMode { mode }
        }
        "supported_topic.add" => AdminOperation::AddSupportedPublicTopic {
            topic_id: form.target_id.trim().to_string(),
        },
        "supported_topic.remove" => AdminOperation::RemoveSupportedPublicTopic {
            topic_id: form.target_id.trim().to_string(),
        },
        "report.set_status" => {
            let status = match form.value.trim() {
                "received" => OperatorReportStatus::Received,
                "reviewing" => OperatorReportStatus::Reviewing,
                "actioned" => OperatorReportStatus::Actioned,
                "dismissed" => OperatorReportStatus::Dismissed,
                _ => bail!("unsupported report status"),
            };
            AdminOperation::SetReportStatus {
                report_id: form.target_id.trim().to_string(),
                status,
            }
        }
        "malware.restrict_post" | "malware.restrict_blob" => AdminOperation::RestrictMalware {
            subject: malware_subject(&form.action),
            report_id: form.target_id.trim().to_string(),
        },
        "malware.release_post" | "malware.release_blob" => {
            AdminOperation::ReleaseMalwareRestriction {
                subject: malware_subject(&form.action),
                subject_id: form.target_id.trim().to_string(),
                reason: form.value.trim().to_string(),
            }
        }
        _ => bail!("unsupported admin operation"),
    };
    validate_admin_operation(&operation)?;
    Ok(operation)
}

pub(crate) fn csrf_matches(expected: &str, supplied: &str) -> bool {
    if expected.len() != supplied.len() || expected.is_empty() {
        return false;
    }
    expected
        .bytes()
        .zip(supplied.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

pub(crate) fn render_action_error(status: StatusCode, message: &str) -> Response {
    (status, Html(render_action_error_page(message))).into_response()
}

fn render_error() -> String {
    "<!doctype html><html lang=\"ja\"><head><meta charset=\"utf-8\"><title>kukuri 運営画面を表示できません</title></head><body><h1>運営情報を取得できません</h1><p>運用手順に従い、IAP 経由で cn-user-api のログを確認してください。</p></body></html>".to_string()
}

pub(crate) fn escape_html(value: &str) -> String {
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
    fn admin_mutations_fail_closed_without_deployment_actor() {
        let response = require_admin_actor(None)
            .expect_err("missing actor must reject writes")
            .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            require_admin_actor(Some("admin@node.example")).expect("explicit actor"),
            "admin@node.example"
        );
    }

    #[test]
    fn dashboard_escapes_database_and_environment_values() {
        let html = render_dashboard(&DashboardView {
            admission_mode: "open".to_string(),
            readiness: "active".to_string(),
            topics: vec![(
                "public_topic".to_string(),
                "<script>alert(1)</script>".to_string(),
                "now".to_string(),
            )],
            reports: vec![ReportView {
                created_at: "now".to_string(),
                id: "report-1".to_string(),
                subject: "event/&danger".to_string(),
                capability: "report_endpoint".to_string(),
                reason: "\"quoted\"".to_string(),
                status: "received".to_string(),
                restrictable: None,
                restricted: false,
            }],
            appeals: vec![AppealReview {
                risk_signal_id: "signal-<script>".to_string(),
                issuer_node_id: "issuer<&>".to_string(),
                target: "post_id".to_string(),
                target_id: "target<script>".to_string(),
                category: "nsfw".to_string(),
                severity: "high".to_string(),
                basis: "classifier_score".to_string(),
                confidence: Some(90),
                visibility: "local".to_string(),
                expires_at: None,
                appeal_status: "disputed".to_string(),
                reports: vec![kukuri_cn_core::AppealReviewReport {
                    id: "report-appeal".to_string(),
                    details: Some("<img src=x onerror=alert(1)>".to_string()),
                    status: "received".to_string(),
                    created_at: "2026-08-15T00:00:00Z".parse().expect("timestamp"),
                }],
            }],
            audit: vec![AuditView {
                occurred_at: "now".to_string(),
                id: "audit-1".to_string(),
                actor: "ops@example.com".to_string(),
                action: "report.set_status".to_string(),
                target: "report/report-1".to_string(),
                change: "received -> reviewing".to_string(),
            }],
            gcp_project: Some("project-id".to_string()),
            actor: Some("ops@example.com".to_string()),
            csrf_token: "csrf-token".to_string(),
            operator_review_enabled: true,
        });

        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("event/&amp;danger"));
        assert!(html.contains("&quot;quoted&quot;"));
        assert!(!html.contains("<img src=x onerror=alert(1)>"));
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
    }

    #[test]
    fn malware_reports_are_actioned_only_by_restricting_their_post() {
        // #1062: malware の通報は汎用の状態変更で actioned にできず、投稿の制限を確認する操作を出す。
        let report = |id: &str, status: &str, restricted: bool| ReportView {
            created_at: "now".to_string(),
            id: id.to_string(),
            subject: "post/post-1".to_string(),
            capability: "community_index".to_string(),
            reason: "malware".to_string(),
            status: status.to_string(),
            restrictable: Some(MalwareSubject::Post),
            restricted,
        };
        // 添付の通報は、その hash の制限を確認する操作を出す（AC-2）。
        let media = ReportView {
            restrictable: Some(MalwareSubject::Blob),
            ..report("report-media", "received", false)
        };
        let html = render_dashboard(&DashboardView {
            admission_mode: "open".to_string(),
            readiness: "active".to_string(),
            topics: Vec::new(),
            reports: vec![
                report("report-reviewing", "reviewing", false),
                report("report-actioned", "actioned", true),
                // #1062 より前に汎用の状態変更で対応済みにした通報は、状態を変えられ、制限もできる。
                report("report-legacy", "actioned", false),
                media,
            ],
            appeals: Vec::new(),
            audit: Vec::new(),
            gcp_project: None,
            actor: Some("ops@example.com".to_string()),
            csrf_token: "csrf".to_string(),
            operator_review_enabled: false,
        });
        assert!(html.contains(r#"name="action" value="malware.restrict_post"><input type="hidden" name="target_id" value="report-reviewing">"#));
        assert!(!html.contains(r#"value="report-actioned"><select"#));
        assert!(!html.contains(r#"<option value="actioned""#));
        assert!(html.contains(r#"value="report-legacy"><select"#));
        assert!(html.contains(r#"value="malware.restrict_post"><input type="hidden" name="target_id" value="report-legacy">"#));
        assert!(html.contains(r#"value="malware.restrict_blob"><input type="hidden" name="target_id" value="report-media"><button class="secondary" type="submit">添付の制限を確認"#));

        let form = AdminActionForm {
            action: "malware.release_post".to_string(),
            target_id: " post-1 ".to_string(),
            value: " cleared ".to_string(),
            ..AdminActionForm::default()
        };
        assert_eq!(
            operation_from_form(&form).unwrap(),
            AdminOperation::ReleaseMalwareRestriction {
                subject: MalwareSubject::Post,
                subject_id: "post-1".to_string(),
                reason: "cleared".to_string(),
            }
        );
        // 添付の hash は形式（BLAKE3 の 16 進 64 桁）を確かめる。
        let hash = "a".repeat(64);
        let blob = AdminActionForm {
            action: "malware.release_blob".to_string(),
            target_id: hash.clone(),
            value: "cleared".to_string(),
            ..AdminActionForm::default()
        };
        assert_eq!(
            operation_from_form(&blob).unwrap(),
            AdminOperation::ReleaseMalwareRestriction {
                subject: MalwareSubject::Blob,
                subject_id: hash,
                reason: "cleared".to_string(),
            }
        );
        assert!(
            operation_from_form(&AdminActionForm {
                target_id: "not-a-hash".to_string(),
                ..blob
            })
            .is_err()
        );
        let empty_reason = AdminActionForm {
            value: " ".to_string(),
            ..form
        };
        assert!(operation_from_form(&empty_reason).is_err());
    }

    #[test]
    fn admin_form_requires_csrf_and_maps_only_supported_runtime_operations() {
        let form = AdminActionForm {
            csrf_token: "token".to_string(),
            action: "admission.set_mode".to_string(),
            target_id: String::new(),
            value: "invite".to_string(),
            ..AdminActionForm::default()
        };
        assert!(csrf_matches("token", form.csrf_token.as_str()));
        assert!(!csrf_matches("token", "wrong"));
        assert!(matches!(
            operation_from_form(&form).unwrap(),
            kukuri_cn_core::AdminOperation::SetAdmissionMode {
                mode: kukuri_cn_core::AdmissionMode::Invite
            }
        ));

        let mut unsupported = form;
        unsupported.action = "provider.set_api_key".to_string();
        assert!(operation_from_form(&unsupported).is_err());
    }

    fn disputed_appeal_version() -> AppealReviewVersion {
        AppealReviewVersion {
            appeal_status: "disputed".to_string(),
            category: "nsfw".to_string(),
            severity: "high".to_string(),
            confidence: Some(90),
            visibility: "local".to_string(),
            expires_at: None,
            reports: vec![("report-1".to_string(), "received".to_string())],
        }
    }

    #[test]
    fn appeal_form_maps_only_supported_review_operations() {
        let expected = disputed_appeal_version();
        let form = AdminActionForm {
            action: "appeal.edit".to_string(),
            target_id: "signal-1".to_string(),
            category: "spam".to_string(),
            severity: "low".to_string(),
            confidence: "20".to_string(),
            ..AdminActionForm::default()
        };
        assert!(matches!(
            appeal_operation_from_form(&form, expected.clone()).expect("edit"),
            AppealReviewOperation::Edit { .. }
        ));
        let mut unsupported = form;
        unsupported.category = "not-a-category".to_string();
        assert!(appeal_operation_from_form(&unsupported, expected).is_err());
    }

    #[test]
    fn appeal_form_rejects_invalid_expires_at_with_japanese_message() {
        // #700: RFC 3339 でない有効期限は解析段階（確認・適用の両方が通る経路）で拒否する。
        let expected = disputed_appeal_version();
        let mut form = AdminActionForm {
            action: "appeal.edit".to_string(),
            target_id: "signal-1".to_string(),
            expires_at: "not-a-timestamp".to_string(),
            ..AdminActionForm::default()
        };
        let error = appeal_operation_from_form(&form, expected.clone()).unwrap_err();
        assert!(error.to_string().contains("RFC 3339"), "{error}");
        assert!(error.to_string().contains("有効期限"), "{error}");

        // 妥当な RFC 3339（時差付き・過去を含む）は受理される。
        for valid in [
            "2026-08-20T09:00:00Z",
            "2026-08-20T18:00:00+09:00",
            "2000-01-01T00:00:00Z",
        ] {
            form.expires_at = valid.to_string();
            assert!(
                appeal_operation_from_form(&form, expected.clone()).is_ok(),
                "{valid} は受理されるべき"
            );
        }
    }

    #[test]
    fn appeal_form_rejects_out_of_range_confidence_with_japanese_message() {
        let expected = disputed_appeal_version();
        // 再発行（適用時にも同じ解析を通る）でも範囲外・数値以外は拒否される。
        for invalid in ["101", "255", "abc", "-1"] {
            let form = AdminActionForm {
                action: "appeal.reissue".to_string(),
                target_id: "signal-1".to_string(),
                confidence: invalid.to_string(),
                ..AdminActionForm::default()
            };
            let error = appeal_operation_from_form(&form, expected.clone()).unwrap_err();
            assert!(
                error.to_string().contains("0 から 100"),
                "{invalid}: {error}"
            );
        }
    }
}
