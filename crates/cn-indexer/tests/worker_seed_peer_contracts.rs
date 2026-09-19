//! Issue #1154: event-driven ingest refreshes active bootstrap peers before fetching content.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use kukuri_blob_service::{BlobService, BlobStatus, StoredBlob};
use kukuri_cn_core::{
    ChannelSecretCipher, IndexScopeKind, MemoryIndexEntryStore, TestDatabase, add_supported_topic,
    connect_postgres, initialize_database, refresh_bootstrap_peer_registration,
};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::participant::IndexerParticipant;
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
    KukuriKeys, ObjectVisibility, PayloadRef, ReplicaId, TopicId, blob_hash,
    build_post_envelope_with_payload, timeline_sort_key,
};
use kukuri_docs_sync::{
    DocFetchPolicy, DocOp, DocQuery, DocRecord, DocsSync, MemoryDocsSync, stable_key,
};
use kukuri_transport::SeedPeer;
use sqlx::PgPool;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const TEST_SIGNER_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const TEST_CIPHER_KEY: &str = "worker-seed-contract-channel-secret-key-0123456789";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn participant(
    pool: &PgPool,
    docs: Arc<dyn DocsSync>,
    blobs: Arc<dyn BlobService>,
    projection: &Arc<MemoryIndexProjection>,
    state: &Arc<IndexerRuntimeState>,
    configured_seed_peers: Vec<SeedPeer>,
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
    let participant = Arc::new(
        IndexerParticipant::new(
            pool.clone(),
            docs,
            entries.clone(),
            projection.clone(),
            pipeline,
            ChannelSecretCipher::from_key_material(TEST_CIPHER_KEY).expect("cipher"),
        )
        .with_configured_seed_peers(configured_seed_peers)
        .with_blob_service(blobs),
    );
    (participant, entries)
}

fn worker_config() -> WorkerConfig {
    WorkerConfig {
        poll_interval: Duration::from_secs(120),
        event_debounce: Duration::from_millis(50),
        backoff_base: Duration::from_millis(100),
        backoff_max: Duration::from_millis(500),
    }
}

async fn register_bootstrap_peer(
    pool: &PgPool,
    subscriber_pubkey: &str,
    endpoint_id: &str,
    addr_hint: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_user.subscriber_accounts (subscriber_pubkey, status)
         VALUES ($1, 'active')",
    )
    .bind(subscriber_pubkey)
    .execute(pool)
    .await?;
    refresh_bootstrap_peer_registration(pool, subscriber_pubkey, endpoint_id, Some(addr_hint))
        .await?;
    Ok(())
}

async fn persist_post(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    topic: &TopicId,
    payload_ref: PayloadRef,
) -> String {
    let keys = KukuriKeys::generate();
    let envelope = build_post_envelope_with_payload(
        &keys,
        topic,
        payload_ref,
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
    )
    .expect("post envelope");
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

struct SeedRecordingDocsSync {
    inner: Arc<MemoryDocsSync>,
    seed_updates: AtomicUsize,
    latest_seed_peers: Mutex<Vec<SeedPeer>>,
}

impl SeedRecordingDocsSync {
    fn new(inner: Arc<MemoryDocsSync>) -> Self {
        Self {
            inner,
            seed_updates: AtomicUsize::new(0),
            latest_seed_peers: Mutex::new(Vec::new()),
        }
    }

    fn seed_update_count(&self) -> usize {
        self.seed_updates.load(Ordering::SeqCst)
    }

    fn latest_seed_peers(&self) -> Vec<SeedPeer> {
        self.latest_seed_peers
            .lock()
            .expect("docs seed peer mutex")
            .clone()
    }
}

#[async_trait]
impl DocsSync for SeedRecordingDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
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
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }

    async fn set_seed_peers(&self, peers: Vec<SeedPeer>) -> Result<()> {
        *self.latest_seed_peers.lock().expect("docs seed peer mutex") = peers;
        self.seed_updates.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct PeerGatedBlobService {
    required_endpoint_id: String,
    body: Vec<u8>,
    fail_next_seed_update: AtomicBool,
    seed_updates: AtomicUsize,
    latest_seed_peers: Mutex<Vec<SeedPeer>>,
}

impl PeerGatedBlobService {
    fn new(required_endpoint_id: String, body: Vec<u8>) -> Self {
        Self {
            required_endpoint_id,
            body,
            fail_next_seed_update: AtomicBool::new(false),
            seed_updates: AtomicUsize::new(0),
            latest_seed_peers: Mutex::new(Vec::new()),
        }
    }

    fn fail_next_seed_update(&self) {
        self.fail_next_seed_update.store(true, Ordering::SeqCst);
    }

    fn seed_update_count(&self) -> usize {
        self.seed_updates.load(Ordering::SeqCst)
    }

    fn latest_seed_peers(&self) -> Vec<SeedPeer> {
        self.latest_seed_peers
            .lock()
            .expect("blob seed peer mutex")
            .clone()
    }

    fn required_peer_is_configured(&self) -> bool {
        self.latest_seed_peers
            .lock()
            .expect("blob seed peer mutex")
            .iter()
            .any(|peer| peer.endpoint_id == self.required_endpoint_id)
    }
}

#[async_trait]
impl BlobService for PeerGatedBlobService {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        Ok(StoredBlob {
            hash: blob_hash(&data),
            mime: mime.to_string(),
            bytes: data.len() as u64,
        })
    }

    async fn fetch_blob(&self, _hash: &kukuri_core::BlobHash) -> Result<Option<Vec<u8>>> {
        Ok(self
            .required_peer_is_configured()
            .then(|| self.body.clone()))
    }

    async fn fetch_blob_ephemeral_bounded(
        &self,
        _hash: &kukuri_core::BlobHash,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>> {
        if self.body.len() as u64 > max_bytes {
            anyhow::bail!("fixture body exceeds bounded fetch limit");
        }
        Ok(self
            .required_peer_is_configured()
            .then(|| self.body.clone()))
    }

    async fn pin_blob(&self, _hash: &kukuri_core::BlobHash) -> Result<()> {
        Ok(())
    }

    async fn blob_status(&self, _hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        Ok(if self.required_peer_is_configured() {
            BlobStatus::Available
        } else {
            BlobStatus::Missing
        })
    }

    async fn local_blob_status(&self, _hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        Ok(BlobStatus::Missing)
    }

    async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
        Ok(())
    }

    async fn set_seed_peers(&self, peers: Vec<SeedPeer>) -> Result<()> {
        self.seed_updates.fetch_add(1, Ordering::SeqCst);
        if self.fail_next_seed_update.swap(false, Ordering::SeqCst) {
            anyhow::bail!("simulated blob seed update failure");
        }
        *self.latest_seed_peers.lock().expect("blob seed peer mutex") = peers;
        Ok(())
    }
}

struct Fixture {
    pool: PgPool,
    topic: TopicId,
    replica: ReplicaId,
    memory: Arc<MemoryDocsSync>,
    docs: Arc<SeedRecordingDocsSync>,
    blobs: Arc<PeerGatedBlobService>,
    projection: Arc<MemoryIndexProjection>,
    entries: Arc<MemoryIndexEntryStore>,
    state: Arc<IndexerRuntimeState>,
    handle: kukuri_cn_indexer::worker::WorkerHandle,
    operator_seed: SeedPeer,
    initial_object_id: String,
}

async fn start_fixture(admin_url: &str, database_name: &str) -> Result<Fixture> {
    let database = TestDatabase::create(admin_url, database_name).await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    let topic = TopicId::new("rust".to_string());
    let replica = kukuri_docs_sync::topic_replica_id("rust");
    register_bootstrap_peer(&pool, &"1".repeat(64), "peer-a", "192.0.2.10:4433").await?;

    let memory = Arc::new(MemoryDocsSync::default());
    let initial_object_id = persist_post(
        memory.as_ref(),
        &replica,
        &topic,
        PayloadRef::InlineText {
            text: "initial post".to_string(),
        },
    )
    .await;
    let docs = Arc::new(SeedRecordingDocsSync::new(memory.clone()));
    let blobs = Arc::new(PeerGatedBlobService::new(
        "peer-b".to_string(),
        b"post fetched from peer B".to_vec(),
    ));
    let operator_seed = SeedPeer {
        endpoint_id: "operator-seed".to_string(),
        addr_hint: Some("192.0.2.1:4433".to_string()),
    };
    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, entries) = participant(
        &pool,
        docs.clone(),
        blobs.clone(),
        &projection,
        &state,
        vec![operator_seed.clone()],
    );
    let handle = IndexerWorker::new(
        participant,
        docs.clone(),
        Arc::clone(&state),
        worker_config(),
    )
    .spawn();
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
    Ok(Fixture {
        pool,
        topic,
        replica,
        memory,
        docs,
        blobs,
        projection,
        entries,
        state,
        handle,
        operator_seed,
        initial_object_id,
    })
}

async fn register_peer_b_and_persist_blob_post(fixture: &Fixture) -> Result<String> {
    register_bootstrap_peer(&fixture.pool, &"2".repeat(64), "peer-b", "192.0.2.20:4433").await?;
    Ok(persist_post(
        fixture.memory.as_ref(),
        &fixture.replica,
        &fixture.topic,
        PayloadRef::BlobText {
            hash: blob_hash(&fixture.blobs.body),
            mime: "text/markdown".to_string(),
            bytes: fixture.blobs.body.len() as u64,
        },
    )
    .await)
}

#[tokio::test]
async fn event_batch_refreshes_seed_peers_before_fetching_blob_text() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping worker seed peer contract; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let fixture = start_fixture(admin_url.as_str(), "cn_worker_event_seed_refresh").await?;
    assert_eq!(fixture.docs.seed_update_count(), 1);
    assert_eq!(fixture.blobs.seed_update_count(), 1);
    assert!(!fixture.blobs.required_peer_is_configured());

    let object_id = register_peer_b_and_persist_blob_post(&fixture).await?;
    wait_until("event ingest after peer refresh", || {
        let projection = fixture.projection.clone();
        let object_id = object_id.clone();
        async move {
            projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", object_id.as_str())
                .await
                .unwrap_or(false)
        }
    })
    .await;

    assert!(
        fixture
            .entries
            .contains(IndexScopeKind::PublicTopic, "rust", object_id.as_str())
    );
    assert_eq!(
        fixture.docs.seed_update_count(),
        2,
        "all keys in one debounce batch share one docs peer refresh"
    );
    assert_eq!(
        fixture.blobs.seed_update_count(),
        2,
        "all keys in one debounce batch share one blob peer refresh"
    );
    for peers in [
        fixture.docs.latest_seed_peers(),
        fixture.blobs.latest_seed_peers(),
    ] {
        assert!(peers.contains(&fixture.operator_seed));
        assert!(peers.iter().any(|peer| peer.endpoint_id == "peer-a"));
        assert!(peers.iter().any(|peer| peer.endpoint_id == "peer-b"));
    }

    fixture.handle.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn failed_seed_refresh_skips_event_ingest_and_recovers_on_next_batch() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping worker seed peer contract; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let fixture = start_fixture(admin_url.as_str(), "cn_worker_event_seed_retry").await?;
    fixture.blobs.fail_next_seed_update();
    let object_id = register_peer_b_and_persist_blob_post(&fixture).await?;

    wait_until("failed seed refresh", || {
        let state = Arc::clone(&fixture.state);
        async move {
            state.snapshot().last_error.as_deref() == Some("simulated blob seed update failure")
        }
    })
    .await;
    assert!(
        !fixture
            .entries
            .contains(IndexScopeKind::PublicTopic, "rust", object_id.as_str())
    );
    assert!(fixture.entries.contains(
        IndexScopeKind::PublicTopic,
        "rust",
        fixture.initial_object_id.as_str()
    ));

    let state_key = stable_key("objects", &format!("{object_id}/state"));
    let state_value = fixture
        .memory
        .query_replica(&fixture.replica, DocQuery::Exact(state_key.clone()))
        .await?
        .into_iter()
        .next()
        .map(|record| serde_json::from_slice(&record.value))
        .transpose()?
        .expect("post state exists");
    fixture
        .memory
        .apply_doc_op(
            &fixture.replica,
            DocOp::SetJson {
                key: state_key,
                value: state_value,
            },
        )
        .await?;
    wait_until("event ingest after seed refresh retry", || {
        let projection = fixture.projection.clone();
        let object_id = object_id.clone();
        async move {
            projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", object_id.as_str())
                .await
                .unwrap_or(false)
        }
    })
    .await;
    assert_eq!(fixture.docs.seed_update_count(), 3);
    assert_eq!(fixture.blobs.seed_update_count(), 3);

    fixture.handle.shutdown().await;
    Ok(())
}
