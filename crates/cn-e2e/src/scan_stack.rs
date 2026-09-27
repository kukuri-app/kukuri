//! E2E harness の走査系（provider / policy / scan service / 常駐ワーカー）の組み立て。
//!
//! 本物のプロバイダ crate を wiremock の URL へ向けて構築し、本番と同じ
//! `build_safety_scan_service` → `IngestPipeline` → `BucketReader` / `IndexMaintenance` → `IndexerWorker`
//! の順で組む。
//! `suspected_threshold` を変えて組み直すと scan 構成の fingerprint が変わり、保存済み verdict の
//! 再利用（#1050）を経ずに再走査させられる。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use kukuri_blob_service::BlobService;
use kukuri_cn_core::{ChannelSecretCipher, PgIndexEntryStore, PgSafetyArtifactStore};
use kukuri_cn_indexer::bucket_reader::BucketReader;
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::state::IndexerRuntimeState;
use kukuri_cn_indexer::worker::{IndexerWorker, WorkerConfig};
use kukuri_cn_indexer::{ArcadeDbProjection, IndexMaintenance};
use kukuri_cn_safety::SafetyProvider;
use kukuri_cn_safety::provider::MediaFetcher;
use kukuri_cn_safety_arachnid::{
    ProjectArachnidShieldProvider, ShieldCredentials, ShieldProviderConfig,
};
use kukuri_cn_safety_runtime::{
    SafetyRuntimeConfig, SafetyRuntimeProviderEntry, SafetyRuntimeProvidersConfig,
    build_safety_scan_service,
};
use kukuri_cn_safety_vlm::{
    CapabilityProfile, VlmCredentials, VlmModerationProvider, VlmProviderConfig, VlmResponseFormat,
};
use kukuri_docs_sync::IrohDocsSync;

/// 判定イベント署名鍵（テスト固定値。既知の有効な secp256k1 secret）。
const TEST_SIGNER_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// wiremock の Arachnid 模擬が要求する資格情報（走行ごとに組み立てる合成値。実物ではない。
/// リテラルの組で持たないのは secret 走査の誤検知を避けるため）。
#[derive(Clone)]
pub(crate) struct SyntheticBasicAuth {
    pub(crate) user: String,
    pub(crate) pass: String,
}

impl SyntheticBasicAuth {
    pub(crate) fn generate(prefix: &str) -> Self {
        Self {
            user: format!("synthetic-{prefix}-user"),
            pass: format!("synthetic-{prefix}-not-a-credential"),
        }
    }
}

const E2E_CHANNEL_SECRET_KEY: &str = "cn-e2e-harness-channel-secret-key-0123456789abcdef";

/// 常駐ワーカーを組むための、走行中は変わらない部品。
pub(crate) struct WorkerParts {
    pub(crate) pool: sqlx::PgPool,
    pub(crate) docs: Arc<IrohDocsSync>,
    pub(crate) blobs: Arc<dyn BlobService>,
    pub(crate) entries: Arc<PgIndexEntryStore>,
    pub(crate) projection: Arc<ArcadeDbProjection>,
    pub(crate) runtime_state: Arc<IndexerRuntimeState>,
    pub(crate) arachnid_url: String,
    pub(crate) vlm_url: String,
    pub(crate) arachnid_auth: SyntheticBasicAuth,
    pub(crate) media_fetcher: Arc<dyn MediaFetcher>,
}

/// 走査系（provider + policy + scan service）と bucket reader / 保守を組み、常駐ワーカーを返す。
///
/// `suspected_threshold` は scan 構成の fingerprint に入る（#1050）。値を変えて組み直すと、
/// 内容が同じ subject でも保存済み verdict を再利用せず再走査する。
pub(crate) fn build_worker(
    parts: &WorkerParts,
    suspected_threshold: Option<u8>,
) -> Result<IndexerWorker> {
    let WorkerParts {
        pool,
        docs,
        blobs,
        entries,
        projection,
        runtime_state,
        arachnid_url,
        vlm_url,
        arachnid_auth,
        media_fetcher,
    } = parts;
    let providers = build_providers(arachnid_url, vlm_url, arachnid_auth, media_fetcher.clone())?;
    let safety_config = SafetyRuntimeConfig {
        providers: SafetyRuntimeProvidersConfig {
            known_csam: Some(SafetyRuntimeProviderEntry {
                provider: kukuri_cn_safety_arachnid::PROVIDER_NAME.to_string(),
                required: true,
            }),
            general: Some(SafetyRuntimeProviderEntry {
                provider: kukuri_cn_safety_vlm::PROVIDER_NAME.to_string(),
                required: true,
            }),
            unknown_csam: None,
        },
        signing_key: Some(TEST_SIGNER_SECRET.to_string()),
        emit_signed_events: true,
        issuer_node_id: None,
        suspected_threshold,
        suspected_signal_visibility: None,
        general_action: None,
    };
    let safety = build_safety_scan_service(
        &safety_config,
        providers,
        Arc::new(PgSafetyArtifactStore::new(pool.clone())),
    )?
    .context("safety scan service must be constructed for the e2e stack")?;
    let pipeline = IngestPipeline::new(
        docs.clone(),
        Arc::new(safety),
        entries.clone(),
        projection.clone(),
    )
    .with_metrics(Arc::clone(runtime_state))
    .with_blob_service(blobs.clone());
    let cipher = ChannelSecretCipher::from_key_material(E2E_CHANNEL_SECRET_KEY)?;
    let reader = BucketReader::new(
        pool.clone(),
        docs.clone(),
        entries.clone(),
        pipeline,
        cipher.clone(),
    )
    .with_blob_seeds(blobs.clone(), Vec::new());
    Ok(IndexerWorker::new(
        Arc::new(reader),
        IndexMaintenance::new(pool.clone(), entries.clone(), projection.clone(), cipher),
        Arc::clone(runtime_state),
        WorkerConfig {
            poll_interval: Duration::from_millis(300),
            demand_interval: Duration::from_millis(300),
        },
    ))
}

/// 本物のプロバイダ実装を wiremock の URL へ向けて構築する。
fn build_providers(
    arachnid_url: &str,
    vlm_url: &str,
    auth: &SyntheticBasicAuth,
    media_fetcher: Arc<dyn MediaFetcher>,
) -> Result<Vec<Arc<dyn SafetyProvider>>> {
    let arachnid = ProjectArachnidShieldProvider::with_credentials(
        &ShieldProviderConfig {
            api_base_url: arachnid_url.to_string(),
            timeout: Duration::from_secs(5),
            ..ShieldProviderConfig::default()
        },
        ShieldCredentials::new(auth.user.as_str(), auth.pass.as_str()),
    )
    .context("failed to build the arachnid provider against wiremock")?
    .with_media_fetcher(Arc::clone(&media_fetcher));
    let vlm = VlmModerationProvider::with_credentials(
        &VlmProviderConfig {
            api_base_url: vlm_url.to_string(),
            api_key_env: "KUKURI_CN_E2E_VLM_API_KEY_UNUSED".to_string(),
            model: "e2e/mock-model".to_string(),
            response_format: VlmResponseFormat::Json,
            timeout: Duration::from_secs(5),
        },
        VlmCredentials::anonymous(),
        CapabilityProfile::General,
    )
    .context("failed to build the vlm provider against wiremock")?
    .with_media_fetcher(media_fetcher);
    Ok(vec![Arc::new(arachnid), Arc::new(vlm)])
}
