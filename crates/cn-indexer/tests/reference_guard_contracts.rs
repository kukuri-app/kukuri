mod ingest_support;
use anyhow::Result;
use async_trait::async_trait;
use ingest_support::*;
use kukuri_cn_core::IndexScopeKind;
use kukuri_cn_indexer::projection::MemoryIndexProjection;
use kukuri_cn_safety::{
    ProviderScanRequest, ProviderScanResult, SafetyCategory, SafetyLabel, SafetyPolicy,
    SafetyProvider, SafetyProviderCapability, ScanError, SubjectKind,
};
use kukuri_cn_safety_runtime::MemorySafetyArtifactStore;
use kukuri_core::TopicId;
use kukuri_docs_sync::{DocOp, DocsSync, MemoryDocsSync, topic_replica_id};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

#[tokio::test]
async fn cached_verdict_does_not_authorize_forged_materialized_body() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let replica = topic_replica_id("rust");
    let topic = TopicId::new("rust");
    let (id, _, _, mut state) =
        persist_post_with_source(&docs, &replica, &topic, "signed body").await;
    let original_state = state.clone();
    let artifacts = Arc::new(MemorySafetyArtifactStore::new());
    let provider = RecordingProvider::new();
    let service = service_with_store(
        artifacts.clone(),
        SafetyPolicy::public_node_default(),
        vec![provider.clone()],
    );
    let (pipeline, entries, _) = pipeline_with(&docs, &projection, (service, artifacts));
    assert_eq!(
        pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
            .await?
            .indexed,
        1
    );
    state["payload_ref"] = serde_json::to_value(kukuri_core::PayloadRef::InlineText {
        text: "forged body".into(),
    })?;
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: format!("objects/{id}/state"),
            value: state,
        },
    )
    .await?;
    assert_eq!(
        pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
            .await?
            .indexed,
        0
    );
    assert_eq!(
        provider.subjects().len(),
        1,
        "signature mismatch must not reach the provider"
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &id));
    // A corrupt replacement must also remove the prior index row, using the key's identity.
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: format!("objects/{id}/state"),
            value: original_state,
        },
    )
    .await?;
    pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &id));
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: format!("objects/{id}/state"),
            value: serde_json::json!({"payload_ref": false}),
        },
    )
    .await?;
    pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &id));
    Ok(())
}

#[tokio::test]
async fn unsupported_scope_cannot_reuse_or_scan_content() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let replica = topic_replica_id("rust");
    let topic = TopicId::new("rust");
    let id = persist_post(&docs, &replica, &topic, "body").await;
    let artifacts = Arc::new(MemorySafetyArtifactStore::new());
    let provider = RecordingProvider::new();
    let service = service_with_store(
        artifacts.clone(),
        SafetyPolicy::public_node_default(),
        vec![provider.clone()],
    );
    let (pipeline, entries, _) = pipeline_with(&docs, &projection, (service, artifacts));
    pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    entries.set_scope_supported(IndexScopeKind::PublicTopic, "rust", false);
    assert_eq!(
        pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
            .await?
            .indexed,
        0
    );
    assert_eq!(provider.subjects().len(), 1);
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &id));
    entries.set_scope_supported(IndexScopeKind::PublicTopic, "rust", true);
    assert_eq!(
        pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
            .await?
            .indexed,
        1
    );
    assert_eq!(provider.subjects().len(), 1);
    Ok(())
}

struct PausingProvider {
    started: Notify,
    release: Notify,
    calls: AtomicUsize,
}
#[async_trait]
impl SafetyProvider for PausingProvider {
    fn name(&self) -> &str {
        "pure-pausing-provider"
    }
    fn capabilities(&self) -> &[SafetyProviderCapability] {
        &[SafetyProviderCapability::GeneralMediaModeration]
    }
    fn supports_content_reuse(&self) -> bool {
        true
    }
    async fn scan(&self, _: &ProviderScanRequest) -> Result<ProviderScanResult, ScanError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        self.release.notified().await;
        let mut result = ProviderScanResult::completed(
            self.name(),
            SafetyProviderCapability::GeneralMediaModeration,
        );
        result.score = Some(80);
        result.labels = vec![SafetyLabel::new(SafetyCategory::Nsfw).with_confidence(80)];
        Ok(result)
    }
}

#[tokio::test]
async fn revocation_while_scanning_blocks_association_but_keeps_valid_other_references()
-> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let replica = topic_replica_id("rust");
    let topic = TopicId::new("rust");
    let first = persist_post(&docs, &replica, &topic, "same body").await;
    let artifacts = Arc::new(MemorySafetyArtifactStore::new());
    let provider = Arc::new(PausingProvider {
        started: Notify::new(),
        release: Notify::new(),
        calls: AtomicUsize::new(0),
    });
    let mut policy = SafetyPolicy::public_node_default();
    policy.require_known_csam = false;
    let service = service_with_store(artifacts.clone(), policy, vec![provider.clone()]);
    let (pipeline, entries, _) = pipeline_with(&docs, &projection, (service, artifacts.clone()));
    let ingest = pipeline.ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica);
    let revoke = async {
        provider.started.notified().await;
        entries.prevent_subject(&first);
        provider.release.notify_one();
    };
    let (result, ()) = tokio::join!(ingest, revoke);
    assert_eq!(result?.indexed, 0);
    assert!(
        artifacts
            .stored_verdict_for(SubjectKind::Post, &first)
            .is_none()
    );
    assert!(artifacts.signals().is_empty());
    let second = persist_post(&docs, &replica, &topic, "same body").await;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        pipeline.ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica),
    )
    .await??;
    assert_eq!(result.indexed, 1);
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &second));
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        1,
        "completed content may serve a separate valid reference"
    );
    assert!(
        artifacts
            .signals()
            .iter()
            .all(|(_, signal)| signal.target_id == second)
    );
    Ok(())
}
