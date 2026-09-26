use std::{
    path::PathBuf,
    sync::Arc,
};

use kukuri_desktop_runtime::{
    AppBuildProfile, ClientHost, DesktopRuntime, default_app_data_dir,
    resolve_app_data_dir_from_env, resolve_db_path_from_env,
};
pub(crate) use kukuri_desktop_runtime::{
    ClientStartupError as StartupError,
    ClientStartupState as DesktopStartupState, ClientStartupStatus as DesktopStartupStatus,
    app_consent_satisfied, consent_required_status,
    failed_startup_status as failed_status, load_app_consent_store, reset_app_consent_at_path,
};
#[cfg(test)]
pub(crate) use kukuri_desktop_runtime::{
    AGE_ATTESTATION_VERSION, APP_LEGAL_DOCUMENTS, AgeAttestationRecord, AppConsentDocumentRecord,
    age_attestation_satisfied, age_attestation_status, app_consent_documents_status,
    current_unix_seconds, save_app_consent_store,
    APP_LEGAL_AUTHORITATIVE_LANGUAGE, APP_LEGAL_EFFECTIVE_DATE, AppConsentStore,
    ClientStartupErrorKind as DesktopStartupErrorKind, LEGAL_BUNDLE_VERSION,
    app_consent_documents_satisfied, app_consent_path,
};
use serde::Serialize;
use tauri::{Emitter, Manager};
use crate::media_previews::MediaPreviewFiles;

/// `manage` 済みのTauri stateでは共有hostの参照だけを保持する。
/// account runtimeの所有・差し替え・停止は`ClientHost`へ集約し、このlockは
/// hostのclone / 差し替えだけの短い臨界区間に限定する(awaitを跨がない)。
pub(crate) struct DesktopState {
    host: std::sync::RwLock<Arc<ClientHost>>,
    pub(crate) app_data_dir: PathBuf,
    pub(crate) media_previews: MediaPreviewFiles,
}

impl DesktopState {
    pub(crate) fn runtime(&self) -> Arc<DesktopRuntime> {
        self.host()
            .runtime()
    }

    pub(crate) fn host(&self) -> Arc<ClientHost> {
        self.host.read().expect("host lock poisoned").clone()
    }

    pub(crate) fn replace_host(&self, next: Arc<ClientHost>) -> Arc<ClientHost> {
        self.media_previews.clear();
        std::mem::replace(&mut *self.host.write().expect("host lock poisoned"), next)
    }
}

/// Tauri コマンドの構造化エラー封筒(WP-C3)。invoke 側には
/// `{"code":"...","message":"..."}` の JSON で届く。message は従来の平文エラーと
/// 同一で、code は機械判定用(ドメイン別 code の拡充は Q6 の領分)。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct CommandError {
    pub(crate) code: String,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) retry_after_seconds: Option<u64>,
}

pub(crate) const COMMAND_FAILED_CODE: &str = "command_failed";
pub(crate) const SCOPE_LIMIT_REACHED_CODE: &str = "SCOPE_LIMIT_REACHED";

impl From<anyhow::Error> for CommandError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            code: COMMAND_FAILED_CODE.to_string(),
            message: error_message(error),
            status: None,
            retry_after_seconds: None,
        }
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self {
            code: COMMAND_FAILED_CODE.to_string(),
            message,
            status: None,
            retry_after_seconds: None,
        }
    }
}

impl From<kukuri_desktop_runtime::CommunityNodeIndexQueryError> for CommandError {
    fn from(error: kukuri_desktop_runtime::CommunityNodeIndexQueryError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            status: error.status,
            retry_after_seconds: error.retry_after_seconds,
        }
    }
}

impl From<kukuri_desktop_runtime::CommunityNodeIndexingRequestError> for CommandError {
    fn from(error: kukuri_desktop_runtime::CommunityNodeIndexingRequestError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            status: error.status,
            retry_after_seconds: error.retry_after_seconds,
        }
    }
}

impl From<kukuri_desktop_runtime::CommunityNodeTesterFeedbackError> for CommandError {
    fn from(error: kukuri_desktop_runtime::CommunityNodeTesterFeedbackError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            status: error.status,
            retry_after_seconds: error.retry_after_seconds,
        }
    }
}

impl From<kukuri_desktop_runtime::CommunityNodeTrustRelationError> for CommandError {
    fn from(error: kukuri_desktop_runtime::CommunityNodeTrustRelationError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            status: error.status,
            retry_after_seconds: None,
        }
    }
}

impl From<kukuri_desktop_runtime::CommunityNodeContentAdvisoryLookupError> for CommandError {
    fn from(error: kukuri_desktop_runtime::CommunityNodeContentAdvisoryLookupError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            status: error.status,
            retry_after_seconds: None,
        }
    }
}

impl From<kukuri_desktop_runtime::CommunityNodeReportError> for CommandError {
    fn from(error: kukuri_desktop_runtime::CommunityNodeReportError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            status: error.status,
            retry_after_seconds: None,
        }
    }
}

pub(crate) fn map_error(error: anyhow::Error) -> CommandError {
    if let Some(rejection) = error.downcast_ref::<kukuri_core::MetaverseResourceRejection>() {
        return CommandError {
            code: rejection.code(),
            message: rejection.to_string(),
            status: Some(
                if rejection.reason == kukuri_core::MetaverseResourceRejectionReason::RateExceeded {
                    429
                } else {
                    422
                },
            ),
            retry_after_seconds: None,
        };
    }
    // 購読する scope の上限(#1221 R2-C)。画面は code で判別し、操作を取り消して説明する。
    if error
        .downcast_ref::<kukuri_desktop_runtime::ScopeLimitReached>()
        .is_some()
    {
        return CommandError {
            code: SCOPE_LIMIT_REACHED_CODE.to_string(),
            message: error_message(error),
            status: None,
            retry_after_seconds: None,
        };
    }
    if let Some(request_error) =
        error.downcast_ref::<kukuri_desktop_runtime::DomeHostingRequestError>()
    {
        return CommandError {
            code: request_error.code.clone(),
            message: request_error.message.clone(),
            status: Some(request_error.status),
            retry_after_seconds: None,
        };
    }
    CommandError::from(error)
}

fn error_message(error: anyhow::Error) -> String {
    format!("{error:#}")
}

/// build の種別ごとの既定 app data dir。開発ビルドは配布版と別の dir を使う(#1105)。
/// `KUKURI_APP_DATA_DIR` / `KUKURI_INSTANCE` はこの dir を基準に解決する。
pub(crate) fn base_app_data_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    let platform_app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|error| format!("failed to resolve app data dir: {error}"))?;
    Ok(default_app_data_dir(
        &platform_app_data_dir,
        AppBuildProfile::current(),
    ))
}

pub(crate) fn resolve_app_data_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    resolve_app_data_dir_from_env(&base_app_data_dir(app_handle)?).map_err(error_message)
}

/// アプリ同意など端末レベルのファイルの命名基準となる flat db path。
/// #859 以降 runtime の db は `accounts/<id>/kukuri.db` に置かれるが、端末レベルの
/// 状態(同意・年齢申告)はアカウントに紐づけないため、従来どおり
/// `<app_data>/kukuri.db` を基準にした兄弟ファイル名を使い続ける。
pub(crate) fn resolve_db_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    resolve_db_path_from_env(&base_app_data_dir(app_handle)?).map_err(error_message)
}

pub(crate) async fn build_desktop_state(
    app_handle: &tauri::AppHandle,
) -> Result<DesktopState, StartupError> {
    let app_data_dir = resolve_app_data_dir(app_handle).map_err(StartupError::unknown)?;
    let host = match ClientHost::start_if_consented(app_data_dir.clone()).await? {
        kukuri_desktop_runtime::ClientHostStart::Ready(host) => host,
        kukuri_desktop_runtime::ClientHostStart::ConsentRequired(_) => {
            return Err(StartupError::unknown(
                "app consent is required before starting the runtime".to_string(),
            ));
        }
    };
    spawn_runtime_event_bridge(app_handle, &host);

    Ok(DesktopState {
        host: std::sync::RwLock::new(host),
        app_data_dir,
        media_previews: MediaPreviewFiles::new(
            app_handle.path().app_cache_dir().ok().map(|dir| dir.join("kukuri-display")),
        ),
    })
}

/// backup／restore中に一時停止したruntimeの後継を構築する。
/// 常駐タスクは`ClientHost::replace_runtime`がevent購読順序を保って開始する。
pub(crate) async fn build_runtime(
    db_path: PathBuf,
) -> Result<Arc<DesktopRuntime>, StartupError> {
    ClientHost::build_detached_runtime(db_path)
        .await
}

#[cfg(test)]
fn distribution_community_node_config(
) -> Result<kukuri_desktop_runtime::CommunityNodeConfig, serde_json::Error> {
    kukuri_desktop_runtime::distribution_community_node_config()
}

#[cfg(test)]
#[derive(serde::Deserialize)]
struct DistributionLegalMetadata {
    controller_name: String,
    contact: String,
}

#[cfg(test)]
fn distribution_legal_metadata() -> Result<DistributionLegalMetadata, serde_json::Error> {
    serde_json::from_str(include_str!("../distribution/legal.json"))
}

fn spawn_runtime_event_bridge(app_handle: &tauri::AppHandle, host: &Arc<ClientHost>) {
    let mut rx = host.subscribe_events();
    let app = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let _ = app.emit("kukuri://runtime-event", &event);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let _ = app.emit(
                        "kukuri://runtime-event",
                        &kukuri_desktop_runtime::RuntimeEvent::AdultMediaLabelEvicted { hash: None },
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

pub(crate) fn reset_app_consent_after_device_restore(
    app_handle: &tauri::AppHandle,
) -> Result<DesktopStartupStatus, String> {
    let db_path = resolve_db_path(app_handle)?;
    reset_app_consent_at_path(&db_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_community_node_config_is_valid() {
        let config = distribution_community_node_config().expect("distribution config");
        assert_eq!(config.nodes.len(), 1);
        assert!(config.nodes[0].base_url.starts_with("https://"));
    }

    // IPC エラー封筒の wire 形状(WP-C3)。TS 側 normalizeInvokeError と対になる
    // 同一バイナリ内契約 — 形状を変える場合は両側同時に変更する。
    #[test]
    fn command_error_serializes_to_code_message_envelope() {
        let error = map_error(anyhow::anyhow!("outer").context("inner"));
        let json = serde_json::to_string(&error).expect("serialize command error");
        assert_eq!(
            json,
            r#"{"code":"command_failed","message":"inner: outer"}"#
        );
    }

    #[test]
    fn scope_limit_is_reported_by_its_code_through_context() {
        let error = map_error(
            anyhow::Error::from(kukuri_desktop_runtime::ScopeLimitReached).context("join"),
        );
        assert_eq!(error.code, SCOPE_LIMIT_REACHED_CODE);
    }

    #[test]
    fn command_error_from_string_preserves_message() {
        let error = CommandError::from("failed to resolve app data dir: boom".to_string());
        assert_eq!(error.code, COMMAND_FAILED_CODE);
        assert_eq!(error.message, "failed to resolve app data dir: boom");
    }

    #[test]
    fn community_index_error_serializes_status_and_retry_after() {
        let error = CommandError::from(kukuri_desktop_runtime::CommunityNodeIndexQueryError {
            code: "RATE_LIMITED".to_string(),
            message: "try again later".to_string(),
            status: Some(429),
            retry_after_seconds: Some(17),
        });
        let json = serde_json::to_string(&error).expect("serialize index error");
        assert_eq!(
            json,
            r#"{"code":"RATE_LIMITED","message":"try again later","status":429,"retry_after_seconds":17}"#
        );
    }

    #[test]
    fn community_indexing_request_error_preserves_stable_conflict_code() {
        let error = CommandError::from(kukuri_desktop_runtime::CommunityNodeIndexingRequestError {
            code: "CHANNEL_SECRET_CONFLICT".to_string(),
            message: "channel capability conflicts with the existing registration".to_string(),
            status: Some(409),
            retry_after_seconds: None,
        });
        let json = serde_json::to_string(&error).expect("serialize indexing request error");
        assert_eq!(
            json,
            r#"{"code":"CHANNEL_SECRET_CONFLICT","message":"channel capability conflicts with the existing registration","status":409}"#
        );
    }

    #[test]
    fn trust_relation_error_preserves_stable_unavailable_code() {
        let error = CommandError::from(kukuri_desktop_runtime::CommunityNodeTrustRelationError {
            code: "RELATION_NOT_FOUND".to_string(),
            message: "no relation observed for this pair".to_string(),
            status: Some(404),
        });
        let json = serde_json::to_string(&error).expect("serialize trust relation error");
        assert_eq!(
            json,
            r#"{"code":"RELATION_NOT_FOUND","message":"no relation observed for this pair","status":404}"#
        );
    }

    fn record(slug: &str, version: i32, accepted_at: i64) -> AppConsentDocumentRecord {
        AppConsentDocumentRecord {
            slug: slug.to_string(),
            version,
            accepted_at,
            language: "ja".to_string(),
            app_version: "0.1.8".to_string(),
            build_profile: None,
        }
    }

    fn attestation(version: i32, attested_at: i64) -> AgeAttestationRecord {
        AgeAttestationRecord {
            version,
            attested_at,
            language: "ja".to_string(),
            app_version: "0.1.8".to_string(),
            build_profile: None,
        }
    }

    #[test]
    fn app_consent_satisfied_requires_every_document_at_current_or_newer_version() {
        assert_eq!(LEGAL_BUNDLE_VERSION, 8);
        assert!(!app_consent_documents_satisfied(&AppConsentStore::default()));

        // terms だけ同意しても不十分。
        let partial = AppConsentStore {
            records: vec![record("terms", LEGAL_BUNDLE_VERSION, 1)],
            age_attestations: Vec::new(),
        };
        assert!(!app_consent_documents_satisfied(&partial));

        // 旧版の記録は再同意が必要。
        let outdated = AppConsentStore {
            records: vec![record("terms", 1, 1), record("privacy", 1, 1)],
            age_attestations: Vec::new(),
        };
        assert!(!app_consent_documents_satisfied(&outdated));

        let satisfied = AppConsentStore {
            records: vec![
                record("terms", LEGAL_BUNDLE_VERSION, 1),
                record("privacy", LEGAL_BUNDLE_VERSION + 1, 1),
            ],
            age_attestations: Vec::new(),
        };
        assert!(app_consent_documents_satisfied(&satisfied));
    }

    // #858: 文書同意が揃っていても年齢自己申告が無ければ gate は開かない。
    // 逆に、申告だけあって文書同意が無い場合も開かない(別状態の独立判定)。
    #[test]
    fn app_consent_satisfied_requires_age_attestation_in_addition_to_documents() {
        let documents_only = AppConsentStore {
            records: vec![
                record("terms", LEGAL_BUNDLE_VERSION, 1),
                record("privacy", LEGAL_BUNDLE_VERSION, 1),
            ],
            age_attestations: Vec::new(),
        };
        assert!(app_consent_documents_satisfied(&documents_only));
        assert!(!age_attestation_satisfied(&documents_only));
        assert!(!app_consent_satisfied(&documents_only));

        let attestation_only = AppConsentStore {
            records: Vec::new(),
            age_attestations: vec![attestation(AGE_ATTESTATION_VERSION, 1)],
        };
        assert!(age_attestation_satisfied(&attestation_only));
        assert!(!app_consent_satisfied(&attestation_only));

        let both = AppConsentStore {
            records: documents_only.records.clone(),
            age_attestations: attestation_only.age_attestations.clone(),
        };
        assert!(app_consent_satisfied(&both));
    }

    #[test]
    fn age_attestation_status_reports_latest_record() {
        let empty_status = age_attestation_status(&AppConsentStore::default());
        assert_eq!(empty_status.current_version, AGE_ATTESTATION_VERSION);
        assert_eq!(empty_status.attested_version, None);
        assert_eq!(empty_status.attested_at, None);

        let store = AppConsentStore {
            records: Vec::new(),
            age_attestations: vec![
                attestation(AGE_ATTESTATION_VERSION, 100),
                attestation(AGE_ATTESTATION_VERSION, 200),
            ],
        };
        let status = age_attestation_status(&store);
        assert_eq!(status.attested_version, Some(AGE_ATTESTATION_VERSION));
        assert_eq!(status.attested_at, Some(200));
    }

    #[test]
    fn app_consent_status_reports_latest_record_per_document() {
        let store = AppConsentStore {
            records: vec![
                record("terms", 1, 100),
                record("terms", LEGAL_BUNDLE_VERSION, 200),
                record("privacy", 1, 150),
            ],
            age_attestations: Vec::new(),
        };
        let status = app_consent_documents_status(&store);
        assert_eq!(status.len(), APP_LEGAL_DOCUMENTS.len());

        let terms = status.iter().find(|doc| doc.slug == "terms").unwrap();
        assert_eq!(terms.current_version, LEGAL_BUNDLE_VERSION);
        assert_eq!(terms.effective_date, APP_LEGAL_EFFECTIVE_DATE);
        assert_eq!(
            terms.authoritative_language,
            APP_LEGAL_AUTHORITATIVE_LANGUAGE
        );
        assert!(terms.material_change);
        let legal = distribution_legal_metadata().expect("distribution legal metadata");
        assert_eq!(terms.controller_name, legal.controller_name);
        assert_eq!(terms.contact, legal.contact);
        assert_eq!(terms.accepted_version, Some(LEGAL_BUNDLE_VERSION));
        assert_eq!(terms.accepted_at, Some(200));
        assert_eq!(terms.accepted_language.as_deref(), Some("ja"));
        assert_eq!(terms.accepted_app_version.as_deref(), Some("0.1.8"));

        let privacy = status.iter().find(|doc| doc.slug == "privacy").unwrap();
        assert_eq!(privacy.accepted_version, Some(1));
    }

    // #857: 旧形式(bundle 単一フラグ)の記録は読み替えず、全文書未同意として
    // 再同意を求める。
    #[test]
    fn legacy_bundle_consent_file_requires_reconsent() {
        let dir = std::env::temp_dir().join(format!(
            "kukuri-consent-legacy-test-{}",
            current_unix_seconds()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("kukuri.db");
        std::fs::write(
            app_consent_path(&db_path),
            r#"{"accepted_bundle_version":2,"accepted_at":1700000000}"#,
        )
        .expect("write legacy consent file");

        let store = load_app_consent_store(&db_path);
        assert!(store.records.is_empty());
        assert!(!app_consent_satisfied(&store));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn canonical_legal_documents_match_the_runtime_bundle_version() {
        const TERMS: &str = include_str!("../../../../docs/legal/terms-of-service.md");
        const PRIVACY: &str = include_str!("../../../../docs/legal/privacy-policy.md");
        const EXTERNAL_TRANSMISSION: &str =
            include_str!("../../../../docs/legal/external-transmission-notice.md");
        let expected = format!("Legal bundle version: {LEGAL_BUNDLE_VERSION}");

        assert!(TERMS.contains(&expected));
        assert!(PRIVACY.contains(&expected));
        assert!(EXTERNAL_TRANSMISSION.contains(&expected));
        for required_clause in [
            "KingYoSun",
            "氏名・住所",
            "定義",
            "利用資格と成人向け表現",
            "18歳以上",
            "公的な年齢確認",
            // #1056: 成人向け表現の第 2 のラベル源(設定した Community Node の推定)。
            "Community Node が推定したラベル",
            "推定を採用する Community Node",
            // #1061: 信頼評価による折りたたみ(第 5 項)とブロック・ミュートの任意提供(第 6 項)。
            "信頼評価に基づいて",
            "任意の同意文書",
            // #1174: 公開投稿のlink preview取得とprivate contentの非対象化。
            "公開投稿の先頭の外部 URL",
            "private channel／DM の URL は自動取得しません",
            "投稿コンテンツの権利帰属",
            "必要な権利または許諾",
            "投稿者の責任",
            "限定的な利用許諾",
            "投稿撤回後の取扱い",
            "広告、宣伝、生成 AI の学習",
            "通報・利用制限",
            "サービスの変更・中断・終了",
            "アカウントを識別する鍵",
            "中央から再発行",
            "Community Node",
            "オープンソースライセンス",
            "消費者契約法",
            "日本法",
            "東京地方裁判所",
            "東京簡易裁判所",
            "規約の変更",
            "変更履歴",
        ] {
            assert!(
                TERMS.contains(required_clause),
                "terms must contain `{required_clause}`"
            );
        }

        for required_clause in [
            "GitHub Releases",
            "自動更新確認",
            "公開鍵",
            "リアクション",
            "検索・索引",
            "行動分析",
            "日本語版を正文",
            "変更履歴",
            // #1056: 推定の照会で送る識別子と、送らない情報。
            "成人向け表現の推定の照会",
            "添付ファイルの識別子",
            // #1061: 信頼評価の照会とブロック・ミュートの提供で送る項目。
            "信頼評価の照会",
            "ブロック・ミュートの提供",
            // #1174: previewの送信先、送信項目、対象外content、transient保持。
            "OGP 画像",
            "URL の path／query",
            "process memory",
        ] {
            assert!(
                PRIVACY.contains(required_clause),
                "privacy policy must contain `{required_clause}`"
            );
        }
        // #1056: 外部送信表示にも推定の照会の送信先・項目を載せる(AC-5)。
        for required_clause in [
            "成人向け表現の推定の照会",
            "添付ファイルの識別子",
            // #1061: 信頼評価の照会の送信先・項目(AC-5)。
            "信頼評価の照会",
            "ブロック・ミュートの提供",
            // #1174: link先とOGP image hostへの自動送信。
            "OGP 画像配信先",
            "process-memory cache",
        ] {
            assert!(
                EXTERNAL_TRANSMISSION.contains(required_clause),
                "external transmission notice must contain `{required_clause}`"
            );
        }
        let legal = distribution_legal_metadata().expect("distribution legal metadata");
        for disclosed_value in [&legal.controller_name, &legal.contact] {
            assert!(TERMS.contains(disclosed_value));
            assert!(PRIVACY.contains(disclosed_value));
            assert!(EXTERNAL_TRANSMISSION.contains(disclosed_value));
        }
    }

    #[test]
    fn external_transmission_notice_matches_distribution_and_updater_config() {
        const EXTERNAL_TRANSMISSION: &str =
            include_str!("../../../../docs/legal/external-transmission-notice.md");
        const PRIVACY: &str = include_str!("../../../../docs/legal/privacy-policy.md");
        const TAURI_CONFIG: &str = include_str!("../tauri.conf.json");
        const STORE_CONFIG: &str = include_str!("../tauri.microsoft-store.conf.json");
        const UPDATE_SCHEDULER: &str = include_str!("../../src/shell/useAppUpdateScheduler.ts");

        let tauri_config: serde_json::Value =
            serde_json::from_str(TAURI_CONFIG).expect("tauri config must be valid json");
        let updater_endpoints = tauri_config
            .pointer("/plugins/updater/endpoints")
            .and_then(serde_json::Value::as_array)
            .expect("updater endpoints");
        assert!(!updater_endpoints.is_empty());
        for endpoint in updater_endpoints {
            let endpoint = endpoint.as_str().expect("updater endpoint string");
            let host = endpoint
                .strip_prefix("https://")
                .and_then(|remainder| remainder.split('/').next())
                .expect("https updater endpoint host");
            assert!(
                host == "github.com" || host.ends_with(".github.com"),
                "new updater host `{host}` must be reviewed in the external-transmission notice"
            );
        }
        assert!(EXTERNAL_TRANSMISSION.contains("GitHub Releases"));
        assert!(PRIVACY.contains("GitHub Releases"));
        assert!(UPDATE_SCHEDULER.contains("const UPDATE_CHECK_INTERVAL_MS = 30 * 60 * 1000;"));
        let store_config: serde_json::Value =
            serde_json::from_str(STORE_CONFIG).expect("Store config must be valid json");
        assert_eq!(
            store_config.pointer("/plugins/updater/active"),
            Some(&serde_json::Value::Bool(false))
        );
        assert!(EXTERNAL_TRANSMISSION.contains("Microsoft Store版はkukuri内から送信しません"));
        assert!(PRIVACY.contains("Microsoft Store版の更新はMicrosoft Store／Windowsへ委譲"));

        let distribution = distribution_community_node_config().expect("distribution config");
        for node in distribution.nodes {
            assert!(
                EXTERNAL_TRANSMISSION.contains(node.base_url.as_str()),
                "distribution node `{}` must be disclosed",
                node.base_url
            );
        }
    }

    #[test]
    fn image_csp_allows_only_local_profile_asset_sources() {
        const TAURI_CONFIG: &str = include_str!("../tauri.conf.json");
        let tauri_config: serde_json::Value =
            serde_json::from_str(TAURI_CONFIG).expect("tauri config must be valid json");
        let image_sources = tauri_config
            .pointer("/app/security/csp/img-src")
            .and_then(serde_json::Value::as_str)
            .expect("img-src must be a string")
            .split_ascii_whitespace()
            .collect::<Vec<_>>();

        assert!(image_sources.contains(&"'self'"));
        assert!(image_sources.contains(&"asset:"));
        assert!(image_sources.contains(&"http://asset.localhost"));
        assert!(image_sources.contains(&"blob:"));
        assert!(image_sources.contains(&"data:"));
        assert!(!image_sources.contains(&"http:"));
        assert!(!image_sources.contains(&"https:"));
    }

    #[test]
    fn app_consent_round_trips_through_disk() {
        let dir =
            std::env::temp_dir().join(format!("kukuri-consent-test-{}", current_unix_seconds()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("kukuri.db");

        assert!(load_app_consent_store(&db_path).records.is_empty());

        let store = AppConsentStore {
            records: vec![
                record("terms", LEGAL_BUNDLE_VERSION, 1_700_000_000),
                record("privacy", LEGAL_BUNDLE_VERSION, 1_700_000_000),
            ],
            age_attestations: vec![attestation(AGE_ATTESTATION_VERSION, 1_700_000_000)],
        };
        save_app_consent_store(&db_path, &store).expect("save consent");

        let loaded = load_app_consent_store(&db_path);
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[0].slug, "terms");
        assert_eq!(loaded.records[0].language, "ja");
        assert_eq!(loaded.records[0].app_version, "0.1.8");
        assert_eq!(loaded.age_attestations.len(), 1);
        assert!(app_consent_satisfied(&loaded));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn startup_state_status_can_be_updated() {
        let state = DesktopStartupState::initializing();
        assert!(matches!(state.status(), DesktopStartupStatus::Initializing));
        state.set_status(DesktopStartupStatus::ConsentRequired {
            documents: app_consent_documents_status(&AppConsentStore::default()),
            age_attestation: age_attestation_status(&AppConsentStore::default()),
        });
        assert!(matches!(
            state.status(),
            DesktopStartupStatus::ConsentRequired { .. }
        ));
        state.set_status(DesktopStartupStatus::Ready);
        assert!(matches!(state.status(), DesktopStartupStatus::Ready));
    }

    // WP-Q2: store 由来でないエラーは、message に "migration"/"sqlite"/"database" を
    // 含んでいても Unknown に分類される(= 旧実装の文字列判定による誤分類が起きない)。
    // Open/Migration の typed 分類は store 側の connect_file_*_is_typed_as_* テストが担保。
    #[test]
    fn classify_startup_error_ignores_message_strings_for_non_store_errors() {
        for message in [
            "some unrelated database noise",
            "migration-like wording in an unrelated failure",
            "failed to connect sqlite database (but not a StoreStartupError)",
        ] {
            let error = StartupError::from_error(anyhow::anyhow!(message));
            assert!(
                matches!(error.kind, DesktopStartupErrorKind::Unknown),
                "non-store error must classify as Unknown regardless of message: {message}"
            );
        }
    }
}
