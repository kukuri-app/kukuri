//! 非 appeal 運営操作の確認・完了・拒否ページ(#740)。
//!
//! `admin_shell` の共通枠に載せ、実行者・対象・変更前後・影響・取引境界を同じ比較枠で並べる。
//! 隠し入力と `preview -> confirm -> apply` の契約(ADR 0029)は `admin.rs` のハンドラが担い、
//! ここでは表示だけを扱う。preview の「変更前」は DB から読まず、apply 時の再取得に委ねる。

use kukuri_cn_core::{AdminOperation, OperatorAction};

use crate::admin::{AdminActionForm, escape_html};
use crate::admin_shell::{
    action_header, code_fact, dashboard_link, facts, render_admin_page, text_fact,
};

const TRANSACTION_BOUNDARY: &str = "状態変更と操作記録は一つの取引で確定します。";

pub(crate) fn render_preview(
    csrf_token: &str,
    actor: &str,
    form: &AdminActionForm,
    operation: &AdminOperation,
) -> String {
    let (title, target, after, impact) = operation_summary(operation);
    let header = action_header("適用前の確認", title);
    let main = format!(
        r#"<section class="dialog">{}<p class="boundary">適用時に現在値と入力をもう一度検証します。</p><form method="post" action="/actions/apply"><input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="action" value="{}"><input type="hidden" name="target_id" value="{}"><input type="hidden" name="value" value="{}"><div class="actions"><button class="primary" type="submit">確認して適用</button>{}</div></form></section>"#,
        facts(&[
            code_fact("運営者", actor),
            code_fact("対象", &target),
            text_fact("変更前", "適用時に現在値を再取得します"),
            code_fact("変更後", &after),
            text_fact("影響", &impact),
            text_fact("取引境界", TRANSACTION_BOUNDARY),
        ]),
        escape_html(csrf_token),
        escape_html(&form.action),
        escape_html(&form.target_id),
        escape_html(&form.value),
        dashboard_link("取り消す", false),
    );
    render_admin_page("運営操作の確認", &header, &main)
}

/// 見出し・対象・変更後の値・影響。
fn operation_summary(operation: &AdminOperation) -> (&'static str, String, String, String) {
    match operation {
        AdminOperation::SetAdmissionMode { mode } => (
            "受け入れ方式を変更しますか",
            "community-node admission".to_string(),
            mode.as_str().to_string(),
            format!(
                "受け入れ方式を {} に変更します。現在接続中の利用者は維持されます。",
                mode.as_str()
            ),
        ),
        AdminOperation::AddSupportedPublicTopic { topic_id } => (
            "対応する公開トピックを追加しますか",
            topic_id.clone(),
            "対応トピックに追加".to_string(),
            "この公開トピックの取り込みを許可します。安全性確認と準備確認は引き続き適用されます。"
                .to_string(),
        ),
        AdminOperation::RemoveSupportedPublicTopic { topic_id } => (
            "対応する公開トピックを削除しますか",
            topic_id.clone(),
            "対応トピックから削除".to_string(),
            "この範囲の今後の取り込みを停止します。索引からの除去と処理状態の整合は非同期です。"
                .to_string(),
        ),
        AdminOperation::SetReportStatus { report_id, status } => (
            "通報の状態を変更しますか",
            report_id.clone(),
            status.as_str().to_string(),
            format!(
                "このノード内の通報状態を {} に変更します。",
                status.as_str()
            ),
        ),
        AdminOperation::RestrictMalwarePost { report_id } => (
            "malware の通報の投稿を制限しますか",
            report_id.clone(),
            "投稿を制限し、通報を actioned".to_string(),
            "この通報の投稿を、このノードの索引・検索・発見・推薦で返さなくします。同じ投稿に有効な制限があれば、それを使います。P2P 上の投稿と他のノードは変わりません。解除するまで続き、通報の保持期限では解除されません。"
                .to_string(),
        ),
        AdminOperation::ReleaseMalwareRestriction { post_id, reason } => (
            "投稿の malware の制限を解除しますか",
            post_id.clone(),
            format!("解除（理由: {reason}）"),
            "この投稿の malware の制限だけを解除します。権利侵害による送信防止、安全性の判定、著者の撤回は引き続き適用されます。索引に残っている投稿はすぐに戻り、制限中に索引から外れた投稿は次に取り込まれたときに戻ります。"
                .to_string(),
        ),
    }
}

pub(crate) fn render_action_success(action: &OperatorAction) -> String {
    let header = action_header("操作完了", "運営操作を適用しました");
    let main = format!(
        r#"<section class="dialog">{}<div class="actions">{}</div></section>"#,
        facts(&[
            code_fact("操作記録の識別子", &action.id),
            code_fact("運営者", &action.actor),
            text_fact("操作", &action.action),
            code_fact(
                "対象",
                &format!("{}/{}", action.target_kind, action.target_id),
            ),
            code_fact("変更前", &action.before.to_string()),
            code_fact("変更後", &action.after.to_string()),
            text_fact("取引境界", "状態変更と操作記録は一つの取引で確定済みです。"),
        ]),
        dashboard_link("ダッシュボードへ戻る", true),
    );
    render_admin_page("運営操作を適用しました", &header, &main)
}

pub(crate) fn render_action_error_page(message: &str) -> String {
    let header = action_header("未適用", "運営操作を拒否しました");
    let main = format!(
        r#"<section class="dialog"><p class="danger">{}</p><p class="boundary">状態と操作記録は変更されていません。</p><div class="actions">{}</div></section>"#,
        escape_html(message),
        dashboard_link("ダッシュボードへ戻る", true),
    );
    render_admin_page("運営操作を拒否しました", &header, &main)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_shell::ADMIN_STYLE;

    fn sample_action() -> OperatorAction {
        OperatorAction {
            id: "audit-1".to_string(),
            occurred_at: "2026-08-22T00:00:00Z".parse().expect("timestamp"),
            actor: "ops@example.com".to_string(),
            action: "admission.set_mode".to_string(),
            target_kind: "admission".to_string(),
            target_id: "community-node".to_string(),
            before: serde_json::json!({ "mode": "open" }),
            after: serde_json::json!({ "mode": "invite" }),
        }
    }

    fn sample_preview() -> String {
        render_preview(
            "csrf",
            "ops@example.com",
            &AdminActionForm {
                csrf_token: "csrf".to_string(),
                action: "admission.set_mode".to_string(),
                target_id: String::new(),
                value: "invite".to_string(),
                ..AdminActionForm::default()
            },
            &AdminOperation::SetAdmissionMode {
                mode: kukuri_cn_core::AdmissionMode::Invite,
            },
        )
    }

    #[test]
    fn action_pages_share_dashboard_shell_and_link_back() {
        // #740: 確認・完了・拒否ページは dashboard と同じ style と header を使う。
        for (stage, html) in [
            ("適用前の確認", sample_preview()),
            ("操作完了", render_action_success(&sample_action())),
            ("未適用", render_action_error_page("拒否理由")),
        ] {
            assert!(html.contains(ADMIN_STYLE), "{stage}: style");
            assert!(
                html.contains("<nav class=\"crumbs\" aria-label=\"現在位置\"><a href=\"/\">コミュニティノード運営</a>"),
                "{stage}: crumbs"
            );
            assert!(
                html.contains(&format!("<span aria-current=\"page\">{stage}</span>")),
                "{stage}: current stage"
            );
            assert!(
                !html.contains("max-width:760px"),
                "{stage}: 旧 simple page の style が残っている"
            );
        }
    }

    #[test]
    fn preview_compares_actor_target_change_impact_and_boundary() {
        let html = sample_preview();
        let facts_start = html.find("<dl class=\"facts\">").expect("facts grid");
        let facts = &html[facts_start..];
        let mut cursor = 0;
        for label in ["運営者", "対象", "変更前", "変更後", "影響", "取引境界"] {
            let needle = format!("<dt>{label}</dt>");
            let at = facts[cursor..]
                .find(&needle)
                .unwrap_or_else(|| panic!("{label} が比較枠に無いか順序が違う"));
            cursor += at + needle.len();
        }
        assert!(html.contains("<dt>変更後</dt><dd><code>invite</code></dd>"));
        assert!(html.contains("<dt>変更前</dt><dd>適用時に現在値を再取得します</dd>"));
        assert!(
            html.contains("<dt>取引境界</dt><dd>状態変更と操作記録は一つの取引で確定します。</dd>")
        );
        assert!(html.contains("<button class=\"primary\" type=\"submit\">確認して適用</button>"));
        assert!(html.contains("<a class=\"button secondary\" href=\"/\">取り消す</a>"));
        // 隠し入力は従来どおり(確認→適用の契約は不変)。
        assert!(html.contains("<input type=\"hidden\" name=\"csrf_token\" value=\"csrf\">"));
        assert!(
            html.contains("<input type=\"hidden\" name=\"action\" value=\"admission.set_mode\">")
        );
        assert!(html.contains("<input type=\"hidden\" name=\"value\" value=\"invite\">"));
    }

    #[test]
    fn preview_escapes_actor_target_and_values() {
        let html = render_preview(
            "csrf",
            "ops<script>@kukuri.app",
            &AdminActionForm {
                csrf_token: "csrf".to_string(),
                action: "supported_topic.add".to_string(),
                target_id: "kukuri:topic:<script>".to_string(),
                value: String::new(),
                ..AdminActionForm::default()
            },
            &AdminOperation::AddSupportedPublicTopic {
                topic_id: "kukuri:topic:<script>".to_string(),
            },
        );
        assert!(!html.contains("ops<script>"));
        assert!(!html.contains("kukuri:topic:<script>"));
        assert!(html.contains("ops&lt;script&gt;@kukuri.app"));
        assert!(html.contains("kukuri:topic:&lt;script&gt;"));
    }

    #[test]
    fn success_compares_before_and_after_in_facts_grid() {
        let html = render_action_success(&sample_action());
        assert!(html.contains("<dl class=\"facts\">"));
        assert!(
            html.contains(
                "<dt>変更前</dt><dd><code>{&quot;mode&quot;:&quot;open&quot;}</code></dd>"
            )
        );
        assert!(html.contains(
            "<dt>変更後</dt><dd><code>{&quot;mode&quot;:&quot;invite&quot;}</code></dd>"
        ));
        assert!(html.contains("<dt>対象</dt><dd><code>admission/community-node</code></dd>"));
        assert!(html.contains("<a class=\"button primary\" href=\"/\">ダッシュボードへ戻る</a>"));
    }

    #[test]
    fn error_page_offers_primary_recovery_and_escapes_message() {
        let html = render_action_error_page("<b>不正</b>");
        assert!(!html.contains("<b>不正</b>"));
        assert!(html.contains("&lt;b&gt;不正&lt;/b&gt;"));
        assert!(html.contains("状態と操作記録は変更されていません。"));
        assert!(html.contains("<a class=\"button primary\" href=\"/\">ダッシュボードへ戻る</a>"));
    }
}
