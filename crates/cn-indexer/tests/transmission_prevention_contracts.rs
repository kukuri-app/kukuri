//! Signed author-withdrawal and node-local transmission-prevention ingestion contracts.

use std::sync::Arc;

use anyhow::Result;
use kukuri_cn_core::{IndexScopeKind, MemoryIndexEntryStore};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::participant::ScopeReplica;
use kukuri_cn_indexer::projection::MemoryIndexProjection;
use kukuri_cn_safety::provider::SubjectKind;
use kukuri_cn_safety::{MockSafetyProvider, ModerationEventSigner};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{
    MemorySafetyArtifactStore, SafetyOrchestrator, SafetyScanService,
    Secp256k1ModerationEventSigner,
};
use kukuri_core::{
    KukuriEnvelope, KukuriKeys, PostWithdrawalReason, TopicId, WithdrawalReasonVisibility,
    build_post_envelope, build_post_withdrawal_envelope,
};
use kukuri_docs_sync::{DocOp, DocsSync, MemoryDocsSync, stable_key, topic_replica_id};

const TEST_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn allow_service() -> (Arc<SafetyScanService>, Arc<MemorySafetyArtifactStore>) {
    let signer = Secp256k1ModerationEventSigner::from_secret(TEST_SECRET).expect("signer");
    let issuer = signer.issuer_node_id().to_string();
    let store = Arc::new(MemorySafetyArtifactStore::new());
    let orchestrator = SafetyOrchestrator::builder(
        &issuer,
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .provider(Arc::new(MockSafetyProvider::known_csam("mock-known-csam")))
    .build()
    .expect("orchestrator");
    let service = SafetyScanService::builder(Arc::new(orchestrator), store.clone())
        .signer(Arc::new(signer))
        .build()
        .expect("service");
    (Arc::new(service), store)
}

fn pipeline_with(
    docs: &Arc<MemoryDocsSync>,
    projection: &Arc<MemoryIndexProjection>,
) -> (
    IngestPipeline,
    Arc<MemoryIndexEntryStore>,
    Arc<MemorySafetyArtifactStore>,
) {
    let (service, store) = allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(store.clone()));
    let pipeline = IngestPipeline::new(docs.clone(), service, entries.clone(), projection.clone());
    (pipeline, entries, store)
}

async fn persist_post(
    docs: &MemoryDocsSync,
    topic: &TopicId,
    body: &str,
) -> Result<(KukuriKeys, KukuriEnvelope, String)> {
    let keys = KukuriKeys::generate();
    let envelope = build_post_envelope(&keys, topic, body, None)?;
    let object = envelope.to_post_object()?.expect("post object");
    let object_id = object.object_id.as_str().to_string();
    let replica = topic_replica_id(topic.as_str());
    docs.open_replica(&replica).await?;
    for (key, value) in [
        (
            stable_key("objects", &format!("{object_id}/state")),
            serde_json::to_value(&object)?,
        ),
        (
            stable_key("objects", &format!("{object_id}/envelope")),
            serde_json::to_value(&envelope)?,
        ),
        (
            stable_key(
                "indexes/timeline",
                &format!(
                    "{}/{object_id}",
                    kukuri_core::timeline_sort_key(object.created_at, &object.object_id)
                ),
            ),
            serde_json::json!({ "object_id": object_id }),
        ),
    ] {
        docs.apply_doc_op(&replica, DocOp::SetJson { key, value })
            .await?;
    }
    Ok((keys, envelope, object_id))
}

#[tokio::test]
async fn active_transmission_prevention_wins_before_scan_and_reingest() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id(topic.as_str());
    let (_, _, object_id) = persist_post(&docs, &topic, "must not be scanned").await?;
    let (pipeline, entries, store) = pipeline_with(&docs, &projection);
    entries.prevent_subject(object_id.clone());

    let scope = ScopeReplica::from_scope(IndexScopeKind::PublicTopic, topic.as_str());
    let first = pipeline
        .ingest_recent_scope(scope.kind, &scope.id, &replica)
        .await?;
    let second = pipeline
        .ingest_recent_scope(scope.kind, &scope.id, &replica)
        .await?;

    assert_eq!(first.deindexed, 1);
    assert_eq!(second.deindexed, 1);
    assert!(!entries.contains(IndexScopeKind::PublicTopic, topic.as_str(), &object_id));
    assert!(
        store.verdict_for(SubjectKind::Post, &object_id).is_none(),
        "legal gate must run before scan"
    );
    Ok(())
}

#[tokio::test]
async fn verified_author_withdrawal_deindexes_and_never_reappears() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id(topic.as_str());
    let (keys, envelope, object_id) = persist_post(&docs, &topic, "withdraw me").await?;
    let withdrawal = build_post_withdrawal_envelope(
        &keys,
        &envelope,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )?;
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: stable_key("withdrawals", &format!("{object_id}/state")),
            value: serde_json::to_value(withdrawal)?,
        },
    )
    .await?;

    let (pipeline, entries, store) = pipeline_with(&docs, &projection);
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, topic.as_str(), &replica)
        .await?;
    assert_eq!(summary.deindexed, 1);
    assert!(!entries.contains(IndexScopeKind::PublicTopic, topic.as_str(), &object_id));
    assert!(
        store.verdict_for(SubjectKind::Post, &object_id).is_none(),
        "withdrawal must run before scan"
    );
    Ok(())
}
