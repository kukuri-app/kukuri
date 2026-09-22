//! #1293: 論理scopeの索引と物理replicaの同期の寿命を区別する。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use kukuri_cn_core::{
    ChannelSecretCipher, IndexScopeKind, MemoryIndexEntryStore, TestDatabase, add_supported_topic,
    connect_postgres, initialize_database,
};
use kukuri_cn_indexer::replica_plan::PublicReplicaReadMode;
use kukuri_cn_indexer::state::IndexerRuntimeState;
use kukuri_cn_indexer::worker::{IndexerWorker, WorkerConfig};
use kukuri_cn_indexer::{
    IndexProjection, IndexerParticipant, IngestPipeline, MemoryIndexProjection, ScopeReplica,
};
use kukuri_cn_safety::{MockSafetyProvider, ModerationEventSigner};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{
    MemorySafetyArtifactStore, SafetyOrchestrator, SafetyScanService,
    Secp256k1ModerationEventSigner,
};
use kukuri_docs_sync::{
    BucketReplica, BucketScope, DocFetchPolicy, DocOp, DocQuery, DocsSync, MemoryDocsSync,
    TimeBucket, topic_replica_id,
};

#[path = "support/bucket_scale.rs"]
mod bucket_scale;

// 停止経路はPostgresを読まない。lazy poolにより外部DB無しで停止のcontractだけを検証する。
fn participant(
    docs: Arc<dyn DocsSync>,
    pool: sqlx::PgPool,
) -> Result<(IndexerParticipant, Arc<MemoryIndexProjection>)> {
    participant_with_provider(docs, pool, Arc::new(MockSafetyProvider::known_csam("mock")))
}

fn participant_with_provider(
    docs: Arc<dyn DocsSync>,
    pool: sqlx::PgPool,
    provider: Arc<dyn kukuri_cn_safety::provider::SafetyProvider>,
) -> Result<(IndexerParticipant, Arc<MemoryIndexProjection>)> {
    let signer = Secp256k1ModerationEventSigner::from_secret(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )?;
    let orchestrator = SafetyOrchestrator::builder(
        signer.issuer_node_id(),
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .provider(provider)
    .build()?;
    let artifacts = Arc::new(MemorySafetyArtifactStore::new());
    let service = Arc::new(
        SafetyScanService::builder(Arc::new(orchestrator), artifacts.clone())
            .signer(Arc::new(signer))
            .build()?,
    );
    let entries = Arc::new(MemoryIndexEntryStore::new(artifacts));
    let projection = Arc::new(MemoryIndexProjection::default());
    let pipeline = IngestPipeline::new(docs.clone(), service, entries.clone(), projection.clone());
    Ok((
        IndexerParticipant::new(
            pool,
            docs,
            entries,
            projection.clone(),
            pipeline,
            ChannelSecretCipher::from_key_material(
                "bucket-contract-unused-key-material-0123456789",
            )?,
        ),
        projection,
    ))
}

#[derive(Default)]
struct NoOpenDocs(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl DocsSync for NoOpenDocs {
    async fn open_replica(&self, _: &kukuri_core::ReplicaId) -> Result<()> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        anyhow::bail!("unexpected namespace open")
    }
    async fn apply_doc_op(&self, _: &kukuri_core::ReplicaId, _: DocOp) -> Result<()> {
        unreachable!()
    }
    async fn query_replica_with_policy(
        &self,
        _: &kukuri_core::ReplicaId,
        _: DocQuery,
        _: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        unreachable!()
    }
    async fn subscribe_replica(
        &self,
        _: &kukuri_core::ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        unreachable!()
    }
    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        unreachable!()
    }
}

#[tokio::test]
async fn mismatched_scope_is_rejected_before_opening_any_replica() -> Result<()> {
    let docs = Arc::new(NoOpenDocs::default());
    let (participant, _) = participant(
        docs.clone(),
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")?,
    )?;
    for replica_id in [
        topic_replica_id("other"),
        BucketReplica::new(
            BucketScope::Topic {
                topic_id: "other".into(),
            },
            TimeBucket::from_index(1)?,
        )?
        .replica_id(),
        BucketReplica::new(
            BucketScope::Author {
                author_pubkey: "author".into(),
            },
            TimeBucket::from_index(1)?,
        )?
        .replica_id(),
    ] {
        assert!(
            participant
                .ingest_scope(&ScopeReplica {
                    kind: IndexScopeKind::PublicTopic,
                    id: "expected".into(),
                    replica_id
                })
                .await
                .is_err()
        );
    }
    assert_eq!(
        docs.0.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "scope guard must dominate open/sync"
    );
    Ok(())
}

#[tokio::test]
async fn a_post_signed_for_another_bucket_is_not_indexed() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let (participant, projection) = participant(
        docs.clone(),
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")?,
    )?;
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: "time-check".into(),
        },
        TimeBucket::from_index(2)?,
    )?;
    let id = post(
        &docs,
        &replica.replica_id(),
        "time-check",
        "wrong bucket",
        86_400,
    )
    .await?;
    let result = participant
        .ingest_scope(&ScopeReplica {
            kind: IndexScopeKind::PublicTopic,
            id: "time-check".into(),
            replica_id: replica.replica_id(),
        })
        .await?;
    assert_eq!(result.indexed, 0);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "time-check", &id)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn a_misplaced_copy_cannot_remove_an_already_indexed_post() -> Result<()> {
    for forged_delete in [false, true] {
        let docs = Arc::new(MemoryDocsSync::default());
        let (participant, projection) = participant(
            docs.clone(),
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")?,
        )?;
        let topic = "copy-check";
        let scope = |day| -> Result<ScopeReplica> {
            Ok(ScopeReplica {
                kind: IndexScopeKind::PublicTopic,
                id: topic.into(),
                replica_id: BucketReplica::new(
                    BucketScope::Topic {
                        topic_id: topic.into(),
                    },
                    TimeBucket::from_index(day)?,
                )?
                .replica_id(),
            })
        };
        let proper = scope(1)?;
        let wrong = scope(2)?;
        let id = post(&docs, &proper.replica_id, topic, "original", 86_400).await?;
        assert_eq!(participant.ingest_scope(&proper).await?.indexed, 1);
        for record in docs
            .query_replica(&proper.replica_id, DocQuery::All)
            .await?
        {
            let mut value: serde_json::Value = serde_json::from_slice(&record.value)?;
            if forged_delete && record.key.ends_with("/state") {
                value["status"] = serde_json::json!("deleted");
            }
            docs.apply_doc_op(
                &wrong.replica_id,
                DocOp::SetJson {
                    key: record.key,
                    value,
                },
            )
            .await?;
        }
        assert_eq!(participant.ingest_scope(&wrong).await?.indexed, 0);
        assert!(
            projection
                .contains_object(IndexScopeKind::PublicTopic, topic, &id)
                .await?,
            "a noncanonical copy must not erase another replica's accepted post"
        );
    }
    Ok(())
}

#[tokio::test]
async fn bucket_posts_are_derived_from_the_signature_not_an_untrusted_state() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let (participant, projection) = participant(
        docs.clone(),
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")?,
    )?;
    let topic = "signed-header";
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.into(),
        },
        TimeBucket::from_index(1)?,
    )?
    .replica_id();
    let id = post(&docs, &replica, topic, "authentic text", 86_400).await?;
    docs.apply_doc_op(
        &replica,
        DocOp::SetBytes {
            key: format!("objects/{id}/state"),
            value: b"not a post header".to_vec(),
        },
    )
    .await?;
    assert_eq!(
        participant
            .ingest_scope(&ScopeReplica {
                kind: IndexScopeKind::PublicTopic,
                id: topic.into(),
                replica_id: replica
            })
            .await?
            .indexed,
        1
    );
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, topic, &id)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn changing_only_the_bucket_marker_reuses_the_signed_post_scan() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let provider = Arc::new(OnceAvailableProvider(std::sync::atomic::AtomicUsize::new(
        0,
    )));
    let (participant, projection) = participant_with_provider(
        docs.clone(),
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")?,
        provider.clone(),
    )?;
    let scope = ScopeReplica {
        kind: IndexScopeKind::PublicTopic,
        id: "marker-reuse".into(),
        replica_id: BucketReplica::new(
            BucketScope::Topic {
                topic_id: "marker-reuse".into(),
            },
            TimeBucket::from_index(1)?,
        )?
        .replica_id(),
    };
    let id = post(
        &docs,
        &scope.replica_id,
        &scope.id,
        "authentic text",
        86_400,
    )
    .await?;
    assert_eq!(participant.ingest_scope(&scope).await?.scans_fresh, 1);
    docs.apply_doc_op(
        &scope.replica_id,
        DocOp::SetBytes {
            key: format!("objects/{id}/state"),
            value: b"untrusted replacement marker".to_vec(),
        },
    )
    .await?;
    let result = participant.ingest_scope(&scope).await?;
    assert_eq!(
        result.scans_fresh, 0,
        "unsigned marker must not trigger a provider call"
    );
    assert_eq!(result.scans_reused, 1);
    assert_eq!(provider.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        projection
            .contains_object(scope.kind, &scope.id, &id)
            .await?
    );
    Ok(())
}

struct OnceAvailableProvider(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl kukuri_cn_safety::provider::SafetyProvider for OnceAvailableProvider {
    fn name(&self) -> &str {
        "once"
    }
    fn capabilities(&self) -> &[kukuri_cn_safety::capability::SafetyProviderCapability] {
        &[kukuri_cn_safety::capability::SafetyProviderCapability::KnownCsamHashMatch]
    }
    async fn scan(
        &self,
        request: &kukuri_cn_safety::provider::ProviderScanRequest,
    ) -> Result<kukuri_cn_safety::provider::ProviderScanResult, kukuri_cn_safety::provider::ScanError>
    {
        let provider = MockSafetyProvider::known_csam("once");
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            provider.scan(request).await
        } else {
            provider.default_unavailable().scan(request).await
        }
    }
}

#[tokio::test]
async fn public_scope_removal_stops_replica_events_and_preserves_local_entries() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let (participant, _) = participant(
        docs.clone(),
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")?,
    )?;
    let replica = topic_replica_id("bucket-lifecycle");
    docs.apply_doc_op(
        &replica,
        DocOp::SetBytes {
            key: "kept".into(),
            value: b"saved data".to_vec(),
        },
    )
    .await?;
    let mut stream = docs.subscribe_replica(&replica).await?;
    participant
        .stop_and_deindex_scope(IndexScopeKind::PublicTopic, "bucket-lifecycle")
        .await?;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("removing a public scope must close its replica subscription")
            .is_none()
    );
    let rows = docs
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact("kept".into()),
            DocFetchPolicy::LocalOnly,
        )
        .await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].value, b"saved data");
    Ok(())
}

async fn post(
    docs: &MemoryDocsSync,
    replica: &kukuri_core::ReplicaId,
    topic: &str,
    text: &str,
    at: i64,
) -> Result<String> {
    use kukuri_core::{
        KukuriKeys, ObjectVisibility, PayloadRef, TopicId,
        build_post_envelope_with_payload_in_channel, sign_envelope_json_at,
    };
    let keys = KukuriKeys::generate();
    let draft = build_post_envelope_with_payload_in_channel(
        &keys,
        &TopicId::new(topic),
        PayloadRef::InlineText { text: text.into() },
        vec![],
        vec![],
        None,
        ObjectVisibility::Public,
        None,
        vec![],
    )?;
    let envelope = sign_envelope_json_at(
        &keys,
        draft.kind,
        draft.tags,
        &serde_json::from_str::<serde_json::Value>(&draft.content)?,
        at,
    )?;
    let object = envelope.to_post_object()?.expect("post");
    let id = object.object_id.as_str().to_string();
    for (suffix, value) in [
        ("state", serde_json::to_value(object)?),
        ("envelope", serde_json::to_value(envelope)?),
    ] {
        docs.apply_doc_op(
            replica,
            DocOp::SetJson {
                key: format!("objects/{id}/{suffix}"),
                value,
            },
        )
        .await?;
    }
    Ok(id)
}

#[tokio::test]
async fn moving_to_bucket_replicas_does_not_deindex_the_still_supported_logical_scope() -> Result<()>
{
    let Some(admin) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        eprintln!("skipping Postgres contract; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(&admin, "cn_bucket_scope").await?;
    let pool = connect_postgres(&database.database_url).await?;
    initialize_database(&pool).await?;
    let topic = "bucket-transition";
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, topic).await?;
    let docs = Arc::new(MemoryDocsSync::default());
    let (participant, projection) = participant(docs.clone(), pool.clone())?;
    let now = chrono::Utc::now().timestamp();
    let legacy = ScopeReplica::from_scope(IndexScopeKind::PublicTopic, topic);
    let old_id = post(
        &docs,
        &legacy.replica_id,
        topic,
        "legacy post",
        now - 3 * 86_400,
    )
    .await?;
    assert_eq!(participant.ingest_scope(&legacy).await?.indexed, 1);
    // process再起動の境界に相当する。保存済みの索引とdocs entryは残し、旧syncだけを閉じる。
    participant.stop_replica(&legacy).await?;
    let current = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.into(),
        },
        TimeBucket::from_unix_seconds(now)?,
    )?;
    let new_id = post(&docs, &current.replica_id(), topic, "bucket post", now).await?;
    let participant = participant.with_public_replica_mode(PublicReplicaReadMode::TimeBucketV1);
    let desired = participant.desired_scopes_at(now).await?;
    assert_eq!(desired.len(), 2);
    assert!(
        desired
            .iter()
            .all(|scope| scope.replica_id != legacy.replica_id)
    );
    let state = Arc::new(IndexerRuntimeState::default());
    let handle = IndexerWorker::new(
        Arc::new(participant),
        docs,
        state.clone(),
        WorkerConfig::default(),
    )
    .spawn();
    let completed = tokio::time::timeout(Duration::from_secs(30), async {
        while state.snapshot().last_sync_at.is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    handle.shutdown().await;
    let old_kept = projection
        .contains_object(IndexScopeKind::PublicTopic, topic, &old_id)
        .await?;
    let new_indexed = projection
        .contains_object(IndexScopeKind::PublicTopic, topic, &new_id)
        .await?;
    pool.close().await;
    database.cleanup().await?;
    completed.expect("bucket ingest pass completed");
    assert!(
        old_kept,
        "changing physical replicas must not remove an authorized logical scope"
    );
    assert!(new_indexed);
    assert_eq!(state.snapshot().deindexed, 0);
    Ok(())
}
