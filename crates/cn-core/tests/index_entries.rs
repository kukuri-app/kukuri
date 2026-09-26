//! #404 fail-closed indexing の DB 制約と verdict state の Postgres integration テスト。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。
//! - `cn_safety.scan_verdicts`: 対象ごとの最新 verdict の upsert（id 据え置き）と取得。
//! - `cn_index.index_entries`: fail-closed 不変条件の **DB 制約**による保証
//!   （`index_only_allow_verdict_content` / verdict 無し entry の拒否 / critical 拒否）。
//! - `filter_surfaceable_objects`: query 境界の fail-closed 突合
//!   （`search_discovery_recommendation_excludes_non_allow` の真実源側）。

use anyhow::Result;
use kukuri_cn_core::{
    IndexEntryStore, IndexScopeKind, NewIndexEntry, PgIndexEntryStore, PgSafetyArtifactStore,
    SurfaceableEntry, TestDatabase, connect_postgres, filter_surfaceable_objects, get_index_entry,
    get_scan_verdict, initialize_database, remove_index_entry, remove_index_scope_page,
    update_scan_verdict_advisories, upsert_index_entry, upsert_scan_verdict,
};
use kukuri_cn_safety::provider::{ProviderScanRequest, SubjectKind};
use kukuri_cn_safety::{
    AdvisorySubjectKind, Basis, ContentAdvisory, MockSafetyProvider, ModerationEventSigner,
    ReasonCode, SafetyAction, SafetyCategory, SafetyLabel, SafetyVerdict,
};
use kukuri_cn_safety_runtime::VerdictPersistMeta;
use kukuri_cn_safety_runtime::{
    SafetyOrchestrator, SafetyScanService, Secp256k1ModerationEventSigner, SystemScanClock,
    UuidEventIdGenerator,
};
use std::sync::Arc;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const TEST_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::test]
async fn indexed_scope_seek_skips_other_posts_in_the_same_scope() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_indexed_scope_seek").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    for (scope, object) in [
        ("a", "post-a1"),
        ("a", "post-a2"),
        ("b", "post-b1"),
        ("c", "post-c1"),
    ] {
        let allow = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            object,
            &verdict(SafetyAction::Allow, false, ReasonCode::NoKnownMatch),
            &VerdictPersistMeta::default(),
        )
        .await?;
        upsert_index_entry(&pool, &entry(scope, object, &allow.id)).await?;
    }
    let store = PgIndexEntryStore::new(pool.clone());
    assert_eq!(
        store.next_scope_after("", "").await?,
        Some((IndexScopeKind::PublicTopic, "a".into()))
    );
    assert_eq!(
        store.next_scope_after("public_topic", "a").await?,
        Some((IndexScopeKind::PublicTopic, "b".into()))
    );
    assert_eq!(
        store.next_scope_after("public_topic", "b").await?,
        Some((IndexScopeKind::PublicTopic, "c".into()))
    );
    assert_eq!(store.next_scope_after("public_topic", "c").await?, None);
    pool.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn verified_withdrawal_prevents_a_later_stale_provider_upsert() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_known_withdrawal").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    let allow = upsert_scan_verdict(
        &pool,
        SubjectKind::Post,
        "post-1",
        &verdict(SafetyAction::Allow, false, ReasonCode::NoKnownMatch),
        &VerdictPersistMeta::default(),
    )
    .await?;
    let candidate = entry("rust", "post-1", allow.id.as_str());
    upsert_index_entry(&pool, &candidate).await?;
    let store = PgIndexEntryStore::new(pool.clone());
    store
        .record_verified_withdrawal(IndexScopeKind::PublicTopic, "rust", "post-1", 1_700_000_000)
        .await?;
    assert!(
        store
            .is_known_withdrawn(IndexScopeKind::PublicTopic, "rust", "post-1", 1_700_000_000)
            .await?
    );
    assert!(
        get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-1")
            .await?
            .is_none()
    );
    assert!(
        upsert_index_entry(&pool, &candidate).await.is_err(),
        "DB trigger must reject stale reinsert"
    );
    pool.close().await;
    database.cleanup().await?;
    Ok(())
}

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn verdict(action: SafetyAction, critical: bool, reason_code: ReasonCode) -> SafetyVerdict {
    SafetyVerdict {
        action,
        labels: Vec::new(),
        advisory_labels: Vec::new(),
        critical,
        reason_code,
        confidence: None,
        provider: Some("mock-known-csam".to_string()),
        provider_capability: None,
        policy_version: "policy-v1-test".to_string(),
        scanned_at: "2026-07-02T09:00:00Z".to_string(),
    }
}

fn entry(scope_id: &str, object_id: &str, verdict_id: &str) -> NewIndexEntry {
    NewIndexEntry {
        scope_kind: IndexScopeKind::PublicTopic,
        scope_id: scope_id.to_string(),
        object_id: object_id.to_string(),
        author_pubkey: "author-pubkey".to_string(),
        created_at: 1_700_000_000,
        source_replica_id: format!("topic::{scope_id}"),
        verdict_id: verdict_id.to_string(),
        verdict_action: "allow".to_string(),
        critical: false,
    }
}

/// scan_verdicts の upsert は対象ごとに 1 行で、id は初回採番のまま最新値へ更新される。
#[tokio::test]
async fn scan_verdict_upsert_keeps_id_and_tracks_latest() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index entries test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_scan_verdicts").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;

        let first = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-1",
            &verdict(SafetyAction::Allow, false, ReasonCode::NoKnownMatch),
            &VerdictPersistMeta::default(),
        )
        .await?;
        assert_eq!(first.action, SafetyAction::Allow);
        assert!(first.is_indexable());

        // 同一対象の再 scan は同じ行（同じ id）を最新 verdict に更新する。
        let second = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-1",
            &verdict(SafetyAction::Exclude, true, ReasonCode::CsamConfirmed),
            &VerdictPersistMeta::default(),
        )
        .await?;
        assert_eq!(second.id, first.id);
        assert_eq!(second.action, SafetyAction::Exclude);
        assert!(second.critical);
        assert!(!second.is_indexable());

        let latest = get_scan_verdict(&pool, SubjectKind::Post, "post-1")
            .await?
            .expect("verdict row exists");
        assert_eq!(latest, second);

        // 別対象は別行（unscanned な対象には行が無い）。
        assert!(
            get_scan_verdict(&pool, SubjectKind::Post, "post-unscanned")
                .await?
                .is_none()
        );
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// fail-closed 不変条件の DB 制約: 非 allow / critical / verdict 無しの index entry は
/// INSERT 自体が拒否される（`index_only_allow_verdict_content` の DB 層）。
#[tokio::test]
async fn index_only_allow_verdict_content_enforced_by_db_constraints() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index entries test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_entries_check").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;

        let allow = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-allow",
            &verdict(SafetyAction::Allow, false, ReasonCode::NoKnownMatch),
            &VerdictPersistMeta::default(),
        )
        .await?;

        // allow + 非 critical + verdict FK が揃った entry のみ insert できる。
        upsert_index_entry(&pool, &entry("rust", "post-allow", allow.id.as_str())).await?;

        // verdict_action が allow 以外は CHECK 制約違反（hold / quarantine / exclude と
        // scan_failed / provider_unavailable / unscanned はいずれも action が allow にならない）。
        for action in ["hold", "quarantine", "exclude"] {
            let mut non_allow = entry("rust", "post-non-allow", allow.id.as_str());
            non_allow.verdict_action = action.to_string();
            let error = upsert_index_entry(&pool, &non_allow).await.unwrap_err();
            assert!(
                error.to_string().contains("check"),
                "expected CHECK violation for `{action}`, got: {error}"
            );
        }

        // critical = true は CHECK 制約違反（critical は search / discovery / recommendation の
        // どこにも入らない）。
        let mut critical_entry = entry("rust", "post-critical", allow.id.as_str());
        critical_entry.critical = true;
        let error = upsert_index_entry(&pool, &critical_entry)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("check"),
            "expected CHECK violation for critical entry, got: {error}"
        );

        // 存在しない verdict を指す entry は FK 制約違反（verdict 無しの index entry を作らない）。
        let ghost = entry("rust", "post-ghost", "verdict-does-not-exist");
        let error = upsert_index_entry(&pool, &ghost).await.unwrap_err();
        assert!(
            error.to_string().contains("foreign key"),
            "expected FK violation for missing verdict, got: {error}"
        );

        // 制約違反の entry は残っていない。
        assert!(
            get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-non-allow")
                .await?
                .is_none()
        );
        assert!(
            get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-critical")
                .await?
                .is_none()
        );
        assert!(
            get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-ghost")
                .await?
                .is_none()
        );
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// index entry の冪等 upsert と de-index（object 単位 / scope 単位）。
#[tokio::test]
async fn index_entry_upsert_is_idempotent_and_deindexable() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index entries test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_entries_crud").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;

        let allow_1 = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-1",
            &verdict(SafetyAction::Allow, false, ReasonCode::NoKnownMatch),
            &VerdictPersistMeta::default(),
        )
        .await?;
        let allow_2 = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-2",
            &verdict(SafetyAction::Allow, false, ReasonCode::NoKnownMatch),
            &VerdictPersistMeta::default(),
        )
        .await?;

        upsert_index_entry(&pool, &entry("rust", "post-1", allow_1.id.as_str())).await?;
        // 同一 (scope, object) への再 upsert は行を増やさず更新する。
        let mut updated = entry("rust", "post-1", allow_1.id.as_str());
        updated.author_pubkey = "author-updated".to_string();
        upsert_index_entry(&pool, &updated).await?;
        let stored = get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-1")
            .await?
            .expect("entry exists");
        assert_eq!(stored.author_pubkey, "author-updated");

        upsert_index_entry(&pool, &entry("rust", "post-2", allow_2.id.as_str())).await?;

        // object 単位の de-index。
        remove_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-1").await?;
        assert!(
            get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-1")
                .await?
                .is_none()
        );

        // scope 単位の de-index（supported 除去 / channel secret 失効）は 1 回あたり上限つき。
        assert_eq!(
            remove_index_scope_page(&pool, IndexScopeKind::PublicTopic, "rust", 128).await?,
            1
        );
        assert!(
            get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-2")
                .await?
                .is_none()
        );
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// query 境界の fail-closed 突合: 真実源に無い hit と、verdict が後から非 allow / critical に
/// 変わった hit は surfacing されない（de-index の遅延に依存しない）。
#[tokio::test]
async fn filter_surfaceable_objects_excludes_non_allow_and_unknown() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index entries test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_entries_gate").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;

        let allow_kept = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-kept",
            &verdict(SafetyAction::Allow, false, ReasonCode::NoKnownMatch),
            &VerdictPersistMeta::default(),
        )
        .await?;
        let allow_flipped = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-flipped",
            &verdict(SafetyAction::Allow, false, ReasonCode::NoKnownMatch),
            &VerdictPersistMeta::default(),
        )
        .await?;
        upsert_index_entry(&pool, &entry("rust", "post-kept", allow_kept.id.as_str())).await?;
        upsert_index_entry(
            &pool,
            &entry("rust", "post-flipped", allow_flipped.id.as_str()),
        )
        .await?;

        // index 後に verdict が exclude / critical へ変わる（同一行の更新なので entry の FK は
        // 常に最新 verdict を指す）。de-index はまだ走っていない想定。
        upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-flipped",
            &verdict(SafetyAction::Exclude, true, ReasonCode::CsamConfirmed),
            &VerdictPersistMeta::default(),
        )
        .await?;

        let candidates = vec![
            ("rust".to_string(), "post-kept".to_string()),
            ("rust".to_string(), "post-flipped".to_string()),
            // 投影残留 / ghost: 真実源に entry が無い hit。
            ("rust".to_string(), "post-projection-only".to_string()),
        ];
        let surfaceable =
            filter_surfaceable_objects(&pool, IndexScopeKind::PublicTopic, &candidates).await?;
        assert_eq!(
            surfaceable,
            vec![SurfaceableEntry {
                source_replica_id: "topic::rust".into(),
                author_pubkey: "author-pubkey".into(),
                created_at: 1_700_000_000,
                scope_id: "rust".to_string(),
                object_id: "post-kept".to_string(),
                content_advisories: Vec::new(),
            }]
        );

        // 空の候補は空を返す（クエリを発行しない）。
        assert!(
            filter_surfaceable_objects(&pool, IndexScopeKind::PublicTopic, &[])
                .await?
                .is_empty()
        );
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// SafetyScanService::scan_and_record（Postgres store）が verdict state を upsert し、
/// verdict_id を返す（#404 の T1 受け入れ条件。allow でも記録される）。
#[tokio::test]
async fn scan_and_record_upserts_verdict_state_via_postgres_store() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index entries test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_scan_verdict_service").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
        let signer = Secp256k1ModerationEventSigner::from_secret(TEST_SECRET)?;
        let issuer = signer.issuer_node_id().to_string();

        let provider =
            MockSafetyProvider::known_csam("mock-known-csam").with_known_hash_match("post-hit");
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

        // allow（既知一致なし）でも verdict state は記録される。
        let outcome = service
            .scan_and_record(&ProviderScanRequest::for_subject(
                SubjectKind::Post,
                "post-clean",
            ))
            .await?;
        assert!(outcome.report.verdict.is_indexable());
        let verdict_id = outcome.verdict_id.expect("verdict id for clean post");
        let stored = get_scan_verdict(&pool, SubjectKind::Post, "post-clean")
            .await?
            .expect("verdict row exists");
        assert_eq!(stored.id, verdict_id);
        assert_eq!(stored.action, SafetyAction::Allow);

        // 既知一致（exclude / critical）も同様に記録される。
        let outcome = service
            .scan_and_record(&ProviderScanRequest::for_subject(
                SubjectKind::Post,
                "post-hit",
            ))
            .await?;
        assert!(!outcome.report.verdict.is_indexable());
        let stored = get_scan_verdict(&pool, SubjectKind::Post, "post-hit")
            .await?
            .expect("verdict row exists");
        assert_eq!(stored.action, SafetyAction::Exclude);
        assert_eq!(stored.reason_code, ReasonCode::CsamConfirmed);
        assert!(stored.critical);
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// #1050: verdict 行は再利用鍵（fingerprint）と descriptive タグを往復できる。
#[tokio::test]
async fn scan_verdict_round_trips_fingerprints_and_derived_tags() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index entries test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_verdict_fingerprints").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
        let meta = VerdictPersistMeta {
            source_fingerprint: Some("state-hash-1".to_string()),
            scan_config_fingerprint: Some("config-1".to_string()),
            derived_tags: vec!["beach".to_string(), "sunset".to_string()],
            advisories: Vec::new(),
        };
        let stored = upsert_scan_verdict(
            &pool,
            SubjectKind::Blob,
            "blob-tags",
            &verdict(SafetyAction::Allow, false, ReasonCode::Clean),
            &meta,
        )
        .await?;
        assert_eq!(stored.source_fingerprint.as_deref(), Some("state-hash-1"));
        assert_eq!(stored.scan_config_fingerprint.as_deref(), Some("config-1"));
        assert_eq!(stored.derived_tags, vec!["beach", "sunset"]);

        let loaded = get_scan_verdict(&pool, SubjectKind::Blob, "blob-tags")
            .await?
            .expect("stored verdict");
        assert_eq!(loaded, stored);
        let record = loaded.to_record();
        assert_eq!(record.id, stored.id);
        assert_eq!(record.derived_tags, vec!["beach", "sunset"]);
        assert!(record.verdict.is_indexable());

        // 再 upsert は fingerprint / タグも最新値へ置き換える（id は据え置き）。
        let refreshed = upsert_scan_verdict(
            &pool,
            SubjectKind::Blob,
            "blob-tags",
            &verdict(SafetyAction::Allow, false, ReasonCode::Clean),
            &VerdictPersistMeta::default(),
        )
        .await?;
        assert_eq!(refreshed.id, stored.id);
        assert!(refreshed.source_fingerprint.is_none());
        assert!(refreshed.derived_tags.is_empty());
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// ADR 0025 §7.1 contract: `index_entry_advisories_derive_from_latest_verdict`。
///
/// index entry は advisory を持たず、既存の verdict FK を通じて最新 verdict 行の
/// `advisory_labels` を返す。post 行の和集合（blob 分を含む）もそのまま同梱され、verdict が
/// 非 allow へ変われば entry ごと落ちる。
#[tokio::test]
async fn index_entry_advisories_derive_from_latest_verdict() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index entries test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_entries_advisory").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        kukuri_cn_core::add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;

        let advisory =
            |subject_kind: AdvisorySubjectKind, subject_id: &str, category| ContentAdvisory {
                issuer_node_id: "issuer-node".to_string(),
                subject_kind,
                subject_id: subject_id.to_string(),
                category,
                label: match category {
                    SafetyCategory::Nsfw => "adult".to_string(),
                    _ => "sensitive".to_string(),
                },
                confidence: Some(84),
                signal_id: format!("signal-{subject_id}"),
                basis: Basis::ClassifierScore,
            };
        let text_advisory = advisory(
            AdvisorySubjectKind::PostId,
            "post-adv",
            SafetyCategory::Nsfw,
        );

        // ラベル付き allow の verdict（本文 text の advisory 付き）。
        let mut labeled = verdict(SafetyAction::Allow, false, ReasonCode::GeneralModeration);
        labeled.advisory_labels = vec![SafetyLabel::new(SafetyCategory::Nsfw).with_confidence(84)];
        let stored = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-adv",
            &labeled,
            &VerdictPersistMeta {
                advisories: vec![text_advisory.clone()],
                ..VerdictPersistMeta::default()
            },
        )
        .await?;
        assert_eq!(stored.advisory_labels, vec![text_advisory.clone()]);
        // 再利用入力へも advisory が復元され、ラベル付き allow として扱える。
        let record = stored.to_record();
        assert!(record.verdict.is_labeled_allow());
        assert_eq!(record.verdict.advisory_labels, labeled.advisory_labels);
        assert_eq!(record.advisories, vec![text_advisory.clone()]);

        let plain = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-plain",
            &verdict(SafetyAction::Allow, false, ReasonCode::NoKnownMatch),
            &VerdictPersistMeta::default(),
        )
        .await?;
        upsert_index_entry(&pool, &entry("rust", "post-adv", stored.id.as_str())).await?;
        upsert_index_entry(&pool, &entry("rust", "post-plain", plain.id.as_str())).await?;

        let candidates = vec![
            ("rust".to_string(), "post-adv".to_string()),
            ("rust".to_string(), "post-plain".to_string()),
        ];
        let surfaceable =
            filter_surfaceable_objects(&pool, IndexScopeKind::PublicTopic, &candidates).await?;
        assert_eq!(
            surfaceable,
            vec![
                SurfaceableEntry {
                    source_replica_id: "topic::rust".into(),
                    author_pubkey: "author-pubkey".into(),
                    created_at: 1_700_000_000,
                    scope_id: "rust".to_string(),
                    object_id: "post-adv".to_string(),
                    content_advisories: vec![text_advisory.clone()],
                },
                SurfaceableEntry {
                    source_replica_id: "topic::rust".into(),
                    author_pubkey: "author-pubkey".into(),
                    created_at: 1_700_000_000,
                    scope_id: "rust".to_string(),
                    object_id: "post-plain".to_string(),
                    content_advisories: Vec::new(),
                },
            ]
        );

        // indexer が参照 blob の advisory との和集合を post 行へ確定させると、entry は
        // 最新 verdict 行からその和集合を返す（index entry 自体は変更しない）。
        let blob_advisory = advisory(
            AdvisorySubjectKind::BlobCid,
            "blob-1",
            SafetyCategory::Objectionable,
        );
        let union = vec![text_advisory.clone(), blob_advisory.clone()];
        update_scan_verdict_advisories(&pool, SubjectKind::Post, "post-adv", &union).await?;
        let refreshed = get_scan_verdict(&pool, SubjectKind::Post, "post-adv")
            .await?
            .expect("verdict");
        assert_eq!(refreshed.advisory_labels, union);
        assert_eq!(
            refreshed.updated_at, stored.updated_at,
            "値の差し替えだけで updated_at は動かない"
        );
        // 自 subject 分だけが再利用入力の advisory_labels へ戻る。
        assert_eq!(refreshed.own_advisories(), vec![text_advisory.clone()]);
        assert_eq!(
            refreshed.to_record().verdict.advisory_labels,
            labeled.advisory_labels
        );
        let surfaceable =
            filter_surfaceable_objects(&pool, IndexScopeKind::PublicTopic, &candidates[..1])
                .await?;
        assert_eq!(surfaceable[0].content_advisories, union);
        assert_eq!(
            get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-adv")
                .await?
                .expect("entry")
                .verdict_id,
            stored.id
        );

        // 同じ値の再書き込みは no-op。
        update_scan_verdict_advisories(&pool, SubjectKind::Post, "post-adv", &union).await?;
        assert_eq!(
            get_scan_verdict(&pool, SubjectKind::Post, "post-adv")
                .await?
                .expect("verdict"),
            refreshed
        );

        // verdict が exclude へ変われば advisory の有無に関係なく entry ごと落ちる（INVAR-1）。
        upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-adv",
            &verdict(SafetyAction::Exclude, false, ReasonCode::GeneralModeration),
            &VerdictPersistMeta::default(),
        )
        .await?;
        let surfaceable =
            filter_surfaceable_objects(&pool, IndexScopeKind::PublicTopic, &candidates).await?;
        assert_eq!(surfaceable.len(), 1);
        assert_eq!(surfaceable[0].object_id, "post-plain");
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}
