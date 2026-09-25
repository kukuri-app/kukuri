use super::super::*;
use kukuri_core::{BlobHash, build_post_envelope};
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};

#[derive(Default)]
struct LocalReadDocs {
    inner: MemoryDocsSync,
    remote: std::sync::Mutex<Option<Arc<dyn DocsSync>>>,
    reads: std::sync::atomic::AtomicUsize,
    remote_requests: std::sync::atomic::AtomicUsize,
    forbidden: Arc<std::sync::atomic::AtomicUsize>,
}

struct LocalReadBlobs(Arc<std::sync::atomic::AtomicUsize>, BlobStatus);

struct PendingRemoteDocs;

#[async_trait]
impl DocsSync for PendingRemoteDocs {
    async fn open_replica(&self, _: &ReplicaId) -> Result<()> {
        anyhow::bail!("remote reader cannot open")
    }
    async fn apply_doc_op(&self, _: &ReplicaId, _: DocOp) -> Result<()> {
        anyhow::bail!("remote reader is read-only")
    }
    async fn query_replica_with_policy(
        &self,
        _: &ReplicaId,
        _: DocQuery,
        _: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        std::future::pending().await
    }
    async fn query_replica_by_author(
        &self,
        _: &ReplicaId,
        _: &str,
        _: &str,
        _: DocFetchPolicy,
    ) -> Result<Option<kukuri_docs_sync::DocRecord>> {
        std::future::pending().await
    }
    async fn subscribe_replica(&self, _: &ReplicaId) -> Result<kukuri_docs_sync::DocEventStream> {
        anyhow::bail!("remote reader cannot subscribe")
    }
    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        anyhow::bail!("remote reader cannot import peers")
    }
}

#[async_trait]
impl BlobService for LocalReadBlobs {
    async fn fetch_local_blob(
        &self,
        _hash: &kukuri_core::BlobHash,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if self.1 == BlobStatus::Available {
            anyhow::bail!("injected local read failure after status check");
        }
        Ok(None)
    }

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
        Ok(self.1.clone())
    }
    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        anyhow::bail!("read only")
    }
}

#[async_trait]
impl DocsSync for LocalReadDocs {
    async fn remote_readers(
        &self,
        _: &ReplicaId,
        _: Option<[u8; 32]>,
        _: Vec<kukuri_transport::SeedPeer>,
    ) -> Result<Vec<Arc<dyn DocsSync>>> {
        self.remote_requests.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .remote
            .lock()
            .expect("remote source poisoned")
            .iter()
            .cloned()
            .collect())
    }
    async fn query_local_source(
        &self,
        replica: &ReplicaId,
        key: &str,
        author: Option<&str>,
        limit: usize,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner
            .query_local_source(replica, key, author, limit)
            .await
    }
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

#[tokio::test]
async fn private_index_source_uses_only_the_joined_epoch_and_stops_after_leave() -> Result<()> {
    let (app, docs) = observed_app();
    let topic = TopicId::new("private-source");
    let channel = ChannelId::new("room");
    let keys = generate_keys();
    app.joined_private_channels.lock().await.insert(
        joined_private_channel_key(topic.as_str(), channel.as_str()),
        JoinedPrivateChannelState {
            generation: 1,
            topic_id: topic.as_str().into(),
            channel_id: channel.clone(),
            label: "room".into(),
            creator_pubkey: keys.public_key_hex(),
            owner_pubkey: keys.public_key_hex(),
            joined_via_pubkey: None,
            audience_kind: ChannelAudienceKind::InviteOnly,
            current_epoch_id: "e1".into(),
            current_epoch_secret_hex: hex::encode([7; 32]),
            archived_epochs: Vec::new(),
        },
    );
    let post = build_post_envelope_with_payload_in_channel(
        &keys,
        &topic,
        PayloadRef::InlineText {
            text: "private body".into(),
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Private,
        Some(&channel),
        Vec::new(),
    )?;
    let bucket = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: channel.as_str().into(),
            epoch_id: "e1".into(),
        },
        TimeBucket::from_unix_seconds(post.created_at)?,
    )?;
    let replica = bucket.replica_id();
    let remote = Arc::new(MemoryDocsSync::default());
    remote
        .register_private_replica_secret(
            &replica,
            &hex::encode(bucket.derive_private_secret(&[7; 32])?),
        )
        .await?;
    persist_post_object(
        remote.as_ref(),
        &replica,
        post.to_post_object()?.unwrap(),
        post.clone(),
    )
    .await?;
    *docs.remote.lock().unwrap() = Some(remote);
    let input = CommunityIndexPostResolveInput {
        key: "private-hit".into(),
        topic: topic.as_str().into(),
        object_id: post.id.as_str().into(),
        author_pubkey: post.pubkey.as_str().into(),
        channel_ref: ChannelRef::PrivateChannel {
            channel_id: channel.clone(),
        },
        source_replica_id: Some(replica.as_str().into()),
    };
    let wrong = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: channel.as_str().into(),
            epoch_id: "e2".into(),
        },
        bucket.bucket(),
    )?
    .replica_id();
    let unresolved = app
        .resolve_community_index_posts(vec![CommunityIndexPostResolveInput {
            key: "wrong-epoch".into(),
            source_replica_id: Some(wrong.as_str().into()),
            ..input.clone()
        }])
        .await?;
    assert!(unresolved.entries[0].post.is_none());
    assert_eq!(docs.remote_requests.load(Ordering::SeqCst), 0);
    let resolved = app
        .resolve_community_index_posts(vec![input.clone()])
        .await?;
    assert_eq!(
        resolved.entries[0]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("private body")
    );
    let calls = docs.remote_requests.load(Ordering::SeqCst);
    app.joined_private_channels
        .lock()
        .await
        .remove(&joined_private_channel_key(
            topic.as_str(),
            channel.as_str(),
        ));
    let after_leave = app.resolve_community_index_posts(vec![input]).await?;
    assert!(after_leave.entries[0].post.is_none());
    assert_eq!(docs.remote_requests.load(Ordering::SeqCst), calls);
    Ok(())
}

#[tokio::test]
async fn private_legacy_page_selects_the_cursor_epoch_without_scanning_every_replica() -> Result<()>
{
    let (app, _) = observed_app();
    let topic = "private-history";
    let channel = ChannelId::new("room");
    let keys = generate_keys();
    let same_day_earlier = format!("epoch-{}-old", 6 * 86_400_000 + 1_800_000);
    let same_day_old = format!("epoch-{}-old", 6 * 86_400_000 + 3_600_000);
    app.joined_private_channels.lock().await.insert(
        joined_private_channel_key(topic, channel.as_str()),
        JoinedPrivateChannelState {
            generation: 1,
            topic_id: topic.into(),
            channel_id: channel.clone(),
            label: "room".into(),
            creator_pubkey: keys.public_key_hex(),
            owner_pubkey: keys.public_key_hex(),
            joined_via_pubkey: None,
            audience_kind: ChannelAudienceKind::InviteOnly,
            current_epoch_id: format!("epoch-{}-current", 6 * 86_400_000 + 43_200_000),
            current_epoch_secret_hex: hex::encode([7; 32]),
            archived_epochs: (1..=5)
                .map(|index| PrivateChannelEpochCapability {
                    epoch_id: format!("epoch-{}-old", index * 86_400_000),
                    namespace_secret_hex: hex::encode([7; 32]),
                })
                .chain(std::iter::once(PrivateChannelEpochCapability {
                    epoch_id: same_day_earlier.clone(),
                    namespace_secret_hex: hex::encode([9; 32]),
                }))
                .chain(std::iter::once(PrivateChannelEpochCapability {
                    epoch_id: same_day_old.clone(),
                    namespace_secret_hex: hex::encode([8; 32]),
                }))
                .collect(),
        },
    );
    let selected = app
        .local_page_replicas(
            topic,
            &TimelineScope::Channel {
                channel_id: channel.clone(),
            },
            Some(3 * 86_400 + 3_600),
            false,
        )
        .await?;
    assert!(selected.len() <= 4);
    assert!(selected.contains(&private_channel_replica_for_epoch(
        channel.as_str(),
        &format!("epoch-{}-old", 3 * 86_400_000)
    )));
    assert!(selected.contains(&private_channel_replica_for_epoch(
        channel.as_str(),
        &format!("epoch-{}-old", 2 * 86_400_000)
    )));
    assert!(!selected.contains(&private_channel_replica_for_epoch(
        channel.as_str(),
        &format!("epoch-{}-old", 86_400_000)
    )));
    let remote = app
        .remote_page_replicas(
            topic,
            &TimelineScope::Channel {
                channel_id: channel.clone(),
            },
            Some(3 * 86_400 + 3_600),
            false,
            false,
        )
        .await?;
    assert!(remote.len() <= 4);
    let historic = BucketReplica::parse(&remote[0].0)?;
    assert!(
        matches!(historic.scope(), BucketScope::PrivateChannel { epoch_id, .. }
        if epoch_id == &format!("epoch-{}-old", 3 * 86_400_000))
    );
    let source = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: channel.as_str().into(),
            epoch_id: same_day_old.clone(),
        },
        TimeBucket::from_unix_seconds(6 * 86_400 + 7_200)?,
    )?
    .replica_id();
    let target = app
        .session_target_candidates(
            topic,
            Some((channel.as_str(), &source, (6 * 86_400 + 7_200) * 1_000)),
            "live-same-day-old",
        )
        .await?;
    assert_eq!(target[0].0, source);
    assert_eq!(
        target[0].1.as_ref().map(|(id, _)| id.as_str()),
        Some(same_day_old.as_str())
    );
    let earlier = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: channel.as_str().into(),
            epoch_id: same_day_earlier.clone(),
        },
        TimeBucket::from_unix_seconds(6 * 86_400 + 2_400)?,
    )?
    .replica_id();
    let earlier_target = app
        .session_target_candidates(
            topic,
            Some((channel.as_str(), &earlier, (6 * 86_400 + 2_400) * 1_000)),
            "live-earlier-in-same-bucket",
        )
        .await?;
    assert_eq!(
        earlier_target[0],
        (earlier, Some((same_day_earlier, hex::encode([9; 32]))))
    );
    Ok(())
}

fn observed_app() -> (AppService, Arc<LocalReadDocs>) {
    observed_app_with_blob_status(BlobStatus::Missing)
}

fn observed_app_with_blob_status(status: BlobStatus) -> (AppService, Arc<LocalReadDocs>) {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(LocalReadDocs::default());
    (
        app_service_from_dependencies(
            store.clone(),
            store,
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            Arc::new(NoopHintTransport),
            docs.clone(),
            Arc::new(LocalReadBlobs(docs.forbidden.clone(), status)),
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
async fn community_index_fetches_a_public_bucket_post_from_a_remote_provider() {
    let (app, docs) = observed_app();
    let remote = Arc::new(MemoryDocsSync::default());
    let topic = TopicId::new("remote-topic");
    let keys = generate_keys();
    let envelope = build_post_envelope(&keys, &topic, "remote body", None).expect("signed post");
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().to_string(),
        },
        TimeBucket::from_unix_seconds(envelope.created_at).expect("time"),
    )
    .expect("bucket")
    .replica_id();
    persist_post_object(
        remote.as_ref(),
        &replica,
        envelope.to_post_object().expect("header").expect("post"),
        envelope.clone(),
    )
    .await
    .expect("remote post");
    let withdrawn =
        build_post_envelope(&keys, &topic, "withdrawn body", None).expect("second signed post");
    persist_post_object(
        remote.as_ref(),
        &replica,
        withdrawn.to_post_object().expect("header").expect("post"),
        withdrawn.clone(),
    )
    .await
    .expect("second remote post");
    let withdrawal = build_post_withdrawal_envelope(
        &keys,
        &withdrawn,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .expect("signed withdrawal");
    remote
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: format!("withdrawals/{}/state", withdrawn.id.as_str()),
                value: serde_json::to_value(withdrawal).expect("json"),
            },
        )
        .await
        .expect("remote withdrawal");
    *docs.remote.lock().expect("remote source poisoned") = Some(remote);
    let response = app
        .resolve_community_index_posts(
            [
                (&envelope, "remote-result"),
                (&withdrawn, "withdrawn-result"),
            ]
            .into_iter()
            .map(|(post, key)| CommunityIndexPostResolveInput {
                key: key.into(),
                topic: topic.as_str().into(),
                object_id: post.id.as_str().into(),
                author_pubkey: post.pubkey.as_str().into(),
                channel_ref: ChannelRef::Public,
                source_replica_id: Some(replica.as_str().into()),
            })
            .collect(),
        )
        .await
        .expect("resolve");
    assert_eq!(
        response.entries[0]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("remote body")
    );
    let tombstone = response.entries[1].post.as_ref().expect("withdrawn post");
    assert!(tombstone.withdrawal.is_some());
    assert!(tombstone.content.is_empty());
    assert_eq!(docs.reads.load(Ordering::SeqCst), 2);
    assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn expired_remote_batch_does_not_return_an_unchecked_cached_post() {
    let (app, docs) = observed_app();
    let topic = TopicId::new("timeout-topic");
    let keys = generate_keys();
    let post = build_post_envelope(&keys, &topic, "old content", None).expect("post");
    let (replica, cached_input) = seed_bucket_post(&docs, &post).await;
    assert!(
        app.resolve_community_index_posts(vec![cached_input.clone()])
            .await
            .expect("initial cache")
            .entries[0]
            .post
            .is_some()
    );
    let withdrawal = build_post_withdrawal_envelope(
        &keys,
        &post,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
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
        .expect("arrived withdrawal");
    *docs.remote.lock().expect("remote source poisoned") = Some(Arc::new(PendingRemoteDocs));
    let mut inputs = (0..8)
        .map(|index| CommunityIndexPostResolveInput {
            key: format!("slow-{index}"),
            topic: topic.as_str().into(),
            object_id: format!("missing-{index}"),
            author_pubkey: "author".into(),
            channel_ref: ChannelRef::Public,
            source_replica_id: Some(replica.as_str().into()),
        })
        .collect::<Vec<_>>();
    inputs.push(cached_input);
    let response = tokio::time::timeout(
        Duration::from_secs(31),
        app.resolve_community_index_posts(inputs),
    )
    .await
    .expect("batch should return by its deadline")
    .expect("resolve");
    assert!(
        response.entries[8].post.is_none(),
        "an input whose withdrawal check did not run must remain unresolved"
    );
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

#[tokio::test]
async fn stale_available_or_pinned_body_status_never_allows_remote_fallback() {
    for status in [BlobStatus::Available, BlobStatus::Pinned] {
        let (app, docs) = observed_app_with_blob_status(status);
        let post = build_post_envelope_with_payload_in_channel(
            &generate_keys(),
            &TopicId::new("topic"),
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
        .expect("signed header");
        let (_, input) = seed_bucket_post(&docs, &post).await;
        let response = app
            .resolve_community_index_posts(vec![input])
            .await
            .expect("partial result");
        assert_eq!(
            docs.forbidden.load(Ordering::SeqCst),
            0,
            "status is not authority to use remote-capable fetch"
        );
        assert_eq!(
            response.entries[0]
                .post
                .as_ref()
                .expect("post")
                .content_status,
            BlobViewStatus::Missing
        );
    }
}
