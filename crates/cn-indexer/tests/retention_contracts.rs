//! CN の受入下限・容量・保持期間（#1221 R5-F）の contract テスト（`KUKURI_CN_RUN_INTEGRATION_TESTS=1`）。
//!
//! - 撤回 → marker の回収 → 撤回を持たない別 provider の旧応答で、撤回済みの投稿が索引へ戻らない。state の作成時刻を
//!   偽っても、署名済み envelope の作成時刻で判定する。
//! - 容量を超えると受入下限が上がり、計数が容量以下になるまで古い順に回収する。
//! - 解除した scope は検索から即座に外れ、索引は 1 回 128 件以内で回収される。撤回 marker は受入下限まで残る。
//! - 回収は途中で止めても、永続化した受入下限から同じ条件で続き、1 回の回収は常に 128 件以内。

use std::sync::Arc;

use anyhow::Result;
use sqlx::PgPool;

use kukuri_cn_core::{
    ChannelSecretCipher, IndexEntryStore, IndexScopeKind, NewIndexEntry, PgIndexEntryStore,
    PgSafetyArtifactStore, RetentionSettings, TestDatabase, add_supported_topic,
    advance_retention_floor, configure_retention, connect_postgres, filter_surfaceable_objects,
    initialize_database, reclaim_expired, remove_supported_topic, upsert_index_entry,
    upsert_scan_verdict,
};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::participant::IndexerParticipant;
use kukuri_cn_indexer::projection::{IndexProjection, IndexedEntry, MemoryIndexProjection};
use kukuri_cn_safety::provider::SubjectKind;
use kukuri_cn_safety::{
    MockSafetyProvider, ModerationEventSigner, ReasonCode, SafetyAction, SafetyVerdict,
};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{
    SafetyOrchestrator, SafetyScanService, Secp256k1ModerationEventSigner, VerdictPersistMeta,
};
use kukuri_core::{
    KukuriEnvelope, KukuriKeys, PostWithdrawalReason, TopicId, WithdrawalReasonVisibility,
    build_post_envelope, build_post_withdrawal_envelope, timeline_sort_key,
};
use kukuri_docs_sync::{DocOp, DocsSync, MemoryDocsSync, topic_replica_id};

const TOPIC: &str = "rust";
const RETENTION_SECS: i64 = 3 * 86_400;

async fn with_database<F, Fut>(prefix: &str, test: F) -> Result<()>
where
    F: FnOnce(String, PgPool) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let Some(admin_url) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        eprintln!("skipping retention test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), prefix).await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    let result = test(database.database_url.clone(), pool.clone()).await;
    pool.close().await;
    database.cleanup().await?;
    result
}

fn pipeline(
    pool: &PgPool,
    docs: Arc<dyn DocsSync>,
    projection: Arc<MemoryIndexProjection>,
) -> Result<IngestPipeline> {
    let signer = Secp256k1ModerationEventSigner::from_secret(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )?;
    let issuer = signer.issuer_node_id().to_string();
    let orchestrator = SafetyOrchestrator::builder(
        &issuer,
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .provider(Arc::new(MockSafetyProvider::known_csam("mock-known-csam")))
    .build()?;
    let scan = SafetyScanService::builder(
        Arc::new(orchestrator),
        Arc::new(PgSafetyArtifactStore::new(pool.clone())),
    )
    .signer(Arc::new(signer))
    .build()?;
    Ok(IngestPipeline::new(
        docs,
        Arc::new(scan),
        Arc::new(PgIndexEntryStore::new(pool.clone())),
        projection,
    ))
}

fn participant(
    pool: &PgPool,
    projection: Arc<MemoryIndexProjection>,
) -> Result<IndexerParticipant> {
    let docs: Arc<dyn DocsSync> = Arc::new(MemoryDocsSync::default());
    Ok(IndexerParticipant::new(
        pool.clone(),
        docs.clone(),
        Arc::new(PgIndexEntryStore::new(pool.clone())),
        projection.clone(),
        pipeline(pool, docs, projection)?,
        ChannelSecretCipher::from_key_material("retention-test-channel-secret-key-0123456789")?,
    ))
}

async fn set(docs: &MemoryDocsSync, key: String, value: serde_json::Value) -> Result<()> {
    let replica = topic_replica_id(TOPIC);
    docs.open_replica(&replica).await?;
    docs.apply_doc_op(&replica, DocOp::SetJson { key, value })
        .await
}

/// 投稿を state / envelope / timeline の key で書く。`state_created_at` で state の作成時刻を偽れる。
async fn write_post(
    docs: &MemoryDocsSync,
    envelope: &KukuriEnvelope,
    state_created_at: i64,
) -> Result<()> {
    let mut object = envelope.to_post_object()?.expect("post object");
    let id = object.object_id.as_str().to_string();
    object.created_at = state_created_at;
    set(
        docs,
        format!("objects/{id}/state"),
        serde_json::to_value(&object)?,
    )
    .await?;
    set(
        docs,
        format!("objects/{id}/envelope"),
        serde_json::to_value(envelope)?,
    )
    .await?;
    set(
        docs,
        format!(
            "indexes/timeline/{}/{id}",
            timeline_sort_key(state_created_at, &object.object_id)
        ),
        serde_json::json!({ "object_id": id }),
    )
    .await
}

async fn index_row(pool: &PgPool, scope: &str, object: &str, created_at: i64) -> Result<()> {
    let verdict = upsert_scan_verdict(
        pool,
        SubjectKind::Post,
        object,
        &SafetyVerdict {
            action: SafetyAction::Allow,
            labels: Vec::new(),
            advisory_labels: Vec::new(),
            critical: false,
            reason_code: ReasonCode::NoKnownMatch,
            confidence: None,
            provider: Some("mock-known-csam".into()),
            provider_capability: None,
            policy_version: "policy-v1-test".into(),
            scanned_at: "2026-09-27T00:00:00Z".into(),
        },
        &VerdictPersistMeta::default(),
    )
    .await?;
    upsert_index_entry(
        pool,
        &NewIndexEntry {
            scope_kind: IndexScopeKind::PublicTopic,
            scope_id: scope.into(),
            object_id: object.into(),
            author_pubkey: "author".into(),
            created_at,
            source_replica_id: format!("topic::{scope}"),
            verdict_id: verdict.id,
            verdict_action: "allow".into(),
            critical: false,
        },
    )
    .await?;
    Ok(())
}

fn projected(scope: &str, object: &str, created_at: i64) -> IndexedEntry {
    IndexedEntry {
        scope_kind: IndexScopeKind::PublicTopic,
        scope_id: scope.into(),
        object_id: object.into(),
        author_pubkey: "author".into(),
        text: "text".into(),
        created_at,
        source_replica_id: format!("topic::{scope}"),
        content_advisories: Vec::new(),
    }
}

async fn count(pool: &PgPool, table: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await?)
}

/// 計数が 5 表の実際の行数と一致することを確かめて返す。
async fn units(pool: &PgPool) -> Result<i64> {
    let units: i64 = sqlx::query_scalar("SELECT units FROM cn_index.retention_state")
        .fetch_one(pool)
        .await?;
    let mut actual = 0;
    for table in [
        "cn_index.index_entries",
        "cn_index.known_post_withdrawals",
        "cn_index.relation_actions",
        "cn_safety.scan_verdicts",
        "cn_safety.content_scan_cache",
    ] {
        actual += count(pool, table).await?;
    }
    assert_eq!(units, actual, "the retention counter must match the rows");
    Ok(units)
}

#[tokio::test]
async fn withdrawn_post_stays_out_after_its_marker_is_reclaimed() -> Result<()> {
    with_database("cn_retention_withdrawal", |_, pool| async move {
        configure_retention(
            &pool,
            RetentionSettings {
                capacity_rows: 1_000_000,
                retention_secs: RETENTION_SECS,
            },
        )
        .await?;
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, TOPIC).await?;
        let keys = KukuriKeys::generate();
        let post = build_post_envelope(&keys, &TopicId::new(TOPIC), "withdrawn later", None)?;
        let id = post.id.as_str().to_string();
        let first = Arc::new(MemoryDocsSync::default());
        write_post(&first, &post, post.created_at).await?;
        let projection = Arc::new(MemoryIndexProjection::new());
        let ingest = pipeline(&pool, first.clone(), projection.clone())?;
        let replica = topic_replica_id(TOPIC);
        assert_eq!(
            ingest
                .ingest_recent_scope(IndexScopeKind::PublicTopic, TOPIC, &replica)
                .await?
                .indexed,
            1
        );
        let withdrawal = build_post_withdrawal_envelope(
            &keys,
            &post,
            1,
            None,
            WithdrawalReasonVisibility::Public,
            Some(PostWithdrawalReason::AuthorRequest),
        )?;
        set(
            &first,
            format!("withdrawals/{id}/state"),
            serde_json::to_value(withdrawal)?,
        )
        .await?;
        ingest
            .ingest_recent_scope(IndexScopeKind::PublicTopic, TOPIC, &replica)
            .await?;
        let marker: i64 = sqlx::query_scalar(
            "SELECT created_at FROM cn_index.known_post_withdrawals WHERE object_id = $1",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            marker, post.created_at,
            "the marker keeps the signed creation time"
        );

        // 受入下限が投稿の作成時刻を越えると、marker は回収される。
        let floor = advance_retention_floor(&pool, post.created_at + RETENTION_SECS + 100).await?;
        assert!(floor > post.created_at);
        while reclaim_expired(&pool, floor, 128).await? > 0 {}
        assert_eq!(count(&pool, "cn_index.known_post_withdrawals").await?, 0);

        // 撤回を持たない別 provider の旧応答も、作成時刻を偽った state も、索引へ戻らない。
        let stale = Arc::new(MemoryDocsSync::default());
        write_post(&stale, &post, post.created_at).await?;
        let forged = Arc::new(MemoryDocsSync::default());
        write_post(&forged, &post, floor + 60).await?;
        for docs in [stale, forged] {
            let summary = pipeline(&pool, docs, projection.clone())?
                .ingest_recent_scope(IndexScopeKind::PublicTopic, TOPIC, &replica)
                .await?;
            assert_eq!(summary.indexed, 0);
            assert_eq!(count(&pool, "cn_index.index_entries").await?, 0);
        }
        assert!(
            !projection
                .contains_object(IndexScopeKind::PublicTopic, TOPIC, &id)
                .await?
        );
        units(&pool).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn capacity_raises_the_floor_until_the_count_fits() -> Result<()> {
    with_database("cn_retention_capacity", |_, pool| async move {
        let now = chrono::Utc::now().timestamp();
        configure_retention(
            &pool,
            RetentionSettings {
                capacity_rows: 10,
                retention_secs: RETENTION_SECS,
            },
        )
        .await?;
        for index in 0..8 {
            index_row(&pool, TOPIC, &format!("post-{index}"), now - 100 + index).await?;
        }
        assert_eq!(units(&pool).await?, 16, "8 index rows and their 8 verdicts");
        let mut floor = 0;
        while units(&pool).await? > 10 {
            floor = advance_retention_floor(&pool, now).await?;
            assert!(reclaim_expired(&pool, floor, 128).await? <= 128);
        }
        assert_eq!(count(&pool, "cn_index.index_entries").await?, 2);
        assert_eq!(
            floor,
            now - 100 + 6,
            "the floor passes only the reclaimed rows"
        );
        // 受入下限より古い投稿は、容量に余裕ができても索引へ入らない。
        assert!(
            index_row(&pool, TOPIC, "late-old-post", now - 100)
                .await
                .is_err()
        );
        index_row(&pool, TOPIC, "new-post", now).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn retired_scope_hides_at_once_and_reclaims_in_bounded_pages() -> Result<()> {
    with_database("cn_retention_scope", |_, pool| async move {
        let now = chrono::Utc::now().timestamp();
        configure_retention(
            &pool,
            RetentionSettings {
                capacity_rows: 1_000_000,
                retention_secs: RETENTION_SECS,
            },
        )
        .await?;
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, "gone").await?;
        let projection = Arc::new(MemoryIndexProjection::new());
        for index in 0..300 {
            let object = format!("post-{index:03}");
            index_row(&pool, "gone", &object, now).await?;
            projection
                .upsert_entry(&projected("gone", &object, now))
                .await?;
        }
        let entries = PgIndexEntryStore::new(pool.clone());
        entries
            .record_verified_withdrawal(IndexScopeKind::PublicTopic, "gone", "withdrawn", now)
            .await?;
        let candidates = vec![("gone".to_string(), "post-000".to_string())];
        assert_eq!(
            filter_surfaceable_objects(&pool, IndexScopeKind::PublicTopic, &candidates)
                .await?
                .len(),
            1
        );
        remove_supported_topic(&pool, IndexScopeKind::PublicTopic, "gone").await?;
        assert!(
            filter_surfaceable_objects(&pool, IndexScopeKind::PublicTopic, &candidates)
                .await?
                .is_empty(),
            "a retired scope leaves search before its rows are reclaimed"
        );
        let participant = participant(&pool, projection.clone())?;
        let mut steps = 0;
        loop {
            let removed = participant
                .retire_scope(IndexScopeKind::PublicTopic, "gone", 128)
                .await?;
            assert!(removed <= 128);
            steps += 1;
            if removed == 0 {
                break;
            }
            // 途中で再び support しても、撤回済みの投稿は戻らない（marker は残る）。
            if steps == 2 {
                add_supported_topic(&pool, IndexScopeKind::PublicTopic, "gone").await?;
                assert!(
                    entries
                        .is_known_withdrawn(IndexScopeKind::PublicTopic, "gone", "withdrawn", now)
                        .await?
                );
                remove_supported_topic(&pool, IndexScopeKind::PublicTopic, "gone").await?;
            }
        }
        assert_eq!(steps, 6, "600 rows in pages of at most 128");
        assert_eq!(count(&pool, "cn_index.index_entries").await?, 0);
        assert_eq!(
            projection
                .count_scope(IndexScopeKind::PublicTopic, "gone")
                .await?,
            0
        );
        assert_eq!(count(&pool, "cn_index.known_post_withdrawals").await?, 1);
        units(&pool).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn reclaim_resumes_from_the_persisted_floor_in_bounded_steps() -> Result<()> {
    with_database("cn_retention_restart", |database_url, pool| async move {
        let now = chrono::Utc::now().timestamp();
        configure_retention(
            &pool,
            RetentionSettings {
                capacity_rows: 1_000_000,
                retention_secs: RETENTION_SECS,
            },
        )
        .await?;
        let projection = Arc::new(MemoryIndexProjection::new());
        let old = now - RETENTION_SECS - 1_000;
        for index in 0..1_000 {
            let object = format!("post-{index:04}");
            index_row(&pool, TOPIC, &object, old + index).await?;
            projection
                .upsert_entry(&projected(TOPIC, &object, old + index))
                .await?;
        }
        // 古い投稿の verdict は、取り込んだ当時（古い時刻）に作られている。
        sqlx::query("UPDATE cn_safety.scan_verdicts SET updated_at = TO_TIMESTAMP($1)")
            .bind(old)
            .execute(&pool)
            .await?;
        index_row(&pool, TOPIC, "fresh", now).await?;
        let first = participant(&pool, projection.clone())?;
        assert_eq!(
            first.reclaim_retention(now, 128).await?,
            128,
            "the projection goes first"
        );
        let floor: i64 = sqlx::query_scalar("SELECT floor FROM cn_index.retention_state")
            .fetch_one(&pool)
            .await?;
        assert_eq!(floor, now - RETENTION_SECS);
        drop(first);

        // 別の接続（再起動後）で、同じ受入下限から続きを回収する。
        let reopened = connect_postgres(database_url.as_str()).await?;
        let second = participant(&reopened, projection.clone())?;
        loop {
            let removed = second.reclaim_retention(now, 128).await?;
            assert!(removed <= 128);
            if removed == 0 {
                break;
            }
        }
        assert_eq!(
            projection
                .count_scope(IndexScopeKind::PublicTopic, TOPIC)
                .await?,
            0
        );
        assert_eq!(
            count(&reopened, "cn_index.index_entries").await?,
            1,
            "only the fresh post remains"
        );
        assert_eq!(units(&reopened).await?, 2, "the fresh post and its verdict");
        reopened.close().await;
        Ok(())
    })
    .await
}
