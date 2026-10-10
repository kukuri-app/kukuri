//! #613 T4 実行系の統合テスト。
//!
//! 3 つの層で「本番と同じ経路」を検証する:
//! 1. **実 iroh ノード 2 台のリモートメディア一時取得**(ゲートなし):
//!    ノード A にだけある blob を、ノード B の `BlobMediaFetcher` が取得できる。取得後も
//!    B のローカル blob 保存領域にデータが残らない（恒久保存しない前提の構造的担保）。
//! 2. **実 iroh ノード 2 台 + 実 Postgres の bucket reader**（`KUKURI_CN_RUN_INTEGRATION_TESTS=1`）:
//!    A の `BlobText` 投稿を B が提供元として読み、本文を一時取得して索引する。
//! 3. **実 Postgres + 実 ArcadeDB**（`KUKURI_CN_RUN_INTEGRATION_TESTS=1` かつ
//!    `KUKURI_CN_RUN_ARCADEDB_TESTS=1`）: 取り込みが実投影へ書き、サポート対象から外すと
//!    保守の巡回で実投影からも消える。

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
use kukuri_cn_indexer::bucket_reader::BucketReader;
use kukuri_cn_indexer::config::{ArcadeDbConfig, MediaFetchConfig};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::maintenance::IndexMaintenance;
use kukuri_cn_indexer::media_fetcher::BlobMediaFetcher;
use kukuri_cn_indexer::projection::{IndexProjection, MemoryIndexProjection};
use kukuri_cn_indexer::query::IndexRecent;
use kukuri_cn_indexer::state::IndexerRuntimeState;
use kukuri_cn_safety::provider::MediaFetcher;
use kukuri_cn_safety::{MockSafetyProvider, ModerationEventSigner};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{
    MemorySafetyArtifactStore, SafetyOrchestrator, SafetyScanService,
    Secp256k1ModerationEventSigner,
};
use kukuri_core::{
    BlobHash, KukuriKeys, ObjectVisibility, PayloadRef, ReplicaId, TopicId,
    build_post_envelope_with_payload,
};
use kukuri_docs_sync::{
    BucketReplica, BucketScope, DocOp, DocsSync, IrohDocsSync, TimeBucket, stable_key,
    topic_replica_id,
};
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

/// 実 iroh 2 台 + 実 Postgres: bucket reader が提供元の `BlobText` を読み、巡回で選んだ提供元から本文を一時取得して
/// 索引する。受信側へ恒久保存しない（#1221 R5-H。media の取得候補は巡回で選んだ提供元に限る）。
#[tokio::test(flavor = "multi_thread")]
async fn two_node_blob_text_ingest_is_searchable_and_ephemeral() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping bucket blob text test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_runtime_blob_text").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;

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
    let now = chrono::Utc::now().timestamp();
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: "rust".into(),
        },
        TimeBucket::from_unix_seconds(now)?,
    )?
    .replica_id();
    let object_id =
        persist_blob_text_post(docs_a.as_ref(), &replica, &TopicId::new("rust"), &stored).await;
    let pubkey = "a".repeat(64);
    sqlx::query("INSERT INTO cn_user.subscriber_accounts(subscriber_pubkey) VALUES ($1)")
        .bind(&pubkey)
        .execute(&pool)
        .await?;
    let (endpoint_id, addr) = loopback_ticket(&node_a)
        .split_once('@')
        .map(|(id, addr)| (id.to_string(), addr.to_string()))
        .expect("ticket");
    sqlx::query(
        "INSERT INTO cn_bootstrap.peer_registrations(subscriber_pubkey, endpoint_id, addr_hint, expires_at)
         VALUES ($1, $2, $3, NOW() + INTERVAL '1 hour')",
    )
    .bind(&pubkey)
    .bind(endpoint_id)
    .bind(addr)
    .execute(&pool)
    .await?;

    let (service, artifact_store) = allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(artifact_store));
    let projection = Arc::new(MemoryIndexProjection::new());
    let pipeline =
        IngestPipeline::new(docs_b.clone(), service, entries.clone(), projection.clone())
            .with_blob_service(blobs_b.clone());
    let reader = BucketReader::new(
        pool.clone(),
        docs_b.clone(),
        entries.clone(),
        pipeline,
        kukuri_cn_core::ChannelSecretCipher::from_key_material(
            "runtime-integration-test-channel-secret-key-0123456789",
        )?,
    )
    .with_blob_seeds(blobs_b.clone(), Vec::new());
    let mut indexed = false;
    for _ in 0..20 {
        reader.poll_once(now, true).await?;
        let hits = projection.search_reader().search(
            Some((IndexScopeKind::PublicTopic, "rust")),
            "テスト",
            10,
        )?;
        if hits.iter().any(|entry| entry.object_id == object_id) {
            indexed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let text = projection
        .entries_in_scope(IndexScopeKind::PublicTopic, "rust")
        .await
        .first()
        .map(|entry| entry.text.clone());
    let blobs_b_local_only = Arc::new(IrohBlobService::new(Arc::clone(&node_b)));
    let local = blobs_b_local_only.blob_status(&stored.hash).await?;

    docs_a.shutdown().await;
    docs_b.shutdown().await;
    node_a.shutdown().await?;
    node_b.shutdown().await?;
    pool.close().await;
    database.cleanup().await?;
    assert!(indexed, "remote BlobText was not ingested and searchable");
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    assert_eq!(text.as_deref(), Some(body));
    assert_eq!(local, BlobStatus::Missing);
    Ok(())
}

/// 実 Postgres + 実 ArcadeDB: 取り込みが実投影に入り、サポート対象から外すと保守の巡回で実投影からも消える。
///
/// `KUKURI_CN_RUN_INTEGRATION_TESTS=1`（Postgres）かつ `KUKURI_CN_RUN_ARCADEDB_TESTS=1`
/// （live ArcadeDB、`COMMUNITY_NODE_ARCADEDB_*`）のときのみ実行する。
#[tokio::test]
async fn ingest_and_maintenance_write_and_deindex_real_arcadedb_projection() -> Result<()> {
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

    let source = Arc::new(ArcadeDbProjection::new(ArcadeDbConfig::from_env())?);
    source.ensure_schema().await?;
    let search_dir = tempfile::tempdir()?;
    let search = Arc::new(kukuri_cn_indexer::search::SearchIndex::open(
        search_dir.path(),
    )?);
    let projection = Arc::new(kukuri_cn_indexer::search::SearchProjection::new(
        source.clone(),
        search.clone(),
    ));

    let (service, artifact_store) = allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(artifact_store));
    let state = Arc::new(IndexerRuntimeState::default());
    let pipeline = IngestPipeline::new(docs.clone(), service, entries.clone(), projection.clone())
        .with_metrics(Arc::clone(&state))
        .with_blob_service(blobs);
    pipeline
        .ingest_changed_keys(
            IndexScopeKind::PublicTopic,
            topic_id.as_str(),
            &replica,
            &[],
        )
        .await?;
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, topic_id.as_str(), &object_id)
            .await?,
        "ingest did not project into real ArcadeDB"
    );
    // 実投影の発見一覧と topic 内 / 横断検索が同じ object を返す。
    let recent = source
        .list_recent(Some((IndexScopeKind::PublicTopic, topic_id.as_str())), 10)
        .await?;
    assert!(recent.iter().any(|entry| entry.object_id == object_id));
    search.commit()?;
    let scoped = search.reader().search(
        Some((IndexScopeKind::PublicTopic, topic_id.as_str())),
        "CUA",
        10,
    )?;
    assert!(scoped.iter().any(|entry| entry.object_id == object_id));
    let all = search.reader().search(None, "テスト", 10)?;
    assert!(all.iter().any(|entry| entry.object_id == object_id));

    // サポート対象から外すと、保守の巡回で実投影からも消える（投影 → 真実源の順）。
    remove_supported_topic(&pool, IndexScopeKind::PublicTopic, topic_id.as_str()).await?;
    IndexMaintenance::new(
        pool.clone(),
        entries.clone(),
        projection.clone(),
        kukuri_cn_core::ChannelSecretCipher::from_key_material(
            "runtime-integration-test-channel-secret-key-0123456789",
        )?,
    )
    .run_pass(chrono::Utc::now().timestamp(), &state)
    .await;
    assert_eq!(
        projection
            .count_scope(IndexScopeKind::PublicTopic, topic_id.as_str())
            .await?,
        0,
        "removed scope was not de-indexed from real ArcadeDB"
    );
    assert!(!entries.contains(IndexScopeKind::PublicTopic, topic_id.as_str(), &object_id));
    pool.close().await;
    database.cleanup().await
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

mod arcadedb_support;
use arcadedb_support::{command as arcadedb_command, read_records as arcadedb_read_records};

#[tokio::test]
async fn arcadedb_recent_listing_reads_only_returned_entries() -> Result<()> {
    if !kukuri_test_support::env_flag_enabled("KUKURI_CN_RUN_ARCADEDB_TESTS") {
        eprintln!("skipping ArcadeDB projection test; set KUKURI_CN_RUN_ARCADEDB_TESTS=1");
        return Ok(());
    }
    let config = ArcadeDbConfig::from_env();
    let projection = ArcadeDbProjection::new(config.clone())?;
    projection.ensure_schema().await?;
    let scope = format!(
        "cnrecent-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    // 作成時刻は他の test の行より新しく重複しない値にし、入れる順とは並びを変える。
    let created_at = |i: usize| 5_000_000_000 + ((i * 7_919) % 20_000) as i64;
    let mut inserted = 0;
    for total in [2_000, 20_000] {
        // 1 取引 1,000 件ずつ入れる（`upsert_entry` を 1 件ずつ呼ぶと遅い）。
        while inserted < total {
            let documents: Vec<_> = (inserted..inserted + 1_000)
                .map(|i| {
                    serde_json::json!({
                        "scope_kind": "public_topic",
                        "scope_id": scope,
                        "object_id": format!("recent-{i}"),
                        "author_pubkey": "author",
                        "text": "recent",
                        "created_at": created_at(i),
                        "source_replica_id": format!("topic::{scope}"),
                    })
                })
                .collect();
            arcadedb_command(
                &config,
                "sqlscript",
                &format!(
                    "INSERT INTO IndexedEntry CONTENT {}; RETURN 1;",
                    serde_json::Value::Array(documents)
                ),
            )
            .await?;
            inserted += 1_000;
        }
        let mut expected: Vec<_> = (0..total)
            .map(|i| (format!("recent-{i}"), created_at(i)))
            .collect();
        expected.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.1));
        expected.truncate(100);
        for filter in [Some((IndexScopeKind::PublicTopic, scope.as_str())), None] {
            let before = arcadedb_read_records(&config).await?;
            let entries = projection.list_recent(filter, 100).await?;
            let read = arcadedb_read_records(&config).await? - before;
            assert_eq!(
                read,
                entries.len() as u64,
                "{total} entries, scope {filter:?}"
            );
            if filter.is_some() {
                let listed: Vec<_> = entries
                    .into_iter()
                    .map(|entry| (entry.object_id, entry.created_at))
                    .collect();
                assert_eq!(listed, expected, "{total} entries");
            }
        }
    }
    assert_eq!(
        projection
            .remove_scope_page(IndexScopeKind::PublicTopic, &scope, inserted)
            .await?,
        inserted
    );
    Ok(())
}

/// 一意索引 `(scope_kind, scope_id, object_id)` の bucket の索引と file の id（圧縮で置き換わると変わる）。
async fn unique_index_files(config: &ArcadeDbConfig) -> Result<Vec<(String, u64)>> {
    let indexes = arcadedb_command(config, "sql", "SELECT FROM schema:indexes").await?;
    let mut files: Vec<_> = indexes["result"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|index| {
            index["properties"] == serde_json::json!([["scope_kind", "scope_id", "object_id"]])
                && !index["associatedBucketId"].is_null()
        })
        .map(|index| {
            (
                index["name"].as_str().unwrap_or_default().to_string(),
                index["fileId"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    files.sort();
    Ok(files)
}

/// 実 ArcadeDB: 一意索引が自動の圧縮を受けた後も、scope の回収はその scope の文書をすべて消し、
/// 数え上げも全件を数える（#1726）。
///
/// ArcadeDB 26.8.1 は、圧縮済みの系列を一意索引の先頭 2 列で引くと、範囲の始まりを含む直前の葉を
/// 飛ばした（ArcadeData/arcadedb#8806）。索引の並びで対象の前に別の scope の文書を置き、圧縮を待って確かめる。
#[tokio::test]
async fn arcadedb_scope_removal_reaches_entries_after_compaction() -> Result<()> {
    if !kukuri_test_support::env_flag_enabled("KUKURI_CN_RUN_ARCADEDB_TESTS") {
        eprintln!("skipping ArcadeDB projection test; set KUKURI_CN_RUN_ARCADEDB_TESTS=1");
        return Ok(());
    }
    let config = ArcadeDbConfig::from_env();
    let projection = ArcadeDbProjection::new(config.clone())?;
    projection.ensure_schema().await?;
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    // 対象の scope の前に、自動の圧縮（可変の頁 10 頁）を起こす数の文書を持つ scope が並ぶ。
    let (target, preceding) = (format!("cnprefix-{run}-m"), format!("cnprefix-{run}-a"));
    let files = unique_index_files(&config).await?;
    for (scope, count) in [(&target, 2_000u64), (&preceding, 50_000)] {
        for start in (0..count).step_by(1_000) {
            let documents: Vec<_> = (start..start + 1_000)
                .map(|i| {
                    serde_json::json!({
                        "scope_kind": "public_topic",
                        "scope_id": scope,
                        // 投稿 id と同じ長さで、並びが入れる順に揃わない値。
                        "object_id": format!("{:064x}", i.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
                        "author_pubkey": "author",
                        "text": "prefix",
                        "created_at": 6_000_000_000 + i as i64,
                        "source_replica_id": format!("topic::{scope}"),
                    })
                })
                .collect();
            arcadedb_command(
                &config,
                "sqlscript",
                &format!(
                    "INSERT INTO IndexedEntry CONTENT {}; RETURN 1;",
                    serde_json::Value::Array(documents)
                ),
            )
            .await?;
        }
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while unique_index_files(&config).await? == files {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "the unique index was not compacted"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let counted = projection
        .count_scope(IndexScopeKind::PublicTopic, &target)
        .await?;
    let mut removed = 0;
    loop {
        let page = projection
            .remove_scope_page(IndexScopeKind::PublicTopic, &target, 128)
            .await?;
        if page == 0 {
            break;
        }
        removed += page;
    }
    // scope_id だけの条件は複合索引の先頭にならないので、残りは全件の走査で数える。
    let left = arcadedb_command(
        &config,
        "sql",
        &format!("SELECT count(*) AS total FROM IndexedEntry WHERE scope_id = '{target}'"),
    )
    .await?;
    assert_eq!(
        (counted, removed, left["result"][0]["total"].as_u64()),
        (2_000, 2_000, Some(0))
    );
    while projection
        .remove_scope_page(IndexScopeKind::PublicTopic, &preceding, 10_000)
        .await?
        > 0
    {}
    Ok(())
}
