//! #413 ingestion 系 contract test（ADR 0025 §2.2 / §2.5 / §6）。
//!
//! in-memory な `DocsSync` + `MemoryIndexProjection` + mock safety provider で ingest pipeline と
//! relay 起動 gate を駆動し、ADR 0025 の ingestion 系 contract を検証する。DB を要さない範囲を対象に
//! するため、supported set の scope ゲート自体（Postgres 依存）は cn-core 側の DB テストに委ね、ここでは
//! pipeline の不変条件（共有 replica 実在のみ / fail-closed / de-index）と relay gate を固定する。

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use kukuri_blob_service::MemoryBlobService;
use kukuri_cn_core::IndexScopeKind;
use kukuri_cn_indexer::PostFetchScheduler;
use kukuri_cn_indexer::config::{MediaFetchConfig, RelayConfig};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::media_fetcher::BlobMediaFetcher;
use kukuri_cn_indexer::participant::ScopeReplica;
use kukuri_cn_indexer::projection::{IndexProjection, MemoryIndexProjection};
use kukuri_cn_indexer::state::IndexerRuntimeState;
use kukuri_cn_safety::provider::{
    MediaFetcher, ProviderScanRequest, ProviderScanResult, ScanError, ScanOutcome, SubjectKind,
};
use kukuri_cn_safety::{
    MockSafetyProvider, ModerationEventSigner, RiskSignalTarget, SafetyCategory, SafetyProvider,
    SafetyProviderCapability,
};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{MemorySafetyArtifactStore, SafetyScanService};
use kukuri_cn_safety_runtime::{
    SafetyOrchestrator, Secp256k1ModerationEventSigner, verify_signed_event,
};
use kukuri_core::TopicId;
use kukuri_docs_sync::{DocOp, DocQuery, DocsSync, MemoryDocsSync, stable_key, topic_replica_id};
use tokio::sync::Barrier;

mod ingest_support;
use ingest_support::*;

/// mock provider が scan 失敗を返す service（fail-closed のテスト用）。
fn scan_failed_service() -> (Arc<SafetyScanService>, Arc<MemorySafetyArtifactStore>) {
    service_with(MockSafetyProvider::known_csam("mock-known-csam").default_failed())
}

/// known CSAM hash match を返す service（exclude のテスト用）。
fn known_csam_service(post_id: &str) -> (Arc<SafetyScanService>, Arc<MemorySafetyArtifactStore>) {
    service_with(MockSafetyProvider::known_csam("mock-known-csam").with_known_hash_match(post_id))
}

struct BarrierProvider {
    barrier: Barrier,
}

#[async_trait]
impl SafetyProvider for BarrierProvider {
    fn name(&self) -> &str {
        "barrier-known-csam"
    }

    fn capabilities(&self) -> &[SafetyProviderCapability] {
        &[SafetyProviderCapability::KnownCsamHashMatch]
    }

    async fn scan(&self, _: &ProviderScanRequest) -> Result<ProviderScanResult, ScanError> {
        self.barrier.wait().await;
        let mut result = ProviderScanResult::completed(
            self.name(),
            SafetyProviderCapability::KnownCsamHashMatch,
        );
        result.outcome = ScanOutcome::NoKnownMatch;
        Ok(result)
    }
}

#[tokio::test]
async fn independent_posts_are_ingested_concurrently_within_the_configured_bound() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    persist_post(&docs, &replica, &topic, "first concurrent post").await;
    persist_post(&docs, &replica, &topic, "second concurrent post").await;

    let provider = Arc::new(BarrierProvider {
        barrier: Barrier::new(2),
    });
    let signer = Secp256k1ModerationEventSigner::from_secret(TEST_SECRET).expect("signer");
    let issuer = signer.issuer_node_id().to_string();
    let store = Arc::new(MemorySafetyArtifactStore::new());
    let orchestrator = SafetyOrchestrator::builder(
        &issuer,
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .provider(provider)
    .build()?;
    let service = Arc::new(
        SafetyScanService::builder(Arc::new(orchestrator), store.clone())
            .signer(Arc::new(signer))
            .build()?,
    );
    let (pipeline, _, _) = pipeline_with(&docs, &projection, (service, store));
    let pipeline = pipeline.with_post_scheduler(Arc::new(PostFetchScheduler::new(2)), 2);

    let summary = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        pipeline.ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica),
    )
    .await??;
    assert_eq!(summary.scanned, 2);
    assert_eq!(summary.indexed, 2);
    Ok(())
}

#[tokio::test]
async fn index_only_indexes_shared_replica_entries() -> Result<()> {
    // 共有 replica に実在する entry のみ index する（ghost 注入を作らない）。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_post(&docs, &replica, &topic, "hello shared replica").await;

    let (pipeline, entries, _) = pipeline_with(&docs, &projection, allow_service());
    let scope = ScopeReplica::from_scope(IndexScopeKind::PublicTopic, "rust");
    let summary = pipeline
        .ingest_recent_scope(scope.kind, &scope.id, &scope.replica_id)
        .await?;

    assert_eq!(summary.scanned, 1);
    assert_eq!(summary.indexed, 1);
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    // 真実源（index_entries）にも記録される（投影とペア。#404）。
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    Ok(())
}

#[tokio::test]
async fn content_not_in_shared_replica_is_not_indexed() -> Result<()> {
    // CN へ直接渡されただけ（= replica に entry が無い）の content は index されない。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    // replica を open するが post entry は入れない（共有 replica に実在しない）。
    let replica = topic_replica_id("empty");
    docs.open_replica(&replica).await?;

    let (pipeline, _, _) = pipeline_with(&docs, &projection, allow_service());
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "empty", &replica)
        .await?;

    assert_eq!(summary.scanned, 0);
    assert_eq!(summary.indexed, 0);
    assert_eq!(
        projection
            .count_scope(IndexScopeKind::PublicTopic, "empty")
            .await?,
        0
    );
    Ok(())
}

/// known-CSAM mock + 一般 moderation（VLM 相当）mock の 2 provider で scan service を組む。
///
/// media scan の verdict / derived タグは VLM 相当の general provider が担い、known-CSAM は
/// `NoKnownMatch` を返す（public_node_default の require_known_csam を満たすため）。
#[tokio::test]
async fn media_scan_unavailable_fails_closed_and_post_is_not_indexed() -> Result<()> {
    // media 参照 post は media blob ごとに scan する（#420、manifest 展開は #609）。media scan
    // が実行不能（VLM provider / MediaFetcher 未構成 = Unavailable）なら post 全体を index
    // しない（worst-case 合成の fail-closed。従来の「media 参照 post は index しない」挙動を
    // 標準経路経由で保存する）。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_media_post(&docs, &replica, &topic, "media caption", true).await;

    // post text scan は allow（NoKnownMatch）を返すが、media blob（未設定 subject）は
    // Unavailable エラーになる mock。
    let provider = MockSafetyProvider::known_csam("mock-known-csam")
        .with_no_known_match(&object_id)
        .default_error(ScanError::Unavailable("no media fetcher".to_string()));
    let (pipeline, entries, store) = pipeline_with(&docs, &projection, service_with(provider));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    // post text の verdict 自体は記録される（allow）が、media scan の fail-closed で index
    // はされない。blob 側の verdict も fail-closed として記録される（subject は manifest 展開
    // 後の blob hash）。
    assert!(store.verdict_for(SubjectKind::Post, &object_id).is_some());
    let blob_verdict = store
        .verdict_for(SubjectKind::Blob, &media_blob_hash())
        .expect("media verdict recorded");
    assert!(!blob_verdict.1.is_indexable());
    Ok(())
}

/// 本番と同じ `MediaFetcher` 境界を通して media blob を取得する known-CSAM provider。
///
/// 本文は `NoKnownMatch`、media は fetch 成功後に `NoKnownMatch` を返す。fetch 失敗はそのまま
/// provider error として返し、取得不能と外部 provider 障害の観測分類を検証できるようにする。
struct FetchingProvider {
    fetcher: Arc<dyn MediaFetcher>,
}

#[async_trait]
impl SafetyProvider for FetchingProvider {
    fn name(&self) -> &str {
        "fetching-known-csam"
    }

    fn capabilities(&self) -> &[SafetyProviderCapability] {
        &[SafetyProviderCapability::KnownCsamHashMatch]
    }

    async fn scan(&self, request: &ProviderScanRequest) -> Result<ProviderScanResult, ScanError> {
        if let Some(media_hint) = request.media_hint.as_deref() {
            self.fetcher
                .fetch(media_hint, request.media_mime.as_deref())
                .await?;
        }
        let mut result = ProviderScanResult::completed(
            self.name(),
            SafetyProviderCapability::KnownCsamHashMatch,
        );
        result.outcome = ScanOutcome::NoKnownMatch;
        Ok(result)
    }
}

#[tokio::test]
async fn missing_media_is_not_counted_as_external_provider_unavailable() -> Result<()> {
    // ピア不在などで media blob を取得できない場合も verdict は fail-closed の
    // ProviderUnavailable になるが、外部 safety provider 障害の監視値には含めない。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    persist_media_post(&docs, &replica, &topic, "missing media", true).await;

    let metrics = Arc::new(IndexerRuntimeState::default());
    let fetcher = BlobMediaFetcher::new(
        Arc::new(MemoryBlobService::default()),
        MediaFetchConfig::default(),
    )
    .with_metrics(Arc::clone(&metrics));
    let provider = Arc::new(FetchingProvider {
        fetcher: Arc::new(fetcher),
    });

    let signer = Secp256k1ModerationEventSigner::from_secret(TEST_SECRET).expect("signer");
    let issuer = signer.issuer_node_id().to_string();
    let store = Arc::new(MemorySafetyArtifactStore::new());
    let orchestrator = SafetyOrchestrator::builder(
        &issuer,
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .provider(provider)
    .build()
    .expect("orchestrator");
    let service = SafetyScanService::builder(Arc::new(orchestrator), store.clone())
        .signer(Arc::new(signer))
        .build()
        .expect("service");
    let (pipeline, _, _) = pipeline_with(&docs, &projection, (Arc::new(service), store));
    let pipeline = pipeline.with_metrics(Arc::clone(&metrics));

    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    let snapshot = metrics.snapshot();

    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert_eq!(snapshot.media_fetch_unavailable, 1);
    assert_eq!(snapshot.provider_unavailable, 0);
    Ok(())
}

#[tokio::test]
async fn missing_media_manifest_fails_closed_and_post_is_not_indexed() -> Result<()> {
    // manifest 参照が replica 上で解決できなければ scan 対象を確定できないため、post を
    // index しない（#609 fail-closed）。blob scan 自体が走らないので blob verdict も無い。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_media_post(&docs, &replica, &topic, "media caption", false).await;

    let (pipeline, entries, store) = pipeline_with(&docs, &projection, allow_service());
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    assert!(
        store
            .verdict_for(SubjectKind::Blob, &media_blob_hash())
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn media_scan_requests_carry_blob_hash_and_mime_from_the_manifest() -> Result<()> {
    // #609: manifest 参照は item blob（hash + mime）へ展開され、mime は scan request の
    // `media_mime` として provider（→ fetcher）まで運ばれる。thumbnail は manifest に mime
    // metadata が無いため media_mime 無しで scan される（fetcher の magic bytes 判定に委ねる）。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_media_post(&docs, &replica, &topic, "media caption", true).await;

    let signer = Secp256k1ModerationEventSigner::from_secret(TEST_SECRET).expect("signer");
    let issuer = signer.issuer_node_id().to_string();
    let store = Arc::new(MemorySafetyArtifactStore::new());
    let recording = RecordingProvider::new();
    let orchestrator = SafetyOrchestrator::builder(
        &issuer,
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .provider(recording.clone())
    .build()
    .expect("orchestrator");
    let service = SafetyScanService::builder(Arc::new(orchestrator), store.clone())
        .signer(Arc::new(signer))
        .build()
        .expect("service");
    let (pipeline, _entries, _store) =
        pipeline_with(&docs, &projection, (Arc::new(service), store));

    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert_eq!(summary.indexed, 1);

    let requests = recording.requests.lock().expect("recording provider mutex");
    let post_requests: Vec<_> = requests
        .iter()
        .filter(|request| request.subject_kind == Some(SubjectKind::Post))
        .collect();
    assert_eq!(post_requests.len(), 1);
    assert_eq!(post_requests[0].subject_id.as_deref(), Some(&*object_id));

    let blob_requests: Vec<_> = requests
        .iter()
        .filter(|request| request.subject_kind == Some(SubjectKind::Blob))
        .collect();
    assert_eq!(blob_requests.len(), 2, "manifest item + thumbnail");
    let item = blob_requests
        .iter()
        .find(|request| request.subject_id.as_deref() == Some(&*media_blob_hash()))
        .expect("manifest item scan request");
    assert_eq!(item.media_hint.as_deref(), Some(&*media_blob_hash()));
    assert_eq!(item.media_mime.as_deref(), Some("image/png"));
    let thumbnail = blob_requests
        .iter()
        .find(|request| request.subject_id.as_deref() == Some(&*media_thumbnail_hash()))
        .expect("thumbnail scan request");
    assert_eq!(
        thumbnail.media_hint.as_deref(),
        Some(&*media_thumbnail_hash())
    );
    assert_eq!(thumbnail.media_mime, None);
    Ok(())
}

#[tokio::test]
async fn allow_media_post_is_indexed_and_searchable_via_derived_tags() -> Result<()> {
    // ADR 0028 contract: derived_tags_only_for_allow_media（indexer 面）+
    // ADR 0025 §2.3: allow media は同一 scan が生成した descriptive タグで検索できる。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_media_post(&docs, &replica, &topic, "media caption", true).await;

    let known = MockSafetyProvider::known_csam("mock-known-csam");
    let vlm = MockSafetyProvider::with_capabilities(
        "mock-vlm",
        vec![kukuri_cn_safety::SafetyProviderCapability::GeneralMediaModeration],
    )
    .with_derived_tags(
        media_blob_hash(),
        vec!["sunset".to_string(), "beach".to_string()],
    );
    let (pipeline, entries, _store) =
        pipeline_with(&docs, &projection, service_with_providers(vec![known, vlm]));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 1);
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));

    // 投影 text は本文 + 派生タグの相乗り。タグ経由の検索が hit する。
    let stored = projection
        .entries_in_scope(IndexScopeKind::PublicTopic, "rust")
        .await;
    assert_eq!(stored.len(), 1);
    assert!(stored[0].text.contains("media caption"));
    assert!(stored[0].text.contains("sunset"));
    let hits = kukuri_cn_indexer::query::IndexQuery::search_scope(
        projection.as_ref(),
        IndexScopeKind::PublicTopic,
        "rust",
        "sunset",
        10,
    )
    .await?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].object_id, object_id);
    Ok(())
}

#[tokio::test]
async fn flagged_media_post_is_not_indexed_and_tags_do_not_leak() -> Result<()> {
    // media scan が非 allow（general suspected → exclude。spam）なら post 全体を index せず、
    // その scan のタグも index に流れない（derived_tags_only_for_allow_media の否定側）。
    // nsfw は ADR 0028 §8 で advisory 付き allow になるため
    // （`general_nsfw_is_indexed_with_advisory_label`）、非 allow 側は spam で固定する。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_media_post(&docs, &replica, &topic, "media caption", true).await;

    let known = MockSafetyProvider::known_csam("mock-known-csam");
    let vlm = MockSafetyProvider::with_capabilities(
        "mock-vlm",
        vec![kukuri_cn_safety::SafetyProviderCapability::GeneralMediaModeration],
    )
    .with_score(
        media_blob_hash(),
        kukuri_cn_safety::SafetyProviderCapability::GeneralMediaModeration,
        SafetyCategory::Spam,
        95,
    )
    .with_derived_tags(media_blob_hash(), vec!["leaked-tag".to_string()]);
    let (pipeline, entries, _store) =
        pipeline_with(&docs, &projection, service_with_providers(vec![known, vlm]));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    assert!(
        projection
            .entries_in_scope(IndexScopeKind::PublicTopic, "rust")
            .await
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn index_excludes_unscanned_and_scan_failed() -> Result<()> {
    // scan 失敗（fail-closed）の content は投影に入らない。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_post(&docs, &replica, &topic, "scan will fail").await;

    let (pipeline, entries, _) = pipeline_with(&docs, &projection, scan_failed_service());
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.scanned, 1);
    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    Ok(())
}

#[tokio::test]
async fn provider_unavailable_is_never_allowed_and_not_indexed() -> Result<()> {
    // provider unavailable は allow に倒れず、真実源にも投影にも入らない（issue #404 受け入れ条件）。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_post(&docs, &replica, &topic, "provider is down").await;

    let metrics = Arc::new(IndexerRuntimeState::default());
    let (pipeline, entries, _) = pipeline_with(&docs, &projection, provider_unavailable_service());
    let pipeline = pipeline.with_metrics(Arc::clone(&metrics));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    assert_eq!(metrics.snapshot().provider_unavailable, 1);
    assert_eq!(metrics.snapshot().media_fetch_unavailable, 0);
    Ok(())
}

#[tokio::test]
async fn index_excludes_non_allow_verdict_content() -> Result<()> {
    // known CSAM hash match（exclude verdict）の content は投影に入らない。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_post(&docs, &replica, &topic, "bad content").await;

    let (pipeline, entries, _) = pipeline_with(&docs, &projection, known_csam_service(&object_id));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    Ok(())
}

#[tokio::test]
async fn reingest_deindexes_when_verdict_flips_to_non_allow() -> Result<()> {
    // 初回 allow で投影されたあと、後続 scan で非 allow になった entry は de-index される。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_post(&docs, &replica, &topic, "flips later").await;

    let (allow_pipeline, entries, _) = pipeline_with(&docs, &projection, allow_service());
    allow_pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));

    // 後続 scan（別 service）で非 allow に変わる。真実源 store は共有し、de-index が両方へ届くこと
    // を確認する。
    let (flip_service, _flip_store) = known_csam_service(&object_id);
    IngestPipeline::new(
        docs.clone(),
        flip_service,
        entries.clone(),
        projection.clone(),
    )
    .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
    .await?;
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    Ok(())
}

#[tokio::test]
async fn indexing_startup_requires_validated_relay() -> Result<()> {
    // 自前 relay も外部 relay も無ければ indexing 起動に失敗する。
    assert!(
        RelayConfig::new(false, vec![])
            .validate_for_startup()
            .is_err()
    );
    // 自前 relay は runtime が接続に使える公開 URL も必要。外部 relay URL でも起動できる。
    assert!(
        RelayConfig::new(true, vec![])
            .with_own_relay_urls(vec!["https://own-relay.example.net".to_string()])
            .validate_for_startup()
            .is_ok()
    );
    assert!(
        RelayConfig::new(true, vec![])
            .validate_for_startup()
            .is_err()
    );
    assert!(
        RelayConfig::new(false, vec!["https://relay.example.net".to_string()])
            .validate_for_startup()
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn deleted_and_tombstoned_objects_are_deindexed() -> Result<()> {
    for status in ["deleted", "tombstoned"] {
        // replica 上で deleted / tombstoned になった object は de-index する。
        let docs = Arc::new(MemoryDocsSync::default());
        let projection = Arc::new(MemoryIndexProjection::new());
        let topic = TopicId::new("rust");
        let replica = topic_replica_id("rust");
        let object_id = persist_post(&docs, &replica, &topic, "to be deleted").await;

        let (pipeline, entries, _) = pipeline_with(&docs, &projection, allow_service());
        pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
            .await?;
        assert!(
            projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
                .await?
        );

        // object state を deleted に更新する。
        let mut object: serde_json::Value = {
            let records = docs
                .query_replica(
                    &replica,
                    DocQuery::Exact(stable_key("objects", &format!("{object_id}/state"))),
                )
                .await?;
            serde_json::from_slice(&records[0].value)?
        };
        object["status"] = serde_json::json!(status);
        docs.apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("objects", &format!("{object_id}/state")),
                value: object,
            },
        )
        .await?;

        // 真実源 store を共有した再 ingest で de-index が両方へ届く。
        let summary = IngestPipeline::new(
            docs.clone(),
            allow_service().0,
            entries.clone(),
            projection.clone(),
        )
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
        assert_eq!(summary.deindexed, 1);
        assert!(
            !projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
                .await?
        );
        assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    }
    Ok(())
}

// --- runtime（pipeline）経由の moderation artifact 記録（#406） ---

#[tokio::test]
async fn ingest_known_csam_records_risk_signal_and_does_not_index() -> Result<()> {
    // known CSAM hash match の post は投影に入らず、risk signal + 署名 event が store に入る。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_post(&docs, &replica, &topic, "bad content").await;

    let (pipeline, entries, store) =
        pipeline_with(&docs, &projection, known_csam_service(&object_id));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));

    // risk signal が trust/relation reads の入力として永続化される（根拠つき、断定ラベルなし）。
    let signals = store.signals();
    assert_eq!(signals.len(), 1);
    let (_, signal) = &signals[0];
    assert_eq!(signal.target, RiskSignalTarget::PostId);
    assert_eq!(signal.target_id, object_id);
    assert_eq!(signal.category, SafetyCategory::Csam);

    // moderation event は実鍵署名済みで検証に通る。
    let events = store.events();
    assert_eq!(events.len(), 1);
    verify_signed_event(&events[0]).expect("event verifies");
    assert_eq!(events[0].body.target_id, object_id);
    Ok(())
}

#[tokio::test]
async fn ingest_scan_failure_is_fail_closed_and_records_no_risk_signal() -> Result<()> {
    // provider failure は runtime（pipeline）経由でも fail-closed: 投影されず、
    // content の safety category を示さないため risk signal も生成されない。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_post(&docs, &replica, &topic, "scan will fail").await;

    let (pipeline, _, store) = pipeline_with(&docs, &projection, scan_failed_service());
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    assert!(store.signals().is_empty(), "no false risk labels");
    Ok(())
}

#[tokio::test]
async fn ingest_allow_records_no_artifacts() -> Result<()> {
    // allow verdict は投影のみで moderation artifact を作らない。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_post(&docs, &replica, &topic, "clean content").await;

    let (pipeline, entries, store) = pipeline_with(&docs, &projection, allow_service());
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;

    assert_eq!(summary.indexed, 1);
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &object_id)
            .await?
    );
    // allow の verdict state は記録される（artifact とは別。index entry の FK 参照先）。
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    assert!(store.events().is_empty());
    assert!(store.signals().is_empty());
    Ok(())
}
