//! 取込みが観測する 2 者間のアクション（#1221 R5-E）の contract テスト（`KUKURI_CN_RUN_INTEGRATION_TESTS=1`）。
//!
//! 返信・repost・引用・リアクションを行として保存し、新しいアクションのときだけ相手の author replica の
//! `graph/follows/<actor>` を 1 key 読む。自分自身へのアクションと private channel の投稿は保存しない。
//! 索引から投稿が消えると起点のアクションも消え、取り消したリアクションとフォローは行が消える。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use sqlx::PgPool;

use kukuri_cn_core::{
    IndexEntryStore, IndexScopeKind, PgIndexEntryStore, PgSafetyArtifactStore, TestDatabase,
    add_supported_topic, connect_postgres, initialize_database,
};
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::projection::MemoryIndexProjection;
use kukuri_cn_safety::{MockSafetyProvider, ModerationEventSigner};
use kukuri_cn_safety_runtime::clock::SystemScanClock;
use kukuri_cn_safety_runtime::id::UuidEventIdGenerator;
use kukuri_cn_safety_runtime::{
    SafetyOrchestrator, SafetyScanService, Secp256k1ModerationEventSigner,
};
use kukuri_core::{
    ChannelId, FollowEdgeDocV1, FollowEdgeStatus, KukuriEnvelope, KukuriKeys, ObjectStatus,
    ObjectVisibility, PayloadRef, Pubkey, ReactionKeyV1, ReplicaId, RepostSourceSnapshotV1,
    TopicId, build_follow_edge_envelope, build_post_envelope,
    build_post_envelope_with_payload_in_channel, build_reaction_envelope, build_repost_envelope,
    deterministic_reaction_id, parse_follow_edge, parse_reaction, timeline_sort_key,
};
use kukuri_docs_sync::{
    DocEventStream, DocFetchPolicy, DocKeyPage, DocKeyQuery, DocOp, DocQuery, DocRecord, DocsSync,
    MemoryDocsSync, author_replica_id, private_channel_replica_id, stable_key, topic_replica_id,
};

const TOPIC: &str = "rust";

/// bucket reader と同じく remote の reader として読む docs（author replica の読取り回数を数える）。
struct RemoteReader {
    inner: Arc<MemoryDocsSync>,
    author_reads: AtomicUsize,
}

#[async_trait]
impl DocsSync for RemoteReader {
    fn remote_reader_id(&self) -> Option<String> {
        Some("provider".into())
    }
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
        if replica_id.as_str().starts_with("author::") {
            self.author_reads.fetch_add(1, Ordering::SeqCst);
        }
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }
    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
    }
    async fn subscribe_replica(&self, replica_id: &ReplicaId) -> Result<DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

fn pipeline(pool: &PgPool, docs: Arc<RemoteReader>) -> Result<IngestPipeline> {
    let signer = Secp256k1ModerationEventSigner::from_secret(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )?;
    let issuer = signer.issuer_node_id().to_string();
    let orchestrator = SafetyOrchestrator::builder(
        &issuer,
        Arc::new(SystemScanClock),
        Arc::new(UuidEventIdGenerator),
    )
    .provider(Arc::new(MockSafetyProvider::known_csam("mock-known-csam")))
    .build()?;
    let scan = SafetyScanService::builder(
        Arc::new(orchestrator),
        Arc::new(PgSafetyArtifactStore::new(pool.clone())),
    )
    .signer(Arc::new(signer))
    .build()?;
    Ok(IngestPipeline::new(
        docs,
        Arc::new(scan),
        Arc::new(PgIndexEntryStore::new(pool.clone())),
        Arc::new(MemoryIndexProjection::new()),
    )
    .with_relation_pool(pool.clone()))
}

async fn set(docs: &MemoryDocsSync, replica: &ReplicaId, key: String, value: serde_json::Value) {
    docs.open_replica(replica).await.expect("open");
    docs.apply_doc_op(replica, DocOp::SetJson { key, value })
        .await
        .expect("doc op");
}

/// 実クライアントと同じ state / envelope / timeline の key で投稿を書く。
async fn write_post(
    docs: &MemoryDocsSync,
    replica: &ReplicaId,
    envelope: &KukuriEnvelope,
) -> String {
    let object = envelope
        .to_post_object()
        .expect("test fixture")
        .expect("test fixture");
    let id = object.object_id.as_str().to_string();
    set(
        docs,
        replica,
        format!("objects/{id}/state"),
        serde_json::to_value(&object).expect("test fixture"),
    )
    .await;
    set(
        docs,
        replica,
        format!("objects/{id}/envelope"),
        serde_json::to_value(envelope).expect("test fixture"),
    )
    .await;
    set(
        docs,
        replica,
        format!(
            "indexes/timeline/{}/{id}",
            timeline_sort_key(object.created_at, &object.object_id)
        ),
        serde_json::json!({ "object_id": id }),
    )
    .await;
    id
}

async fn write_reaction(
    docs: &MemoryDocsSync,
    keys: &KukuriKeys,
    target: &KukuriEnvelope,
    status: ObjectStatus,
) -> String {
    let replica = topic_replica_id(TOPIC);
    let reaction_key = ReactionKeyV1::Emoji {
        emoji: "👍".into()
    };
    let id = deterministic_reaction_id(
        &replica,
        &target.id,
        &keys.public_key(),
        &reaction_key.normalized_key().expect("test fixture"),
    );
    let envelope = build_reaction_envelope(
        keys,
        &TopicId::new(TOPIC),
        None,
        &target.id,
        reaction_key,
        &id,
        status,
    )
    .expect("test fixture");
    let doc = parse_reaction(&envelope)
        .expect("test fixture")
        .expect("test fixture");
    let base = format!("reactions/{}/{}", target.id.as_str(), id.as_str());
    set(
        docs,
        &replica,
        format!("{base}/state"),
        serde_json::to_value(doc).expect("test fixture"),
    )
    .await;
    set(
        docs,
        &replica,
        format!("{base}/envelope"),
        serde_json::to_value(envelope).expect("test fixture"),
    )
    .await;
    format!("{base}/envelope")
}

async fn write_follow(
    docs: &MemoryDocsSync,
    subject: &KukuriKeys,
    target: &Pubkey,
    status: FollowEdgeStatus,
) {
    let envelope = build_follow_edge_envelope(subject, target, status).expect("test fixture");
    let edge = parse_follow_edge(&envelope)
        .expect("test fixture")
        .expect("test fixture");
    let replica = author_replica_id(subject.public_key_hex().as_str());
    set(
        docs,
        &replica,
        stable_key("graph/follows", target.as_str()),
        serde_json::to_value(FollowEdgeDocV1 {
            subject_pubkey: edge.subject_pubkey,
            target_pubkey: edge.target_pubkey,
            status: edge.status,
            updated_at: edge.updated_at,
            envelope_id: edge.envelope_id,
        })
        .expect("test fixture"),
    )
    .await;
    set(
        docs,
        &replica,
        stable_key("envelopes", envelope.id.as_str()),
        serde_json::to_value(&envelope).expect("test fixture"),
    )
    .await;
}

async fn actions(pool: &PgPool) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT kind, actor_pubkey, target_pubkey FROM cn_index.relation_actions
         ORDER BY kind, actor_pubkey, target_pubkey",
    )
    .fetch_all(pool)
    .await?)
}

fn row(kind: &str, actor: &KukuriKeys, target: &KukuriKeys) -> (String, String, String) {
    (kind.into(), actor.public_key_hex(), target.public_key_hex())
}

#[tokio::test]
async fn ingest_records_two_party_actions_and_reads_the_reverse_follow_once() -> Result<()> {
    let Some(admin_url) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        eprintln!("skipping relation ingest test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_relation_ingest").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, TOPIC).await?;
        add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "room").await?;
        sqlx::query(
            "INSERT INTO cn_index.channel_secrets (channel_id, nonce, ciphertext) VALUES ('room', '\\x00', '\\x00')",
        )
        .execute(&pool)
        .await?;
        let memory = Arc::new(MemoryDocsSync::default());
        let docs = Arc::new(RemoteReader {
            inner: memory.clone(),
            author_reads: AtomicUsize::new(0),
        });
        let pipeline = pipeline(&pool, docs.clone())?;
        let [a, b, c, d] = [(); 4].map(|_| KukuriKeys::generate());
        let topic = TopicId::new(TOPIC);
        let replica = topic_replica_id(TOPIC);
        let root = build_post_envelope(&a, &topic, "root by a", None)?;
        write_post(&memory, &replica, &root).await;
        pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, TOPIC, &replica)
            .await?;

        // A は B をフォローしている（C と D はフォローしていない）。
        write_follow(&memory, &a, &b.public_key(), FollowEdgeStatus::Active).await;
        let reply = build_post_envelope(&b, &topic, "reply by b", Some(&root))?;
        let reply_id = write_post(&memory, &replica, &reply).await;
        let repost = build_repost_envelope(
            &c,
            &topic,
            RepostSourceSnapshotV1 {
                source_object_id: root.id.clone(),
                source_topic_id: topic.clone(),
                source_author_pubkey: a.public_key(),
                source_object_kind: "post".into(),
                content: "root by a".into(),
                attachments: Vec::new(),
                reply_to_object_id: None,
                root_id: None,
                content_labels: Vec::new(),
            },
            Some("quote by c"),
        )?;
        write_post(&memory, &replica, &repost).await;
        let reaction_key = write_reaction(&memory, &d, &root, ObjectStatus::Active).await;
        // 自分への返信と private channel の返信は関係にしない。
        write_post(&memory, &replica, &build_post_envelope(&a, &topic, "self", Some(&root))?).await;
        let private_root = build_post_envelope_with_payload_in_channel(
            &a,
            &topic,
            PayloadRef::InlineText { text: "private".into() },
            Vec::new(),
            Vec::new(),
            None,
            ObjectVisibility::Private,
            Some(&ChannelId::new("room")),
            Vec::new(),
        )?;
        let private_reply = build_post_envelope_with_payload_in_channel(
            &b,
            &topic,
            PayloadRef::InlineText { text: "private reply".into() },
            Vec::new(),
            Vec::new(),
            Some(&private_root),
            ObjectVisibility::Private,
            Some(&ChannelId::new("room")),
            Vec::new(),
        )?;
        let channel = private_channel_replica_id("room");
        memory
            .register_private_replica_secret(&channel, &"11".repeat(32))
            .await?;
        write_post(&memory, &channel, &private_root).await;
        write_post(&memory, &channel, &private_reply).await;
        for _ in 0..2 {
            pipeline
                .ingest_recent_scope(IndexScopeKind::PrivateChannel, "room", &channel)
                .await?;
        }

        pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, TOPIC, &replica)
            .await?;
        let mut expected = vec![
            row("follow", &a, &b),
            row("reaction", &d, &a),
            row("reply", &b, &a),
            row("repost", &c, &a),
        ];
        expected.sort();
        assert_eq!(actions(&pool).await?, expected);
        // 新しいアクション 3 件について、相手のフォローを 1 key（doc と署名つき envelope）ずつ読んだ。
        let reads = docs.author_reads.load(Ordering::SeqCst);
        assert_eq!(reads, 4, "follow doc for b/c/d plus b's signed envelope");
        // 同じ内容をもう一度取り込んでも、新しいアクションは無く、author replica を読まない。
        pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, TOPIC, &replica)
            .await?;
        assert_eq!(docs.author_reads.load(Ordering::SeqCst), reads);

        // 取り消したリアクションは変更通知で読み直して消える。
        write_reaction(&memory, &d, &root, ObjectStatus::Deleted).await;
        pipeline
            .ingest_changed_keys(IndexScopeKind::PublicTopic, TOPIC, &replica, &[reaction_key])
            .await?;
        // 返信の投稿が索引から消えると、起点の返信も消える。
        PgIndexEntryStore::new(pool.clone())
            .remove_entry(IndexScopeKind::PublicTopic, TOPIC, &reply_id)
            .await?;
        let mut expected = vec![row("follow", &a, &b), row("repost", &c, &a)];
        expected.sort();
        assert_eq!(actions(&pool).await?, expected);

        // フォローを取り消した後の新しいアクションで、取り消しを読んで行を消す。
        write_follow(&memory, &a, &b.public_key(), FollowEdgeStatus::Revoked).await;
        let again = build_post_envelope(&b, &topic, "second reply by b", Some(&root))?;
        write_post(&memory, &replica, &again).await;
        pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, TOPIC, &replica)
            .await?;
        // 最初の返信は replica に残っているため、索引の窓で再び取り込まれて行が戻る。
        let mut expected = vec![
            row("reply", &b, &a),
            row("reply", &b, &a),
            row("repost", &c, &a),
        ];
        expected.sort();
        assert_eq!(actions(&pool).await?, expected);
        Ok(())
    }
    .await;
    pool.close().await;
    database.cleanup().await?;
    result
}
