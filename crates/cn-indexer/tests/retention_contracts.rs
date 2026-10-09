//! CN の受入下限・容量・保持期間（#1221 R5-F）の contract テスト（`KUKURI_CN_RUN_INTEGRATION_TESTS=1`）。
//!
//! - 撤回 → marker の回収 → 撤回を持たない別 provider の旧応答で、撤回済みの投稿が索引へ戻らない。state の作成時刻を
//!   偽っても、署名済み envelope の作成時刻で判定する。
//! - 容量を超えると受入下限が上がり、計数が容量以下になるまで古い順に回収する。
//! - 解除した scope は検索から即座に外れ、索引は 1 回 128 件以内で回収される。撤回 marker は受入下限まで残る。
//! - 回収は途中で止めても、永続化した受入下限から同じ条件で続き、1 回の回収は常に 128 件以内。
//! - 巡回は索引済みの scope を永続 cursor で 32 件ずつ照合し、support を失った scope だけを回収する（#1221 R5-H）。
//! - 送信防止を適用した投稿の写しは、巡回が上限つきで消し、失敗は次の巡回で消し直す。解除の後の新しい取込で
//!   戻った写しは消さない（#1698）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use sqlx::PgPool;

use kukuri_cn_core::{
    ChannelSecretCipher, IndexEntryStore, IndexScopeKind, NewIndexEntry, NewTransmissionPrevention,
    PgIndexEntryStore, PgSafetyArtifactStore, RetentionSettings, TestDatabase,
    TransmissionPreventionBasis, TransmissionPreventionCapability, add_supported_topic,
    advance_retention_floor, apply_transmission_prevention, configure_retention, connect_postgres,
    filter_surfaceable_objects, initialize_database, reclaim_expired,
    release_transmission_prevention, remove_supported_topic, upsert_index_entry,
    upsert_scan_verdict,
};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::maintenance::IndexMaintenance;
use kukuri_cn_indexer::projection::{IndexProjection, IndexedEntry, MemoryIndexProjection};
use kukuri_cn_indexer::state::IndexerRuntimeState;
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

fn maintenance(pool: &PgPool, projection: Arc<dyn IndexProjection>) -> Result<IndexMaintenance> {
    Ok(IndexMaintenance::new(
        pool.clone(),
        Arc::new(PgIndexEntryStore::new(pool.clone())),
        projection,
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
    // verdict は投稿を取り込んだ当時に作られる（投稿の作成時刻と同じ頃）。
    sqlx::query("UPDATE cn_safety.scan_verdicts SET updated_at = TO_TIMESTAMP($1) WHERE id = $2")
        .bind(created_at)
        .bind(&verdict.id)
        .execute(pool)
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
    Ok(
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
            .fetch_one(pool)
            .await?,
    )
}

/// 計数が 5 表の実際の行数と、索引の行数の計数が索引の行数と一致することを確かめて返す（#1714）。
async fn units(pool: &PgPool) -> Result<i64> {
    let (units, index_entries): (i64, i64) =
        sqlx::query_as("SELECT units, index_entries FROM cn_index.retention_state")
            .fetch_one(pool)
            .await?;
    assert_eq!(
        index_entries,
        count(pool, "cn_index.index_entries").await?,
        "the index entry counter must match the rows"
    );
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
                .ingest_changed_keys(IndexScopeKind::PublicTopic, TOPIC, &replica, &[])
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
            .ingest_changed_keys(IndexScopeKind::PublicTopic, TOPIC, &replica, &[])
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
        let floor =
            advance_retention_floor(&pool, post.created_at + RETENTION_SECS + 100, 128).await?;
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
                .ingest_changed_keys(IndexScopeKind::PublicTopic, TOPIC, &replica, &[])
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
async fn capacity_keeps_the_count_within_b_through_the_worker_pass() -> Result<()> {
    with_database("cn_retention_capacity", |_, pool| async move {
        let now = chrono::Utc::now().timestamp();
        configure_retention(
            &pool,
            RetentionSettings {
                capacity_rows: 200,
                retention_secs: RETENTION_SECS,
            },
        )
        .await?;
        let maintenance = maintenance(&pool, Arc::new(MemoryIndexProjection::new()))?;
        // 流入が容量を大きく上回っても、worker と同じ 1 回の見直しで計数は B 以下へ戻る。
        for cycle in 0..5 {
            for index in 0..150 {
                let created_at = now - 10_000 + cycle * 1_000 + index;
                index_row(&pool, TOPIC, &format!("post-{cycle}-{index}"), created_at).await?;
            }
            assert!(units(&pool).await? > 200);
            maintenance.reclaim_retention_pass(now, 128, 16).await?;
            assert!(units(&pool).await? <= 200, "cycle {cycle}");
        }
        // 残ったのは最新の行で、受入下限より古い投稿は容量に余裕があっても索引へ入らない。
        let floor: i64 = sqlx::query_scalar("SELECT floor FROM cn_index.retention_state")
            .fetch_one(&pool)
            .await?;
        assert!(floor > now - 10_000 + 4_000);
        assert!(
            index_row(&pool, TOPIC, "late-old-post", floor - 1)
                .await
                .is_err()
        );
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
        let maintenance = maintenance(&pool, projection.clone())?;
        let mut steps = 0;
        loop {
            let removed = maintenance
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
        index_row(&pool, TOPIC, "fresh", now).await?;
        let first = maintenance(&pool, projection.clone())?;
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
        let second = maintenance(&reopened, projection.clone())?;
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

#[tokio::test]
async fn pass_retires_unsupported_scopes_by_a_persisted_cursor() -> Result<()> {
    with_database(
        "cn_retention_indexed_cursor",
        |database_url, pool| async move {
            let now = chrono::Utc::now().timestamp();
            configure_retention(
                &pool,
                RetentionSettings {
                    capacity_rows: 1_000_000,
                    retention_secs: RETENTION_SECS,
                },
            )
            .await?;
            for index in 0..80 {
                let topic = format!("topic-{index:02}");
                add_supported_topic(&pool, IndexScopeKind::PublicTopic, &topic).await?;
                index_row(&pool, &topic, "post", now).await?;
            }
            for index in 1..80 {
                remove_supported_topic(
                    &pool,
                    IndexScopeKind::PublicTopic,
                    &format!("topic-{index:02}"),
                )
                .await?;
            }
            let cursor = |pool: PgPool| async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT last_scope_id FROM cn_index.indexed_scope_cursor WHERE id = TRUE",
                )
                .fetch_one(&pool)
                .await
            };
            // 1 回の巡回は索引済みの scope を 32 件だけ照合する（全 scope を読まない）。
            let state = IndexerRuntimeState::default();
            maintenance(&pool, Arc::new(MemoryIndexProjection::new()))?
                .run_pass(now, &state)
                .await;
            assert_eq!(state.snapshot().deindexed, 31);
            assert_eq!(cursor(pool.clone()).await?, "topic-31");

            // 再起動後も、永続した位置から続けて照合する。
            let reopened = connect_postgres(database_url.as_str()).await?;
            let resumed = maintenance(&reopened, Arc::new(MemoryIndexProjection::new()))?;
            resumed.run_pass(now, &state).await;
            assert_eq!(cursor(reopened.clone()).await?, "topic-63");
            resumed.run_pass(now, &state).await;
            assert_eq!(state.snapshot().deindexed, 79);
            let kept: Vec<String> =
                sqlx::query_scalar("SELECT scope_id FROM cn_index.index_entries")
                    .fetch_all(&reopened)
                    .await?;
            assert_eq!(
                kept,
                vec!["topic-00".to_string()],
                "support を保つ scope は残す"
            );
            reopened.close().await;
            Ok(())
        },
    )
    .await
}

/// 最初の削除だけを失敗させる投影（ArcadeDB の一時的な失敗の代わり）。
#[derive(Default)]
struct FailOnceProjection {
    inner: MemoryIndexProjection,
    failed: AtomicBool,
}

#[async_trait]
impl IndexProjection for FailOnceProjection {
    async fn upsert_entry(&self, entry: &IndexedEntry) -> Result<()> {
        self.inner.upsert_entry(entry).await
    }

    async fn contains_object(&self, kind: IndexScopeKind, scope: &str, id: &str) -> Result<bool> {
        self.inner.contains_object(kind, scope, id).await
    }

    async fn count_scope(&self, kind: IndexScopeKind, scope: &str) -> Result<usize> {
        self.inner.count_scope(kind, scope).await
    }

    async fn remove_scope_page(
        &self,
        kind: IndexScopeKind,
        scope: &str,
        n: usize,
    ) -> Result<usize> {
        self.inner.remove_scope_page(kind, scope, n).await
    }

    async fn remove_older_than(&self, floor: i64, limit: usize) -> Result<usize> {
        self.inner.remove_older_than(floor, limit).await
    }

    async fn remove_object(&self, kind: IndexScopeKind, scope: &str, id: &str) -> Result<()> {
        anyhow::ensure!(
            self.failed.swap(true, Ordering::SeqCst),
            "arcadedb is unavailable"
        );
        self.inner.remove_object(kind, scope, id).await
    }
}

fn prevention(object: &str) -> NewTransmissionPrevention {
    NewTransmissionPrevention {
        subject_kind: "post".into(),
        subject_id: object.into(),
        basis: TransmissionPreventionBasis::Privacy,
        capabilities: vec![TransmissionPreventionCapability::Search],
        expires_at: None,
        related_report_id: None,
    }
}

#[tokio::test]
async fn prevention_evicts_projected_copies_in_bounded_passes() -> Result<()> {
    with_database("cn_retention_prevention", |_, pool| async move {
        let now = chrono::Utc::now().timestamp();
        configure_retention(
            &pool,
            RetentionSettings {
                capacity_rows: 1_000_000,
                retention_secs: RETENTION_SECS,
            },
        )
        .await?;
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, "first").await?;
        let projection = Arc::new(FailOnceProjection::default());
        for scope in ["first", "second"] {
            index_row(&pool, scope, "prevented", now).await?;
            projection
                .upsert_entry(&projected(scope, "prevented", now))
                .await?;
        }
        let candidates = vec![("first".to_string(), "prevented".to_string())];
        assert_eq!(
            filter_surfaceable_objects(&pool, IndexScopeKind::PublicTopic, &candidates)
                .await?
                .len(),
            1
        );
        apply_transmission_prevention(&pool, "legal@node.example", &prevention("prevented"))
            .await?;
        assert!(
            filter_surfaceable_objects(&pool, IndexScopeKind::PublicTopic, &candidates)
                .await?
                .is_empty(),
            "the copy leaves search at once"
        );
        let maintenance = maintenance(&pool, projection.clone())?;
        let state = IndexerRuntimeState::default();
        // ArcadeDB が失敗した巡回では、消し待ちを残す。
        maintenance.run_pass(now, &state).await;
        assert_eq!(count(&pool, "cn_index.projection_evictions").await?, 2);
        // 1 回に消すのは上限まで。残りは次の巡回で消す。
        assert_eq!(maintenance.evict_prevented_projections(1).await?, 1);
        assert_eq!(count(&pool, "cn_index.projection_evictions").await?, 1);
        maintenance.run_pass(now, &state).await;
        assert_eq!(count(&pool, "cn_index.projection_evictions").await?, 0);
        assert_eq!(count(&pool, "cn_index.index_entries").await?, 0);
        for scope in ["first", "second"] {
            assert!(
                !projection
                    .contains_object(IndexScopeKind::PublicTopic, scope, "prevented")
                    .await?
            );
        }
        Ok(())
    })
    .await
}

#[tokio::test]
async fn released_prevention_keeps_the_freshly_ingested_copy() -> Result<()> {
    with_database("cn_retention_prevention_release", |_, pool| async move {
        let now = chrono::Utc::now().timestamp();
        configure_retention(
            &pool,
            RetentionSettings {
                capacity_rows: 1_000_000,
                retention_secs: RETENTION_SECS,
            },
        )
        .await?;
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, TOPIC).await?;
        let projection = Arc::new(MemoryIndexProjection::new());
        index_row(&pool, TOPIC, "released", now).await?;
        projection
            .upsert_entry(&projected(TOPIC, "released", now))
            .await?;
        apply_transmission_prevention(&pool, "legal@node.example", &prevention("released")).await?;
        release_transmission_prevention(
            &pool,
            "legal@node.example",
            "post",
            "released",
            "claim resolved",
        )
        .await?;
        // 巡回の前に、解除の後の新しい取込で真実源と写しが戻る。
        index_row(&pool, TOPIC, "released", now).await?;
        projection
            .upsert_entry(&projected(TOPIC, "released", now))
            .await?;
        maintenance(&pool, projection.clone())?
            .run_pass(now, &IndexerRuntimeState::default())
            .await;
        assert!(
            projection
                .contains_object(IndexScopeKind::PublicTopic, TOPIC, "released")
                .await?,
            "an earlier eviction must not remove the copy that came back"
        );
        assert_eq!(count(&pool, "cn_index.projection_evictions").await?, 0);
        Ok(())
    })
    .await
}
