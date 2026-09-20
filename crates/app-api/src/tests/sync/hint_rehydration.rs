use super::*;

#[tokio::test]
async fn list_profile_timeline_restarts_author_subscription_with_cooldown_when_profile_is_empty() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(TrackingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync.clone(),
        blob_service,
        generate_keys(),
    );
    let author_pubkey = "b".repeat(64);

    let timeline = app
        .list_profile_timeline(author_pubkey.as_str(), None, 20)
        .await
        .expect("timeline");
    assert!(timeline.items.is_empty());

    let second_timeline = app
        .list_profile_timeline(author_pubkey.as_str(), None, 20)
        .await
        .expect("second timeline");
    assert!(second_timeline.items.is_empty());

    let subscribed = docs_sync.subscribe_replicas.lock().await.clone();
    assert_eq!(
        subscribed,
        vec![
            author_replica_id(author_pubkey.as_str())
                .as_str()
                .to_string(),
            author_replica_id(author_pubkey.as_str())
                .as_str()
                .to_string()
        ]
    );
}

#[tokio::test]
async fn topic_doc_events_do_not_rehydrate_whole_replica() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        blob_service,
        keys.clone(),
    );
    let topic = TopicId::new("kukuri:topic:incremental-doc-event");

    let _ = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("initial timeline");
    sleep(Duration::from_millis(100)).await;
    docs_sync.clear_queries().await;

    let envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::InlineText {
            text: "remote incremental doc".into(),
        },
        Vec::new(),
        None,
    )
    .await;

    timeout(Duration::from_secs(5), async {
        loop {
            if ObjectProjectionStore::get_object_projection(store.as_ref(), &envelope.id)
                .await
                .expect("get projection")
                .is_some()
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("doc event projection timeout");

    // #1248: 行は署名つき envelope から作るので、key 指定で読むのは `envelope` の key。
    let queries = docs_sync.queries().await;
    assert!(
        queries.iter().any(|(_, query)| {
            *query
                == DocQuery::Exact(stable_key(
                    "objects",
                    &format!("{}/envelope", envelope.id.as_str()),
                ))
        }),
        "expected exact object query after doc event, got {queries:?}"
    );
    assert!(
        queries.iter().all(|(_, query)| {
            !matches!(
                query,
                DocQuery::Prefix(prefix)
                    if prefix == "objects/"
                        || prefix == "reactions/"
                        || prefix == "sessions/live/"
                        || prefix == "sessions/game/"
            )
        }),
        "doc event should not trigger whole-replica rehydrate, got {queries:?}"
    );
}

#[tokio::test]
async fn topic_object_hints_do_not_rehydrate_whole_replica() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport.clone(),
        docs_sync.clone(),
        blob_service,
        keys.clone(),
    );
    let topic = TopicId::new("kukuri:topic:incremental-hint-event");

    let envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::InlineText {
            text: "remote incremental hint".into(),
        },
        Vec::new(),
        None,
    )
    .await;

    let _ = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("initial timeline");
    sleep(Duration::from_millis(100)).await;
    docs_sync.clear_queries().await;

    transport
        .publish_hint(
            &channel_hint_topic_for(topic.as_str(), None),
            GossipHint::TopicObjectsChanged {
                topic_id: topic.clone(),
                objects: vec![HintObjectRef {
                    object_id: envelope.id.as_str().to_string(),
                    object_kind: "post".into(),
                    docs_author: None,
                }],
            },
        )
        .await
        .expect("publish hint");

    timeout(Duration::from_secs(5), async {
        loop {
            let queries = docs_sync.queries().await;
            if !queries.is_empty() {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("hint handling timeout");

    // #1248: 行は署名つき envelope から作るので、key 指定で読むのは `envelope` の key。
    let queries = docs_sync.queries().await;
    assert!(
        queries.iter().any(|(_, query)| {
            *query
                == DocQuery::Exact(stable_key(
                    "objects",
                    &format!("{}/envelope", envelope.id.as_str()),
                ))
        }),
        "expected exact object query after hint, got {queries:?}"
    );
    assert!(
        queries.iter().all(|(_, query)| {
            !matches!(
                query,
                DocQuery::Prefix(prefix)
                    if prefix == "objects/"
                        || prefix == "reactions/"
                        || prefix == "sessions/live/"
                        || prefix == "sessions/game/"
            )
        }),
        "hint should not trigger whole-replica rehydrate, got {queries:?}"
    );
}

#[tokio::test]
async fn topic_reaction_hints_rehydrate_only_target_reactions() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport.clone(),
        docs_sync.clone(),
        blob_service,
        keys.clone(),
    );
    let topic = TopicId::new("kukuri:topic:incremental-reaction-hint");
    let replica = topic_replica_id(topic.as_str());

    let envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::InlineText {
            text: "remote reaction target".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    let reaction_key = ReactionKeyV1::Emoji {
        emoji: "👍".into()
    };
    let reaction_id = deterministic_reaction_id(
        &replica,
        &envelope.id,
        &keys.public_key(),
        reaction_key
            .normalized_key()
            .expect("normalized reaction key")
            .as_str(),
    );
    let reaction_envelope = build_reaction_envelope(
        &keys,
        &topic,
        None,
        &envelope.id,
        reaction_key,
        &reaction_id,
        ObjectStatus::Active,
    )
    .expect("build reaction envelope");
    let reaction = parse_reaction(&reaction_envelope)
        .expect("parse reaction envelope")
        .expect("reaction doc");
    persist_reaction_doc(docs_sync.as_ref(), &replica, &reaction, &reaction_envelope)
        .await
        .expect("persist reaction doc");

    let _ = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("initial timeline");
    sleep(Duration::from_millis(100)).await;
    docs_sync.clear_queries().await;

    transport
        .publish_hint(
            &channel_hint_topic_for(topic.as_str(), None),
            GossipHint::TopicObjectsChanged {
                topic_id: topic.clone(),
                objects: vec![HintObjectRef {
                    object_id: envelope.id.as_str().to_string(),
                    object_kind: "reaction".into(),
                    docs_author: None,
                }],
            },
        )
        .await
        .expect("publish reaction hint");

    timeout(Duration::from_secs(5), async {
        loop {
            if !docs_sync.queries().await.is_empty() {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("reaction hint handling timeout");

    let queries = docs_sync.queries().await;
    // #1239: 対象の reaction を、上限つきの key の一覧と、reaction ごとの envelope の key 指定で読む。
    // 対象の reaction の総数ぶんの prefix 読みも、replica の走査もしない。
    assert!(
        queries.iter().any(|(_, query)| {
            *query
                == DocQuery::Exact(stable_key(
                    "reactions",
                    &format!("{}/{}/envelope", envelope.id.as_str(), reaction_id.as_str()),
                ))
        }),
        "expected the target's reaction envelope to be read by key after the hint, got {queries:?}"
    );
    assert!(
        queries
            .iter()
            .all(|(_, query)| matches!(query, DocQuery::Exact(_))),
        "a reaction hint must not read a prefix, got {queries:?}"
    );
}

#[tokio::test]
async fn public_topic_recovery_keeps_prompting_a_resync_without_scanning_the_replica() {
    let store = Arc::new(MemoryStore::default());
    let topic = TopicId::new("kukuri:topic:live-peer-docs-probe");
    let transport = Arc::new(StaticTransport::new(PeerSnapshot {
        connected: true,
        peer_count: 1,
        connected_peers: vec!["peer-a".into()],
        configured_peers: vec!["peer-a".into()],
        subscribed_topics: vec![topic.as_str().to_string()],
        active_path: Default::default(),
        fallback_peer_ids: Vec::new(),
        pending_events: 0,
        status_detail: "live peer connected".into(),
        last_error: None,
        topic_diagnostics: vec![TopicPeerSnapshot {
            topic: topic.as_str().to_string(),
            joined: true,
            peer_count: 1,
            connected_peers: vec!["peer-a".into()],
            configured_peer_ids: vec!["peer-a".into()],
            missing_peer_ids: Vec::new(),
            active_path: Default::default(),
            rendezvous_peer_ids: Vec::new(),
            fallback_peer_ids: Vec::new(),
            last_received_at: None,
            status_detail: "live peer connected".into(),
            last_error: None,
        }],
    }));
    let docs_sync = Arc::new(CountingDocsSync::with_assist_peer_ids(vec!["peer-a"]));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport.clone(),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );

    let _ = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("initial timeline");
    sleep(Duration::from_millis(100)).await;
    docs_sync.clear_queries().await;

    transport
        .publish_hint(
            &channel_hint_topic_for(topic.as_str(), None),
            GossipHint::TopicObjectsChanged {
                topic_id: topic.clone(),
                objects: vec![HintObjectRef {
                    object_id: "missing-post".into(),
                    object_kind: "post".into(),
                    docs_author: None,
                }],
            },
        )
        .await
        .expect("publish hint miss");

    // #1239: 個別反映が 0 件でも replica を走査しない。docs の支援 peer がいるあいだは、再 sync を
    // backoff つきで促し続ける(届いた entry は docs の event と窓の追いつきが反映する)。
    let restarts_before = docs_sync.restarts().await;
    timeout(Duration::from_secs(5), async {
        while docs_sync.restarts().await == restarts_before {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the hint miss must prompt a replica re-sync");
    let restarts_after_hint = docs_sync.restarts().await;
    timeout(
        Duration::from_millis(
            (PUBLIC_TOPIC_RECOVERY_GRACE_MS + PUBLIC_TOPIC_RECOVERY_BACKOFF_MS[0]) as u64 + 3_000,
        ),
        async {
            while docs_sync.restarts().await == restarts_after_hint {
                sleep(Duration::from_millis(50)).await;
            }
        },
    )
    .await
    .expect("the periodic recovery must keep prompting the re-sync");
    assert_eq!(
        docs_sync.object_scans().await,
        0,
        "neither the hint miss nor the recovery tick scans the replica"
    );
    app.shutdown().await;
}

#[tokio::test]
async fn topic_session_hints_retry_until_manifest_blob_is_available() {
    let docs_sync = Arc::new(kukuri_docs_sync::MemoryDocsSync::default());
    let blob_service = Arc::new(DelayedBlobService::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let owner_store = Arc::new(MemoryStore::default());
    let remote_store = Arc::new(MemoryStore::default());
    let owner_app = app_service_from_dependencies(
        owner_store.clone(),
        owner_store,
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        generate_keys(),
    );
    let remote_services = ServiceHandles::new(
        remote_store.clone(),
        remote_store.clone(),
        transport,
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:incremental-live-session-retry");
    let replica = topic_replica_id(topic.as_str());

    let session_id = owner_app
        .create_live_session(
            topic.as_str(),
            CreateLiveSessionInput {
                title: "retry live".into(),
                description: "delayed manifest".into(),
            },
        )
        .await
        .expect("create live session");
    let state: LiveSessionStateDocV1 = serde_json::from_slice(
        &docs_sync
            .query_replica(
                &replica,
                DocQuery::Exact(stable_key("sessions/live", &format!("{session_id}/state"))),
            )
            .await
            .expect("fetch live state")
            .first()
            .expect("live state")
            .value,
    )
    .expect("parse live state");
    blob_service
        .delay_hash(&state.current_manifest.hash, 2)
        .await;

    let hydrated = hydrate_subscription_hint(
        &remote_services,
        topic.as_str(),
        &replica,
        &GossipHint::SessionChanged {
            topic_id: topic.clone(),
            session_id: session_id.clone(),
            object_kind: "live-session".into(),
        },
    )
    .await
    .expect("hydrate live hint");

    assert_eq!(hydrated, 1);
    assert!(
        LiveGameProjectionStore::list_topic_live_sessions(remote_store.as_ref(), topic.as_str())
            .await
            .expect("list remote live sessions")
            .iter()
            .any(|session| session.session_id == session_id),
        "expected live session projection after retry"
    );
}
