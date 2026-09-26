//! #1109: moderation 構成の変更後の再 scan と content advisory の整合（Postgres integration）。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。
//! - `rescan_allow_without_labels_expires_stale_advisory_signal`（AC-1 / AC-2 / AC-4、TR-1 / TR-6）
//! - `rescan_keeps_only_current_advisory_categories`（AC-2、TR-2）
//! - `rescan_does_not_expire_protected_signals`（AC-3、TR-3）
//! - `non_indexable_rescan_keeps_advisory_signals`（TR-4）

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use kukuri_cn_core::{
    IndexScopeKind, NewCommunityNodeReport, NewIndexEntry, PgSafetyArtifactStore,
    RiskSignalMetadataEdit, TestDatabase, connect_postgres, dispute_risk_signal,
    edit_risk_signal_detection_metadata, filter_surfaceable_objects, get_risk_signal,
    initialize_database, insert_community_node_appeal, list_content_advisories_for_subjects,
    list_risk_signals_for_target, list_trust_risk_inputs, update_risk_signal_appeal_status,
    upsert_index_entry,
};
use kukuri_cn_safety::event::ModerationEventSigner;
use kukuri_cn_safety::{
    AppealStatus, ContentAdvisory, GeneralAction, ProviderScanRequest, ProviderScanResult,
    RiskSignalTarget, SafetyCategory, SafetyLabel, SafetyPolicy, SafetyProvider,
    SafetyProviderCapability, ScanError, Severity, SubjectKind,
};
use kukuri_cn_safety_runtime::{
    SafetyOrchestrator, SafetyScanService, Secp256k1ModerationEventSigner, SystemScanClock,
    UuidEventIdGenerator,
};
use sqlx::PgPool;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const TEST_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SCOPE_ID: &str = "kukuri:topic:advisory-rescan";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

/// 構成ごとに固定の結果を返す provider。name が違えば scan 構成の fingerprint も変わる。
struct Scripted {
    name: &'static str,
    labels: Vec<SafetyLabel>,
    fail: bool,
}

#[async_trait]
impl SafetyProvider for Scripted {
    fn name(&self) -> &str {
        self.name
    }
    fn capabilities(&self) -> &[SafetyProviderCapability] {
        &[SafetyProviderCapability::GeneralMediaModeration]
    }
    async fn scan(&self, _: &ProviderScanRequest) -> Result<ProviderScanResult, ScanError> {
        if self.fail {
            return Err(ScanError::Unavailable("provider down".into()));
        }
        let mut result = ProviderScanResult::completed(
            self.name,
            SafetyProviderCapability::GeneralMediaModeration,
        );
        result.score = self
            .labels
            .iter()
            .filter_map(|label| label.confidence)
            .max();
        result.labels = self.labels.clone();
        Ok(result)
    }
}

fn nsfw(confidence: u8) -> SafetyLabel {
    SafetyLabel::new(SafetyCategory::Nsfw).with_confidence(confidence)
}

fn objectionable(confidence: u8) -> SafetyLabel {
    SafetyLabel::new(SafetyCategory::Objectionable).with_confidence(confidence)
}

fn signer() -> Secp256k1ModerationEventSigner {
    Secp256k1ModerationEventSigner::from_secret(TEST_SECRET).expect("test signer")
}

fn issuer() -> String {
    signer().issuer_node_id().to_string()
}

fn service(
    pool: &PgPool,
    name: &'static str,
    general_action: GeneralAction,
    labels: Vec<SafetyLabel>,
    fail: bool,
) -> SafetyScanService {
    let mut policy = SafetyPolicy::public_node_default();
    policy.require_known_csam = false;
    policy.general_action = general_action;
    let orchestrator = SafetyOrchestrator::builder(
        issuer(),
        Arc::new(SystemScanClock::new()),
        Arc::new(UuidEventIdGenerator::new()),
    )
    .provider(Arc::new(Scripted { name, labels, fail }))
    .policy(policy)
    .build()
    .expect("orchestrator");
    SafetyScanService::builder(
        Arc::new(orchestrator),
        Arc::new(PgSafetyArtifactStore::new(pool.clone())),
    )
    .signer(Arc::new(signer()))
    .build()
    .expect("service")
}

/// 旧構成（self-host VLM、policy v2 相当 = general を exclude）。
fn old_exclude_service(pool: &PgPool, labels: Vec<SafetyLabel>) -> SafetyScanService {
    service(pool, "old-vlm", GeneralAction::Exclude, labels, false)
}

/// 新構成（OpenAI、policy v3 = label 付き allow）。
fn new_service(pool: &PgPool, labels: Vec<SafetyLabel>) -> SafetyScanService {
    service(pool, "openai", GeneralAction::Label, labels, false)
}

fn post_request(post_id: &str) -> ProviderScanRequest {
    ProviderScanRequest::for_subject(SubjectKind::Post, post_id).with_text("body")
}

async fn scan(service: &SafetyScanService, post_id: &str) -> Result<()> {
    service
        .scan_or_reuse(&post_request(post_id), Some("author"), "state-hash")
        .await?;
    Ok(())
}

/// タイムラインの advisory 照会（`POST /v1/advisories/lookup` の読み口）。
async fn lookup(pool: &PgPool, post_id: &str) -> Result<Vec<ContentAdvisory>> {
    list_content_advisories_for_subjects(
        pool,
        &issuer(),
        &[post_id.to_string()],
        &[],
        &chrono::Utc::now().to_rfc3339(),
    )
    .await
}

/// 見つけるの index read（query 境界が verdict 行から導く `content_advisories`）。
async fn index_read(pool: &PgPool, post_id: &str) -> Result<Vec<ContentAdvisory>> {
    let verdict = kukuri_cn_core::get_scan_verdict(pool, SubjectKind::Post, post_id)
        .await?
        .expect("verdict row");
    upsert_index_entry(
        pool,
        &NewIndexEntry {
            scope_kind: IndexScopeKind::PublicTopic,
            scope_id: SCOPE_ID.to_string(),
            object_id: post_id.to_string(),
            author_pubkey: "author".to_string(),
            created_at: 1_700_000_000,
            source_replica_id: format!("topic::{SCOPE_ID}"),
            verdict_id: verdict.id,
            verdict_action: "allow".to_string(),
            critical: false,
        },
    )
    .await?;
    let entries = filter_surfaceable_objects(
        pool,
        IndexScopeKind::PublicTopic,
        &[(SCOPE_ID.to_string(), post_id.to_string())],
    )
    .await?;
    assert_eq!(entries.len(), 1, "allow verdict must be surfaceable");
    Ok(entries
        .into_iter()
        .next()
        .expect("surfaceable entry")
        .content_advisories)
}

fn categories(advisories: &[ContentAdvisory]) -> Vec<(SafetyCategory, String)> {
    let mut categories: Vec<_> = advisories
        .iter()
        .map(|advisory| (advisory.category, advisory.signal_id.clone()))
        .collect();
    categories.sort_by_key(|(category, _)| format!("{category:?}"));
    categories
}

async fn count(pool: &PgPool, table: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM cn_safety.{table}"))
            .fetch_one(pool)
            .await?,
    )
}

/// 著者の trust read が消費する入力（nsfw は basis に寄与 0 で並ぶ）。
async fn trust_inputs(pool: &PgPool) -> Result<kukuri_cn_trust::TrustRiskInputs> {
    list_trust_risk_inputs(
        pool,
        RiskSignalTarget::UserPubkey,
        "author",
        &chrono::Utc::now().to_rfc3339(),
    )
    .await
}

async fn post_signals(
    pool: &PgPool,
    post_id: &str,
) -> Result<Vec<kukuri_cn_core::StoredRiskSignal>> {
    list_risk_signals_for_target(pool, RiskSignalTarget::PostId, post_id).await
}

async fn with_database<F, Fut>(prefix: &str, body: F) -> Result<()>
where
    F: FnOnce(PgPool) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping #1109 integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), prefix).await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, SCOPE_ID).await?;
        body(pool.clone()).await
    }
    .await;
    pool.close().await;
    database.cleanup().await?;
    result
}

/// AC-1 / AC-2 / AC-4: 旧構成で high の nsfw signal を持つ投稿が、新構成の再 scan で
/// allow・advisory なしになった後、advisory 照会も index read も advisory を返さない。
/// signal 行は削除せず失効だけを刻み、signed event は増えない。
#[tokio::test]
async fn rescan_allow_without_labels_expires_stale_advisory_signal() -> Result<()> {
    with_database("cn_1109_rescan_expire", |pool| async move {
        let post = "post-6f0b053b";
        scan(&old_exclude_service(&pool, vec![nsfw(84)]), post).await?;
        let old = post_signals(&pool, post).await?;
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].signal.severity, Severity::High);
        assert_eq!(lookup(&pool, post).await?.len(), 1, "precondition");
        let events_before = count(&pool, "signed_moderation_events").await?;
        assert_eq!(trust_inputs(&pool).await?.relative.len(), 1);

        scan(&new_service(&pool, Vec::new()), post).await?;

        assert!(index_read(&pool, post).await?.is_empty());
        assert!(
            lookup(&pool, post).await?.is_empty(),
            "advisory lookup must follow the current allow verdict without labels"
        );
        let after = post_signals(&pool, post).await?;
        assert_eq!(after.len(), 1, "the stale signal is expired, not deleted");
        assert_eq!(after[0].id, old[0].id);
        assert!(after[0].signal.expires_at.is_some());
        assert_eq!(after[0].signal.appeal_status, Some(AppealStatus::None));
        assert_eq!(
            count(&pool, "signed_moderation_events").await?,
            events_before,
            "expiry must not issue a signed moderation event"
        );
        assert!(
            trust_inputs(&pool).await?.is_empty(),
            "the expired advisory leaves the author's trust basis"
        );

        // TR-6: 次の構成で同じ category を再検知したら、失効行を戻さず新しい活性行を作る。
        let relabel = service(
            &pool,
            "openai-v2",
            GeneralAction::Label,
            vec![nsfw(91)],
            false,
        );
        scan(&relabel, post).await?;
        let relabeled = lookup(&pool, post).await?;
        assert_eq!(relabeled.len(), 1);
        assert_ne!(relabeled[0].signal_id, old[0].id);
        assert_eq!(
            categories(&index_read(&pool, post).await?),
            categories(&relabeled)
        );
        assert!(
            get_risk_signal(&pool, &old[0].id)
                .await?
                .expect("old signal")
                .signal
                .expires_at
                .is_some()
        );
        Ok(())
    })
    .await
}

/// AC-2 / TR-2: 現在の判定に残る category の signal は集約更新し、消えた category だけ失効させる。
#[tokio::test]
async fn rescan_keeps_only_current_advisory_categories() -> Result<()> {
    with_database("cn_1109_rescan_partial", |pool| async move {
        let post = "post-partial";
        let first = service(
            &pool,
            "openai",
            GeneralAction::Label,
            vec![nsfw(80), objectionable(75)],
            false,
        );
        scan(&first, post).await?;
        assert_eq!(lookup(&pool, post).await?.len(), 2);
        assert_eq!(index_read(&pool, post).await?.len(), 2);

        let second = service(
            &pool,
            "openai-next",
            GeneralAction::Label,
            vec![objectionable(88)],
            false,
        );
        scan(&second, post).await?;

        let looked_up = lookup(&pool, post).await?;
        assert_eq!(
            categories(&looked_up),
            categories(&index_read(&pool, post).await?)
        );
        assert_eq!(looked_up.len(), 1);
        assert_eq!(looked_up[0].category, SafetyCategory::Objectionable);
        assert_eq!(looked_up[0].confidence, Some(88));
        let signals = post_signals(&pool, post).await?;
        assert_eq!(signals.len(), 2, "no new objectionable row");
        for stored in signals {
            let expired = stored.signal.expires_at.is_some();
            assert_eq!(expired, stored.signal.category == SafetyCategory::Nsfw);
        }
        Ok(())
    })
    .await
}

/// AC-3 / TR-3: operator 確定・appeal 中・認容済み・棄却済みの signal と、advisory-only 以外の
/// category は、再 scan が allow・advisory なしでも失効させない。
#[tokio::test]
async fn rescan_does_not_expire_protected_signals() -> Result<()> {
    with_database("cn_1109_rescan_protected", |pool| async move {
        let old = old_exclude_service(&pool, vec![nsfw(84)]);
        for post in [
            "post-disputed",
            "post-cleared",
            "post-rejected",
            "post-adjusted",
        ] {
            scan(&old, post).await?;
        }
        let id_of = |signals: Vec<kukuri_cn_core::StoredRiskSignal>| signals[0].id.clone();

        let disputed = id_of(post_signals(&pool, "post-disputed").await?);
        dispute_risk_signal(&pool, &disputed).await?;

        let cleared = id_of(post_signals(&pool, "post-cleared").await?);
        dispute_risk_signal(&pool, &cleared).await?;
        update_risk_signal_appeal_status(&pool, &cleared, AppealStatus::Cleared).await?;

        // 棄却 = 判定を維持した審査。appeal 通報から参照されたまま None に戻る。
        let rejected = id_of(post_signals(&pool, "post-rejected").await?);
        insert_community_node_appeal(
            &pool,
            &issuer(),
            &rejected,
            &NewCommunityNodeReport {
                subject_kind: "post".to_string(),
                subject_id: "post-rejected".to_string(),
                capability: "moderation".to_string(),
                reason: "false_positive".to_string(),
                details: None,
                reporter_contact: None,
                appeal_risk_signal_id: Some(rejected.clone()),
            },
        )
        .await?;
        update_risk_signal_appeal_status(&pool, &rejected, AppealStatus::None).await?;

        let adjusted = id_of(post_signals(&pool, "post-adjusted").await?);
        let edit = RiskSignalMetadataEdit {
            confidence: Some(60),
            ..RiskSignalMetadataEdit::default()
        };
        edit_risk_signal_detection_metadata(&pool, &adjusted, &edit, true).await?;

        // spam は advisory-only ではない（trust 相対成分）。
        let spam = service(
            &pool,
            "old-vlm",
            GeneralAction::Exclude,
            vec![SafetyLabel::new(SafetyCategory::Spam).with_confidence(90)],
            false,
        );
        scan(&spam, "post-spam").await?;

        let mut before = Vec::new();
        for post in [
            "post-disputed",
            "post-cleared",
            "post-rejected",
            "post-adjusted",
            "post-spam",
        ] {
            before.push(post_signals(&pool, post).await?);
        }

        let new = new_service(&pool, Vec::new());
        for post in [
            "post-disputed",
            "post-cleared",
            "post-rejected",
            "post-adjusted",
            "post-spam",
        ] {
            scan(&new, post).await?;
        }

        let mut after = Vec::new();
        for post in [
            "post-disputed",
            "post-cleared",
            "post-rejected",
            "post-adjusted",
            "post-spam",
        ] {
            after.push(post_signals(&pool, post).await?);
        }
        assert_eq!(after, before, "protected signals must stay unchanged");
        for signals in &after {
            assert!(
                signals
                    .iter()
                    .all(|stored| stored.signal.expires_at.is_none())
            );
        }
        Ok(())
    })
    .await
}

/// TR-4: hold / exclude / 失敗の再 scan は現在の判定として advisory を消さない。
#[tokio::test]
async fn non_indexable_rescan_keeps_advisory_signals() -> Result<()> {
    with_database("cn_1109_rescan_non_indexable", |pool| async move {
        let post = "post-labeled";
        scan(&new_service(&pool, vec![nsfw(80)]), post).await?;
        let before = post_signals(&pool, post).await?;
        assert_eq!(before.len(), 1);

        let failing = service(&pool, "openai-down", GeneralAction::Label, Vec::new(), true);
        scan(&failing, post).await?;
        let hold = service(
            &pool,
            "openai-hold",
            GeneralAction::Hold,
            vec![objectionable(90)],
            false,
        );
        scan(&hold, post).await?;

        let after = post_signals(&pool, post).await?;
        let nsfw_after: Vec<_> = after
            .iter()
            .filter(|stored| stored.signal.category == SafetyCategory::Nsfw)
            .collect();
        assert_eq!(nsfw_after.len(), 1);
        assert_eq!(nsfw_after[0].id, before[0].id);
        assert!(nsfw_after[0].signal.expires_at.is_none());
        assert!(
            lookup(&pool, post)
                .await?
                .iter()
                .any(|advisory| advisory.category == SafetyCategory::Nsfw)
        );
        Ok(())
    })
    .await
}
