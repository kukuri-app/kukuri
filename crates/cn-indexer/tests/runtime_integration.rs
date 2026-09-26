//! #613 T4 実行系の統合テスト。
//!
//! 3 つの層で「本番と同じ経路」を検証する:
//! 1. **実 iroh ノード 2 台のレプリカ同期 → 取り込み**（ゲートなし。ループバックで完結）:
//!    ノード A に置いた投稿が、ピア接続したノード B の取り込みパイプラインから見え、
//!    スキャンを通って索引に入る。
//! 2. **実 iroh ノード 2 台のリモートメディア一時取得**(ゲートなし):
//!    ノード A にだけある blob を、ノード B の `BlobMediaFetcher` が取得できる。取得後も
//!    B のローカル blob 保存領域にデータが残らない（恒久保存しない前提の構造的担保）。
//! 3. **実 Postgres + 実 ArcadeDB の常駐ワーカー**（`KUKURI_CN_RUN_INTEGRATION_TESTS=1` かつ
//!    `KUKURI_CN_RUN_ARCADEDB_TESTS=1`）: ワーカーが実投影へ書き、サポート対象から外すと
//!    実投影からも消える。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use kukuri_blob_service::{
    BlobService, BlobStatus, IrohBlobService, MemoryBlobService, StoredBlob,
};
use kukuri_cn_core::TestDatabase;
use kukuri_cn_core::{
    IndexScopeKind, MemoryIndexEntryStore, add_supported_topic, connect_postgres,
    initialize_database, remove_supported_topic,
};
use kukuri_cn_indexer::ArcadeDbProjection;
use kukuri_cn_indexer::config::{ArcadeDbConfig, MediaFetchConfig};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::media_fetcher::BlobMediaFetcher;
use kukuri_cn_indexer::participant::IndexerParticipant;
use kukuri_cn_indexer::projection::{IndexProjection, MemoryIndexProjection};
use kukuri_cn_indexer::query::IndexQuery;
use kukuri_cn_indexer::state::IndexerRuntimeState;
use kukuri_cn_indexer::worker::{IndexerWorker, WorkerConfig};
use kukuri_cn_safety::provider::MediaFetcher;
use kukuri_cn_safety::{MockSafetyProvider, ModerationEventSigner};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{
    MemorySafetyArtifactStore, SafetyOrchestrator, SafetyScanService,
    Secp256k1ModerationEventSigner,
};
use kukuri_core::{
    BlobHash, KukuriKeys, ObjectVisibility, PayloadRef, ReplicaId, TopicId, build_post_envelope,
    build_post_envelope_with_payload,
};
use kukuri_docs_sync::{DocOp, DocsSync, IrohDocsSync, stable_key, topic_replica_id};
use kukuri_iroh_node::IrohDocsNode;

const TEST_SIGNER_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

/// mock provider（known CSAM = NoKnownMatch → allow）の scan service と、その artifact store。
fn allow_service() -> (Arc<SafetyScanService>, Arc<MemorySafetyArtifactStore>) {
    let signer = Secp256k1ModerationEventSigner::from_secret(TEST_SIGNER_SECRET).expect("signer");
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
    let service = Arc::new(
        SafetyScanService::builder(Arc::new(orchestrator), store.clone())
            .signer(Arc::new(signer))
            .build()
            .expect("service"),
    );
    (service, store)
}

/// ノードのピア接続チケット（`<endpoint_id>@<host:port>`）を組む。ループバック割り当て前提。
fn loopback_ticket(node: &IrohDocsNode) -> String {
    let socket = node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("bound socket");
    format!("{}@{}", node.endpoint().addr().id, socket)
}

/// 本文 text の post envelope を共有 replica に実在させ、object_id を返す。
async fn persist_post(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    body: &str,
) -> String {
    let keys = KukuriKeys::generate();
    let envelope = build_post_envelope(&keys, topic, body, None).expect("envelope");
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

/// app-api の通常投稿と同じ `BlobText` を共有 replica に配置する。
async fn persist_blob_text_post(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    stored: &StoredBlob,
) -> String {
    let keys = KukuriKeys::generate();
    let envelope = build_post_envelope_with_payload(
        &keys,
        topic,
        PayloadRef::BlobText {
            hash: stored.hash.clone(),
            mime: stored.mime.clone(),
            bytes: stored.bytes,
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
    )
    .expect("blob text envelope");
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

/// 実 iroh 2 台: A の投稿が B のレプリカ同期 → スキャン → 索引まで通る。
#[tokio::test(flavor = "multi_thread")]
async fn two_node_replica_sync_feeds_ingest() -> Result<()> {
    let node_a = IrohDocsNode::memory().await?;
    let node_b = IrohDocsNode::memory().await?;
    let docs_a = Arc::new(IrohDocsSync::new(Arc::clone(&node_a)));
    let docs_b = Arc::new(IrohDocsSync::new(Arc::clone(&node_b)));

    // A に投稿を置き、B は A をピアとして学ぶ。
    let topic = TopicId::new("rust".to_string());
    let replica = topic_replica_id("rust");
    let object_id = persist_post(docs_a.as_ref(), &replica, &topic, "hello from node a").await;
    docs_b.import_peer_ticket(&loopback_ticket(&node_a)).await?;
    docs_b.open_replica(&replica).await?;

    // B 側の取り込みパイプライン（mock 安全性プロバイダ + メモリ内の真実源 / 投影）。
    let (service, artifact_store) = allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(artifact_store));
    let projection = Arc::new(MemoryIndexProjection::default());
    let pipeline =
        IngestPipeline::new(docs_b.clone(), service, entries.clone(), projection.clone());

    // レプリカ同期は非同期に進むため、取り込みが成立するまで再実行する（冪等）。
    let mut indexed = false;
    for _ in 0..600 {
        let _ = pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
            .await;
        if projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", object_id.as_str())
            .await
            .unwrap_or(false)
        {
            indexed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(indexed, "post from node A was not ingested on node B");
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", object_id.as_str()));

    docs_a.shutdown().await;
    docs_b.shutdown().await;
    node_a.shutdown().await?;
    node_b.shutdown().await?;
    Ok(())
}

/// 実 iroh 2 台: B は A の blob を一時取得でき、取得後も B のローカル保存領域に残らない。
#[tokio::test(flavor = "multi_thread")]
async fn two_node_remote_media_fetch_is_ephemeral() -> Result<()> {
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
    ];

    let node_a = IrohDocsNode::memory().await?;
    let node_b = IrohDocsNode::memory().await?;

    // A にだけ blob を置く。
    let blobs_a = Arc::new(IrohBlobService::new(Arc::clone(&node_a)));
    let stored = blobs_a.put_blob(TINY_PNG.to_vec(), "image/png").await?;

    // B の一時取得器（観測状態つき）。A をピアとして学ぶ。
    let blobs_b = Arc::new(IrohBlobService::new(Arc::clone(&node_b)));
    blobs_b
        .import_peer_ticket(&loopback_ticket(&node_a))
        .await?;
    let state = Arc::new(IndexerRuntimeState::default());
    let fetcher = BlobMediaFetcher::new(blobs_b.clone(), MediaFetchConfig::default())
        .with_metrics(Arc::clone(&state));

    let media = fetcher
        .fetch(stored.hash.as_str(), Some("image/png"))
        .await
        .expect("remote ephemeral fetch succeeds");
    assert_eq!(media.bytes, TINY_PNG.to_vec());
    assert_eq!(media.content_type, "image/png");
    assert_eq!(state.snapshot().media_fetch_success, 1);

    // 取得後も B のローカル保存領域に blob が残らない（store 非経由の一時取得）。
    // `blob_status` はピア経由の取得も試すため、ピアを知らない別サービスから見る
    // （ローカル store に実在しなければ Missing になる）。
    let blobs_b_local_only = Arc::new(IrohBlobService::new(Arc::clone(&node_b)));
    let status = blobs_b_local_only
        .blob_status(&BlobHash::new(stored.hash.as_str().to_string()))
        .await?;
    assert_eq!(status, BlobStatus::Missing);

    node_a.shutdown().await?;
    node_b.shutdown().await?;
    Ok(())
}

/// 実 iroh 2 台: `BlobText` を replica 同期後に一時取得して索引し、受信側へ恒久保存しない。
#[tokio::test(flavor = "multi_thread")]
async fn two_node_blob_text_ingest_is_searchable_and_ephemeral() -> Result<()> {
    let node_a = IrohDocsNode::memory().await?;
    let node_b = IrohDocsNode::memory().await?;
    let docs_a = Arc::new(IrohDocsSync::new(Arc::clone(&node_a)));
    let docs_b = Arc::new(IrohDocsSync::new(Arc::clone(&node_b)));
    let blobs_a = Arc::new(IrohBlobService::new(Arc::clone(&node_a)));
    let blobs_b = Arc::new(IrohBlobService::new(Arc::clone(&node_b)));

    let body = "Community Index CUA E2E テスト";
    let stored = blobs_a
        .put_blob(body.as_bytes().to_vec(), "text/markdown")
        .await?;
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_blob_text_post(docs_a.as_ref(), &replica, &topic, &stored).await;
    let ticket = loopback_ticket(&node_a);
    docs_b.import_peer_ticket(&ticket).await?;
    docs_b.open_replica(&replica).await?;
    blobs_b.import_peer_ticket(&ticket).await?;

    let (service, artifact_store) = allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(artifact_store));
    let projection = Arc::new(MemoryIndexProjection::new());
    let pipeline =
        IngestPipeline::new(docs_b.clone(), service, entries.clone(), projection.clone())
            .with_blob_service(blobs_b.clone());

    let mut indexed = false;
    for _ in 0..600 {
        let _ = pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
            .await;
        let hits = projection
            .search_scope(IndexScopeKind::PublicTopic, "rust", "テスト", 10)
            .await
            .unwrap_or_default();
        if hits.iter().any(|entry| entry.object_id == object_id) {
            indexed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(indexed, "remote BlobText was not ingested and searchable");
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    assert_eq!(
        projection
            .entries_in_scope(IndexScopeKind::PublicTopic, "rust")
            .await[0]
            .text,
        body
    );

    let blobs_b_local_only = Arc::new(IrohBlobService::new(Arc::clone(&node_b)));
    assert_eq!(
        blobs_b_local_only.blob_status(&stored.hash).await?,
        BlobStatus::Missing
    );

    docs_a.shutdown().await;
    docs_b.shutdown().await;
    node_a.shutdown().await?;
    node_b.shutdown().await?;
    Ok(())
}

/// 実 Postgres + 実 ArcadeDB: 常駐ワーカーの取り込みが実投影に入り、サポート対象から
/// 外すと実投影からも消える。
///
/// `KUKURI_CN_RUN_INTEGRATION_TESTS=1`（Postgres）かつ `KUKURI_CN_RUN_ARCADEDB_TESTS=1`
/// （live ArcadeDB、`COMMUNITY_NODE_ARCADEDB_*`）のときのみ実行する。
#[tokio::test]
async fn worker_writes_and_deindexes_real_arcadedb_projection() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping runtime integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    if !kukuri_test_support::env_flag_enabled("KUKURI_CN_RUN_ARCADEDB_TESTS") {
        eprintln!("skipping ArcadeDB runtime test; set KUKURI_CN_RUN_ARCADEDB_TESTS=1");
        return Ok(());
    }

    let database = TestDatabase::create(admin_url.as_str(), "cn_runtime_arcadedb").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    // 走行間の残留データと衝突しない一意 topic（ArcadeDB は共有インスタンスのため）。
    let topic_id = format!(
        "cnworker-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, topic_id.as_str()).await?;

    let topic = TopicId::new(topic_id.clone());
    let replica = topic_replica_id(topic_id.as_str());
    let docs = Arc::new(kukuri_docs_sync::MemoryDocsSync::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let stored = blobs
        .put_blob(
            "real projection CUA E2E テスト".as_bytes().to_vec(),
            "text/markdown",
        )
        .await?;
    let object_id = persist_blob_text_post(docs.as_ref(), &replica, &topic, &stored).await;

    let projection = Arc::new(ArcadeDbProjection::new(ArcadeDbConfig::from_env())?);
    projection.ensure_schema().await?;

    let (service, artifact_store) = allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(artifact_store));
    let state = Arc::new(IndexerRuntimeState::default());
    let pipeline = IngestPipeline::new(docs.clone(), service, entries.clone(), projection.clone())
        .with_metrics(Arc::clone(&state));
    let participant = Arc::new(
        IndexerParticipant::new(
            pool.clone(),
            docs.clone(),
            entries.clone(),
            projection.clone(),
            pipeline,
            kukuri_cn_core::ChannelSecretCipher::from_key_material(
                "runtime-integration-test-channel-secret-key-0123456789",
            )?,
        )
        .with_blob_service(blobs),
    );
    let worker = IndexerWorker::new(
        participant,
        docs.clone(),
        Arc::clone(&state),
        WorkerConfig {
            poll_interval: Duration::from_millis(300),
            event_debounce: Duration::from_millis(50),
            backoff_base: Duration::from_millis(100),
            backoff_max: Duration::from_millis(500),
        },
    );
    let handle = worker.spawn();

    // ワーカー経由で実 ArcadeDB 投影に入る。
    let mut visible = false;
    for _ in 0..600 {
        if projection
            .contains_object(IndexScopeKind::PublicTopic, topic_id.as_str(), &object_id)
            .await
            .unwrap_or(false)
        {
            visible = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(visible, "worker did not project into real ArcadeDB");
    // 実投影の発見一覧と topic 内 / 横断検索が同じ object を返す。
    let recent = projection
        .list_recent(Some((IndexScopeKind::PublicTopic, topic_id.as_str())), 10)
        .await?;
    assert!(recent.iter().any(|entry| entry.object_id == object_id));
    let scoped = projection
        .search_scope(IndexScopeKind::PublicTopic, topic_id.as_str(), "CUA", 10)
        .await?;
    assert!(scoped.iter().any(|entry| entry.object_id == object_id));
    let all = projection.search_all("テスト", 10).await?;
    assert!(all.iter().any(|entry| entry.object_id == object_id));

    // サポート対象から外すと、実投影からも消える（真実源 → 投影の順）。
    remove_supported_topic(&pool, IndexScopeKind::PublicTopic, topic_id.as_str()).await?;
    let mut removed = false;
    for _ in 0..600 {
        if projection
            .count_scope(IndexScopeKind::PublicTopic, topic_id.as_str())
            .await
            .unwrap_or(usize::MAX)
            == 0
        {
            removed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        removed,
        "removed scope was not de-indexed from real ArcadeDB"
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, topic_id.as_str(), &object_id));

    handle.shutdown().await;
    Ok(())
}

/// 実 ArcadeDB: 受入下限と解除 scope の回収は 1 回あたり上限つきで消し、消した件数を返す（#1221 R5-F）。
#[tokio::test]
async fn arcadedb_projection_reclaims_in_bounded_pages() -> Result<()> {
    if !kukuri_test_support::env_flag_enabled("KUKURI_CN_RUN_ARCADEDB_TESTS") {
        eprintln!("skipping ArcadeDB projection test; set KUKURI_CN_RUN_ARCADEDB_TESTS=1");
        return Ok(());
    }
    let projection = ArcadeDbProjection::new(ArcadeDbConfig::from_env())?;
    projection.ensure_schema().await?;
    let scope = format!(
        "cnreclaim-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    // 他の test の行と重ならないよう、作成時刻は 1970 年の値にする。
    for (object, created_at) in [
        ("old-1", 1),
        ("old-2", 2),
        ("old-3", 3),
        ("kept", 4_000_000_000),
    ] {
        projection
            .upsert_entry(&kukuri_cn_indexer::projection::IndexedEntry {
                scope_kind: IndexScopeKind::PublicTopic,
                scope_id: scope.clone(),
                object_id: object.into(),
                author_pubkey: "author".into(),
                text: "reclaim".into(),
                created_at,
                source_replica_id: format!("topic::{scope}"),
                content_advisories: Vec::new(),
            })
            .await?;
    }
    assert_eq!(projection.remove_older_than(10, 2).await?, 2);
    assert_eq!(projection.remove_older_than(10, 2).await?, 1);
    assert_eq!(
        projection
            .count_scope(IndexScopeKind::PublicTopic, &scope)
            .await?,
        1
    );
    assert_eq!(
        projection
            .remove_scope_page(IndexScopeKind::PublicTopic, &scope, 5)
            .await?,
        1
    );
    assert_eq!(
        projection
            .count_scope(IndexScopeKind::PublicTopic, &scope)
            .await?,
        0
    );
    Ok(())
}
