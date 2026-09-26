//! Issue #741: `PayloadRef::BlobText` 本文の取得・検証・検索投影 contract。

use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use kukuri_blob_service::{BlobService, BlobStatus, MemoryBlobService, StoredBlob};
use kukuri_cn_core::{IndexScopeKind, MemoryIndexEntryStore};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::projection::{IndexProjection, MemoryIndexProjection};
use kukuri_cn_safety::{MockSafetyProvider, ModerationEventSigner};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{
    MemorySafetyArtifactStore, SafetyOrchestrator, SafetyScanService,
    Secp256k1ModerationEventSigner,
};
use kukuri_core::{
    KukuriKeys, ObjectVisibility, PayloadRef, ReplicaId, TopicId, blob_hash,
    build_post_envelope_with_payload,
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
) -> (IngestPipeline, Arc<MemoryIndexEntryStore>) {
    let (service, store) = allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(store));
    let pipeline = IngestPipeline::new(docs.clone(), service, entries.clone(), projection.clone());
    (pipeline, entries)
}

async fn persist_blob_text_post(
    docs: &MemoryDocsSync,
    blobs: &MemoryBlobService,
    replica: &ReplicaId,
    topic: &TopicId,
    body: &str,
) -> String {
    let stored = blobs
        .put_blob(body.as_bytes().to_vec(), "text/markdown")
        .await
        .expect("store body blob");
    persist_blob_text_ref(
        docs,
        replica,
        topic,
        PayloadRef::BlobText {
            hash: stored.hash,
            mime: stored.mime,
            bytes: stored.bytes,
        },
    )
    .await
}

async fn persist_blob_text_ref(
    docs: &MemoryDocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    payload_ref: PayloadRef,
) -> String {
    persist_blob_text_refs(docs, replica, topic, payload_ref.clone(), payload_ref).await
}

async fn persist_blob_text_refs(
    docs: &MemoryDocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    envelope_payload_ref: PayloadRef,
    state_payload_ref: PayloadRef,
) -> String {
    let keys = KukuriKeys::generate();
    let envelope = build_post_envelope_with_payload(
        &keys,
        topic,
        envelope_payload_ref,
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
    )
    .expect("blob text envelope");
    let mut object = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object present");
    object.payload_ref = state_payload_ref;
    let object_id = object.object_id.as_str().to_string();
    docs.open_replica(replica).await.expect("open");
    docs.apply_doc_op(
        replica,
        DocOp::SetJson {
            key: stable_key("objects", &format!("{object_id}/state")),
            value: serde_json::to_value(&object).expect("state json"),
        },
    )
    .await
    .expect("state op");
    docs.apply_doc_op(
        replica,
        DocOp::SetJson {
            key: stable_key("objects", &format!("{object_id}/envelope")),
            value: serde_json::to_value(&envelope).expect("envelope json"),
        },
    )
    .await
    .expect("envelope op");
    docs.apply_doc_op(
        replica,
        DocOp::SetJson {
            key: stable_key(
                "indexes/timeline",
                &format!(
                    "{}/{object_id}",
                    kukuri_core::timeline_sort_key(object.created_at, &object.object_id)
                ),
            ),
            value: serde_json::json!({ "object_id": object_id }),
        },
    )
    .await
    .expect("timeline op");
    object_id
}

/// 本文の一時取得の失敗の種類。
#[derive(Clone, Copy, Debug)]
enum FetchFailure {
    /// どのピアからも取得できない（`Ok(None)`）。
    Missing,
    /// 取得処理そのものの失敗（`Err`）。
    Error,
}

#[derive(Default)]
struct SwitchableBlobService {
    body: Mutex<Option<Vec<u8>>>,
    failure: Mutex<Option<FetchFailure>>,
}

impl SwitchableBlobService {
    fn with_body(body: Vec<u8>) -> Self {
        Self {
            body: Mutex::new(Some(body)),
            failure: Mutex::new(None),
        }
    }

    fn set_body(&self, body: Option<Vec<u8>>) {
        *self.body.lock().expect("blob body mutex") = body;
    }

    fn set_failure(&self, failure: Option<FetchFailure>) {
        *self.failure.lock().expect("blob failure mutex") = failure;
    }
}

#[async_trait]
impl BlobService for SwitchableBlobService {
    async fn fetch_blob_ephemeral_bounded(
        &self,
        _hash: &kukuri_core::BlobHash,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>> {
        match *self.failure.lock().expect("blob failure mutex") {
            Some(FetchFailure::Missing) => return Ok(None),
            Some(FetchFailure::Error) => anyhow::bail!("simulated blob fetch failure"),
            None => {}
        }
        let body = self.body.lock().expect("blob body mutex");
        if body
            .as_ref()
            .is_some_and(|bytes| bytes.len() as u64 > max_bytes)
        {
            return Err(kukuri_iroh_node::remote_fetch::BlobTooLarge { limit: max_bytes }.into());
        }
        Ok(body.clone())
    }
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        let hash = blob_hash(&data);
        let bytes = data.len() as u64;
        self.set_body(Some(data));
        Ok(StoredBlob {
            hash,
            mime: mime.to_string(),
            bytes,
        })
    }

    async fn fetch_blob(&self, _hash: &kukuri_core::BlobHash) -> Result<Option<Vec<u8>>> {
        Ok(self.body.lock().expect("blob body mutex").clone())
    }

    async fn pin_blob(&self, _hash: &kukuri_core::BlobHash) -> Result<()> {
        Ok(())
    }

    async fn blob_status(&self, _hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        Ok(if self.body.lock().expect("blob body mutex").is_some() {
            BlobStatus::Available
        } else {
            BlobStatus::Missing
        })
    }

    async fn local_blob_status(&self, hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        self.blob_status(hash).await
    }

    async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn blob_text_post_body_is_searchable_in_scope_and_across_supported_entries() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_blob_text_post(
        &docs,
        blobs.as_ref(),
        &replica,
        &topic,
        "Community Index CUA E2E テスト",
    )
    .await;

    let (pipeline, entries) = pipeline_with(&docs, &projection);
    let summary = pipeline
        .with_blob_service(blobs)
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 1);
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    let stored = projection
        .entries_in_scope(IndexScopeKind::PublicTopic, "rust")
        .await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].text, "Community Index CUA E2E テスト");

    for query in ["CUA", "テスト"] {
        let scoped = kukuri_cn_indexer::query::IndexQuery::search_scope(
            projection.as_ref(),
            IndexScopeKind::PublicTopic,
            "rust",
            query,
            10,
        )
        .await?;
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].object_id, object_id);

        let all = kukuri_cn_indexer::query::IndexQuery::search_all(projection.as_ref(), query, 10)
            .await?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].object_id, object_id);
    }
    Ok(())
}

async fn assert_blob_text_rejected(
    payload_ref: PayloadRef,
    blob_service: Option<Arc<dyn BlobService>>,
) -> Result<()> {
    assert_blob_text_refs_rejected(payload_ref.clone(), payload_ref, blob_service).await
}

async fn assert_blob_text_refs_rejected(
    envelope_payload_ref: PayloadRef,
    state_payload_ref: PayloadRef,
    blob_service: Option<Arc<dyn BlobService>>,
) -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_blob_text_refs(
        &docs,
        &replica,
        &topic,
        envelope_payload_ref,
        state_payload_ref,
    )
    .await;
    let (pipeline, entries) = pipeline_with(&docs, &projection);
    let pipeline = match blob_service {
        Some(blob_service) => pipeline.with_blob_service(blob_service),
        None => pipeline,
    };

    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert_eq!(summary.scanned, 1);
    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn blob_text_validation_failures_are_not_indexed() -> Result<()> {
    let valid = b"body".to_vec();
    let valid_ref = PayloadRef::BlobText {
        hash: blob_hash(&valid),
        mime: "text/markdown".to_string(),
        bytes: valid.len() as u64,
    };

    assert_blob_text_rejected(valid_ref.clone(), None).await?;
    assert_blob_text_rejected(
        valid_ref.clone(),
        Some(Arc::new(SwitchableBlobService::default())),
    )
    .await?;
    assert_blob_text_rejected(
        valid_ref.clone(),
        Some(Arc::new(SwitchableBlobService::with_body(b"evil".to_vec()))),
    )
    .await?;
    assert_blob_text_rejected(
        valid_ref.clone(),
        Some(Arc::new(SwitchableBlobService::with_body(b"x".to_vec()))),
    )
    .await?;

    let mismatched_state_ref = PayloadRef::BlobText {
        hash: blob_hash(&valid),
        mime: "text/plain".to_string(),
        bytes: valid.len() as u64,
    };
    assert_blob_text_refs_rejected(
        valid_ref,
        mismatched_state_ref,
        Some(Arc::new(SwitchableBlobService::with_body(valid))),
    )
    .await?;

    let invalid_utf8 = vec![0xff, 0xfe];
    assert_blob_text_rejected(
        PayloadRef::BlobText {
            hash: blob_hash(&invalid_utf8),
            mime: "text/markdown".to_string(),
            bytes: invalid_utf8.len() as u64,
        },
        Some(Arc::new(SwitchableBlobService::with_body(invalid_utf8))),
    )
    .await?;

    let oversized = vec![b'a'; 40_001];
    assert_blob_text_rejected(
        PayloadRef::BlobText {
            hash: blob_hash(&oversized),
            mime: "text/markdown".to_string(),
            bytes: oversized.len() as u64,
        },
        Some(Arc::new(SwitchableBlobService::with_body(oversized))),
    )
    .await?;
    Ok(())
}

/// 本文の一時取得が失敗しても（未取得・取得エラー）索引済み entry は保持し、
/// 取得できた本文が検証に失敗した場合（確定した理由）は de-index する（#1090）。
#[tokio::test]
async fn blob_text_fetch_failure_keeps_an_existing_entry_until_validation_fails() -> Result<()> {
    let body = b"Community Index body".to_vec();
    let payload_ref = PayloadRef::BlobText {
        hash: blob_hash(&body),
        mime: "text/markdown".to_string(),
        bytes: body.len() as u64,
    };
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_blob_text_ref(&docs, &replica, &topic, payload_ref).await;
    let blobs = Arc::new(SwitchableBlobService::with_body(body));
    let (pipeline, entries) = pipeline_with(&docs, &projection);
    let pipeline = pipeline.with_blob_service(blobs.clone());

    let initial = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert_eq!(initial.indexed, 1);
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );

    for failure in [FetchFailure::Missing, FetchFailure::Error] {
        blobs.set_failure(Some(failure));
        let retry = pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
            .await?;
        assert_eq!(retry.indexed, 0, "{failure:?}");
        assert_eq!(retry.deindexed, 0, "{failure:?}");
        assert!(
            entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id),
            "{failure:?}: a transient fetch failure must keep the truth entry"
        );
        assert!(
            projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
                .await?,
            "{failure:?}: a transient fetch failure must keep the projection"
        );
    }

    // 取得できた本文が hash と一致しない場合は確定した理由として消す。
    blobs.set_failure(None);
    blobs.set_body(Some(b"Community Index evil".to_vec()));
    let retry = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert_eq!(retry.indexed, 0);
    assert_eq!(retry.skipped_non_allow, 1);
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    Ok(())
}
