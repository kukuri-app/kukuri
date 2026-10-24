//! Opt-in benign live probe. Normal CI never sends data to OpenAI.
//! Uses real PostgreSQL, production provider/decoder/fetcher and signed post ingest.
//! Source replica/blob store and projection are in-memory; known-CSAM is a benign test double.
#![cfg(target_os = "linux")]
use anyhow::{Result, ensure};
use async_trait::async_trait;
use kukuri_blob_service::{BlobService, MemoryBlobService};
use kukuri_cn_core::{
    IndexScopeKind, PgIndexEntryStore, PgModerationBudget, PgSafetyArtifactStore, TestDatabase,
    add_supported_topic, connect_postgres, get_index_entry, initialize_database,
};
use kukuri_cn_indexer::{
    config::MediaFetchConfig, ingest::IngestPipeline, media_fetcher::BlobMediaFetcher,
    projection::MemoryIndexProjection, state::IndexerRuntimeState,
};
use kukuri_cn_safety::{
    ModerationEventSigner, ProviderScanRequest, ProviderScanResult, SafetyProvider,
    SafetyProviderCapability, ScanError, ScanOutcome,
};
use kukuri_cn_safety_openai::{
    ModerationClient, ModerationConfig, ModerationCredentials, OpenAiModerationProvider,
};
use kukuri_cn_safety_runtime::{
    SafetyOrchestrator, SafetyScanService, Secp256k1ModerationEventSigner, SystemScanClock,
    UuidEventIdGenerator,
};
use kukuri_cn_safety_video::{FfmpegVideoExtractor, VideoExtractConfig};
use kukuri_core::{
    AssetRef, AssetRole, KukuriKeys, ObjectVisibility, PayloadRef, TopicId,
    build_post_envelope_with_payload,
};
use kukuri_docs_sync::{DocOp, DocsSync, MemoryDocsSync, topic_replica_id};
use std::{sync::Arc, time::Instant};

struct BenignKnownHash;
#[async_trait]
impl SafetyProvider for BenignKnownHash {
    fn name(&self) -> &str {
        "benign-live-fixture-known-hash"
    }
    fn capabilities(&self) -> &[SafetyProviderCapability] {
        &[SafetyProviderCapability::KnownCsamHashMatch]
    }
    fn supports_content_reuse(&self) -> bool {
        true
    }
    async fn scan(&self, _: &ProviderScanRequest) -> Result<ProviderScanResult, ScanError> {
        let mut result = ProviderScanResult::completed(self.name(), self.capabilities()[0]);
        result.outcome = ScanOutcome::NoKnownMatch;
        Ok(result)
    }
}

fn pipeline(
    pool: &sqlx::PgPool,
    docs: Arc<MemoryDocsSync>,
    blobs: Arc<MemoryBlobService>,
    metrics: Arc<IndexerRuntimeState>,
) -> Result<IngestPipeline> {
    let client = Arc::new(ModerationClient::new(
        ModerationConfig::from_env()?,
        ModerationCredentials::from_env()?,
        Arc::new(PgModerationBudget::new(pool.clone())),
    )?);
    metrics.set_moderation_metrics(Some(client.metrics()));
    let provider = OpenAiModerationProvider::new(client)
        .with_video_extractor(Arc::new(FfmpegVideoExtractor::new(
            VideoExtractConfig::from_env()?,
        )?))
        .with_media_fetcher(Arc::new(
            BlobMediaFetcher::new(blobs, MediaFetchConfig::default()).with_metrics(metrics.clone()),
        ));
    let signer = Arc::new(Secp256k1ModerationEventSigner::from_secret(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )?);
    let orchestrator = SafetyOrchestrator::builder(
        signer.issuer_node_id(),
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .provider(Arc::new(BenignKnownHash))
    .provider(Arc::new(provider))
    .build()?;
    let scan = SafetyScanService::builder(
        Arc::new(orchestrator),
        Arc::new(PgSafetyArtifactStore::new(pool.clone())),
    )
    .signer(signer)
    .build()?;
    Ok(IngestPipeline::new(
        docs,
        Arc::new(scan),
        Arc::new(PgIndexEntryStore::new(pool.clone())),
        Arc::new(MemoryIndexProjection::new()),
    )
    .with_metrics(metrics))
}

async fn post(docs: &MemoryDocsSync, blob: &AssetRef) -> Result<String> {
    let envelope = build_post_envelope_with_payload(
        &KukuriKeys::generate(),
        &TopicId::new("benign-live-probe"),
        PayloadRef::InlineText {
            text: "A blue test pattern for a benign moderation check.".into(),
        },
        vec![blob.clone()],
        vec![],
        None,
        ObjectVisibility::Public,
    )?;
    let object = envelope.to_post_object()?.expect("post");
    let replica = topic_replica_id("benign-live-probe");
    docs.open_replica(&replica).await?;
    for (suffix, value) in [
        ("state", serde_json::to_value(&object)?),
        ("envelope", serde_json::to_value(&envelope)?),
    ] {
        docs.apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: format!("objects/{}/{suffix}", object.object_id.as_str()),
                value,
            },
        )
        .await?;
    }
    Ok(object.object_id.as_str().to_owned())
}

fn generated_video(seconds: &str, audio: bool) -> Result<Vec<u8>> {
    let dir = tempfile::tempdir_in("/dev/shm")?;
    let output = dir.path().join("benign.mp4");
    let mut command = std::process::Command::new("/usr/bin/ffmpeg");
    command.env_clear().args([
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=blue:s=320x180:r=12",
    ]);
    if audio {
        command.args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=8000",
            "-c:a",
            "aac",
        ]);
    }
    command
        .args([
            "-t", seconds, "-c:v", "libx264", "-threads", "1", "-pix_fmt", "yuv420p",
        ])
        .arg(&output);
    ensure!(
        command.status()?.success(),
        "synthetic fixture generation failed"
    );
    Ok(std::fs::read(output)?)
}

#[tokio::test]
async fn benign_live_cold_and_reused_post_ingest() -> Result<()> {
    if std::env::var("KUKURI_CN_RUN_LIVE_MODERATION_TESTS").as_deref() != Ok("1") {
        return Ok(());
    }
    let admin = std::env::var("COMMUNITY_NODE_DATABASE_URL")?;
    let db = TestDatabase::create(&admin, "cn_1060_live").await?;
    let pool = connect_postgres(&db.database_url).await?;
    let result = async {
        initialize_database(&pool).await?;
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, "benign-live-probe").await?;
        let docs = Arc::new(MemoryDocsSync::default());
        let blobs = Arc::new(MemoryBlobService::default());
        let metrics = Arc::new(IndexerRuntimeState::default());
        let first = pipeline(&pool, docs.clone(), blobs.clone(), metrics.clone())?;
        let fixtures = [
            ("mp4-single", "video/mp4", include_bytes!("../../cn-safety-video/src/probes/benign.mp4").to_vec(), 1),
            ("webm-single", "video/webm", include_bytes!("../../cn-safety-video/src/probes/benign.webm").to_vec(), 1),
            ("mp4-audio-12s", "video/mp4", generated_video("12", true)?, 3),
            ("mp4-60s", "video/mp4", generated_video("60", false)?, 8),
        ];
        let replica = topic_replica_id("benign-live-probe");
        let mut assets = Vec::new();
        for (name, mime, bytes, frames) in fixtures {
            let len = bytes.len();
            let stored = blobs.put_blob(bytes, mime).await?;
            let asset = AssetRef { hash: stored.hash, mime: mime.into(), bytes: len as u64, role: AssetRole::VideoManifest, origin: None };
            let id = post(&docs, &asset).await?;
            let before = metrics.snapshot();
            let started = Instant::now();
            let summary = first.ingest_changed_keys(IndexScopeKind::PublicTopic, "benign-live-probe", &replica, &[format!("objects/{id}/state")]).await?;
            ensure!(summary.indexed == 1, "benign fixture {name} failed closed: {summary:?}");
            let after = metrics.snapshot();
            ensure!(after.moderation.frames_extracted - before.moderation.frames_extracted == frames);
            ensure!(after.media_fetch_success - before.media_fetch_success == 1);
            println!("LIVE {}", serde_json::json!({"fixture": name, "input_bytes": len, "cold_ms": started.elapsed().as_millis(), "frames": frames, "api_attempts": after.moderation.api_attempts - before.moderation.api_attempts, "decode_ms": after.moderation.decode_duration_ms - before.moderation.decode_duration_ms}));
            assets.push(asset);
        }
        drop(first);
        let restarted_metrics = Arc::new(IndexerRuntimeState::default());
        let restarted = pipeline(&pool, docs.clone(), blobs, restarted_metrics.clone())?;
        for asset in assets {
            let id = post(&docs, &asset).await?;
            let started = Instant::now();
            let summary = restarted.ingest_changed_keys(IndexScopeKind::PublicTopic, "benign-live-probe", &replica, &[format!("objects/{id}/state")]).await?;
            ensure!(summary.indexed == 1 && summary.scans_fresh == 0 && summary.scans_reused == 2);
            ensure!(get_index_entry(&pool, IndexScopeKind::PublicTopic, "benign-live-probe", &id).await?.is_some());
            let counters = restarted_metrics.snapshot();
            ensure!(counters.media_fetch_success == 0 && counters.moderation.api_attempts == 0 && counters.moderation.video_decode_attempts == 0);
            println!("REUSED {}", serde_json::json!({"millis": started.elapsed().as_millis(), "fetch": 0, "decode": 0, "api_attempts": 0}));
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    pool.close().await;
    db.cleanup().await?;
    result
}
