//! #613 T2/T3 常駐ワーカーのループ契約テスト。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 Postgres（scope 管理 state）に接続して実行する。
//! docs 同期・投影・真実源はメモリ内実装 + mock 安全性プロバイダで、ワーカーのループ契約を固定する:
//! - 起動時にサポート対象を取り込み、レプリカの変更通知で追加の投稿を取り込む。
//! - サポート対象から外れた scope / 秘密鍵が失効した private channel は、次の見直しで索引解除される。
//! - 1 つの scope の失敗が他の scope の取り込みを妨げない（再試行間隔つき）。
//! - 再起動後にサポート対象が復元される。停止後はワーカーが動いていないと観測できる。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use kukuri_cn_core::TestDatabase;
use kukuri_cn_core::{
    ChannelSecretCipher, IndexScopeKind, MemoryIndexEntryStore, add_supported_topic,
    connect_postgres, initialize_database, mark_index_demand, register_channel_secret,
    remove_channel_secret, remove_supported_topic,
};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::participant::{IndexerParticipant, ScopeReplica};
use kukuri_cn_indexer::projection::{IndexProjection, MemoryIndexProjection};
use kukuri_cn_indexer::state::IndexerRuntimeState;
use kukuri_cn_indexer::worker::{IndexerWorker, WorkerConfig};
use kukuri_cn_safety::{MockSafetyProvider, ModerationEventSigner};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{
    MemorySafetyArtifactStore, SafetyOrchestrator, SafetyScanService,
    Secp256k1ModerationEventSigner,
};
use kukuri_core::{
    ChannelId, KukuriKeys, ObjectVisibility, PayloadRef, ReplicaId, TopicId,
    build_post_envelope_with_payload_in_channel, timeline_sort_key,
};
use kukuri_docs_sync::{
    DocFetchPolicy, DocOp, DocQuery, DocRecord, DocsSync, MemoryDocsSync, stable_key,
};
use sqlx::postgres::PgPool;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const TEST_SIGNER_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const TEST_CIPHER_KEY: &str = "worker-contract-test-channel-secret-key-0123456789";
const TEST_NAMESPACE_SECRET: &str =
    "0303030303030303030303030303030303030303030303030303030303030303";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn cipher() -> ChannelSecretCipher {
    ChannelSecretCipher::from_key_material(TEST_CIPHER_KEY).expect("cipher")
}

/// participant を組む（docs 同期は差し替え可能）。真実源（メモリ内実装）は scan service と
/// 同じ artifact store を参照し、verdict への外部キー相当を成立させる。
fn participant_with_docs(
    pool: &PgPool,
    docs: Arc<dyn DocsSync>,
    projection: &Arc<MemoryIndexProjection>,
    state: &Arc<IndexerRuntimeState>,
) -> (Arc<IndexerParticipant>, Arc<MemoryIndexEntryStore>) {
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
    let entries = Arc::new(MemoryIndexEntryStore::new(store));
    let pipeline = IngestPipeline::new(docs.clone(), service, entries.clone(), projection.clone())
        .with_metrics(Arc::clone(state));
    let participant = Arc::new(IndexerParticipant::new(
        pool.clone(),
        docs,
        entries.clone(),
        projection.clone(),
        pipeline,
        cipher(),
    ));
    (participant, entries)
}

/// テスト用に間隔を短縮したワーカー設定。
fn fast_config(poll_interval: Duration) -> WorkerConfig {
    WorkerConfig {
        poll_interval,
        event_debounce: Duration::from_millis(50),
        backoff_base: Duration::from_millis(100),
        backoff_max: Duration::from_millis(500),
    }
}

/// 本文 text の post envelope を共有 replica に実在させ、object_id を返す。
async fn persist_post(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    body: &str,
) -> String {
    persist_post_in_channel(docs, replica, topic, body, None).await
}

async fn persist_post_in_channel(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    body: &str,
    channel: Option<&str>,
) -> String {
    let keys = KukuriKeys::generate();
    let channel_id = channel.map(ChannelId::new);
    let envelope = build_post_envelope_with_payload_in_channel(
        &keys,
        topic,
        PayloadRef::InlineText { text: body.into() },
        vec![],
        vec![],
        None,
        if channel.is_some() {
            ObjectVisibility::Private
        } else {
            ObjectVisibility::Public
        },
        channel_id.as_ref(),
        Vec::new(),
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
    // 実クライアント（app-api `persist_post_object`）は索引 key も同時に書く（#1065）。
    let sort_key = timeline_sort_key(object.created_at, &object.object_id);
    for key in [
        stable_key("indexes/timeline", &format!("{sort_key}/{object_id}")),
        stable_key(
            "indexes/thread",
            &format!("{object_id}/{sort_key}/{object_id}"),
        ),
    ] {
        docs.apply_doc_op(
            replica,
            DocOp::SetJson {
                key,
                value: serde_json::json!({ "object_id": object_id }),
            },
        )
        .await
        .expect("index op");
    }
    object_id
}

/// 条件が成立するまで待つ（最長 30 秒。CI の並行負荷を考慮して余裕を持たせる）。
async fn wait_until<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..600 {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn worker_ingests_on_startup_and_reacts_to_replica_events() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping worker contract test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_worker_events").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    // 追加は需要として登録される（#1221 R5-E）。ここでは定期の見直しが需要を作らないことを確かめる。
    sqlx::query("UPDATE cn_index.supported_topics SET last_index_demand_at = NULL")
        .execute(&pool)
        .await?;
    let topic = TopicId::new("rust".to_string());
    let replica = kukuri_docs_sync::topic_replica_id("rust");
    let docs = Arc::new(MemoryDocsSync::default());
    persist_post(docs.as_ref(), &replica, &topic, "hello worker").await;

    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, entries) = participant_with_docs(&pool, docs.clone(), &projection, &state);

    // 定期見直しを長くして、2 件目が「変更通知」経由で取り込まれることを確かめる。
    let worker = IndexerWorker::new(
        participant,
        docs.clone(),
        Arc::clone(&state),
        fast_config(Duration::from_secs(120)),
    );
    let handle = worker.spawn();

    // 起動直後の 1 巡でサポート対象が取り込まれる。
    wait_until("initial ingest", || {
        let projection = projection.clone();
        async move {
            projection
                .count_scope(IndexScopeKind::PublicTopic, "rust")
                .await
                .unwrap_or(0)
                == 1
        }
    })
    .await;
    assert!(state.snapshot().worker_running);
    assert!(state.snapshot().last_sync_at.is_some());
    assert_eq!(state.snapshot().opened_scopes, 1);
    let initial_demand: bool = sqlx::query_scalar(
        "SELECT last_index_demand_at IS NOT NULL FROM cn_index.supported_topics
         WHERE kind = 'public_topic' AND id = 'rust'",
    )
    .fetch_one(&pool)
    .await?;
    assert!(!initial_demand, "periodic reuse is not new-content demand");

    // レプリカの変更通知で 2 件目が取り込まれる（定期見直しはまだ先）。
    let second = persist_post(docs.as_ref(), &replica, &topic, "event driven post").await;
    wait_until("event driven ingest", || {
        let projection = projection.clone();
        let second = second.clone();
        async move {
            projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", second.as_str())
                .await
                .unwrap_or(false)
        }
    })
    .await;
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", second.as_str()));
    wait_until("verified new-content demand", || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT last_index_demand_at IS NOT NULL FROM cn_index.supported_topics
                 WHERE kind = 'public_topic' AND id = 'rust'",
            )
            .fetch_one(&pool)
            .await
            .unwrap_or(false)
        }
    })
    .await;

    handle.shutdown().await;
    assert!(!state.snapshot().worker_running);
    Ok(())
}

#[tokio::test]
async fn removed_scope_is_deindexed_on_next_pass() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping worker contract test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_worker_deindex").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    let topic = TopicId::new("rust".to_string());
    let replica = kukuri_docs_sync::topic_replica_id("rust");
    let docs = Arc::new(MemoryDocsSync::default());
    let object_id = persist_post(docs.as_ref(), &replica, &topic, "to be removed").await;

    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, entries) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let worker = IndexerWorker::new(
        participant,
        docs.clone(),
        Arc::clone(&state),
        fast_config(Duration::from_millis(200)),
    );
    let handle = worker.spawn();

    wait_until("initial ingest", || {
        let projection = projection.clone();
        let object_id = object_id.clone();
        async move {
            projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", object_id.as_str())
                .await
                .unwrap_or(false)
        }
    })
    .await;

    // サポート対象から外すと、次の見直しで真実源 → 投影の順に索引解除される。
    remove_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    wait_until("scope de-index", || {
        let projection = projection.clone();
        async move {
            projection
                .count_scope(IndexScopeKind::PublicTopic, "rust")
                .await
                .unwrap_or(usize::MAX)
                == 0
        }
    })
    .await;
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", object_id.as_str()));

    handle.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn revoked_channel_secret_deindexes_private_channel() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping worker contract test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_worker_secret").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    // 購読リクエスト経路の結果（サポート対象入り + 秘密鍵登録済み）を直接作る。
    add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "secret-room").await?;
    register_channel_secret(&pool, &cipher(), "secret-room", TEST_NAMESPACE_SECRET).await?;

    let topic = TopicId::new("secret-room".to_string());
    let replica = kukuri_docs_sync::private_channel_replica_id("secret-room");
    let docs = Arc::new(MemoryDocsSync::default());
    // 投稿の下準備として、docs 側にも capability を登録してから replica を開く
    // （本番ではワーカーの restore_scopes が登録する。ここでは投稿を先に置くため）。
    docs.register_private_replica_secret(&replica, TEST_NAMESPACE_SECRET)
        .await?;
    let object_id = persist_post_in_channel(
        docs.as_ref(),
        &replica,
        &topic,
        "private post",
        Some("secret-room"),
    )
    .await;

    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, entries) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let worker = IndexerWorker::new(
        participant,
        docs.clone(),
        Arc::clone(&state),
        fast_config(Duration::from_millis(200)),
    );
    let handle = worker.spawn();

    // 秘密鍵が登録済みの private channel は公開トピックと同じループで取り込まれる。
    wait_until("private channel ingest", || {
        let projection = projection.clone();
        let object_id = object_id.clone();
        async move {
            projection
                .contains_object(
                    IndexScopeKind::PrivateChannel,
                    "secret-room",
                    object_id.as_str(),
                )
                .await
                .unwrap_or(false)
        }
    })
    .await;

    // 秘密鍵の失効で、次の見直しで索引解除される（鍵が無ければ索引しない）。
    remove_channel_secret(&pool, "secret-room").await?;
    wait_until("private channel de-index", || {
        let projection = projection.clone();
        async move {
            projection
                .count_scope(IndexScopeKind::PrivateChannel, "secret-room")
                .await
                .unwrap_or(usize::MAX)
                == 0
        }
    })
    .await;
    assert!(!entries.contains(
        IndexScopeKind::PrivateChannel,
        "secret-room",
        object_id.as_str()
    ));

    handle.shutdown().await;
    Ok(())
}

/// 特定 replica の走査だけ失敗する docs 同期（他 scope の取り込みを妨げない検証用）。
struct FailingScopeDocsSync {
    inner: Arc<MemoryDocsSync>,
    failing_replica: String,
}

#[async_trait]
impl DocsSync for FailingScopeDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> anyhow::Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> anyhow::Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> anyhow::Result<Vec<DocRecord>> {
        if replica_id.as_str() == self.failing_replica {
            anyhow::bail!("simulated replica query failure");
        }
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> anyhow::Result<kukuri_docs_sync::DocKeyPage> {
        if replica_id.as_str() == self.failing_replica {
            anyhow::bail!("simulated replica query failure");
        }
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> anyhow::Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> anyhow::Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

#[tokio::test]
async fn failing_scope_backs_off_without_blocking_others() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping worker contract test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_worker_backoff").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "healthy").await?;
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "broken").await?;

    let memory = Arc::new(MemoryDocsSync::default());
    let healthy_topic = TopicId::new("healthy".to_string());
    let healthy_replica = kukuri_docs_sync::topic_replica_id("healthy");
    let object_id = persist_post(
        memory.as_ref(),
        &healthy_replica,
        &healthy_topic,
        "still works",
    )
    .await;
    let docs: Arc<dyn DocsSync> = Arc::new(FailingScopeDocsSync {
        inner: memory.clone(),
        failing_replica: kukuri_docs_sync::topic_replica_id("broken")
            .as_str()
            .to_string(),
    });

    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, _entries) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let worker = IndexerWorker::new(
        participant,
        docs,
        Arc::clone(&state),
        fast_config(Duration::from_millis(200)),
    );
    let handle = worker.spawn();

    // 壊れた scope があっても健全な scope は取り込まれ、ワーカーは動き続ける。
    wait_until("healthy scope ingest", || {
        let projection = projection.clone();
        let object_id = object_id.clone();
        async move {
            projection
                .contains_object(IndexScopeKind::PublicTopic, "healthy", object_id.as_str())
                .await
                .unwrap_or(false)
        }
    })
    .await;
    wait_until("failure is observable", || {
        let state = Arc::clone(&state);
        async move {
            let snapshot = state.snapshot();
            snapshot.worker_running
                && snapshot.last_error.is_some()
                && snapshot.last_error_scope.as_deref() == Some("topic::broken")
        }
    })
    .await;

    let snapshot = state.snapshot();
    assert!(
        snapshot.last_ingest_at.is_some(),
        "healthy scope success must remain observable"
    );
    assert!(
        snapshot.last_sync_at.is_none(),
        "a full pass with any failed scope must not be recorded as successful"
    );

    handle.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn worker_restart_restores_supported_scopes() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping worker contract test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_worker_restart").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    let topic = TopicId::new("rust".to_string());
    let replica = kukuri_docs_sync::topic_replica_id("rust");
    let docs = Arc::new(MemoryDocsSync::default());
    let object_id = persist_post(docs.as_ref(), &replica, &topic, "survives restart").await;

    let projection = Arc::new(MemoryIndexProjection::default());

    // 1 回目のワーカー: 取り込んで停止する。
    let first_state = Arc::new(IndexerRuntimeState::default());
    let (participant, _entries) =
        participant_with_docs(&pool, docs.clone(), &projection, &first_state);
    let first = IndexerWorker::new(
        participant,
        docs.clone(),
        Arc::clone(&first_state),
        fast_config(Duration::from_millis(200)),
    );
    let first_handle = first.spawn();
    wait_until("first ingest", || {
        let projection = projection.clone();
        let object_id = object_id.clone();
        async move {
            projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", object_id.as_str())
                .await
                .unwrap_or(false)
        }
    })
    .await;
    first_handle.shutdown().await;
    assert!(!first_state.snapshot().worker_running);

    // 2 回目のワーカー: scope 管理 state からサポート対象が復元され、取り込みが再開する。
    let second_state = Arc::new(IndexerRuntimeState::default());
    let (participant, _entries) =
        participant_with_docs(&pool, docs.clone(), &projection, &second_state);
    let second = IndexerWorker::new(
        participant,
        docs.clone(),
        Arc::clone(&second_state),
        fast_config(Duration::from_millis(200)),
    );
    let second_handle = second.spawn();
    wait_until("restart restore", || {
        let state = Arc::clone(&second_state);
        async move {
            let snapshot = state.snapshot();
            snapshot.worker_running && snapshot.opened_scopes == 1 && snapshot.indexed >= 1
        }
    })
    .await;

    second_handle.shutdown().await;
    Ok(())
}

/// `objects/` prefix 全走査の回数を数える docs 同期（変更通知駆動の取り込みが key 単位である検証用）。
struct CountingDocsSync {
    inner: Arc<MemoryDocsSync>,
    whole_scope_queries: std::sync::atomic::AtomicUsize,
    exact_queries: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl DocsSync for CountingDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> anyhow::Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> anyhow::Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> anyhow::Result<Vec<DocRecord>> {
        if matches!(&query, DocQuery::Prefix(prefix) if prefix == &stable_key("objects", "")) {
            self.whole_scope_queries
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        if matches!(&query, DocQuery::Exact(_)) {
            self.exact_queries
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> anyhow::Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> anyhow::Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> anyhow::Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

#[tokio::test]
async fn worker_event_ingest_processes_only_changed_object_and_records_metrics() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping worker contract test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_worker_key_scoped").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    let topic = TopicId::new("rust".to_string());
    let replica = kukuri_docs_sync::topic_replica_id("rust");
    let inner = Arc::new(MemoryDocsSync::default());
    persist_post(inner.as_ref(), &replica, &topic, "first post").await;
    let docs = Arc::new(CountingDocsSync {
        inner: inner.clone(),
        whole_scope_queries: std::sync::atomic::AtomicUsize::new(0),
        exact_queries: std::sync::atomic::AtomicUsize::new(0),
    });

    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, entries) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let worker = IndexerWorker::new(
        participant,
        docs.clone(),
        Arc::clone(&state),
        fast_config(Duration::from_secs(120)),
    );
    let handle = worker.spawn();

    wait_until("initial ingest", || {
        let projection = projection.clone();
        async move {
            projection
                .count_scope(IndexScopeKind::PublicTopic, "rust")
                .await
                .unwrap_or(0)
                == 1
        }
    })
    .await;
    let whole_scope_after_startup = docs
        .whole_scope_queries
        .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        whole_scope_after_startup, 0,
        "startup must use a bounded index page"
    );
    let snapshot = state.snapshot();
    assert!(snapshot.last_pass_duration_ms.is_some());
    assert_eq!(snapshot.scans_fresh, 1);
    assert_eq!(snapshot.scans_reused, 0);
    assert!(snapshot.last_index_lag_secs.is_some());

    // 変更通知で 2 件目だけが取り込まれ、`objects/` prefix の全走査は増えない。
    let second = persist_post(inner.as_ref(), &replica, &topic, "event driven post").await;
    wait_until("event driven ingest", || {
        let projection = projection.clone();
        let second = second.clone();
        async move {
            projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", second.as_str())
                .await
                .unwrap_or(false)
        }
    })
    .await;
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", second.as_str()));
    assert_eq!(
        docs.whole_scope_queries
            .load(std::sync::atomic::Ordering::SeqCst),
        whole_scope_after_startup,
        "event-driven ingest must not rescan the whole scope"
    );
    wait_until("event ingest completion metric", || {
        let state = state.clone();
        async move { state.snapshot().last_event_ingest_duration_ms.is_some() }
    })
    .await;
    let snapshot = state.snapshot();
    assert!(snapshot.last_event_ingest_duration_ms.is_some());
    assert_eq!(
        snapshot.scans_fresh, 2,
        "only the new object reached the provider"
    );
    // #1065: 索引 key を含む実クライアントの key 集合でも全体見直しへ倒れない。
    assert_eq!(snapshot.scans_reused, 0);

    handle.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn periodic_public_poll_stops_at_the_current_index_window() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping worker contract test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_worker_bounded_window").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;

    let replica = kukuri_docs_sync::topic_replica_id("rust");
    let inner = Arc::new(MemoryDocsSync::default());
    inner.open_replica(&replica).await?;
    let created_at = chrono::Utc::now().timestamp();
    for index in 0..1_001 {
        let id = format!("object-{index:04}");
        inner
            .apply_doc_op(
                &replica,
                DocOp::SetBytes {
                    key: stable_key("indexes/timeline", &format!("{created_at:020}-{id}/{id}")),
                    value: Vec::new(),
                },
            )
            .await?;
    }
    let docs = Arc::new(CountingDocsSync {
        inner,
        whole_scope_queries: std::sync::atomic::AtomicUsize::new(0),
        exact_queries: std::sync::atomic::AtomicUsize::new(0),
    });
    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, _) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let event_participant = participant.clone();
    let worker = IndexerWorker::new(
        participant,
        docs.clone(),
        Arc::clone(&state),
        fast_config(Duration::from_secs(120)),
    );
    let handle = worker.spawn();
    wait_until("bounded public poll", || {
        let state = Arc::clone(&state);
        async move { state.snapshot().last_pass_duration_ms.is_some() }
    })
    .await;
    assert_eq!(
        docs.whole_scope_queries
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        docs.exact_queries.load(std::sync::atomic::Ordering::SeqCst),
        300,
        "100 selected IDs read state, envelope, and withdrawal once each"
    );
    event_participant
        .ingest_changed_keys(
            &ScopeReplica::from_scope(IndexScopeKind::PublicTopic, "rust"),
            &["unregistered/object/state".into()],
        )
        .await?;
    assert_eq!(
        docs.whole_scope_queries
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "unknown public keys must not trigger a full objects prefix read"
    );
    assert_eq!(
        docs.exact_queries.load(std::sync::atomic::Ordering::SeqCst),
        600,
        "unknown keys reuse the same 100-ID index window"
    );
    handle.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn private_periodic_and_unknown_key_reads_stop_at_the_current_window() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_private_bounded_window").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "secret-room").await?;
    register_channel_secret(&pool, &cipher(), "secret-room", TEST_NAMESPACE_SECRET).await?;

    let replica = kukuri_docs_sync::private_channel_replica_id("secret-room");
    let inner = Arc::new(MemoryDocsSync::default());
    inner
        .register_private_replica_secret(&replica, TEST_NAMESPACE_SECRET)
        .await?;
    let post = persist_post_in_channel(
        inner.as_ref(),
        &replica,
        &TopicId::new("secret-room"),
        "current private post",
        Some("secret-room"),
    )
    .await;
    let older = chrono::Utc::now().timestamp() - 60;
    for index in 0..1_001 {
        let id = format!("older-{index:04}");
        inner
            .apply_doc_op(
                &replica,
                DocOp::SetBytes {
                    key: stable_key("indexes/timeline", &format!("{older:020}-{id}/{id}")),
                    value: Vec::new(),
                },
            )
            .await?;
    }
    let docs = Arc::new(CountingDocsSync {
        inner,
        whole_scope_queries: std::sync::atomic::AtomicUsize::new(0),
        exact_queries: std::sync::atomic::AtomicUsize::new(0),
    });
    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, _) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let event_participant = participant.clone();
    let handle = IndexerWorker::new(
        participant,
        docs.clone(),
        state,
        fast_config(Duration::from_secs(120)),
    )
    .spawn();
    wait_until("bounded private poll", || {
        let projection = projection.clone();
        let post = post.clone();
        async move {
            projection
                .contains_object(IndexScopeKind::PrivateChannel, "secret-room", &post)
                .await
                .unwrap_or(false)
        }
    })
    .await;
    assert_eq!(
        docs.whole_scope_queries
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "private startup must not read the full objects prefix"
    );
    let first_exact = docs.exact_queries.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        (300..=330).contains(&first_exact),
        "one 100-ID window plus the verified post"
    );
    event_participant
        .ingest_changed_keys(
            &ScopeReplica::from_scope(IndexScopeKind::PrivateChannel, "secret-room"),
            &["unregistered/object/state".into()],
        )
        .await?;
    assert_eq!(
        docs.whole_scope_queries
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "unknown private keys must not read the full objects prefix"
    );
    let second_exact = docs.exact_queries.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        (300..=330).contains(&(second_exact - first_exact)),
        "unknown key revisits one bounded private window"
    );
    handle.shutdown().await;
    Ok(())
}

#[path = "worker_contracts/scope_admission.rs"]
mod scope_admission;
