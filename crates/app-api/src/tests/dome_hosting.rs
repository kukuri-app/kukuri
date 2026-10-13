use super::*;
use crate::{
    ActivateCommunityNodeDomeHostingInput, CloseDomeHostingInput, CommitDomeLayoutInput,
    DomeLayoutCommitOutcome, PrepareCommunityNodeDomeHostingInput, StartOwnerDomeHostingInput,
    SubmitDomeSessionInput,
};
use kukuri_core::{
    DomeHostingStateKindV1, DomeSessionInputKindV1, KukuriKeys, MetaversePersistentPropV1,
    MetaversePrimitive, SignedDomeHostingLeaseV1, SpatialContextV1, accept_dome_hosting_lease,
};

#[tokio::test]
async fn owner_explicitly_transfers_hosting_to_one_community_node() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:dome-hosting";
    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "Hosted Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create Dome");
    let context = SpatialContextV1::Topic {
        topic_id: kukuri_core::TopicId::new(topic),
    };

    let initial = app
        .get_dome_hosting(context.clone(), &room_id)
        .await
        .expect("initial hosting");
    assert_eq!(initial.state.kind, DomeHostingStateKindV1::Closed);

    let owner_hosted = app
        .start_owner_dome_hosting(StartOwnerDomeHostingInput {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: room_id.clone(),
            endpoint_id: "owner-endpoint".into(),
            lease_duration_millis: 60_000,
        })
        .await
        .expect("start owner hosting");
    assert_eq!(owner_hosted.state.kind, DomeHostingStateKindV1::OwnerHosted);
    assert_eq!(owner_hosted.state.lease_epoch, Some(1));

    let node_keys = KukuriKeys::generate();
    let transferring = app
        .prepare_community_node_dome_hosting(PrepareCommunityNodeDomeHostingInput {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: room_id.clone(),
            node_id: node_keys.public_key_hex(),
            api_base_url: "https://node.example".into(),
            lease_duration_millis: 60_000,
        })
        .await
        .expect("prepare Community Node hosting");
    assert_eq!(
        transferring.state.kind,
        DomeHostingStateKindV1::Transferring
    );
    assert_eq!(transferring.state.lease_epoch, Some(2));

    let still_transferring = app
        .get_dome_hosting(context.clone(), &room_id)
        .await
        .expect("owner online must not reclaim");
    assert_eq!(
        still_transferring.state.kind,
        DomeHostingStateKindV1::Transferring
    );

    let signed_lease: SignedDomeHostingLeaseV1 = serde_json::from_str(
        transferring
            .signed_lease_json
            .as_deref()
            .expect("signed lease JSON"),
    )
    .expect("decode signed lease");
    let acceptance = accept_dome_hosting_lease(
        &node_keys,
        &signed_lease,
        "community-session-1",
        signed_lease.lease.issued_at,
    )
    .expect("node acceptance");
    let active = app
        .activate_community_node_dome_hosting(ActivateCommunityNodeDomeHostingInput {
            spatial_context: context.clone(),
            instance_id: room_id.clone(),
            signed_acceptance_json: serde_json::to_string(&acceptance).unwrap(),
        })
        .await
        .expect("activate Community Node");
    assert_eq!(
        active.state.kind,
        DomeHostingStateKindV1::CommunityNodeHosted
    );
    assert_eq!(
        active.state.session_id.as_deref(),
        Some("community-session-1")
    );

    let closed = app
        .close_dome_hosting(CloseDomeHostingInput {
            expected_generation: None,
            spatial_context: context,
            instance_id: room_id,
        })
        .await
        .expect("close hosting");
    assert_eq!(closed.state.kind, DomeHostingStateKindV1::Closed);
}

#[tokio::test]
async fn owner_layout_commit_is_explicit_idempotent_and_restarts_from_new_revision() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:dome-layout";
    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "Layout Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .unwrap();
    let context = SpatialContextV1::Topic {
        topic_id: kukuri_core::TopicId::new(topic),
    };
    app.start_owner_dome_hosting(StartOwnerDomeHostingInput {
        expected_generation: None,
        spatial_context: context.clone(),
        instance_id: room_id.clone(),
        endpoint_id: "owner-endpoint".into(),
        lease_duration_millis: 60_000,
    })
    .await
    .unwrap();

    app.submit_dome_session_input(SubmitDomeSessionInput {
        expected_generation: None,
        spatial_context: context.clone(),
        instance_id: room_id.clone(),
        sequence: 1,
        input: DomeSessionInputKindV1::UpsertPersistentProp {
            prop: MetaversePersistentPropV1 {
                prop_id: "layout-prop".into(),
                asset_ref: None,
                primitive_fallback: MetaversePrimitive::Cube,
                position: [100, 100, 100],
                rotation: [0, 0, 0],
                scale: [100, 100, 100],
                visual_only: false,
                interactions: Vec::new(),
                collider: None,
            },
        },
    })
    .await
    .unwrap();

    let committed = app
        .commit_dome_layout(CommitDomeLayoutInput {
            spatial_context: context.clone(),
            instance_id: room_id.clone(),
            operation_id: "layout-operation-1".into(),
            signed_candidate_json: None,
        })
        .await
        .unwrap();
    assert_eq!(committed.outcome, DomeLayoutCommitOutcome::Committed);
    assert_eq!(committed.revision, 2);
    assert_eq!(committed.hosting.state.lease_epoch, Some(2));
    assert!(committed.signed_commit_json.is_some());

    let retried = app
        .commit_dome_layout(CommitDomeLayoutInput {
            spatial_context: context.clone(),
            instance_id: room_id.clone(),
            operation_id: "layout-operation-1".into(),
            signed_candidate_json: None,
        })
        .await
        .unwrap();
    assert_eq!(retried.revision, 2);
    assert_eq!(retried.manifest_blob_hash, committed.manifest_blob_hash);

    let no_op = app
        .commit_dome_layout(CommitDomeLayoutInput {
            spatial_context: context,
            instance_id: room_id,
            operation_id: "layout-operation-2".into(),
            signed_candidate_json: None,
        })
        .await
        .unwrap();
    assert_eq!(no_op.outcome, DomeLayoutCommitOutcome::NoOp);
    assert_eq!(no_op.revision, 2);
}

// 稼働の状態の読取りは、先に進んだ Dome の時計を戻さず、失敗しない(#1803)。読取りの時刻は lock の前に取るので、
// 待つ間に session の処理(snapshot・input)が後の時刻で時計を進めていることがある。
#[tokio::test]
async fn reading_the_hosting_after_the_session_clock_moved_ahead_succeeds() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:dome-hosting-clock";
    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "Hosted Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create Dome");
    let context = SpatialContextV1::Topic {
        topic_id: kukuri_core::TopicId::new(topic),
    };
    app.start_owner_dome_hosting(StartOwnerDomeHostingInput {
        expected_generation: None,
        spatial_context: context.clone(),
        instance_id: room_id.clone(),
        endpoint_id: "owner-endpoint".into(),
        lease_duration_millis: 60_000,
    })
    .await
    .expect("start owner hosting");
    app.dome_host_sessions
        .lock()
        .await
        .get_mut(&room_id)
        .expect("the hosted session")
        .advance_to(Utc::now().timestamp_millis() + 1_000)
        .expect("move the session clock ahead");

    let hosting = app
        .get_dome_hosting(context, &room_id)
        .await
        .expect("read the hosting");
    assert_eq!(hosting.state.kind, DomeHostingStateKindV1::OwnerHosted);
}
