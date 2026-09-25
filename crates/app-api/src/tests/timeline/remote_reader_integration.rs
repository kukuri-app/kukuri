use super::super::*;
use kukuri_core::build_post_envelope;
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};
use kukuri_store::PostWithdrawalStore;

#[cfg(feature = "iroh-integration-tests")]
struct ScopedReadHints(SeedPeer);

#[cfg(feature = "iroh-integration-tests")]
struct RealPublicPair {
    publisher_node: Arc<kukuri_iroh_node::IrohDocsNode>,
    client_node: Arc<kukuri_iroh_node::IrohDocsNode>,
    publisher: kukuri_docs_sync::IrohDocsSync,
    client: Arc<kukuri_docs_sync::IrohDocsSync>,
    app: AppService,
    store: Arc<MemoryStore>,
}

#[cfg(feature = "iroh-integration-tests")]
impl RealPublicPair {
    async fn new(keys: KukuriKeys) -> Result<Self> {
        let publisher_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
        let client_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
        let publisher = kukuri_docs_sync::IrohDocsSync::new(publisher_node.clone());
        let client = Arc::new(kukuri_docs_sync::IrohDocsSync::new(client_node.clone()));
        let socket = publisher_node
            .endpoint()
            .bound_sockets()
            .into_iter()
            .next()
            .expect("socket");
        client
            .import_peer_ticket(&format!("{}@{socket}", publisher_node.endpoint().addr().id))
            .await?;
        let store = Arc::new(MemoryStore::default());
        let app = app_service_from_dependencies(
            store.clone(),
            store.clone(),
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            Arc::new(NoopHintTransport),
            client.clone(),
            Arc::new(MemoryBlobService::default()),
            keys,
        );
        Ok(Self {
            publisher_node,
            client_node,
            publisher,
            client,
            app,
            store,
        })
    }

    async fn finish(self) -> Result<()> {
        self.app.shutdown().await;
        self.client.shutdown().await;
        self.publisher.shutdown().await;
        self.client_node.shutdown().await?;
        self.publisher_node.shutdown().await
    }
}

#[cfg(feature = "iroh-integration-tests")]
#[async_trait]
impl HintTransport for ScopedReadHints {
    async fn subscribe_hints(&self, _: &TopicId) -> Result<kukuri_transport::HintStream> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
    async fn unsubscribe_hints(&self, _: &TopicId) -> Result<()> {
        Ok(())
    }
    async fn publish_hint(&self, _: &TopicId, _: GossipHint) -> Result<()> {
        Ok(())
    }
    async fn topic_read_candidates(&self, _: &TopicId) -> Result<Vec<SeedPeer>> {
        Ok(vec![self.0.clone()])
    }
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn real_iroh_private_reader_stops_after_leave() -> Result<()> {
    let publisher_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let client_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let publisher = kukuri_docs_sync::IrohDocsSync::new(publisher_node.clone());
    let client = Arc::new(kukuri_docs_sync::IrohDocsSync::new(client_node.clone()));
    let topic = TopicId::new("real-private-source");
    let channel = ChannelId::new("room");
    let keys = generate_keys();
    let current_day = TimeBucket::from_unix_seconds(Utc::now().timestamp())?;
    let current_epoch = format!("epoch-{}-current", current_day.start_seconds() * 1_000);
    let old_at = current_day.start_seconds() as i64 - 10 * 86_400 + 3_600;
    let old_epoch = format!("epoch-{}-old", (old_at - 3_600) * 1_000);
    let post = build_post_envelope_with_payload_in_channel(
        &keys,
        &topic,
        PayloadRef::InlineText {
            text: "private over quic".into(),
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
            epoch_id: current_epoch.clone(),
        },
        TimeBucket::from_unix_seconds(post.created_at)?,
    )?;
    let replica = bucket.replica_id();
    publisher
        .register_private_replica_secret(
            &replica,
            &hex::encode(bucket.derive_private_secret(&[7; 32])?),
        )
        .await?;
    persist_post_object(
        &publisher,
        &replica,
        post.to_post_object()?.expect("post"),
        post.clone(),
    )
    .await?;
    let old_base = build_post_envelope_with_payload_in_channel(
        &keys,
        &topic,
        PayloadRef::InlineText {
            text: "archived private".into(),
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Private,
        Some(&channel),
        Vec::new(),
    )?;
    let old_payload: serde_json::Value = serde_json::from_str(&old_base.content)?;
    let old_post = kukuri_core::sign_envelope_json_at(
        &keys,
        old_base.kind,
        old_base.tags,
        &old_payload,
        old_at,
    )?;
    let old_bucket = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: channel.as_str().into(),
            epoch_id: old_epoch.clone(),
        },
        TimeBucket::from_unix_seconds(old_at)?,
    )?;
    let old_replica = old_bucket.replica_id();
    publisher
        .register_private_replica_secret(
            &old_replica,
            &hex::encode(old_bucket.derive_private_secret(&[8; 32])?),
        )
        .await?;
    persist_post_object(
        &publisher,
        &old_replica,
        old_post.to_post_object()?.expect("post"),
        old_post.clone(),
    )
    .await?;
    let socket = publisher_node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("socket");
    let peer = SeedPeer {
        endpoint_id: publisher_node.endpoint().addr().id.to_string(),
        addr_hint: Some(socket.to_string()),
    };
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(ScopedReadHints(peer)),
        client.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
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
            current_epoch_id: current_epoch,
            current_epoch_secret_hex: hex::encode([7; 32]),
            archived_epochs: vec![PrivateChannelEpochCapability {
                epoch_id: old_epoch,
                namespace_secret_hex: hex::encode([8; 32]),
            }],
        },
    );
    let input = CommunityIndexPostResolveInput {
        key: "private-quic".into(),
        topic: topic.as_str().into(),
        object_id: post.id.as_str().into(),
        author_pubkey: post.pubkey.as_str().into(),
        channel_ref: ChannelRef::PrivateChannel {
            channel_id: channel.clone(),
        },
        source_replica_id: Some(replica.as_str().into()),
    };
    let result = app
        .resolve_community_index_posts(vec![input.clone()])
        .await?;
    assert_eq!(
        result.entries[0]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("private over quic")
    );
    assert_eq!(client_node.docs().list().await?.count().await, 0);
    let page = app
        .list_timeline_scoped(
            topic.as_str(),
            TimelineScope::Channel {
                channel_id: channel.clone(),
            },
            None,
            20,
        )
        .await?;
    assert!(
        page.items
            .iter()
            .any(|item| item.object_id == post.id.as_str())
    );
    let history = app
        .list_timeline_scoped(
            topic.as_str(),
            TimelineScope::Channel {
                channel_id: channel.clone(),
            },
            Some(TimelineCursor {
                created_at: old_at + 1,
                object_id: EnvelopeId::from("z"),
            }),
            20,
        )
        .await?;
    assert!(
        history
            .items
            .iter()
            .any(|item| item.object_id == old_post.id.as_str())
    );
    assert_eq!(
        app.resolve_signed_post_envelope(&old_post.id)
            .await?
            .expect("archived signed source")
            .id,
        old_post.id,
    );
    app.joined_private_channels
        .lock()
        .await
        .remove(&joined_private_channel_key(
            topic.as_str(),
            channel.as_str(),
        ));
    assert!(
        app.resolve_community_index_posts(vec![input])
            .await?
            .entries[0]
            .post
            .is_none()
    );
    app.shutdown().await;
    client.shutdown().await;
    publisher.shutdown().await;
    client_node.shutdown().await?;
    publisher_node.shutdown().await?;
    Ok(())
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn real_iroh_page_selects_previous_and_cursor_named_historic_buckets() -> Result<()> {
    let topic = TopicId::new("remote-historic-page");
    let keys = generate_keys();
    let pair = RealPublicPair::new(generate_keys()).await?;
    let current = TimeBucket::from_unix_seconds(Utc::now().timestamp())?;
    let previous_at = current.start_seconds() as i64 - 10;
    let historic_at = previous_at - 100 * 86_400;
    let mut posts = Vec::new();
    for (body, at) in [("previous", previous_at), ("historic", historic_at)] {
        let base = build_post_envelope(&keys, &topic, body, None)?;
        let payload: serde_json::Value = serde_json::from_str(&base.content)?;
        let post = kukuri_core::sign_envelope_json_at(&keys, base.kind, base.tags, &payload, at)?;
        let replica = BucketReplica::new(
            BucketScope::Topic {
                topic_id: topic.as_str().to_owned(),
            },
            TimeBucket::from_unix_seconds(at)?,
        )?
        .replica_id();
        persist_post_object(
            &pair.publisher,
            &replica,
            post.to_post_object()?.expect("post"),
            post.clone(),
        )
        .await?;
        posts.push((post, replica));
    }
    let withdrawal = build_post_withdrawal_envelope(
        &keys,
        &posts[1].0,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )?;
    pair.store
        .put_post_withdrawal(post_withdrawal_row(
            verify_post_withdrawal(&withdrawal, &posts[1].0)?,
            &posts[1].1,
        ))
        .await?;
    let first = pair.app.list_timeline(topic.as_str(), None, 20).await?;
    assert!(
        first
            .items
            .iter()
            .any(|item| item.object_id == posts[0].0.id.as_str())
    );
    assert!(
        !first
            .items
            .iter()
            .any(|item| item.object_id == posts[1].0.id.as_str())
    );
    let historic = pair
        .app
        .list_timeline(
            topic.as_str(),
            Some(TimelineCursor {
                created_at: historic_at + 1,
                object_id: EnvelopeId::from("z"),
            }),
            20,
        )
        .await?;
    let withdrawn = historic
        .items
        .iter()
        .find(|item| item.object_id == posts[1].0.id.as_str())
        .expect("withdrawn historic post");
    assert!(withdrawn.withdrawal.is_some());
    assert!(withdrawn.content.is_empty());
    for (post, replica) in posts {
        assert!(
            pair.client
                .query_local_source(
                    &replica,
                    &format!("objects/{}/state", post.id.as_str()),
                    None,
                    1
                )
                .await?
                .is_empty()
        );
    }
    pair.finish().await
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn real_iroh_timeline_reads_a_bucket_page_without_importing_it() -> Result<()> {
    let topic = TopicId::new("real-remote-page");
    let author = generate_keys();
    let pair = RealPublicPair::new(author.clone()).await?;
    let envelope = build_post_envelope(&author, &topic, "page body", None)?;
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().to_owned(),
        },
        TimeBucket::from_unix_seconds(envelope.created_at)?,
    )?
    .replica_id();
    let object_id = envelope.id.clone();
    persist_post_object(
        &pair.publisher,
        &replica,
        envelope.to_post_object()?.expect("post"),
        envelope,
    )
    .await?;
    let page = pair.app.list_timeline(topic.as_str(), None, 20).await?;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].content, "page body");
    assert_eq!(
        pair.app
            .bookmark_post(topic.as_str(), object_id.as_str())
            .await?
            .post
            .object_id,
        object_id.as_str()
    );
    pair.app
        .withdraw_post(
            topic.as_str(),
            object_id.as_str(),
            ChannelRef::Public,
            None,
            WithdrawalReasonVisibility::Private,
            None,
        )
        .await?;
    assert!(
        pair.app
            .services
            .projection_store
            .get_post_withdrawal(&object_id)
            .await?
            .is_some()
    );
    assert!(
        pair.client
            .query_local_source(
                &replica,
                &format!("objects/{}/state", object_id.as_str()),
                None,
                1
            )
            .await?
            .is_empty(),
        "the remote bucket was not imported into the client's docs store"
    );
    pair.finish().await
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn real_iroh_thread_reads_a_bucket_page_without_importing_it() -> Result<()> {
    let pair = RealPublicPair::new(generate_keys()).await?;
    let topic = TopicId::new("real-remote-thread");
    let keys = generate_keys();
    let root = build_post_envelope(&keys, &topic, "root", None)?;
    let reply = build_post_envelope(&keys, &topic, "reply", Some(&root))?;
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().to_owned(),
        },
        TimeBucket::from_unix_seconds(root.created_at)?,
    )?
    .replica_id();
    for envelope in [root.clone(), reply.clone()] {
        persist_post_object(
            &pair.publisher,
            &replica,
            envelope.to_post_object()?.expect("post"),
            envelope,
        )
        .await?;
    }
    let page = pair
        .app
        .list_thread(topic.as_str(), root.id.as_str(), None, 20)
        .await?;
    assert!(
        page.items
            .iter()
            .any(|item| item.object_id == reply.id.as_str())
    );
    let reply_row = pair
        .store
        .get_object_projection(&reply.id)
        .await?
        .expect("reply row");
    pair.store
        .rebuild_object_projections(vec![reply_row.clone()])
        .await?;
    pair.app.reflect_reply_targets_for_rows(&[reply_row]).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if pair.store.get_object_projection(&root.id).await?.is_some() {
                break Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await??;
    assert!(
        pair.client
            .query_local_source(
                &replica,
                &format!("objects/{}/state", reply.id.as_str()),
                None,
                1
            )
            .await?
            .is_empty()
    );
    pair.finish().await
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn real_iroh_client_reads_a_public_bucket_without_importing_it() -> Result<()> {
    let pair = RealPublicPair::new(generate_keys()).await?;
    let topic = TopicId::new("real-remote-topic");
    let envelope = build_post_envelope(&generate_keys(), &topic, "real remote body", None)?;
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().to_owned(),
        },
        TimeBucket::from_unix_seconds(envelope.created_at)?,
    )?
    .replica_id();
    persist_post_object(
        &pair.publisher,
        &replica,
        envelope.to_post_object()?.expect("post"),
        envelope.clone(),
    )
    .await?;
    let response = pair
        .app
        .resolve_community_index_posts(vec![CommunityIndexPostResolveInput {
            key: "remote".into(),
            topic: topic.as_str().into(),
            object_id: envelope.id.as_str().into(),
            author_pubkey: envelope.pubkey.as_str().into(),
            channel_ref: ChannelRef::Public,
            source_replica_id: Some(replica.as_str().into()),
        }])
        .await?;
    assert_eq!(
        response.entries[0]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("real remote body")
    );
    let legacy_post = build_post_envelope(&generate_keys(), &topic, "old writer", None)?;
    let legacy = topic_replica_id(topic.as_str());
    persist_post_object(
        &pair.publisher,
        &legacy,
        legacy_post.to_post_object()?.expect("post"),
        legacy_post.clone(),
    )
    .await?;
    let old = pair
        .app
        .resolve_community_index_posts(vec![CommunityIndexPostResolveInput {
            key: "legacy".into(),
            topic: topic.as_str().into(),
            object_id: legacy_post.id.as_str().into(),
            author_pubkey: legacy_post.pubkey.as_str().into(),
            channel_ref: ChannelRef::Public,
            source_replica_id: Some(legacy.as_str().into()),
        }])
        .await?;
    assert_eq!(
        old.entries[0]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("old writer")
    );
    assert_eq!(pair.client_node.docs().list().await?.count().await, 0);
    let repost_source = build_post_envelope(&generate_keys(), &topic, "for repost", None)?;
    persist_post_object(
        &pair.publisher,
        &replica,
        repost_source.to_post_object()?.expect("post"),
        repost_source.clone(),
    )
    .await?;
    let repost = pair
        .app
        .create_repost(
            "repost-destination",
            topic.as_str(),
            repost_source.id.as_str(),
            Some("remote source"),
        )
        .await?;
    assert!(!repost.is_empty());
    assert!(
        pair.client
            .query_local_source(
                &replica,
                &format!("objects/{}/state", repost_source.id.as_str()),
                None,
                1,
            )
            .await?
            .is_empty()
    );
    pair.finish().await
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn two_providers_in_one_bucket_both_reach_the_timeline() -> Result<()> {
    let pair = RealPublicPair::new(generate_keys()).await?;
    let second_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let second = kukuri_docs_sync::IrohDocsSync::new(second_node.clone());
    let socket = second_node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("socket");
    pair.client
        .import_peer_ticket(&format!("{}@{socket}", second_node.endpoint().addr().id))
        .await?;
    let topic = TopicId::new("two-bucket-providers");
    let first_post = build_post_envelope(&generate_keys(), &topic, "first", None)?;
    let second_post = build_post_envelope(&generate_keys(), &topic, "second", None)?;
    let bucket = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().into(),
        },
        TimeBucket::from_unix_seconds(first_post.created_at)?,
    )?
    .replica_id();
    persist_post_object(
        &pair.publisher,
        &bucket,
        first_post.to_post_object()?.expect("post"),
        first_post.clone(),
    )
    .await?;
    persist_post_object(
        &second,
        &bucket,
        second_post.to_post_object()?.expect("post"),
        second_post.clone(),
    )
    .await?;
    let page = pair.app.list_timeline(topic.as_str(), None, 20).await?;
    assert!(
        page.items
            .iter()
            .any(|item| item.object_id == first_post.id.as_str())
    );
    assert!(
        page.items
            .iter()
            .any(|item| item.object_id == second_post.id.as_str())
    );
    second.shutdown().await;
    second_node.shutdown().await?;
    pair.finish().await
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn remote_missing_window_keeps_its_cursor_and_resumes() -> Result<()> {
    let pair = RealPublicPair::new(generate_keys()).await?;
    let topic = TopicId::new("remote-missing-cursor");
    let post = build_post_envelope(&generate_keys(), &topic, "after retry", None)?;
    let bucket = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().into(),
        },
        TimeBucket::from_unix_seconds(post.created_at)?,
    )?
    .replica_id();
    for index in 0..40 {
        let id = format!("missing-{index:03}");
        pair.publisher
            .apply_doc_op(
                &bucket,
                DocOp::SetJson {
                    key: format!("indexes/timeline/{:020}-{id}/{id}", post.created_at + 1),
                    value: serde_json::json!({"object_id": id}),
                },
            )
            .await?;
    }
    persist_post_object(
        &pair.publisher,
        &bucket,
        post.to_post_object()?.expect("post"),
        post.clone(),
    )
    .await?;
    let first = pair.app.list_timeline(topic.as_str(), None, 20).await?;
    let cursor = first.next_cursor.expect("the next page remains reachable");
    let repeated = pair.app.list_timeline(topic.as_str(), None, 20).await?;
    assert_eq!(repeated.next_cursor, Some(cursor.clone()));
    pair.app.services.range_checks.expire_all_for_test().await;
    let resumed = pair.app.list_timeline(topic.as_str(), None, 20).await?;
    assert!(
        resumed
            .items
            .iter()
            .any(|item| item.object_id == post.id.as_str())
    );
    let older = pair
        .app
        .list_timeline(topic.as_str(), Some(cursor), 20)
        .await?;
    assert!(
        older
            .items
            .iter()
            .any(|item| item.object_id == post.id.as_str())
    );
    pair.finish().await
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn legacy_missing_window_keeps_new_bucket_reachable() -> Result<()> {
    let pair = RealPublicPair::new(generate_keys()).await?;
    let topic = TopicId::new("legacy-gaps-with-new-bucket");
    let post = build_post_envelope(&generate_keys(), &topic, "new bucket", None)?;
    let bucket = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().into(),
        },
        TimeBucket::from_unix_seconds(post.created_at)?,
    )?
    .replica_id();
    persist_post_object(
        &pair.publisher,
        &bucket,
        post.to_post_object()?.expect("post"),
        post.clone(),
    )
    .await?;
    let legacy = topic_replica_id(topic.as_str());
    for index in 0..200 {
        let id = format!("legacy-missing-{index:03}");
        pair.client
            .apply_doc_op(
                &legacy,
                DocOp::SetJson {
                    key: format!("indexes/timeline/{:020}-{id}/{id}", post.created_at + 1),
                    value: serde_json::json!({"object_id": id}),
                },
            )
            .await?;
    }
    let first = pair.app.list_timeline(topic.as_str(), None, 20).await?;
    let next = first
        .next_cursor
        .expect("unavailable legacy range remains pageable");
    let second = pair
        .app
        .list_timeline(topic.as_str(), Some(next), 20)
        .await?;
    assert!(
        second
            .items
            .iter()
            .any(|item| item.object_id == post.id.as_str())
    );
    pair.finish().await
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn remote_page_reads_past_twenty_missing_records() -> Result<()> {
    let pair = RealPublicPair::new(generate_keys()).await?;
    let topic = TopicId::new("remote-page-gaps");
    let post = build_post_envelope(&generate_keys(), &topic, "after gaps", None)?;
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().into(),
        },
        TimeBucket::from_unix_seconds(post.created_at)?,
    )?
    .replica_id();
    for index in 0..20 {
        let id = format!("missing-{index:02}");
        pair.publisher
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: format!("indexes/timeline/{:020}-{id}/{id}", post.created_at + 1),
                    value: serde_json::json!({"object_id": id}),
                },
            )
            .await?;
    }
    persist_post_object(
        &pair.publisher,
        &replica,
        post.to_post_object()?.expect("post"),
        post.clone(),
    )
    .await?;
    let page = pair.app.list_timeline(topic.as_str(), None, 20).await?;
    assert!(
        page.items
            .iter()
            .any(|item| item.object_id == post.id.as_str())
    );
    assert!(page.unavailable_count >= 20);
    pair.finish().await
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn unknown_index_sources_do_not_create_local_namespaces() -> Result<()> {
    let node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let docs = Arc::new(kukuri_docs_sync::IrohDocsSync::new(node.clone()));
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let before = node.docs().list().await?.count().await;
    for day in 1..=10 {
        let replica = BucketReplica::new(
            BucketScope::Topic {
                topic_id: "source-missing".into(),
            },
            TimeBucket::from_index(day)?,
        )?
        .replica_id();
        let result = app
            .resolve_community_index_posts(vec![CommunityIndexPostResolveInput {
                key: "missing".into(),
                topic: "source-missing".into(),
                object_id: "missing".into(),
                author_pubkey: "author".into(),
                channel_ref: ChannelRef::Public,
                source_replica_id: Some(replica.0),
            }])
            .await?;
        assert!(result.entries[0].post.is_none());
    }
    let after = node.docs().list().await?.count().await;
    app.shutdown().await;
    docs.shutdown().await;
    node.shutdown().await?;
    assert_eq!(
        after, before,
        "a missing locator must not import a namespace"
    );
    Ok(())
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn cached_index_post_survives_removed_namespace_without_recreating_it() -> Result<()> {
    let node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let docs = Arc::new(kukuri_docs_sync::IrohDocsSync::new(node.clone()));
    let store = Arc::new(MemoryStore::default());
    let keys = generate_keys();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        keys.clone(),
    );
    let post =
        kukuri_core::build_post_envelope(&keys, &TopicId::new("source-cached"), "kept", None)?;
    let header = post.to_post_object()?.expect("post header");
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: "source-cached".into(),
        },
        TimeBucket::from_unix_seconds(post.created_at)?,
    )?
    .replica_id();
    persist_post_object(docs.as_ref(), &replica, header, post.clone()).await?;
    let input = CommunityIndexPostResolveInput {
        key: "cached".into(),
        topic: "source-cached".into(),
        object_id: post.id.0,
        author_pubkey: post.pubkey.0,
        channel_ref: ChannelRef::Public,
        source_replica_id: Some(replica.0.clone()),
    };
    let first = app
        .resolve_community_index_posts(vec![input.clone()])
        .await?;
    assert_eq!(
        first.entries[0].post.as_ref().expect("cached").content,
        "kept"
    );
    docs.close_replica(&replica).await?;
    let (namespace, _) = node.docs().list().await?.next().await.expect("namespace")?;
    node.docs().drop_doc(namespace).await?;
    let restored = app.resolve_community_index_posts(vec![input]).await?;
    assert_eq!(
        restored.entries[0]
            .post
            .as_ref()
            .expect("saved projection")
            .content,
        "kept"
    );
    assert_eq!(node.docs().list().await?.count().await, 0);
    app.shutdown().await;
    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}
