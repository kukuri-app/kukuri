//! ingest 系 contract test の共有 fixture（`ingestion_contracts.rs` / `verdict_reuse_contracts.rs`）。
//!
//! docs 同期・投影・真実源はメモリ内実装、safety provider は mock / 記録用 double で、
//! `IngestPipeline` を DB 非依存に駆動する。
#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use kukuri_cn_core::MemoryIndexEntryStore;
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::projection::MemoryIndexProjection;
use kukuri_cn_safety::provider::{ProviderScanRequest, ProviderScanResult, ScanError, ScanOutcome};
use kukuri_cn_safety::{
    MockSafetyProvider, ModerationEventSigner, SafetyPolicy, SafetyProvider,
    SafetyProviderCapability,
};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{
    MemorySafetyArtifactStore, SafetyOrchestrator, SafetyScanService,
    Secp256k1ModerationEventSigner,
};
use kukuri_core::{
    KukuriEnvelope, KukuriKeys, KukuriMediaManifestV1, KukuriPostObjectV1, MediaManifestItem,
    ObjectVisibility, PayloadRef, ReplicaId, TopicId, blob_hash, build_media_manifest_envelope,
    build_post_envelope, build_post_envelope_with_payload, timeline_sort_key,
};
use kukuri_docs_sync::{DocOp, DocsSync, MemoryDocsSync, stable_key};

pub const TEST_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";

pub fn service_with(
    provider: MockSafetyProvider,
) -> (Arc<SafetyScanService>, Arc<MemorySafetyArtifactStore>) {
    let signer = Secp256k1ModerationEventSigner::from_secret(TEST_SECRET).expect("signer");
    let issuer = signer.issuer_node_id().to_string();
    let store = Arc::new(MemorySafetyArtifactStore::new());
    let orchestrator = SafetyOrchestrator::builder(
        &issuer,
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .provider(Arc::new(provider))
    .build()
    .expect("orchestrator");
    let service = SafetyScanService::builder(Arc::new(orchestrator), store.clone())
        .signer(Arc::new(signer))
        .build()
        .expect("service");
    (Arc::new(service), store)
}

/// mock provider で allow を返す service（known CSAM = NoKnownMatch、脅威スコア無し）。
///
/// `public_node_default` policy は known CSAM provider を必須とするため、allow を得るには
/// `KnownCsamHashMatch` provider が `NoKnownMatch` を返す必要がある。
pub fn allow_service() -> (Arc<SafetyScanService>, Arc<MemorySafetyArtifactStore>) {
    service_with(MockSafetyProvider::known_csam("mock-known-csam"))
}

/// mock provider の呼び出し自体が利用不可エラーになる service（fail-closed → hold）。
pub fn provider_unavailable_service() -> (Arc<SafetyScanService>, Arc<MemorySafetyArtifactStore>) {
    service_with(
        MockSafetyProvider::known_csam("mock-known-csam")
            .default_error(ScanError::Unavailable("mock provider down".to_string())),
    )
}

/// 任意の provider 集合と policy で service を組む（store を外から共有できる）。
pub fn service_with_store(
    store: Arc<MemorySafetyArtifactStore>,
    policy: SafetyPolicy,
    providers: Vec<Arc<dyn SafetyProvider>>,
) -> Arc<SafetyScanService> {
    let signer = Secp256k1ModerationEventSigner::from_secret(TEST_SECRET).expect("signer");
    let issuer = signer.issuer_node_id().to_string();
    let mut orchestrator = SafetyOrchestrator::builder(
        &issuer,
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .policy(policy);
    for provider in providers {
        orchestrator = orchestrator.provider(provider);
    }
    let orchestrator = orchestrator.build().expect("orchestrator");
    Arc::new(
        SafetyScanService::builder(Arc::new(orchestrator), store)
            .signer(Arc::new(signer))
            .build()
            .expect("service"),
    )
}

/// pipeline を組む。真実源（メモリ内実装）は scan service と同じ artifact store を参照し、
/// verdict への外部キー相当を成立させる。
pub fn pipeline_with(
    docs: &Arc<MemoryDocsSync>,
    projection: &Arc<MemoryIndexProjection>,
    (service, store): (Arc<SafetyScanService>, Arc<MemorySafetyArtifactStore>),
) -> (
    IngestPipeline,
    Arc<MemoryIndexEntryStore>,
    Arc<MemorySafetyArtifactStore>,
) {
    let entries = Arc::new(MemoryIndexEntryStore::new(store.clone()));
    let pipeline = IngestPipeline::new(docs.clone(), service, entries.clone(), projection.clone());
    (pipeline, entries, store)
}

/// 本文 text の post envelope を共有 replica に実在させる（app-api の persist と同じ key 形状）。
///
/// 返り値は object_id。
pub async fn persist_post(
    docs: &MemoryDocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    body: &str,
) -> String {
    persist_post_with_source(docs, replica, topic, body).await.0
}

/// 実クライアント（app-api `persist_post_object`）が 1 投稿で共有 replica に書く key（#1065）。
///
/// state / envelope に加えて timeline / thread の索引 key も書くため、変更通知には 4 key が届く。
pub fn client_post_keys(object: &KukuriPostObjectV1) -> Vec<String> {
    let object_id = object.object_id.as_str();
    let sort_key = timeline_sort_key(object.created_at, &object.object_id);
    let root_id = object.root.as_ref().map_or(object_id, |root| root.as_str());
    vec![
        stable_key("objects", &format!("{object_id}/state")),
        stable_key("objects", &format!("{object_id}/envelope")),
        stable_key("indexes/timeline", &format!("{sort_key}/{object_id}")),
        stable_key(
            "indexes/thread",
            &format!("{root_id}/{sort_key}/{object_id}"),
        ),
    ]
}

/// 現在の索引窓から読まれるように、投稿の timeline 索引 key を書く（#1221 R5-E。全件取込は撤去済み）。
pub async fn write_timeline_index(
    docs: &MemoryDocsSync,
    replica: &ReplicaId,
    object: &KukuriPostObjectV1,
) {
    let key = client_post_keys(object)[2].clone();
    docs.apply_doc_op(
        replica,
        DocOp::SetJson {
            key,
            value: serde_json::json!({ "object_id": object.object_id.as_str() }),
        },
    )
    .await
    .expect("timeline index op");
}

/// `persist_post` に加えて署名鍵 / envelope / state JSON も返す（撤回・state 変更の再現用）。
pub async fn persist_post_with_source(
    docs: &MemoryDocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    body: &str,
) -> (String, KukuriKeys, KukuriEnvelope, serde_json::Value) {
    let (object_id, keys, envelope, state, _) =
        persist_client_post(docs, replica, topic, body).await;
    (object_id, keys, envelope, state)
}

/// 実クライアントと同じ key 集合で post を書き、書いた key も返す（#1065）。
pub async fn persist_client_post(
    docs: &MemoryDocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    body: &str,
) -> (
    String,
    KukuriKeys,
    KukuriEnvelope,
    serde_json::Value,
    Vec<String>,
) {
    let keys = KukuriKeys::generate();
    let envelope = build_post_envelope(&keys, topic, body, None).expect("envelope");
    let object = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object present");
    let object_id = object.object_id.as_str().to_string();
    let state = serde_json::to_value(&object).expect("state json");
    let written = client_post_keys(&object);
    docs.open_replica(replica).await.expect("open");
    for key in &written {
        let value = if key.ends_with("/state") {
            state.clone()
        } else if key.ends_with("/envelope") {
            serde_json::to_value(&envelope).expect("envelope json")
        } else {
            serde_json::json!({ "object_id": object_id })
        };
        docs.apply_doc_op(
            replica,
            DocOp::SetJson {
                key: key.clone(),
                value,
            },
        )
        .await
        .expect("client post op");
    }
    (object_id, keys, envelope, state, written)
}

/// 受け取った scan request を記録する known-CSAM provider（provider 呼び出し回数の検証用）。
///
/// すべての subject に `NoKnownMatch`（allow 側）を返し、`public_node_default` policy の
/// require_known_csam を満たしつつ request の中身だけを観測する。
pub struct RecordingProvider {
    pub requests: std::sync::Mutex<Vec<ProviderScanRequest>>,
}

impl RecordingProvider {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            requests: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// 記録された request の subject_id 列（順序どおり）。
    pub fn subjects(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("recording provider mutex")
            .iter()
            .filter_map(|request| request.subject_id.clone())
            .collect()
    }
}

impl Default for RecordingProvider {
    fn default() -> Self {
        Self {
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl SafetyProvider for RecordingProvider {
    fn name(&self) -> &str {
        "recording-known-csam"
    }

    fn capabilities(&self) -> &[SafetyProviderCapability] {
        &[SafetyProviderCapability::KnownCsamHashMatch]
    }

    async fn scan(&self, request: &ProviderScanRequest) -> Result<ProviderScanResult, ScanError> {
        self.requests
            .lock()
            .expect("recording provider mutex")
            .push(request.clone());
        let mut result = ProviderScanResult::completed(
            self.name(),
            SafetyProviderCapability::KnownCsamHashMatch,
        );
        result.outcome = ScanOutcome::NoKnownMatch;
        Ok(result)
    }
}

// --- media 参照 post と複数 provider の fixture（`ingestion_contracts.rs` から共有化。#1054） ---

pub const MEDIA_MANIFEST_ID: &str = "media-manifest-test";
/// manifest item の blob 本体（scan 対象 hash はここから導出する）。
pub const MEDIA_BLOB_BYTES: &[u8] = b"tiny-png-bytes";
/// manifest item の thumbnail blob 本体（mime metadata を持たない scan 対象）。
pub const MEDIA_THUMBNAIL_BYTES: &[u8] = b"tiny-thumbnail-bytes";

pub fn media_blob_hash() -> String {
    blob_hash(MEDIA_BLOB_BYTES).as_str().to_string()
}

pub fn media_thumbnail_hash() -> String {
    blob_hash(MEDIA_THUMBNAIL_BYTES).as_str().to_string()
}

/// manifest 参照つき media post を共有 replica に実在させる（#609: manifest は署名済み envelope
/// として `manifests/media/<id>/{state,envelope}` に persist する。app-api と同じ key 形状）。
///
/// `persist_manifest = false` で「post は manifest を参照するが replica に manifest が無い」
/// fail-closed 系の状況を作れる。返り値は object_id。
pub async fn persist_media_post(
    docs: &MemoryDocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    body: &str,
    persist_manifest: bool,
) -> String {
    let keys = KukuriKeys::generate();
    let envelope = build_post_envelope_with_payload(
        &keys,
        topic,
        PayloadRef::InlineText {
            text: body.to_string(),
        },
        Vec::new(),
        vec![MEDIA_MANIFEST_ID.to_string()],
        None,
        ObjectVisibility::Public,
    )
    .expect("envelope");
    let object = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object present");
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
    write_timeline_index(docs, replica, &object).await;

    if persist_manifest {
        let manifest = KukuriMediaManifestV1 {
            manifest_id: MEDIA_MANIFEST_ID.to_string(),
            owner_pubkey: keys.public_key(),
            created_at: 1_719_900_000,
            items: vec![MediaManifestItem {
                blob_hash: blob_hash(MEDIA_BLOB_BYTES),
                mime: "image/png".to_string(),
                size: MEDIA_BLOB_BYTES.len() as u64,
                width: None,
                height: None,
                duration_ms: None,
                codec: None,
                thumbnail_blob_hash: Some(blob_hash(MEDIA_THUMBNAIL_BYTES)),
            }],
        };
        let manifest_envelope =
            build_media_manifest_envelope(&keys, topic, &manifest).expect("manifest envelope");
        docs.apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("manifests/media", &format!("{MEDIA_MANIFEST_ID}/state")),
                value: serde_json::to_value(&manifest).expect("manifest json"),
            },
        )
        .await
        .expect("manifest state op");
        docs.apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("manifests/media", &format!("{MEDIA_MANIFEST_ID}/envelope")),
                value: serde_json::to_value(&manifest_envelope).expect("manifest envelope json"),
            },
        )
        .await
        .expect("manifest envelope op");
    }
    object_id
}

pub fn service_with_providers(
    providers: Vec<MockSafetyProvider>,
) -> (Arc<SafetyScanService>, Arc<MemorySafetyArtifactStore>) {
    let signer = Secp256k1ModerationEventSigner::from_secret(TEST_SECRET).expect("signer");
    let issuer = signer.issuer_node_id().to_string();
    let store = Arc::new(MemorySafetyArtifactStore::new());
    let mut orchestrator = SafetyOrchestrator::builder(
        &issuer,
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    );
    for provider in providers {
        orchestrator = orchestrator.provider(Arc::new(provider));
    }
    let orchestrator = orchestrator.build().expect("orchestrator");
    let service = SafetyScanService::builder(Arc::new(orchestrator), store.clone())
        .signer(Arc::new(signer))
        .build()
        .expect("service");
    (Arc::new(service), store)
}
