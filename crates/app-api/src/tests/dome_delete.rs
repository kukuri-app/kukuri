use super::*;
use crate::{DeleteDomeInput, StartOwnerDomeHostingInput};
use kukuri_core::{DomeInstanceStatusV1, SpatialContextV1, TopicId};
use kukuri_docs_sync::{DocEventStream, DocFetchPolicy};
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct DeleteFaultDocs {
    inner: MemoryDocsSync,
    writes: AtomicUsize,
    fail_at: AtomicUsize,
}
#[async_trait]
impl DocsSync for DeleteFaultDocs {
    async fn open_replica(&self, replica: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica).await
    }
    async fn apply_doc_op(&self, replica: &ReplicaId, op: DocOp) -> Result<()> {
        let sequence = self.writes.fetch_add(1, Ordering::SeqCst) + 1;
        if sequence == self.fail_at.load(Ordering::SeqCst) {
            anyhow::bail!("injected delete write failure");
        }
        self.inner.apply_doc_op(replica, op).await
    }
    async fn query_replica_with_policy(
        &self,
        replica: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.inner
            .query_replica_with_policy(replica, query, policy)
            .await
    }
    async fn query_replica_keys(
        &self,
        replica: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica, query).await
    }
    async fn subscribe_replica(&self, replica: &ReplicaId) -> Result<DocEventStream> {
        self.inner.subscribe_replica(replica).await
    }
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

#[tokio::test]
async fn dome_delete_write_failures_remain_retryable_after_restart() {
    for (hosted, fail_at) in (1..=5)
        .map(|n| (false, n))
        .chain((1..=7).map(|n| (true, n)))
    {
        let docs = Arc::new(DeleteFaultDocs::default());
        let store = Arc::new(MemoryStore::default());
        let transport = Arc::new(FakeTransport::new("delete-fault", FakeNetwork::default()));
        let app = AppService::from_handles(ServiceHandles::new(
            store.clone(),
            store,
            transport.clone(),
            transport,
            docs.clone(),
            Arc::new(MemoryBlobService::default()),
            generate_keys(),
        ));
        let topic = "kukuri:topic:delete-fault";
        let context = SpatialContextV1::Topic {
            topic_id: TopicId::new(topic),
        };
        let room = app
            .create_metaverse_room(topic, create_input())
            .await
            .unwrap();
        let input = DeleteDomeInput {
            spatial_context: context.clone(),
            instance_id: room.clone(),
            expected_generation: 1,
            operation_id: "fault".into(),
        };
        if hosted {
            app.start_owner_dome_hosting(StartOwnerDomeHostingInput {
                expected_generation: None,
                spatial_context: context.clone(),
                instance_id: room.clone(),
                endpoint_id: "fault-owner".into(),
                lease_duration_millis: 60_000,
            })
            .await
            .unwrap();
        }
        docs.writes.store(0, Ordering::SeqCst);
        docs.fail_at.store(fail_at, Ordering::SeqCst);
        let first = app.delete_dome(input.clone()).await;
        assert!(first.is_err(), "fault point {fail_at} must be reached");
        docs.fail_at.store(0, Ordering::SeqCst);
        let restarted = AppService::from_handles(app.services.clone());
        if fail_at > 1 {
            let pending = restarted
                .list_pending_dome_deletions(context)
                .await
                .unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].request, input);
        }
        assert!(restarted.delete_dome(input).await.unwrap().deleted);
        assert!(restarted.list_game_rooms(topic).await.unwrap().is_empty());
        assert_eq!(
            restarted
                .create_metaverse_room(topic, create_input())
                .await
                .unwrap(),
            room
        );
    }
}

fn create_input() -> CreateMetaverseRoomInput {
    CreateMetaverseRoomInput {
        title: "Deletion contract".into(),
        description: String::new(),
        max_peers: Some(8),
    }
}

#[tokio::test]
async fn stale_management_hosting_requests_cannot_mutate_recreated_dome() {
    for action in ["start", "stop", "delegate"] {
        let app = AppService::new(
            Arc::new(MemoryStore::default()),
            Arc::new(FakeTransport::new(
                "stale-management",
                FakeNetwork::default(),
            )),
        );
        let topic = "kukuri:topic:stale-management";
        let context = SpatialContextV1::Topic {
            topic_id: TopicId::new(topic),
        };
        let id = app
            .create_metaverse_room(topic, create_input())
            .await
            .unwrap();
        app.delete_dome(DeleteDomeInput {
            spatial_context: context.clone(),
            instance_id: id.clone(),
            expected_generation: 1,
            operation_id: "replace".into(),
        })
        .await
        .unwrap();
        app.create_metaverse_room(topic, create_input())
            .await
            .unwrap();
        app.start_owner_dome_hosting(StartOwnerDomeHostingInput {
            expected_generation: Some(2),
            spatial_context: context.clone(),
            instance_id: id.clone(),
            endpoint_id: "new-owner".into(),
            lease_duration_millis: 60_000,
        })
        .await
        .unwrap();
        let replica = topic_replica_id(topic);
        let before = app
            .services
            .docs_sync
            .query_replica(&replica, DocQuery::Prefix(String::new()))
            .await
            .unwrap();
        let result = match action {
            "start" => app
                .start_owner_dome_hosting(StartOwnerDomeHostingInput {
                    expected_generation: Some(1),
                    spatial_context: context.clone(),
                    instance_id: id.clone(),
                    endpoint_id: "stale-owner".into(),
                    lease_duration_millis: 60_000,
                })
                .await
                .map(|_| ()),
            "stop" => app
                .close_dome_hosting(crate::CloseDomeHostingInput {
                    expected_generation: Some(1),
                    spatial_context: context.clone(),
                    instance_id: id.clone(),
                })
                .await
                .map(|_| ()),
            _ => app
                .prepare_community_node_dome_hosting(crate::PrepareCommunityNodeDomeHostingInput {
                    expected_generation: Some(1),
                    spatial_context: context.clone(),
                    instance_id: id.clone(),
                    node_id: generate_keys().public_key_hex(),
                    api_base_url: "https://node.example".into(),
                    lease_duration_millis: 60_000,
                })
                .await
                .map(|_| ()),
        };
        assert!(result.is_err(), "stale {action} must be rejected");
        let after = app
            .services
            .docs_sync
            .query_replica(&replica, DocQuery::Prefix(String::new()))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
        assert_eq!(
            app.dome_host_sessions
                .lock()
                .await
                .get(&id)
                .unwrap()
                .lease()
                .instance_generation,
            2
        );
    }
}

#[tokio::test]
async fn stopped_dome_delete_recreates_same_id_with_new_generation_and_old_retry_is_harmless() {
    let app = AppService::new(
        Arc::new(MemoryStore::default()),
        Arc::new(FakeTransport::new("self", FakeNetwork::default())),
    );
    let topic = "kukuri:topic:delete-contract";
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    let room = app
        .create_metaverse_room(topic, create_input())
        .await
        .unwrap();
    assert!(
        app.create_metaverse_room(topic, create_input())
            .await
            .is_err()
    );
    let input = DeleteDomeInput {
        spatial_context: context.clone(),
        instance_id: room.clone(),
        expected_generation: 1,
        operation_id: "delete-one".into(),
    };
    assert!(app.delete_dome(input.clone()).await.unwrap().deleted);
    assert!(app.list_game_rooms(topic).await.unwrap().is_empty());
    assert_eq!(
        app.create_metaverse_room(topic, create_input())
            .await
            .unwrap(),
        room
    );
    let before = app
        .fetch_dome_instance_manifest(&topic_replica_id(topic), &app.keys().public_key())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.1.generation, 2);
    assert_eq!(before.1.status, DomeInstanceStatusV1::Active);
    assert!(app.delete_dome(input.clone()).await.unwrap().deleted);
    let after = app
        .fetch_dome_instance_manifest(&topic_replica_id(topic), &app.keys().public_key())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before, after);
    let mut different = input;
    different.operation_id = "different".into();
    assert!(app.delete_dome(different).await.is_err());
    assert_eq!(app.list_game_rooms(topic).await.unwrap().len(), 1);
}

#[tokio::test]
async fn deletion_rejects_wrong_context_identity_and_generation_without_changing_records() {
    let app = AppService::new(
        Arc::new(MemoryStore::default()),
        Arc::new(FakeTransport::new("self", FakeNetwork::default())),
    );
    let topic = "kukuri:topic:delete-guards";
    let room = app
        .create_metaverse_room(topic, create_input())
        .await
        .unwrap();
    let replica = topic_replica_id(topic);
    let before = app
        .services
        .docs_sync
        .query_replica(&replica, DocQuery::Prefix(String::new()))
        .await
        .unwrap();
    for (id, generation, target) in [
        ("another-owner", 1, topic),
        (room.as_str(), 2, topic),
        (room.as_str(), 1, "kukuri:topic:other"),
    ] {
        let input = DeleteDomeInput {
            spatial_context: SpatialContextV1::Topic {
                topic_id: TopicId::new(target),
            },
            instance_id: id.into(),
            expected_generation: generation,
            operation_id: "invalid".into(),
        };
        assert!(app.delete_dome(input).await.is_err());
        let after = app
            .services
            .docs_sync
            .query_replica(&replica, DocQuery::Prefix(String::new()))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&before).unwrap(),
            serde_json::to_value(&after).unwrap()
        );
    }
    assert!(!app.has_topic_subscription("kukuri:topic:other").await);
}

#[tokio::test]
async fn active_dome_deletion_stops_host_and_survives_service_restart() {
    let app = AppService::new(
        Arc::new(MemoryStore::default()),
        Arc::new(FakeTransport::new("self", FakeNetwork::default())),
    );
    let topic = "kukuri:topic:delete-restart";
    let room = app
        .create_metaverse_room(topic, create_input())
        .await
        .unwrap();
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    app.start_owner_dome_hosting(StartOwnerDomeHostingInput {
        expected_generation: None,
        spatial_context: context.clone(),
        instance_id: room.clone(),
        endpoint_id: "owner".into(),
        lease_duration_millis: 60_000,
    })
    .await
    .unwrap();
    let input = DeleteDomeInput {
        spatial_context: context.clone(),
        instance_id: room.clone(),
        expected_generation: 1,
        operation_id: "restart".into(),
    };
    app.delete_dome(input.clone()).await.unwrap();
    assert!(!app.dome_host_sessions.lock().await.contains_key(&room));
    let restarted = AppService::from_handles(app.services.clone());
    assert!(restarted.list_game_rooms(topic).await.unwrap().is_empty());
    assert!(restarted.delete_dome(input).await.unwrap().deleted);
    assert_eq!(
        restarted
            .create_metaverse_room(topic, create_input())
            .await
            .unwrap(),
        room
    );
    let hosted = restarted
        .start_owner_dome_hosting(StartOwnerDomeHostingInput {
            expected_generation: None,
            spatial_context: context,
            instance_id: room,
            endpoint_id: "owner".into(),
            lease_duration_millis: 60_000,
        })
        .await
        .unwrap();
    assert_eq!(hosted.state.lease_epoch, Some(2));
}

#[tokio::test]
async fn stale_session_inputs_cannot_mutate_recreated_dome() {
    use crate::SubmitDomeSessionInput;
    use kukuri_core::DomeSessionInputKindV1;
    let app = AppService::new(
        Arc::new(MemoryStore::default()),
        Arc::new(FakeTransport::new("stale-input", FakeNetwork::default())),
    );
    let topic = "kukuri:topic:stale-input";
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    let id = app
        .create_metaverse_room(topic, create_input())
        .await
        .unwrap();
    app.start_owner_dome_hosting(StartOwnerDomeHostingInput {
        expected_generation: Some(1),
        spatial_context: context.clone(),
        instance_id: id.clone(),
        endpoint_id: "owner".into(),
        lease_duration_millis: 60_000,
    })
    .await
    .unwrap();
    let mut prop = app.list_game_rooms(topic).await.unwrap()[0]
        .metaverse
        .as_ref()
        .unwrap()
        .dome
        .customization
        .persistent_props[0]
        .clone();
    prop.position[0] += 250;
    let delayed = [
        DomeSessionInputKindV1::Join {
            avatar_collider: None,
        },
        DomeSessionInputKindV1::UpsertPersistentProp { prop },
    ];
    app.delete_dome(DeleteDomeInput {
        spatial_context: context.clone(),
        instance_id: id.clone(),
        expected_generation: 1,
        operation_id: "replace-input".into(),
    })
    .await
    .unwrap();
    app.create_metaverse_room(topic, create_input())
        .await
        .unwrap();
    app.start_owner_dome_hosting(StartOwnerDomeHostingInput {
        expected_generation: Some(2),
        spatial_context: context.clone(),
        instance_id: id.clone(),
        endpoint_id: "owner".into(),
        lease_duration_millis: 60_000,
    })
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let before = app
        .dome_host_sessions
        .lock()
        .await
        .get_mut(&id)
        .unwrap()
        .signed_snapshot(now)
        .unwrap()
        .snapshot;
    for (i, input) in delayed.into_iter().enumerate() {
        let result = app
            .submit_dome_session_input(SubmitDomeSessionInput {
                expected_generation: Some(1),
                spatial_context: context.clone(),
                instance_id: id.clone(),
                sequence: i as u64 + 1,
                input,
            })
            .await;
        assert!(
            result.is_err(),
            "stale input must be rejected before mutation"
        );
    }
    let after = app
        .dome_host_sessions
        .lock()
        .await
        .get_mut(&id)
        .unwrap()
        .signed_snapshot(now)
        .unwrap()
        .snapshot;
    assert_eq!(before.bodies, after.bodies);
    assert_eq!(
        app.dome_host_sessions
            .lock()
            .await
            .get(&id)
            .unwrap()
            .participant_count(),
        0
    );
}
