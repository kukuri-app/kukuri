//! signed moderation event / risk signal の永続化と配布境界の Postgres integration テスト（#405）。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。
//! - signed moderation event の保存・取得・冪等・ロード後署名検証。
//! - risk signal の保存・取得・対象別一覧。
//! - visibility 配布境界（local 除外 / subscribed_nodes / public）と expires_at 失効。

use std::sync::Arc;

use anyhow::Result;
use kukuri_cn_core::{
    DistributionAudience, PgSafetyArtifactStore, TestDatabase, connect_postgres,
    dispute_risk_signal, get_risk_signal, get_scan_verdict, get_signed_moderation_event,
    initialize_database, list_distributable_moderation_events, list_distributable_risk_signals,
    list_risk_signals_for_target, list_trust_basis_page, persist_risk_signal,
    persist_risk_signal_deduplicated, persist_signed_moderation_event,
    reissue_corrected_risk_signal, update_risk_signal_appeal_status,
};
use kukuri_cn_safety::event::{ModerationEventBody, SignedModerationEvent, issue_signed_event};
use kukuri_cn_safety::provider::{ProviderScanRequest, SubjectKind};
use kukuri_cn_safety::{
    AppealStatus, Basis, MockSafetyProvider, ModerationAction, ModerationEventSigner, ReasonCode,
    RiskSignalTarget, SafetyCategory, SafetyLabel, SafetyPolicy, SafetyProviderCapability,
    SafetyRiskSignal, Severity, Visibility,
};
use kukuri_cn_safety_runtime::{
    SafetyOrchestrator, SafetyScanService, ScanDisposition, Secp256k1ModerationEventSigner,
    SystemScanClock, UuidEventIdGenerator, verify_signed_event,
};
use kukuri_cn_trust::TrustComponentKind;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const TEST_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

#[allow(clippy::unwrap_used)] // test fixture helper
fn signer() -> Secp256k1ModerationEventSigner {
    Secp256k1ModerationEventSigner::from_secret(TEST_SECRET).unwrap()
}

fn event_body(
    issuer: &str,
    id: &str,
    target_id: &str,
    visibility: Visibility,
) -> ModerationEventBody {
    ModerationEventBody {
        id: id.to_string(),
        issuer_node_id: issuer.to_string(),
        target_type: SubjectKind::Blob,
        target_id: target_id.to_string(),
        action: ModerationAction::Exclude,
        labels: vec![SafetyLabel::new(SafetyCategory::Csam)],
        reason_code: ReasonCode::CsamConfirmed,
        severity: Severity::Critical,
        confidence: Some(98),
        basis: Basis::KnownHashMatch,
        visibility,
        policy_version: "2026-06-public-node-v1".to_string(),
        created_at: "2026-06-29T00:00:00Z".to_string(),
    }
}

fn risk_signal(
    target_id: &str,
    visibility: Visibility,
    expires_at: Option<&str>,
) -> SafetyRiskSignal {
    SafetyRiskSignal {
        target: RiskSignalTarget::BlobCid,
        target_id: target_id.to_string(),
        category: SafetyCategory::Csam,
        severity: Severity::Critical,
        basis: Basis::KnownHashMatch,
        confidence: None,
        visibility,
        expires_at: expires_at.map(str::to_string),
        appeal_status: Some(AppealStatus::None),
    }
}

#[tokio::test]
async fn signed_moderation_event_persists_and_verifies_after_load() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_safety_events").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let signer = signer();
        let issuer = signer.issuer_node_id().to_string();

        let signed: SignedModerationEvent = issue_signed_event(
            event_body(&issuer, "evt-1", "bafy-1", Visibility::SubscribedNodes),
            &signer,
        );
        verify_signed_event(&signed).expect("freshly signed event verifies");

        let stored = persist_signed_moderation_event(&pool, &signed).await?;
        // 保存した event は body / signature ごとロードでき、署名検証が通る
        // （event_created_at が原文のまま保持され canonical digest が一致する）。
        assert_eq!(stored.event, signed);
        verify_signed_event(&stored.event).expect("loaded event still verifies");

        let fetched = get_signed_moderation_event(&pool, "evt-1")
            .await?
            .expect("event exists");
        assert_eq!(fetched.event, signed);

        // 同一 id の再 insert は冪等（最初の writer が権威。重複保存しない）。
        let again = persist_signed_moderation_event(&pool, &signed).await?;
        assert_eq!(again.event, signed);

        // body を改竄した event は保存前署名検証で拒否し、DB に残さない。
        let mut tampered = signed.clone();
        tampered.body.id = "evt-tampered".to_string();
        tampered.body.target_id = "bafy-tampered".to_string();
        assert!(
            persist_signed_moderation_event(&pool, &tampered)
                .await
                .is_err()
        );
        assert!(
            get_signed_moderation_event(&pool, "evt-tampered")
                .await?
                .is_none()
        );

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn moderation_event_distribution_excludes_local() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_safety_event_dist").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let signer = signer();
        let issuer = signer.issuer_node_id().to_string();

        for (id, target, visibility) in [
            ("evt-local", "bafy-local", Visibility::Local),
            (
                "evt-subscribed",
                "bafy-subscribed",
                Visibility::SubscribedNodes,
            ),
            ("evt-public", "bafy-public", Visibility::Public),
        ] {
            let signed = issue_signed_event(event_body(&issuer, id, target, visibility), &signer);
            persist_signed_moderation_event(&pool, &signed).await?;
        }

        // subscribed audience は subscribed_nodes + public を見るが local は見ない。
        let subscribed = list_distributable_moderation_events(
            &pool,
            DistributionAudience::SubscribedNodes,
            50,
            0,
        )
        .await?;
        let ids: Vec<&str> = subscribed
            .iter()
            .map(|e| e.event.body.id.as_str())
            .collect();
        assert!(ids.contains(&"evt-subscribed"));
        assert!(ids.contains(&"evt-public"));
        assert!(!ids.contains(&"evt-local"), "local must not distribute");

        // public audience は public のみ。
        let public =
            list_distributable_moderation_events(&pool, DistributionAudience::Public, 50, 0)
                .await?;
        let public_ids: Vec<&str> = public.iter().map(|e| e.event.body.id.as_str()).collect();
        assert_eq!(public_ids, vec!["evt-public"]);

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn risk_signal_persists_and_distribution_respects_visibility_and_expiry() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_safety_risk").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let issuer = "node-issuer";

        // local: 配布しない。
        persist_risk_signal(
            &pool,
            issuer,
            &risk_signal("bafy-local", Visibility::Local, None),
        )
        .await?;
        // subscribed: 配布する（無期限）。
        let subscribed_stored = persist_risk_signal(
            &pool,
            issuer,
            &risk_signal("bafy-sub", Visibility::SubscribedNodes, None),
        )
        .await?;
        // public だが失効済み: 配布しない。
        persist_risk_signal(
            &pool,
            issuer,
            &risk_signal(
                "bafy-expired",
                Visibility::Public,
                Some("2020-01-01T00:00:00Z"),
            ),
        )
        .await?;
        // public かつ未失効: 配布する。
        persist_risk_signal(
            &pool,
            issuer,
            &risk_signal(
                "bafy-future",
                Visibility::Public,
                Some("2999-01-01T00:00:00Z"),
            ),
        )
        .await?;

        // 対象別一覧（visibility 非依存・node-local 参照）。
        let by_target =
            list_risk_signals_for_target(&pool, RiskSignalTarget::BlobCid, "bafy-sub").await?;
        assert_eq!(by_target.len(), 1);
        assert_eq!(by_target[0].id, subscribed_stored.id);
        assert_eq!(by_target[0].issuer_node_id, issuer);

        // 配布境界（now=2026 時点）。
        let now = "2026-06-29T00:00:00Z";
        let subscribed = list_distributable_risk_signals(
            &pool,
            DistributionAudience::SubscribedNodes,
            now,
            50,
            0,
        )
        .await?;
        let targets: Vec<&str> = subscribed
            .iter()
            .map(|s| s.signal.target_id.as_str())
            .collect();
        assert!(targets.contains(&"bafy-sub"));
        assert!(targets.contains(&"bafy-future"));
        assert!(
            !targets.contains(&"bafy-local"),
            "local must not distribute"
        );
        assert!(
            !targets.contains(&"bafy-expired"),
            "expired must not distribute"
        );

        // public audience は public のみ（未失効）。
        let public =
            list_distributable_risk_signals(&pool, DistributionAudience::Public, now, 50, 0)
                .await?;
        let public_targets: Vec<&str> =
            public.iter().map(|s| s.signal.target_id.as_str()).collect();
        assert_eq!(public_targets, vec!["bafy-future"]);

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn content_scan_is_attributed_to_author_and_appeal_updates_trust_input() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_safety_attribution").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let signer = signer();
        let issuer = signer.issuer_node_id().to_string();
        let provider = MockSafetyProvider::known_csam("mock-known-csam")
            .with_known_hash_match("post-attributed");
        let orchestrator = Arc::new(
            SafetyOrchestrator::builder(
                &issuer,
                Arc::new(SystemScanClock),
                Arc::new(UuidEventIdGenerator),
            )
            .provider(Arc::new(provider))
            .build()?,
        );
        let store = Arc::new(PgSafetyArtifactStore::new(pool.clone()));
        let service = SafetyScanService::builder(orchestrator, store)
            .signer(Arc::new(signer))
            .build()?;

        let outcome = service
            .scan_and_record_for_author(
                &ProviderScanRequest::for_subject(SubjectKind::Post, "post-attributed"),
                "author-pubkey",
            )
            .await?;
        let signal_id = outcome
            .persisted_signal_id
            .expect("known match persists a risk signal");

        let attributed = list_trust_basis_page(&pool, "author-pubkey", None)
            .await?
            .inputs;
        assert_eq!(attributed.len(), 1);
        assert_eq!(attributed[0].component, TrustComponentKind::Absolute);
        assert_eq!(attributed[0].signal_id, signal_id);

        dispute_risk_signal(&pool, &signal_id).await?;
        update_risk_signal_appeal_status(&pool, &signal_id, AppealStatus::Cleared).await?;
        let cleared = list_trust_basis_page(&pool, "author-pubkey", None)
            .await?
            .inputs;
        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].appeal_status, AppealStatus::Cleared);

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn empty_target_id_is_rejected() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_safety_empty").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let signer = signer();
        let issuer = signer.issuer_node_id().to_string();

        // 空白 target_id の moderation event は保存しない。
        let signed = issue_signed_event(
            event_body(&issuer, "evt-empty", "   ", Visibility::Public),
            &signer,
        );
        assert!(
            persist_signed_moderation_event(&pool, &signed)
                .await
                .is_err()
        );

        // 空白 target_id の risk signal も保存しない。
        assert!(
            persist_risk_signal(&pool, &issuer, &risk_signal("  ", Visibility::Public, None))
                .await
                .is_err()
        );

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

// --- runtime 結線（#406）: SafetyScanService + Postgres store の integration ---

#[tokio::test]
async fn scan_and_record_persists_artifacts_via_postgres_store() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_safety_runtime").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let signer = signer();
        let issuer = signer.issuer_node_id().to_string();

        // 本番構成と同型: SystemScanClock + UuidEventIdGenerator + Postgres store。
        // provider のみ mock（known hash match を決定論的に発火させる）。
        let provider =
            MockSafetyProvider::known_csam("mock-known-csam").with_known_hash_match("post-406");
        let orchestrator = SafetyOrchestrator::builder(
            &issuer,
            Arc::new(SystemScanClock::new()),
            Arc::new(UuidEventIdGenerator::new()),
        )
        .provider(Arc::new(provider))
        .build()?;
        let service = SafetyScanService::builder(
            Arc::new(orchestrator),
            Arc::new(PgSafetyArtifactStore::new(pool.clone())),
        )
        .signer(Arc::new(signer))
        .build()?;

        let outcome = service
            .scan_and_record(&ProviderScanRequest::for_subject(
                SubjectKind::Post,
                "post-406",
            ))
            .await?;
        assert!(!outcome.report.verdict.is_indexable());

        // signed moderation event が cn_safety schema に入り、ロード後も署名検証が通る。
        let event = outcome.signed_event.expect("signed moderation event");
        let stored_event = get_signed_moderation_event(&pool, &event.body.id)
            .await?
            .expect("event persisted");
        assert_eq!(stored_event.event, event);
        verify_signed_event(&stored_event.event).expect("loaded event verifies");

        // risk signal も採番 id で入り、対象別一覧から読み戻せる。
        let signal_id = outcome.persisted_signal_id.expect("risk signal id");
        let stored_signal = get_risk_signal(&pool, &signal_id)
            .await?
            .expect("risk signal persisted");
        assert_eq!(stored_signal.issuer_node_id, issuer);
        assert_eq!(stored_signal.signal.category, SafetyCategory::Csam);
        let listed =
            list_risk_signals_for_target(&pool, RiskSignalTarget::PostId, "post-406").await?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, signal_id);

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn trust_basis_partitions_absolute_and_relative_from_postgres() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_trust_inputs").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let issuer = signer().issuer_node_id().to_string();

        // 同一 user pubkey に critical（csam）と一般（nsfw）の signal を保存する。
        let mut csam = risk_signal("pubkey-406", Visibility::Local, None);
        csam.target = RiskSignalTarget::UserPubkey;
        let mut nsfw = risk_signal("pubkey-406", Visibility::Local, None);
        nsfw.target = RiskSignalTarget::UserPubkey;
        nsfw.category = SafetyCategory::Nsfw;
        nsfw.severity = Severity::Medium;
        persist_risk_signal(&pool, &issuer, &csam).await?;
        persist_risk_signal(&pool, &issuer, &nsfw).await?;

        // basis は category で絶対 / 相対成分へ振り分け、絶対成分を先に並べる（ADR 0026 §2.7 / §10）。
        let inputs = list_trust_basis_page(&pool, "pubkey-406", None)
            .await?
            .inputs;
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].component, TrustComponentKind::Absolute);
        assert_eq!(inputs[0].category, SafetyCategory::Csam);
        assert_eq!(inputs[0].issuer_node_id, issuer);
        assert_eq!(inputs[1].component, TrustComponentKind::Relative);
        assert_eq!(inputs[1].category, SafetyCategory::Nsfw);

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

// --- #1050: risk signal の集約（同一鍵は 1 行）と保存済み verdict の再利用 ---

fn nsfw_signal(target_id: &str, confidence: u8) -> SafetyRiskSignal {
    SafetyRiskSignal {
        target: RiskSignalTarget::PostId,
        target_id: target_id.to_string(),
        category: SafetyCategory::Nsfw,
        severity: Severity::High,
        basis: Basis::ClassifierScore,
        confidence: Some(confidence),
        visibility: Visibility::Local,
        expires_at: None,
        appeal_status: Some(AppealStatus::None),
    }
}

async fn count_signals(pool: &sqlx::PgPool, target_id: &str) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM cn_safety.risk_signals WHERE target_id = $1",
    )
    .bind(target_id)
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn rescan_with_same_key_updates_signal_instead_of_inserting() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_signal_dedupe").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let issuer = signer().issuer_node_id().to_string();

        let first = persist_risk_signal_deduplicated(
            &pool,
            &issuer,
            &nsfw_signal("post-dup", 84),
            Some("author-a"),
        )
        .await?;
        assert!(first.newly_created);

        // 申し立て中でも再 scan は行を増やさず、appeal_status を触らない。
        dispute_risk_signal(&pool, &first.stored.id).await?;
        let second = persist_risk_signal_deduplicated(
            &pool,
            &issuer,
            &nsfw_signal("post-dup", 91),
            Some("author-a"),
        )
        .await?;
        assert!(!second.newly_created);
        assert_eq!(second.stored.id, first.stored.id);
        assert_eq!(second.stored.persisted_at, first.stored.persisted_at);
        assert_eq!(second.stored.signal.confidence, Some(91));
        assert_eq!(
            second.stored.signal.appeal_status,
            Some(AppealStatus::Disputed)
        );
        assert_eq!(count_signals(&pool, "post-dup").await?, 1);

        // basis も 1 件（同一投稿の寄与は signal 1 件分を超えない）。
        let inputs = list_trust_basis_page(&pool, "author-a", None).await?.inputs;
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].component, TrustComponentKind::Relative);

        // 別 category（別鍵）は別行になる。
        let mut spam = nsfw_signal("post-dup", 70);
        spam.category = SafetyCategory::Spam;
        let third =
            persist_risk_signal_deduplicated(&pool, &issuer, &spam, Some("author-a")).await?;
        assert!(third.newly_created);
        assert_eq!(count_signals(&pool, "post-dup").await?, 2);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn cleared_signal_with_same_key_is_not_resurrected() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_signal_cleared").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let issuer = signer().issuer_node_id().to_string();

        let first = persist_risk_signal(&pool, &issuer, &nsfw_signal("post-cleared", 84)).await?;
        dispute_risk_signal(&pool, &first.id).await?;
        update_risk_signal_appeal_status(&pool, &first.id, AppealStatus::Cleared).await?;

        let again = persist_risk_signal_deduplicated(
            &pool,
            &issuer,
            &nsfw_signal("post-cleared", 95),
            None,
        )
        .await?;
        assert!(
            !again.newly_created,
            "cleared decision must not be overridden by a rescan"
        );
        assert_eq!(again.stored.id, first.id);
        assert_eq!(
            again.stored.signal.appeal_status,
            Some(AppealStatus::Cleared)
        );
        assert_eq!(
            again.stored.signal.confidence,
            Some(84),
            "cleared row is left untouched"
        );
        assert_eq!(count_signals(&pool, "post-cleared").await?, 1);

        // cn-cli の再発行（旧行を失効 → 新行 insert）は引き続き通り、以後の再 scan は新行へ集約する。
        let reissued = reissue_corrected_risk_signal(
            &pool,
            &first.id,
            &kukuri_cn_core::RiskSignalCorrection {
                category: None,
                severity: Some(Severity::Low),
                confidence: Some(10),
                visibility: None,
            },
            "2026-09-15T12:00:00Z",
            true,
        )
        .await?;
        assert_ne!(reissued.id, first.id);
        assert_eq!(count_signals(&pool, "post-cleared").await?, 2);
        let merged = persist_risk_signal_deduplicated(
            &pool,
            &issuer,
            &nsfw_signal("post-cleared", 60),
            None,
        )
        .await?;
        assert!(!merged.newly_created);
        assert_eq!(merged.stored.id, reissued.id);
        assert_eq!(count_signals(&pool, "post-cleared").await?, 2);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn postgres_store_reuses_verdict_and_does_not_duplicate_artifacts() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_verdict_reuse").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let signer = signer();
        let issuer = signer.issuer_node_id().to_string();
        let provider = MockSafetyProvider::with_capabilities(
            "mock-general",
            vec![SafetyProviderCapability::GeneralMediaModeration],
        )
        .with_score(
            "post-reuse",
            SafetyProviderCapability::GeneralMediaModeration,
            SafetyCategory::Nsfw,
            84,
        );
        let policy = SafetyPolicy {
            require_known_csam: false,
            ..SafetyPolicy::public_node_default()
        };
        let orchestrator = Arc::new(
            SafetyOrchestrator::builder(
                &issuer,
                Arc::new(SystemScanClock),
                Arc::new(UuidEventIdGenerator),
            )
            .policy(policy)
            .provider(Arc::new(provider))
            .build()?,
        );
        let store = Arc::new(PgSafetyArtifactStore::new(pool.clone()));
        let service = SafetyScanService::builder(orchestrator, store)
            .signer(Arc::new(signer))
            .build()?;
        let request = ProviderScanRequest::for_subject(SubjectKind::Post, "post-reuse")
            .with_text("sexy test");

        let first = service
            .scan_or_reuse(&request, Some("author-a"), "state-hash-1")
            .await?;
        assert_eq!(first.disposition, ScanDisposition::Fresh);
        assert!(first.signed_event.is_some());
        let stored = get_scan_verdict(&pool, SubjectKind::Post, "post-reuse")
            .await?
            .expect("verdict persisted");
        assert_eq!(stored.source_fingerprint.as_deref(), Some("state-hash-1"));
        assert_eq!(
            stored.scan_config_fingerprint.as_deref(),
            Some(service.scan_config_fingerprint())
        );

        let second = service
            .scan_or_reuse(&request, Some("author-b"), "state-hash-1")
            .await?;
        assert_eq!(second.disposition, ScanDisposition::Reused);
        assert_eq!(second.verdict_id, first.verdict_id);
        assert!(second.signed_event.is_none());

        let events = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM cn_safety.signed_moderation_events WHERE target_id = $1",
        )
        .bind("post-reuse")
        .fetch_one(&pool)
        .await?;
        assert_eq!(events, 1);
        assert_eq!(count_signals(&pool, "post-reuse").await?, 1);
        // 再利用でも 2 人目の著者は関連付けられる（共有 subject の trust 入力を落とさない）。
        for author in ["author-a", "author-b"] {
            let inputs = list_trust_basis_page(&pool, author, None).await?.inputs;
            assert_eq!(inputs.len(), 1, "{author}");
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}
