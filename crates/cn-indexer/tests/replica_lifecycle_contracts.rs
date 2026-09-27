//! #1293: bucket の読取りは、署名済みの作成時刻と scope が一致する投稿だけを索引する。

use std::sync::Arc;

use anyhow::Result;
use kukuri_cn_core::{IndexScopeKind, MemoryIndexEntryStore};
use kukuri_cn_indexer::{IndexProjection, IngestPipeline, MemoryIndexProjection};
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

fn pipeline(docs: Arc<dyn DocsSync>) -> Result<(IngestPipeline, Arc<MemoryIndexProjection>)> {
    pipeline_with_provider(docs, Arc::new(MockSafetyProvider::known_csam("mock")))
}

fn pipeline_with_provider(
    docs: Arc<dyn DocsSync>,
    provider: Arc<dyn kukuri_cn_safety::provider::SafetyProvider>,
) -> Result<(IngestPipeline, Arc<MemoryIndexProjection>)> {
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
    Ok((
        IngestPipeline::new(docs, service, entries, projection.clone()),
        projection,
    ))
}

fn topic_bucket(topic: &str, day: u64) -> Result<kukuri_core::ReplicaId> {
    Ok(BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.into(),
        },
        TimeBucket::from_index(day)?,
    )?
    .replica_id())
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
    let (pipeline, _) = pipeline(docs.clone())?;
    for replica_id in [
        topic_replica_id("other"),
        topic_bucket("other", 1)?,
        BucketReplica::new(
            BucketScope::Author {
                author_pubkey: "author".into(),
            },
            TimeBucket::from_index(1)?,
        )?
        .replica_id(),
    ] {
        assert!(
            pipeline
                .ingest_changed_keys(IndexScopeKind::PublicTopic, "expected", &replica_id, &[])
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
    let (pipeline, projection) = pipeline(docs.clone())?;
    let replica = topic_bucket("time-check", 2)?;
    let id = post(&docs, &replica, "time-check", "wrong bucket", 86_400).await?;
    let result = pipeline
        .ingest_changed_keys(IndexScopeKind::PublicTopic, "time-check", &replica, &[])
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
        let (pipeline, projection) = pipeline(docs.clone())?;
        let topic = "copy-check";
        let proper = topic_bucket(topic, 1)?;
        let wrong = topic_bucket(topic, 2)?;
        let id = post(&docs, &proper, topic, "original", 86_400).await?;
        let ingest = |replica| {
            pipeline.ingest_changed_keys(IndexScopeKind::PublicTopic, topic, replica, &[])
        };
        assert_eq!(ingest(&proper).await?.indexed, 1);
        for record in docs.query_replica(&proper, DocQuery::All).await? {
            let mut value: serde_json::Value = serde_json::from_slice(&record.value)?;
            if forged_delete && record.key.ends_with("/state") {
                value["status"] = serde_json::json!("deleted");
            }
            docs.apply_doc_op(
                &wrong,
                DocOp::SetJson {
                    key: record.key,
                    value,
                },
            )
            .await?;
        }
        assert_eq!(ingest(&wrong).await?.indexed, 0);
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
    let (pipeline, projection) = pipeline(docs.clone())?;
    let topic = "signed-header";
    let replica = topic_bucket(topic, 1)?;
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
        pipeline
            .ingest_changed_keys(IndexScopeKind::PublicTopic, topic, &replica, &[])
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
    let (pipeline, projection) = pipeline_with_provider(docs.clone(), provider.clone())?;
    let topic = "marker-reuse";
    let replica = topic_bucket(topic, 1)?;
    let id = post(&docs, &replica, topic, "authentic text", 86_400).await?;
    let ingest = || pipeline.ingest_changed_keys(IndexScopeKind::PublicTopic, topic, &replica, &[]);
    assert_eq!(ingest().await?.scans_fresh, 1);
    docs.apply_doc_op(
        &replica,
        DocOp::SetBytes {
            key: format!("objects/{id}/state"),
            value: b"untrusted replacement marker".to_vec(),
        },
    )
    .await?;
    let result = ingest().await?;
    assert_eq!(
        result.scans_fresh, 0,
        "unsigned marker must not trigger a provider call"
    );
    assert_eq!(result.scans_reused, 1);
    assert_eq!(provider.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, topic, &id)
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
    let sort_key = kukuri_core::timeline_sort_key(object.created_at, &object.object_id);
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
    docs.apply_doc_op(
        replica,
        DocOp::SetJson {
            key: format!("indexes/timeline/{sort_key}/{id}"),
            value: serde_json::json!({ "object_id": id }),
        },
    )
    .await?;
    Ok(id)
}
