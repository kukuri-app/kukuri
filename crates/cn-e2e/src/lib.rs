//! #616 全構成 E2E の harness。
//!
//! 本番相当の構成を 1 プロセス + 外部ミドルウェアで再現する:
//! - 実 Postgres（まっさらな migration。`TestDatabase`）・実 ArcadeDB（投影）・実 Redis
//! - 同一プロセス内の cn-user-api（HTTP で叩く）・cn-indexer 常駐ワーカー・cn-iroh-relay
//! - 実 iroh ノード 2 台（投稿者ノード / indexer ノード。ループバックで同期）
//! - プロバイダ（Project Arachnid Shield / 視覚言語モデル）は wiremock による模擬。
//!   実装は本物のプロバイダ crate を使い、HTTP 応答だけを合成する。
//!   実在の違法メディアは一切使わない。
//!
//! 発火条件: `KUKURI_CN_RUN_E2E_TESTS=1`（`cargo xtask cn-e2e` が compose で
//! ミドルウェアを用意して設定する）。未設定なら各テストは skip する。

mod scan_stack;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use tempfile::TempDir;
use wiremock::matchers::{basic_auth, body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use kukuri_blob_service::{BlobService, BlobStatus, IrohBlobService};
use kukuri_cn_core::{
    IndexScopeKind, JwtConfig, PgIndexEntryStore, TestDatabase, add_supported_topic,
    connect_postgres, initialize_database, readiness_context_fingerprint,
    record_readiness_activation,
};
use kukuri_cn_indexer::ArcadeDbProjection;
use kukuri_cn_indexer::config::{ArcadeDbConfig, MediaFetchConfig};
use kukuri_cn_indexer::media_fetcher::BlobMediaFetcher;
use kukuri_cn_indexer::participant::IndexerParticipant;
use kukuri_cn_indexer::projection::IndexProjection;
use kukuri_cn_indexer::state::{IndexerRuntimeState, IndexerStateSnapshot};
use kukuri_cn_indexer::worker::{IndexerWorker, WorkerConfig, WorkerHandle};
use kukuri_cn_iroh_relay::{IrohRelayConfig, SpawnedIrohRelay};
use kukuri_cn_operator::READINESS_CHECK_IDS;
use kukuri_cn_safety::provider::MediaFetcher;
use kukuri_cn_user_api::{UserApiConfig, app_router, build_state};
use kukuri_core::{
    AssetRef, AssetRole, BlobHash, KukuriKeys, KukuriMediaManifestV1, KukuriPostObjectV1,
    MediaManifestItem, ObjectVisibility, PayloadRef, ReplicaId, TopicId,
    build_media_manifest_envelope, build_post_envelope_with_payload, generate_keys,
    timeline_sort_key,
};
use kukuri_docs_sync::{
    DocEventStream, DocFetchPolicy, DocOp, DocQuery, DocRecord, DocsSync, IrohDocsSync, stable_key,
    topic_replica_id,
};
use kukuri_iroh_node::IrohDocsNode;
use kukuri_transport::{
    DhtDiscoveryOptions, SeedPeer, TransportNetworkConfig, TransportRelayConfig,
};

use crate::scan_stack::{SyntheticBasicAuth, build_participant};

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const DEFAULT_RENDEZVOUS_REDIS_URL: &str = "redis://127.0.0.1:16379/";

async fn persist_timeline_index(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    object: &KukuriPostObjectV1,
) -> Result<()> {
    let object_id = object.object_id.as_str();
    let sort_key = timeline_sort_key(object.created_at, &object.object_id);
    docs.apply_doc_op(
        replica,
        DocOp::SetJson {
            key: stable_key("indexes/timeline", &format!("{sort_key}/{object_id}")),
            value: serde_json::json!({ "object_id": object_id }),
        },
    )
    .await
}

/// E2E の発火判定。`KUKURI_CN_RUN_E2E_TESTS=1` のときだけ管理用 DB URL を返す。
pub fn e2e_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_E2E_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn rendezvous_redis_url() -> String {
    std::env::var("COMMUNITY_NODE_RENDEZVOUS_REDIS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_RENDEZVOUS_REDIS_URL.to_string())
}

/// 投稿者側（利用者アプリ相当）の一式。
pub struct AuthorNode {
    pub node: Arc<IrohDocsNode>,
    pub docs: Arc<IrohDocsSync>,
    pub blobs: Arc<IrohBlobService>,
    pub keys: KukuriKeys,
    _data_dir: TempDir,
}

/// 実 `IrohDocsSync` の query 境界だけを可逆に失敗させるE2E専用ラッパー。
///
/// production runtimeに障害注入用envやendpointを持ち込まず、残りの同期・Postgres・ArcadeDB・
/// user-apiは本物の構成のままreplica query failureを再現する。
struct FaultInjectingDocsSync {
    inner: Arc<IrohDocsSync>,
    fail_queries: Arc<AtomicBool>,
}

#[cfg(test)]
mod docs_wrapper_tests;

#[async_trait]
impl DocsSync for FaultInjectingDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn close_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.close_replica(replica_id).await
    }

    async fn query_replica_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<DocRecord>> {
        if self.fail_queries.load(Ordering::SeqCst) {
            anyhow::bail!("injected replica query failure for {}", replica_id.as_str());
        }
        self.inner
            .query_replica_by_author(replica_id, docs_author, key, policy)
            .await
    }

    async fn register_private_replica_secret(
        &self,
        replica_id: &ReplicaId,
        namespace_secret_hex: &str,
    ) -> Result<()> {
        self.inner
            .register_private_replica_secret(replica_id, namespace_secret_hex)
            .await
    }

    async fn remove_private_replica_secret(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.remove_private_replica_secret(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        if self.fail_queries.load(Ordering::SeqCst) {
            anyhow::bail!("injected replica query failure for {}", replica_id.as_str());
        }
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        if self.fail_queries.load(Ordering::SeqCst) {
            anyhow::bail!("injected replica query failure for {}", replica_id.as_str());
        }
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn subscribe_replica(&self, replica_id: &ReplicaId) -> Result<DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }

    async fn learn_peer(&self, endpoint_id: &str) -> Result<()> {
        self.inner.learn_peer(endpoint_id).await
    }

    async fn restart_replica_sync(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.restart_replica_sync(replica_id).await
    }

    async fn set_seed_peers(&self, peers: Vec<SeedPeer>) -> Result<()> {
        self.inner.set_seed_peers(peers).await
    }

    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        self.inner.assist_peer_ids().await
    }
}

/// 全構成 E2E の稼働一式。`boot` で本番相当の順序（migration → 投影 schema →
/// 対象範囲の登録 → 常駐ワーカー → 有効化記録 → API 公開）で立ち上がる。
pub struct E2eStack {
    pub database: TestDatabase,
    pub pool: sqlx::PgPool,
    /// この走行専用の一意な公開トピック（ArcadeDB は共有インスタンスのため）。
    pub topic_id: String,
    /// Project Arachnid Shield の模擬（既定応答: `no-known-match`）。
    pub arachnid: MockServer,
    /// 視覚言語モデルの模擬（既定応答: 分類なし = 許可）。
    pub vlm: MockServer,
    pub author: AuthorNode,
    pub indexer_node: Arc<IrohDocsNode>,
    pub indexer_docs: Arc<IrohDocsSync>,
    pub runtime_state: Arc<IndexerRuntimeState>,
    pub projection: Arc<ArcadeDbProjection>,
    pub entries: Arc<PgIndexEntryStore>,
    pub api_base_url: String,
    arachnid_auth: SyntheticBasicAuth,
    media_fetcher: Arc<dyn MediaFetcher>,
    participant: Arc<IndexerParticipant>,
    worker_docs: Arc<dyn DocsSync>,
    replica_query_failure: Arc<AtomicBool>,
    worker: Option<WorkerHandle>,
    api_task: tokio::task::JoinHandle<()>,
    _relay: SpawnedIrohRelay,
    _indexer_data_dir: TempDir,
}

/// Shield の `ScannedMedia` 応答（合成値のみ。実在ハッシュを含まない）。
pub fn arachnid_scanned_media_body(
    classification: &str,
    match_type: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "classification": classification,
        "match_type": match_type,
        "near_match_details": [],
        "sha1_base32": "e2e-submitted-sha1",
        "sha256_hex": "e2e-submitted-sha256",
        "size_bytes": 4,
    })
}

/// OpenAI 互換 chat completion 応答（`choices[0].message.content` のみが判定に使われる）。
pub fn vlm_chat_body(content: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-e2e",
        "object": "chat.completion",
        "model": "e2e/mock-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }]
    })
}

/// 既定の許可応答を両プロバイダ模擬へ載せる。
async fn mount_default_allow_mocks(
    arachnid: &MockServer,
    vlm: &MockServer,
    auth: &SyntheticBasicAuth,
) {
    Mock::given(method("POST"))
        .and(path("/v1/media"))
        .and(basic_auth(auth.user.clone(), auth.pass.clone()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(arachnid_scanned_media_body("no-known-match", None)),
        )
        .mount(arachnid)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(vlm_chat_body(r#"{"categories":[],"tags":[]}"#)),
        )
        .mount(vlm)
        .await;
}

/// ノードのピア接続チケット（`<endpoint_id>@<host:port>`）。ループバック割り当て前提。
fn loopback_ticket(node: &IrohDocsNode) -> Result<String> {
    let socket = node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .context("iroh node has no bound socket")?;
    Ok(format!("{}@{}", node.endpoint().addr().id, socket))
}

/// `E2eStack::boot_with` の調整点。既定は本番相当の既定値。
#[derive(Default)]
pub struct E2eOptions {
    /// メディア一時取得の制限（大きさ超過などの障害経路を再現するときに上書きする）。
    pub media_fetch: Option<MediaFetchConfig>,
}

impl E2eStack {
    /// 全構成を立ち上げる。発火条件を満たさない環境では `None` を返す（テストは skip）。
    pub async fn boot(prefix: &str) -> Result<Option<Self>> {
        Self::boot_with(prefix, E2eOptions::default()).await
    }

    /// 調整点つきで全構成を立ち上げる。
    pub async fn boot_with(prefix: &str, options: E2eOptions) -> Result<Option<Self>> {
        let Some(admin_url) = e2e_admin_database_url() else {
            eprintln!("skipping cn-e2e test; run via `cargo xtask cn-e2e`");
            return Ok(None);
        };
        // 失敗時の原因調査用（`RUST_LOG` で上書き可能。多重初期化は無害）。
        kukuri_cn_runtime_support::init_tracing("info,kukuri_cn_indexer=debug");

        // 1. まっさらな Postgres（migration 全適用）。
        let database = TestDatabase::create(admin_url.as_str(), prefix).await?;
        let pool = connect_postgres(database.database_url.as_str()).await?;
        initialize_database(&pool).await?;

        // 2. プロバイダ模擬（既定は許可応答）。
        let arachnid = MockServer::start().await;
        let vlm = MockServer::start().await;
        let arachnid_auth = SyntheticBasicAuth::generate(prefix);
        mount_default_allow_mocks(&arachnid, &vlm, &arachnid_auth).await;

        // 3. 同一プロセス内の cn-iroh-relay（ノードの relay 構成に実 URL を渡す）。
        let relay = kukuri_cn_iroh_relay::spawn_server(IrohRelayConfig {
            http_bind_addr: "127.0.0.1:0".parse().expect("loopback bind addr"),
            tls: None,
            client_rx_limit: None,
        })
        .await
        .context("failed to spawn the in-process iroh relay")?;
        let relay_config = TransportRelayConfig {
            iroh_relay_urls: vec![format!("http://{}", relay.http_addr())],
        }
        .normalized();

        // 4. 実 iroh ノード 2 台（投稿者 / indexer）。
        let author_dir = TempDir::new()?;
        let author_node = IrohDocsNode::persistent_with_discovery_config(
            author_dir.path(),
            TransportNetworkConfig::loopback(),
            DhtDiscoveryOptions::disabled(),
            relay_config.clone(),
        )
        .await?;
        let indexer_dir = TempDir::new()?;
        let indexer_node = IrohDocsNode::persistent_with_discovery_config(
            indexer_dir.path(),
            TransportNetworkConfig::loopback(),
            DhtDiscoveryOptions::disabled(),
            relay_config,
        )
        .await?;
        let author = AuthorNode {
            docs: Arc::new(IrohDocsSync::new(Arc::clone(&author_node))),
            blobs: Arc::new(IrohBlobService::new(Arc::clone(&author_node))),
            keys: generate_keys(),
            node: author_node,
            _data_dir: author_dir,
        };
        let indexer_docs = Arc::new(IrohDocsSync::new(Arc::clone(&indexer_node)));
        let replica_query_failure = Arc::new(AtomicBool::new(false));
        let worker_docs: Arc<dyn DocsSync> = Arc::new(FaultInjectingDocsSync {
            inner: Arc::clone(&indexer_docs),
            fail_queries: Arc::clone(&replica_query_failure),
        });
        let indexer_blobs = Arc::new(IrohBlobService::new(Arc::clone(&indexer_node)));
        let indexer_blob_service: Arc<dyn BlobService> = indexer_blobs.clone();
        let ticket = loopback_ticket(&author.node)?;
        indexer_docs.import_peer_ticket(&ticket).await?;
        indexer_blobs.import_peer_ticket(&ticket).await?;

        // 5. 走査系（本物のプロバイダ実装 + wiremock、真実源は実 Postgres）。
        let runtime_state = Arc::new(IndexerRuntimeState::default());
        let media_fetch_config = options.media_fetch.unwrap_or_default();
        let media_fetcher: Arc<dyn MediaFetcher> = Arc::new(
            BlobMediaFetcher::new(indexer_blob_service, media_fetch_config)
                .with_metrics(Arc::clone(&runtime_state)),
        );
        // 6. 索引の真実源（実 Postgres）と投影（実 ArcadeDB）。
        let entries = Arc::new(PgIndexEntryStore::new(pool.clone()));
        let projection = Arc::new(
            ArcadeDbProjection::new(ArcadeDbConfig::from_env())
                .context("failed to build the ArcadeDB projection client")?,
        );
        projection
            .ensure_schema()
            .await
            .context("ArcadeDB is unreachable; run via `cargo xtask cn-e2e`")?;

        // 7. 走査系（本物のプロバイダ実装 + wiremock、真実源は実 Postgres）と常駐ワーカー
        //    （本番と同じ participant / pipeline 構成）。
        let participant = build_participant(
            &pool,
            &worker_docs,
            &entries,
            &projection,
            &runtime_state,
            &arachnid.uri(),
            &vlm.uri(),
            &arachnid_auth,
            Arc::clone(&media_fetcher),
            None,
        )?;
        let worker = IndexerWorker::new(
            Arc::clone(&participant),
            Arc::clone(&worker_docs),
            Arc::clone(&runtime_state),
            WorkerConfig {
                poll_interval: Duration::from_millis(300),
                event_debounce: Duration::from_millis(50),
                backoff_base: Duration::from_millis(100),
                backoff_max: Duration::from_millis(500),
            },
        );
        let worker = worker.spawn();

        // 8. この走行専用の公開トピックを索引対象に登録する。
        //
        // 登録の前に投稿者側 replica を開いておく（namespace を実在させる）。indexer 側の
        // 初回同期 join は相手に namespace が無いと `NotFound` で中断され、そのまま再試行
        // されないため、投稿者側が先に replica を持っていることが同期成立の前提になる
        // （実運用でも投稿が存在する topic だけが索引対象になるため、この順序が本番相当）。
        let topic_id = format!(
            "e2e-{prefix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        author
            .docs
            .open_replica(&topic_replica_id(topic_id.as_str()))
            .await?;
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, topic_id.as_str()).await?;

        // 9. 有効化の関門（T3）を通し、cn-user-api を公開する。
        record_readiness_activation(
            &pool,
            Utc::now(),
            "public-node",
            &READINESS_CHECK_IDS,
            &readiness_context_fingerprint("public-node", "cn-e2e-v1", b""),
            &serde_json::json!([]),
        )
        .await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind the e2e user-api listener")?;
        let addr = listener.local_addr()?;
        let api_base_url = format!("http://{addr}");
        let state = build_state(&UserApiConfig {
            bind_addr: addr,
            database_url: database.database_url.clone(),
            rendezvous_redis_url: rendezvous_redis_url(),
            rendezvous_key_prefix: format!("cn:e2e:{prefix}"),
            base_url: api_base_url.clone(),
            public_base_url: api_base_url.clone(),
            connectivity_urls: vec![format!("http://{}", relay.http_addr())],
            jwt_config: JwtConfig::new("kukuri-cn-e2e", "e2e-test-secret", 3600),
            operator_config_path: None,
            channel_secret_key: None,
            legal_data_key: None,
            index_query_enabled: true,
            trust_read_enabled: true,
            relation_distance_optout_min_proximity: Some(0.5),
            deployment_revision: "cn-e2e-v1".to_string(),
            readiness_activation_max_age_secs: 3600,
            expected_issuer_node_id: None,
        })
        .await?;
        let app = app_router(state);
        let api_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("e2e user-api server");
        });

        Ok(Some(Self {
            database,
            pool,
            topic_id,
            arachnid,
            vlm,
            author,
            indexer_node,
            indexer_docs,
            runtime_state,
            projection,
            entries,
            api_base_url,
            arachnid_auth,
            media_fetcher,
            participant,
            worker_docs,
            replica_query_failure,
            worker: Some(worker),
            api_task,
            _relay: relay,
            _indexer_data_dir: indexer_dir,
        }))
    }

    /// 投稿者ノードに本文だけの投稿を置き、object_id を返す。
    pub async fn publish_text_post(&self, body: &str) -> Result<String> {
        self.publish_post(&self.author.keys.clone(), body, Vec::new())
            .await
    }

    /// 指定した鍵の著者として本文だけの投稿を置く（複数著者の再現用。ノードは共有）。
    pub async fn publish_text_post_as(&self, keys: &KukuriKeys, body: &str) -> Result<String> {
        self.publish_post(keys, body, Vec::new()).await
    }

    /// 投稿者ノードに blob を置き、それを添付した投稿を置く。(object_id, blob hash) を返す。
    pub async fn publish_image_post(
        &self,
        body: &str,
        bytes: &[u8],
        mime: &str,
    ) -> Result<(String, String)> {
        let stored = self.author.blobs.put_blob(bytes.to_vec(), mime).await?;
        let attachment = AssetRef {
            hash: BlobHash::new(stored.hash.as_str().to_string()),
            mime: mime.to_string(),
            bytes: bytes.len() as u64,
            role: AssetRole::ImageOriginal,
        };
        let object_id = self
            .publish_post(&self.author.keys.clone(), body, vec![attachment])
            .await?;
        Ok((object_id, stored.hash.as_str().to_string()))
    }

    /// Publish a manifest whose thumbnail has no MIME metadata and bytes that
    /// cannot be recognized by magic-byte sniffing. The indexer must hold the
    /// whole post fail-closed while keeping the fetched blob ephemeral.
    pub async fn publish_post_with_unknown_mime_thumbnail(
        &self,
        body: &str,
    ) -> Result<(String, String)> {
        const ITEM_BYTES: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        ];
        const UNKNOWN_THUMBNAIL_BYTES: &[u8] = b"unrecognized-thumbnail-fixture";
        let item = self
            .author
            .blobs
            .put_blob(ITEM_BYTES.to_vec(), "image/png")
            .await?;
        let thumbnail = self
            .author
            .blobs
            .put_blob(UNKNOWN_THUMBNAIL_BYTES.to_vec(), "application/octet-stream")
            .await?;
        let manifest_id = format!(
            "e2e-manifest-{}-{}",
            self.topic_id,
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let topic = TopicId::new(self.topic_id.clone());
        let replica = topic_replica_id(self.topic_id.as_str());
        let manifest = KukuriMediaManifestV1 {
            manifest_id: manifest_id.clone(),
            owner_pubkey: self.author.keys.public_key(),
            created_at: Utc::now().timestamp(),
            items: vec![MediaManifestItem {
                blob_hash: BlobHash::new(item.hash.as_str().to_string()),
                mime: "image/png".to_string(),
                size: ITEM_BYTES.len() as u64,
                width: None,
                height: None,
                duration_ms: None,
                codec: None,
                thumbnail_blob_hash: Some(BlobHash::new(thumbnail.hash.as_str().to_string())),
            }],
        };
        let manifest_envelope =
            build_media_manifest_envelope(&self.author.keys, &topic, &manifest)?;
        let post_envelope = build_post_envelope_with_payload(
            &self.author.keys,
            &topic,
            PayloadRef::InlineText {
                text: body.to_string(),
            },
            Vec::new(),
            vec![manifest_id.clone()],
            None,
            ObjectVisibility::Public,
        )?;
        let object = post_envelope
            .to_post_object()?
            .context("post envelope must yield a post object")?;
        let object_id = object.object_id.as_str().to_string();
        let docs = &self.author.docs;
        docs.open_replica(&replica).await?;
        for (key, value) in [
            (
                stable_key("objects", &format!("{object_id}/state")),
                serde_json::to_value(&object)?,
            ),
            (
                stable_key("objects", &format!("{object_id}/envelope")),
                serde_json::to_value(&post_envelope)?,
            ),
            (
                stable_key("manifests/media", &format!("{manifest_id}/state")),
                serde_json::to_value(&manifest)?,
            ),
            (
                stable_key("manifests/media", &format!("{manifest_id}/envelope")),
                serde_json::to_value(&manifest_envelope)?,
            ),
        ] {
            docs.apply_doc_op(&replica, DocOp::SetJson { key, value })
                .await?;
        }
        persist_timeline_index(docs.as_ref(), &replica, &object).await?;
        Ok((object_id, thumbnail.hash.as_str().to_string()))
    }

    /// blob の実体を置かずに、指定 hash を参照する添付つき投稿を置く（不達の再現用）。
    pub async fn publish_post_with_missing_media(
        &self,
        body: &str,
        missing_hash: &str,
        mime: &str,
    ) -> Result<String> {
        let attachment = AssetRef {
            hash: BlobHash::new(missing_hash.to_string()),
            mime: mime.to_string(),
            bytes: 4,
            role: AssetRole::ImageOriginal,
        };
        self.publish_post(&self.author.keys.clone(), body, vec![attachment])
            .await
    }

    async fn publish_post(
        &self,
        keys: &KukuriKeys,
        body: &str,
        attachments: Vec<AssetRef>,
    ) -> Result<String> {
        let topic = TopicId::new(self.topic_id.clone());
        let replica = topic_replica_id(self.topic_id.as_str());
        let envelope = build_post_envelope_with_payload(
            keys,
            &topic,
            PayloadRef::InlineText {
                text: body.to_string(),
            },
            attachments,
            Vec::new(),
            None,
            ObjectVisibility::Public,
        )?;
        let object = envelope
            .to_post_object()?
            .context("post envelope must yield a post object")?;
        let object_id = object.object_id.as_str().to_string();
        let docs = &self.author.docs;
        docs.open_replica(&replica).await?;
        docs.apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("objects", &format!("{object_id}/state")),
                value: serde_json::to_value(&object)?,
            },
        )
        .await?;
        docs.apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("objects", &format!("{object_id}/envelope")),
                value: serde_json::to_value(&envelope)?,
            },
        )
        .await?;
        persist_timeline_index(docs.as_ref(), &replica, &object).await?;
        Ok(object_id)
    }

    /// 対象 object が実 ArcadeDB 投影へ入るまで待つ（最長 timeout）。
    pub async fn wait_for_projection(&self, object_id: &str, timeout: Duration) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if self
                .projection
                .contains_object(
                    IndexScopeKind::PublicTopic,
                    self.topic_id.as_str(),
                    object_id,
                )
                .await
                .unwrap_or(false)
            {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        eprintln!(
            "wait_for_projection timed out; indexer state: {:?}",
            self.runtime_state.snapshot()
        );
        Ok(false)
    }

    /// 指定 Content-Type のメディア走査に対する Arachnid の応答を上書きする
    /// （既定の `no-known-match` より優先される）。
    pub async fn mount_arachnid_response_for_content_type(
        &self,
        content_type: &str,
        body: serde_json::Value,
    ) {
        Mock::given(method("POST"))
            .and(path("/v1/media"))
            .and(basic_auth(
                self.arachnid_auth.user.clone(),
                self.arachnid_auth.pass.clone(),
            ))
            .and(header("content-type", content_type))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .with_priority(1)
            .mount(&self.arachnid)
            .await;
    }

    /// 全メディア走査に対する Arachnid の応答を遅延させる（時間切れの再現。
    /// プロバイダ側の応答待ち上限は 5 秒）。
    pub async fn mount_arachnid_delay(&self, delay: Duration) {
        Mock::given(method("POST"))
            .and(path("/v1/media"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(arachnid_scanned_media_body("no-known-match", None))
                    .set_delay(delay),
            )
            .with_priority(1)
            .mount(&self.arachnid)
            .await;
    }

    /// 指定の目印文字列を含む走査要求に対する視覚言語モデルの応答を上書きする
    /// （既定の許可応答より優先される）。`content` は chat 応答の本文 JSON 文字列。
    pub async fn mount_vlm_response_for_marker(&self, marker: &str, content: &str) {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_string_contains(marker))
            .respond_with(ResponseTemplate::new(200).set_body_json(vlm_chat_body(content)))
            .with_priority(1)
            .mount(&self.vlm)
            .await;
    }

    /// 観測状態が条件を満たすまで待つ（最長 timeout）。満たせば真。
    pub async fn wait_for_state(
        &self,
        predicate: impl Fn(&IndexerStateSnapshot) -> bool,
        timeout: Duration,
    ) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if predicate(&self.runtime_state.snapshot()) {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        eprintln!(
            "wait_for_state timed out; indexer state: {:?}",
            self.runtime_state.snapshot()
        );
        Ok(false)
    }

    /// 認証 + 同意を通し、bearer access token を返す。
    pub async fn authenticate(&self, client: &Client) -> Result<String> {
        self.authenticate_as(client, &generate_keys()).await
    }

    /// 指定した鍵で認証 + 同意を通し、bearer access token を返す
    /// （trust / relation read の viewer は bearer の鍵に固定されるため）。
    pub async fn authenticate_as(&self, client: &Client, keys: &KukuriKeys) -> Result<String> {
        let pubkey = keys.public_key_hex();
        let base_url = &self.api_base_url;
        let challenge = client
            .post(format!("{base_url}/v1/auth/challenge"))
            .json(&serde_json::json!({ "pubkey": pubkey }))
            .send()
            .await?
            .error_for_status()?
            .json::<kukuri_cn_protocol::AuthChallengeResponse>()
            .await?;
        let auth_envelope_json = kukuri_cn_protocol::build_auth_envelope_json(
            keys,
            challenge.challenge.as_str(),
            base_url,
        )?;
        let verify = client
            .post(format!("{base_url}/v1/auth/verify"))
            .json(&serde_json::json!({
                "auth_envelope_json": auth_envelope_json,
                "endpoint_id": "e2e-peer",
            }))
            .send()
            .await?
            .error_for_status()?
            .json::<kukuri_cn_protocol::AuthVerifyResponse>()
            .await?;
        let policies = client
            .get(format!("{base_url}/v1/policies"))
            .send()
            .await?
            .error_for_status()?
            .json::<kukuri_cn_protocol::CommunityNodePoliciesResponse>()
            .await?;
        client
            .post(format!("{base_url}/v1/consents"))
            .bearer_auth(verify.access_token.as_str())
            .json(&kukuri_cn_protocol::AcceptConsentsRequest {
                policy_slugs: Vec::new(),
                policy_snapshot_revision: policies.policy_snapshot_revision,
            })
            .send()
            .await?
            .error_for_status()?;
        Ok(verify.access_token)
    }

    /// 常駐ワーカーを停止して起動し直す（プロセス再起動後の対象範囲・取り込みの復元を模す）。
    pub async fn restart_worker(&mut self) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            worker.shutdown().await;
        }
        let worker = IndexerWorker::new(
            Arc::clone(&self.participant),
            Arc::clone(&self.worker_docs),
            Arc::clone(&self.runtime_state),
            WorkerConfig {
                poll_interval: Duration::from_millis(300),
                event_debounce: Duration::from_millis(50),
                backoff_base: Duration::from_millis(100),
                backoff_max: Duration::from_millis(500),
            },
        );
        self.worker = Some(worker.spawn());
        Ok(())
    }

    /// scan 構成（suspected 閾値）を変えて常駐ワーカーを起動し直す（#1050）。
    ///
    /// 内容と scan 構成が同じ subject は保存済み verdict を再利用するため、provider 応答の
    /// 変化を既存 entry に反映させるには構成の変更（= fingerprint の変化）が要る。運用では
    /// policy / provider 設定の更新と再起動に相当する。
    pub async fn restart_worker_with_suspected_threshold(
        &mut self,
        threshold: Option<u8>,
    ) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            worker.shutdown().await;
        }
        self.participant = build_participant(
            &self.pool,
            &self.worker_docs,
            &self.entries,
            &self.projection,
            &self.runtime_state,
            &self.arachnid.uri(),
            &self.vlm.uri(),
            &self.arachnid_auth,
            Arc::clone(&self.media_fetcher),
            threshold,
        )?;
        self.restart_worker().await
    }

    /// E2E内のdocs replica query障害を有効化・解除する。
    pub fn set_replica_query_failure(&self, enabled: bool) {
        self.replica_query_failure.store(enabled, Ordering::SeqCst);
    }

    /// 既定の許可応答を両プロバイダ模擬へ載せ直す（`MockServer::reset` 後の復旧用）。
    pub async fn restore_default_provider_mocks(&self) {
        mount_default_allow_mocks(&self.arachnid, &self.vlm, &self.arachnid_auth).await;
    }

    /// API 応答（`entries` 配列）から object_id 列を取り出す。
    pub fn entry_ids(body: &serde_json::Value) -> Vec<String> {
        body["entries"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry["object_id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// indexer ノードのローカル blob 保存領域に hash の実体が残っていないことを確かめる。
    ///
    /// `blob_status` はピア経由の取得も試すため、ピアを知らない別サービスから見る
    /// （ローカル store に実在しなければ `Missing` になる）。
    pub async fn blob_is_absent_locally(&self, hash: &str) -> Result<bool> {
        let local_only = IrohBlobService::new(Arc::clone(&self.indexer_node));
        let status = local_only
            .blob_status(&BlobHash::new(hash.to_string()))
            .await?;
        Ok(status == BlobStatus::Missing)
    }

    /// 全構成を停止する（ワーカー → docs 購読 → ノード → API）。
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            worker.shutdown().await;
        }
        self.indexer_docs.shutdown().await;
        self.author.docs.shutdown().await;
        self.indexer_node.shutdown().await?;
        self.author.node.shutdown().await?;
        self.api_task.abort();
        self.database.cleanup().await
    }
}
