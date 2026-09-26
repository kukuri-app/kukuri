use super::*;
use kukuri_docs_sync::{DocEventStream, DocFetchPolicy};
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct ConnectionIoProbe {
    inner: MemoryDocsSync,
    io: AtomicUsize,
    writes: AtomicUsize,
    secrets: AtomicUsize,
}
#[async_trait]
impl DocsSync for ConnectionIoProbe {
    async fn register_private_replica_secret(
        &self,
        replica: &ReplicaId,
        namespace_secret_hex: &str,
    ) -> Result<()> {
        self.secrets.fetch_add(1, Ordering::SeqCst);
        self.inner
            .register_private_replica_secret(replica, namespace_secret_hex)
            .await
    }
    async fn open_replica(&self, replica: &ReplicaId) -> Result<()> {
        self.io.fetch_add(1, Ordering::SeqCst);
        self.inner.open_replica(replica).await
    }
    async fn apply_doc_op(&self, replica: &ReplicaId, op: DocOp) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.apply_doc_op(replica, op).await
    }
    async fn query_replica_with_policy(
        &self,
        replica: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.io.fetch_add(1, Ordering::SeqCst);
        self.inner
            .query_replica_with_policy(replica, query, policy)
            .await
    }
    async fn query_replica_keys(
        &self,
        replica: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.io.fetch_add(1, Ordering::SeqCst);
        self.inner.query_replica_keys(replica, query).await
    }
    async fn subscribe_replica(&self, replica: &ReplicaId) -> Result<DocEventStream> {
        self.io.fetch_add(1, Ordering::SeqCst);
        self.inner.subscribe_replica(replica).await
    }
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.io.fetch_add(1, Ordering::SeqCst);
        self.inner.import_peer_ticket(ticket).await
    }
}

#[tokio::test]
async fn unknown_connection_context_rejects_all_actions_before_io_and_projection() {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(ConnectionIoProbe::default());
    let transport = Arc::new(FakeTransport::new(
        "connection-guard",
        FakeNetwork::default(),
    ));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:connection-guard";
    let context = SpatialContextV1::Channel {
        topic_id: TopicId::new(topic),
        channel_id: kukuri_core::ChannelId::new("unknown"),
    };
    assert!(
        app.list_dome_connection_topology(context.clone())
            .await
            .is_err()
    );
    assert!(
        app.create_dome_connection_proposal(CreateDomeConnectionProposalInput {
            proposal_id: "test-proposal".into(),
            spatial_context: context.clone(),
            proposer_instance_id: "a".into(),
            receiver_instance_id: "b".into(),
            proposer_direction: DomeDirection::East,
        })
        .await
        .is_err()
    );
    assert!(
        app.accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
            spatial_context: context.clone(),
            proposal_id: "test-proposal".into()
        })
        .await
        .is_err()
    );
    assert!(
        app.withdraw_dome_connection_proposal(WithdrawDomeConnectionProposalInput {
            spatial_context: context.clone(),
            proposal_id: "test-proposal".into()
        })
        .await
        .is_err()
    );
    assert!(
        app.revoke_dome_connection(RevokeDomeConnectionInput {
            spatial_context: context.clone(),
            connection_id: "test-connection".into()
        })
        .await
        .is_err()
    );
    assert_eq!(docs.io.load(Ordering::SeqCst), 0);
    assert_eq!(docs.writes.load(Ordering::SeqCst), 0);
    assert!(!app.has_topic_subscription(topic).await);
    assert!(
        store
            .get_dome_connection_projection(&context.canonical_id())
            .await
            .unwrap()
            .is_none()
    );
    // A permitted local read may refresh the derived projection, but never writes shared docs.
    let public = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    app.list_dome_connection_topology(public.clone())
        .await
        .unwrap();
    assert_eq!(docs.writes.load(Ordering::SeqCst), 0);
    assert!(!app.has_topic_subscription(topic).await);
    assert!(
        store
            .get_dome_connection_projection(&public.canonical_id())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn connection_map_reads_do_not_start_subscriptions_even_for_unknown_channels() {
    let app = AppService::new(
        Arc::new(MemoryStore::default()),
        Arc::new(FakeTransport::new("map-reader", FakeNetwork::default())),
    );
    for channel in [None, Some("unknown-channel")] {
        let topic = format!("kukuri:topic:map-read-{}", channel.unwrap_or("public"));
        let context = match channel {
            Some(channel_id) => SpatialContextV1::Channel {
                topic_id: TopicId::new(&topic),
                channel_id: kukuri_core::ChannelId::new(channel_id),
            },
            None => SpatialContextV1::Topic {
                topic_id: TopicId::new(&topic),
            },
        };
        let result = app.list_dome_connection_topology(context).await;
        assert_eq!(result.is_ok(), channel.is_none());
        assert!(
            !app.has_topic_subscription(&topic).await,
            "map read must not start network subscription"
        );
    }
}

#[tokio::test]
async fn restored_friend_only_map_read_does_not_rotate_or_subscribe() {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(ConnectionIoProbe::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        Arc::new(NoopHintTransport),
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:map-friend-only";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "map read".into(),
            audience_kind: ChannelAudienceKind::FriendOnly,
        })
        .await
        .unwrap();
    let state = app
        .joined_private_channel_state(topic, &channel.channel_id)
        .await
        .unwrap();
    let peer = generate_keys();
    persist_private_channel_participant(
        docs.as_ref(),
        &peer,
        &PrivateChannelParticipantDocV1 {
            channel_id: state.channel_id.clone(),
            topic_id: TopicId::new(&state.topic_id),
            epoch_id: state.current_epoch_id.clone(),
            participant_pubkey: peer.public_key(),
            joined_at: 1,
            is_owner: false,
            join_mode: None,
            sponsor_pubkey: None,
            share_token_id: None,
            left_at: None,
        },
        &current_private_channel_replica_id(&state),
    )
    .await
    .unwrap();
    app.services
        .store
        .upsert_follow_edge(FollowEdge {
            subject_pubkey: peer.public_key(),
            target_pubkey: Pubkey::from(app.current_author_pubkey().as_str()),
            status: FollowEdgeStatus::Active,
            updated_at: 1,
            envelope_id: EnvelopeId::from("follow-peer-local".to_string()),
        })
        .await
        .unwrap();
    assert!(
        app.private_channel_diagnostics(&state)
            .await
            .unwrap()
            .rotation_required
    );
    // Restore the existing capability without invoking subscription or grant redemption.
    let reader = AppService::from_handles(app.services.clone());
    reader.joined_private_channels.lock().await.insert(
        joined_private_channel_key(topic, &channel.channel_id),
        state.clone(),
    );
    let writes_before = docs.writes.load(Ordering::SeqCst);
    let secrets_before = docs.secrets.load(Ordering::SeqCst);
    reader
        .list_dome_connection_topology(SpatialContextV1::Channel {
            topic_id: TopicId::new(topic),
            channel_id: state.channel_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        docs.writes.load(Ordering::SeqCst),
        writes_before,
        "viewing must not rotate the epoch or publish records"
    );
    assert_eq!(docs.secrets.load(Ordering::SeqCst), secrets_before);
    assert_eq!(
        reader.subscription_registry.scope_leases.lock().await.len(),
        0
    );
    assert!(!reader.has_topic_subscription(topic).await);
    assert_eq!(
        reader
            .joined_private_channel_state(topic, &channel.channel_id)
            .await
            .unwrap()
            .current_epoch_id,
        state.current_epoch_id
    );
}
use kukuri_core::{DomeDirection, DomeProposalDerivedStatusV1, SpatialContextV1};

fn app_with_shared_dome_services(
    docs_sync: Arc<MemoryDocsSync>,
    blob_service: Arc<MemoryBlobService>,
    keys: KukuriKeys,
) -> AppService {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        keys,
    )
}

async fn open_proposal_fixture(
    suffix: &str,
) -> (AppService, AppService, SpatialContextV1, String, String) {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let proposer_keys = generate_keys();
    let receiver_keys = generate_keys();
    let proposer_pubkey = proposer_keys.public_key_hex();
    let receiver_pubkey = receiver_keys.public_key_hex();
    let proposer =
        app_with_shared_dome_services(docs_sync.clone(), blob_service.clone(), proposer_keys);
    let receiver = app_with_shared_dome_services(docs_sync, blob_service, receiver_keys);
    let topic = format!("kukuri:topic:dome-open-proposal-{suffix}");
    // block での解除は、開いている列(lease のある topic)の Dome 接続を対象にする(#1221 R2-C)。
    for app in [&proposer, &receiver] {
        display_topic(app, &topic)
            .await
            .expect("open the Dome column");
    }
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic.clone()),
    };
    let proposer_instance = proposer
        .create_metaverse_room(
            &topic,
            CreateMetaverseRoomInput {
                title: "Proposer Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create proposer Dome");
    let receiver_instance = receiver
        .create_metaverse_room(
            &topic,
            CreateMetaverseRoomInput {
                title: "Receiver Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create receiver Dome");
    proposer
        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
            proposal_id: format!("proposal-{suffix}"),
            spatial_context: context.clone(),
            proposer_instance_id: proposer_instance,
            receiver_instance_id: receiver_instance,
            proposer_direction: DomeDirection::East,
        })
        .await
        .expect("create proposal");
    (
        proposer,
        receiver,
        context,
        proposer_pubkey,
        receiver_pubkey,
    )
}

#[tokio::test]
async fn dome_connection_proposal_accept_and_revoke_round_trip() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let proposer_keys = generate_keys();
    let receiver_keys = generate_keys();
    let proposer =
        app_with_shared_dome_services(docs_sync.clone(), blob_service.clone(), proposer_keys);
    let receiver = app_with_shared_dome_services(docs_sync, blob_service, receiver_keys);
    let topic = "kukuri:topic:dome-connection-round-trip";
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    let proposer_instance = proposer
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "Proposer Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create proposer Dome");
    let receiver_instance = receiver
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "Receiver Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create receiver Dome");

    let proposal = proposer
        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
            proposal_id: "proposal-round-trip".into(),
            spatial_context: context.clone(),
            proposer_instance_id: proposer_instance.clone(),
            receiver_instance_id: receiver_instance.clone(),
            proposer_direction: DomeDirection::East,
        })
        .await
        .expect("create proposal");
    assert_eq!(proposal.status, DomeProposalDerivedStatusV1::Proposed);
    assert_eq!(proposal.proposal.receiver.direction, DomeDirection::West);
    let replayed = proposer
        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
            proposal_id: "proposal-round-trip".into(),
            spatial_context: context.clone(),
            proposer_instance_id: proposer_instance.clone(),
            receiver_instance_id: receiver_instance,
            proposer_direction: DomeDirection::East,
        })
        .await
        .expect("replay proposal operation");
    assert_eq!(replayed.connection_id, proposal.connection_id);
    assert!(
        proposer
            .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
                proposal_id: "proposal-round-trip".into(),
                spatial_context: context.clone(),
                proposer_instance_id: proposer_instance,
                receiver_instance_id: "different-instance".into(),
                proposer_direction: DomeDirection::East,
            })
            .await
            .is_err()
    );

    let connection = receiver
        .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
            spatial_context: context.clone(),
            proposal_id: "proposal-round-trip".into(),
        })
        .await
        .expect("accept proposal");
    assert_eq!(
        connection.record.status,
        kukuri_core::DomeConnectionStatusV1::Active
    );

    let proposer_view = proposer
        .list_dome_connection_topology(context.clone())
        .await
        .expect("proposer topology");
    let receiver_view = receiver
        .list_dome_connection_topology(context.clone())
        .await
        .expect("receiver topology");
    assert_eq!(proposer_view.resolution, receiver_view.resolution);
    assert_eq!(proposer_view.resolution.topology.components.len(), 1);
    assert_eq!(
        proposer_view.proposals[0].status,
        DomeProposalDerivedStatusV1::Accepted
    );

    let revoke = receiver.revoke_dome_connection(RevokeDomeConnectionInput {
        spatial_context: context.clone(),
        connection_id: connection.record.agreement.connection_id,
    });
    tokio::pin!(revoke);
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
        _ = &mut revoke => panic!("normal revoke must expose the draining interval"),
    }
    let draining = proposer
        .list_dome_connection_topology(context.clone())
        .await
        .expect("draining topology");
    assert_eq!(
        draining.connections[0].record.status,
        kukuri_core::DomeConnectionStatusV1::Draining
    );
    assert!(
        draining.connections[0]
            .record
            .lifecycle_deadline_at
            .is_some()
    );
    assert_eq!(draining.resolution.topology.components.len(), 1);
    revoke.await.expect("revoke Connection");
    let split = proposer
        .list_dome_connection_topology(context)
        .await
        .expect("split topology");
    assert_eq!(split.resolution.topology.components.len(), 2);
    assert!(split.resolution.topology.active_connection_ids.is_empty());
}

#[tokio::test]
async fn owner_block_revokes_connection_and_unblock_does_not_restore_it() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let proposer_keys = generate_keys();
    let receiver_keys = generate_keys();
    let receiver_pubkey = receiver_keys.public_key_hex();
    let proposer =
        app_with_shared_dome_services(docs_sync.clone(), blob_service.clone(), proposer_keys);
    let receiver = app_with_shared_dome_services(docs_sync, blob_service, receiver_keys);
    let topic = "kukuri:topic:dome-connection-owner-block";
    for app in [&proposer, &receiver] {
        display_topic(app, topic)
            .await
            .expect("open the Dome column");
    }
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    let proposer_instance = proposer
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "Proposer Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create proposer Dome");
    let receiver_instance = receiver
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "Receiver Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create receiver Dome");
    proposer
        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
            proposal_id: "proposal-owner-block".into(),
            spatial_context: context.clone(),
            proposer_instance_id: proposer_instance,
            receiver_instance_id: receiver_instance,
            proposer_direction: DomeDirection::East,
        })
        .await
        .expect("create proposal");
    receiver
        .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
            spatial_context: context.clone(),
            proposal_id: "proposal-owner-block".into(),
        })
        .await
        .expect("accept proposal");

    proposer
        .block_author(receiver_pubkey.as_str())
        .await
        .expect("block endpoint owner");
    let blocked = proposer
        .list_dome_connection_topology(context.clone())
        .await
        .expect("blocked topology");
    assert!(blocked.resolution.topology.active_connection_ids.is_empty());
    assert_eq!(
        blocked.connections[0].record.lifecycle_reason,
        Some(kukuri_core::DomeConnectionTerminalReasonV1::OwnersBlocked)
    );

    proposer
        .unblock_author(receiver_pubkey.as_str())
        .await
        .expect("unblock endpoint owner");
    let unblocked = proposer
        .list_dome_connection_topology(context)
        .await
        .expect("topology after unblock");
    assert!(
        unblocked
            .resolution
            .topology
            .active_connection_ids
            .is_empty()
    );
    assert_eq!(
        unblocked.connections[0].record.status,
        kukuri_core::DomeConnectionStatusV1::Revoked
    );
}

#[tokio::test]
async fn proposer_block_discards_open_proposal_and_unblock_does_not_restore_it() {
    let (proposer, receiver, context, _, receiver_pubkey) =
        open_proposal_fixture("proposer-block").await;

    proposer
        .block_author(&receiver_pubkey)
        .await
        .expect("block receiver owner");
    let blocked = proposer
        .list_dome_connection_topology(context.clone())
        .await
        .expect("blocked topology");
    assert_eq!(
        blocked.proposals[0].status,
        DomeProposalDerivedStatusV1::Discarded
    );
    assert_eq!(
        blocked.proposals[0].terminal_reason,
        Some(kukuri_core::DomeConnectionTerminalReasonV1::OwnersBlocked)
    );
    assert!(blocked.connections.is_empty());
    assert!(
        receiver
            .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
                spatial_context: context.clone(),
                proposal_id: "proposal-proposer-block".into(),
            })
            .await
            .is_err()
    );

    proposer
        .unblock_author(&receiver_pubkey)
        .await
        .expect("unblock receiver owner");
    let unblocked = proposer
        .list_dome_connection_topology(context)
        .await
        .expect("topology after unblock");
    assert_eq!(
        unblocked.proposals[0].status,
        DomeProposalDerivedStatusV1::Discarded
    );
}

#[tokio::test]
async fn receiver_block_discards_open_proposal_and_prevents_accept() {
    let (proposer, receiver, context, proposer_pubkey, _) =
        open_proposal_fixture("receiver-block").await;

    receiver
        .block_author(&proposer_pubkey)
        .await
        .expect("block proposer owner");
    let blocked = receiver
        .list_dome_connection_topology(context.clone())
        .await
        .expect("blocked topology");
    assert_eq!(
        blocked.proposals[0].status,
        DomeProposalDerivedStatusV1::Discarded
    );
    assert_eq!(
        blocked.proposals[0].terminal_reason,
        Some(kukuri_core::DomeConnectionTerminalReasonV1::OwnersBlocked)
    );
    assert!(blocked.connections.is_empty());
    assert!(
        receiver
            .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
                spatial_context: context.clone(),
                proposal_id: "proposal-receiver-block".into(),
            })
            .await
            .is_err()
    );
    assert!(
        proposer
            .list_dome_connection_topology(context)
            .await
            .expect("proposer topology")
            .connections
            .is_empty()
    );
}

#[tokio::test]
async fn accept_rechecks_owner_block_before_persisting_connection_records() {
    let (_, receiver, context, proposer_pubkey, _) =
        open_proposal_fixture("accept-block-recheck").await;
    let block = build_block_edge_envelope(
        receiver.services.keys.as_ref(),
        &Pubkey::from(proposer_pubkey),
        BlockEdgeStatus::Active,
    )
    .expect("build block edge");
    receiver
        .services
        .store
        .put_envelope(block)
        .await
        .expect("store block edge without reconciliation");

    let error = receiver
        .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
            spatial_context: context.clone(),
            proposal_id: "proposal-accept-block-recheck".into(),
        })
        .await
        .expect_err("block must be rechecked before accept persistence");
    assert!(error.to_string().contains("DOME_CONNECTION_OWNERS_BLOCKED"));
    let topology = receiver
        .list_dome_connection_topology(context)
        .await
        .expect("topology after rejected accept");
    assert!(topology.connections.is_empty());
    assert_eq!(
        topology.proposals[0].terminal_reason,
        Some(kukuri_core::DomeConnectionTerminalReasonV1::OwnersBlocked)
    );
}

#[tokio::test]
async fn only_proposer_can_withdraw_and_only_receiver_can_accept() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let proposer =
        app_with_shared_dome_services(docs_sync.clone(), blob_service.clone(), generate_keys());
    let receiver =
        app_with_shared_dome_services(docs_sync.clone(), blob_service.clone(), generate_keys());
    let outsider = app_with_shared_dome_services(docs_sync, blob_service, generate_keys());
    let topic = "kukuri:topic:dome-connection-auth";
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    let proposer_instance = proposer
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "A".into(),
                description: String::new(),
                max_peers: None,
            },
        )
        .await
        .expect("create A");
    let receiver_instance = receiver
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "B".into(),
                description: String::new(),
                max_peers: None,
            },
        )
        .await
        .expect("create B");
    assert!(
        proposer
            .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
                proposal_id: "../invalid".into(),
                spatial_context: context.clone(),
                proposer_instance_id: proposer_instance.clone(),
                receiver_instance_id: receiver_instance.clone(),
                proposer_direction: DomeDirection::North,
            })
            .await
            .is_err()
    );
    proposer
        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
            proposal_id: "proposal-auth".into(),
            spatial_context: context.clone(),
            proposer_instance_id: proposer_instance,
            receiver_instance_id: receiver_instance,
            proposer_direction: DomeDirection::North,
        })
        .await
        .expect("create proposal");

    assert!(
        outsider
            .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
                spatial_context: context.clone(),
                proposal_id: "proposal-auth".into(),
            })
            .await
            .is_err()
    );
    assert!(
        receiver
            .withdraw_dome_connection_proposal(WithdrawDomeConnectionProposalInput {
                spatial_context: context.clone(),
                proposal_id: "proposal-auth".into(),
            })
            .await
            .is_err()
    );
    let withdrawn = proposer
        .withdraw_dome_connection_proposal(WithdrawDomeConnectionProposalInput {
            spatial_context: context,
            proposal_id: "proposal-auth".into(),
        })
        .await
        .expect("withdraw proposal");
    assert_eq!(withdrawn.status, DomeProposalDerivedStatusV1::Discarded);
}
