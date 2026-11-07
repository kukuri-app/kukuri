use anyhow::Result;
use async_trait::async_trait;
use kukuri_cn_core::{
    PgContentScanStore, PgModerationBudget, PgSafetyArtifactStore, TestDatabase, connect_postgres,
    initialize_database, list_content_advisories_for_subjects,
};
use kukuri_cn_safety::{
    ProviderScanRequest, ProviderScanResult, SafetyCategory, SafetyLabel, SafetyPolicy,
    SafetyProvider, SafetyProviderCapability, ScanError, ScanOutcome, SubjectKind,
};
use kukuri_cn_safety_openai::{BudgetConfig, ModerationBudget};
use kukuri_cn_safety_runtime::{
    SafetyOrchestrator, SafetyScanService, SystemScanClock, UuidEventIdGenerator,
    content_cache::{CONTENT_SCAN_LEASE, ContentScanStore},
};
use sqlx::PgPool;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    )
}
const ISSUER: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

#[derive(Default)]
struct Counter(AtomicUsize);
#[async_trait]
impl SafetyProvider for Counter {
    fn name(&self) -> &str {
        "content-test"
    }
    fn supports_content_reuse(&self) -> bool {
        true
    }
    fn capabilities(&self) -> &[SafetyProviderCapability] {
        &[SafetyProviderCapability::GeneralMediaModeration]
    }
    async fn scan(&self, _: &ProviderScanRequest) -> Result<ProviderScanResult, ScanError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(30)).await;
        let mut result = ProviderScanResult::completed(
            self.name(),
            SafetyProviderCapability::GeneralMediaModeration,
        );
        result.score = Some(80);
        result.labels = vec![
            SafetyLabel::new(SafetyCategory::Nsfw).with_confidence(80),
            SafetyLabel::new(SafetyCategory::Objectionable).with_confidence(23),
        ];
        Ok(result)
    }
}
fn service(pool: &PgPool, provider: Arc<Counter>) -> SafetyScanService {
    let mut policy = SafetyPolicy::public_node_default();
    policy.require_known_csam = false;
    let orchestrator = SafetyOrchestrator::builder(
        ISSUER,
        Arc::new(SystemScanClock::new()),
        Arc::new(UuidEventIdGenerator::new()),
    )
    .provider(provider)
    .policy(policy)
    .build()
    .expect("orchestrator");
    SafetyScanService::builder(
        Arc::new(orchestrator),
        Arc::new(PgSafetyArtifactStore::new(pool.clone())),
    )
    .without_signed_events(ISSUER)
    .build()
    .expect("service")
}

#[tokio::test]
async fn postgres_content_reuse_spans_services_authors_and_restart() -> Result<()> {
    let Some(admin) = database_url() else {
        eprintln!("integration disabled");
        return Ok(());
    };
    let db = TestDatabase::create(&admin, "cn_1060_content").await?;
    let pool = connect_postgres(&db.database_url).await?;
    let result=async{
        initialize_database(&pool).await?;
        let provider=Arc::new(Counter::default());let a=service(&pool,provider.clone());let b=service(&pool,provider.clone());
        let ra=ProviderScanRequest::for_subject(SubjectKind::Post,"post-a").with_text("same body");
        let rb=ProviderScanRequest::for_subject(SubjectKind::Post,"post-b").with_text("same body");
        let (a,b)=tokio::join!(a.scan_or_reuse(&ra,Some("alice"),"state-a"),b.scan_or_reuse(&rb,Some("bob"),"state-b"));
        assert!(a?.report.verdict.is_labeled_allow());assert!(b?.report.verdict.is_labeled_allow());
        assert_eq!(provider.0.load(Ordering::SeqCst),1);
        let labels=list_content_advisories_for_subjects(&pool,ISSUER,&["post-a".into(),"post-b".into()],&[],&chrono::Utc::now().to_rfc3339()).await?;
        assert_eq!(labels.len(),4);
        assert!(labels.iter().filter(|label|label.category==SafetyCategory::Objectionable).all(|label|label.confidence==Some(23)));
        let associations:i64=sqlx::query_scalar("SELECT COUNT(*) FROM cn_safety.risk_signal_subject_authors WHERE target_id='post-b' AND author_pubkey='alice'").fetch_one(&pool).await?;
        assert_eq!(associations,0,"must not copy the original author");
        pool.close().await;
        let restarted_pool=connect_postgres(&db.database_url).await?;let restarted_provider=Arc::new(Counter::default());
        let restarted=service(&restarted_pool,restarted_provider.clone());
        let request=ProviderScanRequest::for_subject(SubjectKind::Post,"post-c").with_text("same body");
        restarted.scan_or_reuse(&request,Some("charlie"),"state-c").await?;
        assert_eq!(restarted_provider.0.load(Ordering::SeqCst),0);
        let rows:i64=sqlx::query_scalar("SELECT COUNT(*) FROM cn_safety.content_scan_cache").fetch_one(&restarted_pool).await?;
        assert_eq!(rows,1);restarted_pool.close().await;Ok(())
    }.await;
    pool.close().await;
    db.cleanup().await?;
    result
}

#[tokio::test]
async fn postgres_claim_fences_stale_owner_and_rejects_incomplete_results() -> Result<()> {
    let Some(admin) = database_url() else {
        eprintln!("integration disabled");
        return Ok(());
    };
    let db = TestDatabase::create(&admin, "cn_1060_claim").await?;
    let pool = connect_postgres(&db.database_url).await?;
    let result=async{
        initialize_database(&pool).await?;let store=PgContentScanStore::new(pool.clone());let key="a".repeat(64);
        let first=uuid::Uuid::new_v4().to_string();let second=uuid::Uuid::new_v4().to_string();
        assert!(store.claim(&key,&first,CONTENT_SCAN_LEASE).await?);assert!(!store.claim(&key,&second,CONTENT_SCAN_LEASE).await?);
        sqlx::query("UPDATE cn_safety.content_scan_claims SET expires_at=clock_timestamp()-interval '1 second' WHERE cache_key=$1").bind(&key).execute(&pool).await?;
        assert!(store.claim(&key,&second,CONTENT_SCAN_LEASE).await?);
        let completed=ProviderScanResult::completed("content-test",SafetyProviderCapability::GeneralMediaModeration);
        assert!(!store.complete(&key,&first,std::slice::from_ref(&completed)).await?);
        store.release(&key,&first).await?;
        let mut failed=completed.clone();failed.outcome=ScanOutcome::Failed;
        assert!(store.complete(&key,&second,&[failed]).await.is_err());assert!(store.load(&key).await?.is_none());
        assert!(store.complete(&key,&second,&[completed]).await?);assert!(store.load(&key).await?.is_some());
        sqlx::query("UPDATE cn_safety.content_scan_cache SET scan_results='[{}]'::jsonb WHERE cache_key=$1").bind(&key).execute(&pool).await?;
        assert!(store.load(&key).await?.is_none(),"corrupt metadata must not be clean");Ok(())
    }.await;
    pool.close().await;
    db.cleanup().await?;
    result
}

#[tokio::test]
async fn postgres_budget_is_atomic_shared_and_survives_restart() -> Result<()> {
    let Some(admin) = database_url() else {
        eprintln!("integration disabled");
        return Ok(());
    };
    let db = TestDatabase::create(&admin, "cn_1060_budget").await?;
    let pool = connect_postgres(&db.database_url).await?;
    let result=async{
        initialize_database(&pool).await?;
        let a=PgModerationBudget::new(pool.clone());let b=PgModerationBudget::new(pool.clone());
        let limits=BudgetConfig{requests_per_minute:2,requests_per_day:3,tokens_per_minute:10};
        let (ra,rb)=tokio::join!(a.reserve(6,&limits),b.reserve(6,&limits));
        assert_eq!([ra?,rb?].iter().filter(|delay|delay.is_zero()).count(),1);
        assert!(a.reserve(4,&limits).await?.is_zero());
        assert!(!b.reserve(1,&limits).await?.is_zero());
        let rows:i64=sqlx::query_scalar("SELECT COUNT(*) FROM cn_safety.moderation_budget_reservations").fetch_one(&pool).await?;assert_eq!(rows,2);
        pool.close().await;let restarted=connect_postgres(&db.database_url).await?;let budget=PgModerationBudget::new(restarted.clone());
        assert!(!budget.reserve(1,&limits).await?.is_zero(),"restart must not reset the minute");
        sqlx::query("UPDATE cn_safety.moderation_budget_reservations SET admitted_at=clock_timestamp()-interval '65 seconds'").execute(&restarted).await?;
        assert!(budget.reserve(1,&limits).await?.is_zero());assert!(budget.reserve(1,&limits).await?>Duration::from_secs(3600),"daily limit must remain consumed");
        assert!(budget.reserve(11,&limits).await.is_err());
        restarted.close().await;Ok(())
    }.await;
    pool.close().await;
    db.cleanup().await?;
    result
}

#[tokio::test]
async fn postgres_claim_reclaims_a_bounded_batch_of_unrelated_expired_keys() -> Result<()> {
    let Some(admin) = database_url() else {
        eprintln!("integration disabled");
        return Ok(());
    };
    let db = TestDatabase::create(&admin, "cn_2055_claim").await?;
    let pool = connect_postgres(&db.database_url).await?;
    let result = async {
        initialize_database(&pool).await?;
        let store = PgContentScanStore::new(pool.clone());
        let owner = uuid::Uuid::new_v4().to_string();
        let old_owner = uuid::Uuid::new_v4().to_string();
        let key = "f".repeat(64);
        for count in [256_i32, 4096] {
            sqlx::query("TRUNCATE cn_safety.content_scan_claims").execute(&pool).await?;
            sqlx::query("INSERT INTO cn_safety.content_scan_claims (cache_key, owner, expires_at) SELECT lpad(to_hex(i), 64, '0'), $1::uuid, clock_timestamp() - interval '2 hours' FROM generate_series(1, $2) i")
                .bind(&old_owner).bind(count).execute(&pool).await?;
            // 対象keyは掃除の最古batchに入らなくても、既存のUPSERTで引き継げる。
            sqlx::query("INSERT INTO cn_safety.content_scan_claims VALUES ($1, $2::uuid, clock_timestamp() - interval '1 hour')")
                .bind(&key).bind(&old_owner).execute(&pool).await?;
            assert!(store.claim(&key, &owner, CONTENT_SCAN_LEASE).await?);
            let expired: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cn_safety.content_scan_claims WHERE expires_at <= clock_timestamp()")
                .fetch_one(&pool).await?;
            assert_eq!(expired, i64::from(count) - 128, "one claim must reclaim just one fixed batch");
            let current: String = sqlx::query_scalar("SELECT owner::text FROM cn_safety.content_scan_claims WHERE cache_key = $1")
                .bind(&key).fetch_one(&pool).await?;
            assert_eq!(current, owner);
            assert!(!store.claim(&key, &old_owner, CONTENT_SCAN_LEASE).await?);
            let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cn_safety.content_scan_claims WHERE expires_at <= clock_timestamp()")
                .fetch_one(&pool).await?;
            assert_eq!(remaining, i64::from(count) - 256, "later claims keep reclaiming bounded batches");
        }
        Ok(())
    }.await;
    pool.close().await;
    db.cleanup().await?;
    result
}
