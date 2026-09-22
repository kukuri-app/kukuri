use super::super::*;
use kukuri_core::BlobHash;
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};

#[derive(Default)]
struct LocalReadDocs {
    inner: MemoryDocsSync,
    reads: std::sync::atomic::AtomicUsize,
    forbidden: Arc<std::sync::atomic::AtomicUsize>,
}

struct LocalReadBlobs(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait]
impl BlobService for LocalReadBlobs {
    async fn put_blob(&self, _: Vec<u8>, _: &str) -> Result<StoredBlob> {
        anyhow::bail!("read only")
    }
    async fn fetch_blob(&self, _: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    async fn pin_blob(&self, _: &BlobHash) -> Result<()> {
        anyhow::bail!("read only")
    }
    async fn blob_status(&self, _: &BlobHash) -> Result<BlobStatus> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(BlobStatus::Missing)
    }
    async fn local_blob_status(&self, _: &BlobHash) -> Result<BlobStatus> {
        Ok(BlobStatus::Missing)
    }
    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        anyhow::bail!("read only")
    }
}

#[async_trait]
impl DocsSync for LocalReadDocs {
    async fn open_replica(&self, _: &ReplicaId) -> Result<()> {
        self.forbidden.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("resolver must not start sync")
    }
    async fn apply_doc_op(&self, _: &ReplicaId, _: DocOp) -> Result<()> {
        anyhow::bail!("read only")
    }
    async fn query_replica_with_policy(
        &self,
        replica: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        if policy != DocFetchPolicy::LocalOnly {
            self.forbidden.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("remote read");
        }
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner
            .query_replica_with_policy(replica, query, policy)
            .await
    }
    async fn query_replica_keys(
        &self,
        replica: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.query_replica_keys(replica, query).await
    }
    async fn query_replica_by_author(
        &self,
        replica: &ReplicaId,
        author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<kukuri_docs_sync::DocRecord>> {
        if policy != DocFetchPolicy::LocalOnly {
            self.forbidden.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("remote read");
        }
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner
            .query_replica_by_author(replica, author, key, policy)
            .await
    }
    async fn subscribe_replica(&self, _: &ReplicaId) -> Result<kukuri_docs_sync::DocEventStream> {
        self.forbidden.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("resolver must not subscribe")
    }
    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        anyhow::bail!("read only")
    }
}

fn observed_app() -> (AppService, Arc<LocalReadDocs>) {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(LocalReadDocs::default());
    (
        app_service_from_dependencies(
            store.clone(),
            store,
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            Arc::new(NoopHintTransport),
            docs.clone(),
            Arc::new(LocalReadBlobs(docs.forbidden.clone())),
            generate_keys(),
        ),
        docs,
    )
}

async fn seed_bucket_post(
    docs: &LocalReadDocs,
    envelope: &KukuriEnvelope,
) -> (ReplicaId, CommunityIndexPostResolveInput) {
    let header = envelope.to_post_object().expect("header").expect("post");
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: header.topic_id.as_str().into(),
        },
        TimeBucket::from_unix_seconds(envelope.created_at).expect("time"),
    )
    .expect("bucket")
    .replica_id();
    let input = CommunityIndexPostResolveInput {
        key: envelope.id.as_str().into(),
        topic: header.topic_id.as_str().into(),
        object_id: envelope.id.as_str().into(),
        author_pubkey: envelope.pubkey.as_str().into(),
        channel_ref: ChannelRef::Public,
        source_replica_id: Some(replica.as_str().into()),
    };
    persist_post_object(&docs.inner, &replica, header, envelope.clone())
        .await
        .expect("seed");
    (replica, input)
}

#[tokio::test]
async fn cached_index_post_rechecks_a_locally_arrived_withdrawal() {
    let (app, docs) = observed_app();
    let keys = generate_keys();
    let post = kukuri_core::build_post_envelope(&keys, &TopicId::new("topic"), "hide later", None)
        .expect("post");
    let (replica, mut input) = seed_bucket_post(&docs, &post).await;
    assert_eq!(
        app.resolve_community_index_posts(vec![input.clone()])
            .await
            .expect("first")
            .entries[0]
            .post
            .as_ref()
            .expect("resolved")
            .content,
        "hide later"
    );
    let withdrawal = build_post_withdrawal_envelope(
        &keys,
        &post,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("withdrawal");
    docs.inner
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: format!("withdrawals/{}/state", post.id.as_str()),
                value: serde_json::to_value(withdrawal).expect("json"),
            },
        )
        .await
        .expect("local arrival");
    // 次のCN応答の手がかりが違っても、検証済みcacheの保存先を優先して確認する。
    input.source_replica_id = Some(topic_replica_id("topic").as_str().into());
    let response = app
        .resolve_community_index_posts(vec![input])
        .await
        .expect("second");
    let withdrawn = response.entries[0].post.as_ref().expect("post tombstone");
    assert!(withdrawn.withdrawal.is_some());
    assert!(withdrawn.content.is_empty());
    assert!(withdrawn.attachments.is_empty());
    assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn resolving_a_bucket_repost_does_not_spawn_remote_withdrawal_reads() {
    let (app, docs) = observed_app();
    let post = kukuri_core::build_repost_envelope(
        &generate_keys(),
        &TopicId::new("topic"),
        RepostSourceSnapshotV1 {
            source_object_id: EnvelopeId::from("source"),
            source_topic_id: TopicId::new("source-topic"),
            source_author_pubkey: Pubkey::from("source-author"),
            source_object_kind: "post".into(),
            content: "snapshot".into(),
            attachments: Vec::new(),
            reply_to_object_id: None,
            root_id: None,
            content_labels: Vec::new(),
        },
        Some("commentary"),
    )
    .expect("repost");
    let (_, input) = seed_bucket_post(&docs, &post).await;
    let response = app
        .resolve_community_index_posts(vec![input])
        .await
        .expect("resolve");
    assert!(response.entries[0].post.is_some());
    sleep(Duration::from_millis(100)).await;
    assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn resolving_a_bucket_reply_does_not_fetch_the_nonlocal_parent_body() {
    let (app, docs) = observed_app();
    let topic = TopicId::new("topic");
    let keys = generate_keys();
    let parent = build_post_envelope_with_payload_in_channel(
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: BlobHash::new("a".repeat(64)),
            mime: "text/plain".into(),
            bytes: 12,
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("parent");
    let reply =
        kukuri_core::build_post_envelope(&keys, &topic, "reply", Some(&parent)).expect("reply");
    seed_bucket_post(&docs, &parent).await;
    let (_, input) = seed_bucket_post(&docs, &reply).await;
    let response = app
        .resolve_community_index_posts(vec![input])
        .await
        .expect("resolve");
    assert!(response.entries[0].post.is_some());
    sleep(Duration::from_millis(100)).await;
    assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn related_post_withdrawals_are_reflected_before_local_index_views() {
    for repost in [false, true] {
        let (app, docs) = observed_app();
        let topic = TopicId::new("topic");
        let keys = generate_keys();
        let parent = kukuri_core::build_post_envelope(&keys, &topic, "withdrawn source", None)
            .expect("parent");
        let (replica, parent_input) = seed_bucket_post(&docs, &parent).await;
        app.resolve_community_index_posts(vec![parent_input])
            .await
            .expect("cache parent");
        let withdrawal = build_post_withdrawal_envelope(
            &keys,
            &parent,
            1,
            None,
            WithdrawalReasonVisibility::Private,
            None,
        )
        .expect("withdrawal");
        docs.inner
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: format!("withdrawals/{}/state", parent.id.as_str()),
                    value: serde_json::to_value(withdrawal).expect("json"),
                },
            )
            .await
            .expect("arrived locally");
        let child = if repost {
            kukuri_core::build_repost_envelope(
                &keys,
                &topic,
                RepostSourceSnapshotV1 {
                    source_object_id: parent.id.clone(),
                    source_topic_id: topic.clone(),
                    source_author_pubkey: parent.pubkey.clone(),
                    source_object_kind: "post".into(),
                    content: "withdrawn source".into(),
                    attachments: Vec::new(),
                    reply_to_object_id: None,
                    root_id: None,
                    content_labels: Vec::new(),
                },
                Some("commentary"),
            )
            .expect("repost")
        } else {
            kukuri_core::build_post_envelope(&keys, &topic, "reply", Some(&parent)).expect("reply")
        };
        let (_, input) = seed_bucket_post(&docs, &child).await;
        let response = app
            .resolve_community_index_posts(vec![input])
            .await
            .expect("resolve child");
        let view = response.entries[0].post.as_ref().expect("child");
        if repost {
            assert!(
                view.repost_of
                    .as_ref()
                    .expect("snapshot")
                    .content
                    .is_empty()
            );
        } else {
            assert!(
                view.reply_preview
                    .as_ref()
                    .expect("parent preview")
                    .content
                    .is_empty()
            );
        }
        assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn community_index_resolves_the_signed_post_in_the_supplied_bucket() {
    let (app, docs) = observed_app();
    let topic = TopicId::new("kukuri:topic:index-bucket");
    let envelope = kukuri_core::build_post_envelope(&generate_keys(), &topic, "bucket body", None)
        .expect("signed post");
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().to_string(),
        },
        TimeBucket::from_unix_seconds(envelope.created_at).expect("time"),
    )
    .expect("bucket")
    .replica_id();
    persist_post_object(
        &docs.inner,
        &replica,
        envelope.to_post_object().expect("header").expect("post"),
        envelope.clone(),
    )
    .await
    .expect("local bucket post");
    // serde境界を通す。旧readerは未知fieldを無視して旧topic replicaを探すため、このcontractに失敗する。
    let input = serde_json::from_value::<CommunityIndexPostResolveInput>(serde_json::json!({
        "key": "bucket-result",
        "topic": topic.as_str(),
        "object_id": envelope.id.as_str(),
        "author_pubkey": envelope.pubkey.as_str(),
        "channel_ref": { "kind": "public" },
        "source_replica_id": replica.as_str(),
    }))
    .expect("wire input");
    let response = app
        .resolve_community_index_posts(vec![input])
        .await
        .expect("resolve");
    assert_eq!(
        response.entries[0]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("bucket body")
    );
    assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_index_locators_do_not_read_docs_or_start_sync() {
    let (app, docs) = observed_app();
    for source in [
        "topic::other",
        "bucket::v2::topic::746f706963::1",
        "bucket::v1::topic::6f74686572::1",
        "bucket::v1::author::61::1",
        "bucket::v1::channel::63::65::1",
        "bucket::v1::topic::746f706963::01",
    ] {
        let input = serde_json::from_value::<CommunityIndexPostResolveInput>(serde_json::json!({
            "key": source, "topic": "topic", "object_id": "post", "author_pubkey": "author",
            "channel_ref": {"kind":"public"}, "source_replica_id": source,
        }))
        .expect("input");
        let response = app
            .resolve_community_index_posts(vec![input])
            .await
            .expect("unresolved");
        assert!(response.entries[0].post.is_none());
    }
    assert_eq!(docs.reads.load(Ordering::SeqCst), 0);
    assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_bucket_locator_does_not_make_a_misplaced_signature_canonical() {
    let (app, docs) = observed_app();
    let topic = TopicId::new("topic");
    let envelope = kukuri_core::build_post_envelope(&generate_keys(), &topic, "wrong day", None)
        .expect("signed post");
    let day = TimeBucket::from_unix_seconds(envelope.created_at)
        .expect("day")
        .index();
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: "topic".into(),
        },
        TimeBucket::from_index(day + 1).expect("next day"),
    )
    .expect("bucket")
    .replica_id();
    persist_post_object(
        &docs.inner,
        &replica,
        envelope.to_post_object().expect("header").expect("post"),
        envelope.clone(),
    )
    .await
    .expect("misplaced copy");
    let input: CommunityIndexPostResolveInput = serde_json::from_value(serde_json::json!({
        "key": "copy", "topic": "topic", "object_id": envelope.id.as_str(),
        "author_pubkey": envelope.pubkey.as_str(), "channel_ref": {"kind":"public"},
        "source_replica_id": replica.as_str(),
    }))
    .expect("input");
    let response = app
        .resolve_community_index_posts(vec![input])
        .await
        .expect("unresolved");
    assert!(response.entries[0].post.is_none());
    assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn one_cached_scope_mismatch_does_not_hide_a_valid_index_result() {
    let (app, docs) = observed_app();
    let other = kukuri_core::build_post_envelope(
        &generate_keys(),
        &TopicId::new("other"),
        "other body",
        None,
    )
    .expect("other");
    let (_, other_input) = seed_bucket_post(&docs, &other).await;
    app.resolve_community_index_posts(vec![other_input.clone()])
        .await
        .expect("cache other scope");
    let good = kukuri_core::build_post_envelope(
        &generate_keys(),
        &TopicId::new("topic"),
        "good body",
        None,
    )
    .expect("good");
    let (_, good_input) = seed_bucket_post(&docs, &good).await;
    let bad_input = CommunityIndexPostResolveInput {
        topic: good_input.topic.clone(),
        source_replica_id: good_input.source_replica_id.clone(),
        ..other_input
    };
    let response = app
        .resolve_community_index_posts(vec![bad_input, good_input])
        .await
        .expect("mixed results");
    assert!(response.entries[0].post.is_none());
    assert_eq!(
        response.entries[1]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("good body")
    );
    assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
}
