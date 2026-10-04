//! #1527: 所有者の端末の Dome host が、別の端末の participant の要求を Community Node と同じ確認点で処理する。
//! 別の端末の鍵で署名した要求を、所有者の端末の受け口へ直接渡す(transport の test double)。

use super::dome_connections::{app_with_shared_dome_services, show_domes};
use super::*;
use crate::{
    AcceptDomeConnectionProposalInput, CreateDomeConnectionProposalInput,
    StartOwnerDomeHostingInput, SubmitDomeSessionInput,
};
use kukuri_core::{
    ChannelAudienceKind, ChannelId, CreatePrivateChannelInput, DomeDirection,
    DomePhysicsSnapshotV1, DomeSessionInputKindV1, DomeSessionInputV1, DomeSessionRequestV1,
    DomeSessionResponseV1, DomeTransitionAdmissionRequestV1, KukuriKeys,
    MetaverseResourceRejection, SpatialContextV1, build_dome_spatial_access_proof,
    build_signed_dome_session_input,
};

const TOPIC: &str = "kukuri:topic:dome-session";

async fn hosted_dome(
    owner: &AppService,
    channel: ChannelRef,
    max_peers: u32,
) -> (SpatialContextV1, String) {
    let context = match &channel {
        ChannelRef::PrivateChannel { channel_id } => SpatialContextV1::Channel {
            topic_id: TopicId::new(TOPIC),
            channel_id: channel_id.clone(),
        },
        _ => SpatialContextV1::Topic {
            topic_id: TopicId::new(TOPIC),
        },
    };
    let dome = owner
        .create_metaverse_room_in_channel(
            TOPIC,
            channel,
            CreateMetaverseRoomInput {
                title: "Dome".into(),
                description: String::new(),
                max_peers: Some(max_peers),
            },
        )
        .await
        .expect("create Dome");
    owner
        .start_owner_dome_hosting(StartOwnerDomeHostingInput {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: dome.clone(),
            endpoint_id: "owner-endpoint".into(),
            lease_duration_millis: 60_000,
        })
        .await
        .expect("start owner hosting");
    (context, dome)
}

/// 別の端末の participant が、host の lease と session に束縛して署名した input の要求。
async fn input_request(
    owner: &AppService,
    visitor: &KukuriKeys,
    dome: &str,
    sequence: u64,
    input: DomeSessionInputKindV1,
) -> Vec<u8> {
    let (lease, session_id) = {
        let sessions = owner.dome_host_sessions.lock().await;
        let runtime = sessions.get(dome).expect("owner session");
        (runtime.lease().clone(), runtime.session_id().to_string())
    };
    let signed_input = build_signed_dome_session_input(
        visitor,
        DomeSessionInputV1 {
            input_id: format!("input-{dome}-{sequence}"),
            instance_id: dome.to_string(),
            instance_generation: lease.instance_generation,
            lease_epoch: lease.epoch,
            session_id,
            participant_pubkey: visitor.public_key(),
            sequence,
            sent_at: Utc::now().timestamp_millis(),
            input,
        },
    )
    .expect("signed input");
    serde_json::to_vec(&DomeSessionRequestV1::Input { signed_input }).expect("request")
}

async fn serve(owner: &AppService, request: Vec<u8>) -> DomeSessionResponseV1 {
    serde_json::from_slice(&owner.serve_dome_session_request(&request).await).expect("response")
}

fn snapshot(response: DomeSessionResponseV1) -> DomePhysicsSnapshotV1 {
    match response {
        DomeSessionResponseV1::Snapshot { signed_snapshot } => signed_snapshot.snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn rejection(response: DomeSessionResponseV1) -> String {
    match response {
        DomeSessionResponseV1::Rejected { message, .. } => message,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

fn has_avatar(snapshot: &DomePhysicsSnapshotV1, keys: &KukuriKeys) -> bool {
    let entity_id = format!("avatar:{}", keys.public_key().as_str());
    snapshot
        .bodies
        .iter()
        .any(|body| body.entity_id == entity_id)
}

async fn owner_input(
    owner: &AppService,
    context: &SpatialContextV1,
    dome: &str,
    sequence: u64,
    input: DomeSessionInputKindV1,
) -> DomePhysicsSnapshotV1 {
    owner
        .submit_dome_session_input(SubmitDomeSessionInput {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: dome.to_string(),
            sequence,
            input,
        })
        .await
        .expect("owner input")
        .snapshot
}

const JOIN: DomeSessionInputKindV1 = DomeSessionInputKindV1::Join {
    avatar_collider: None,
};

// 入室・滞在・退室(AC-1 の 1・2)。input が止まった participant は 30 秒で除去される。
#[tokio::test]
async fn a_visitor_enters_stays_and_times_out_through_the_owner_device() {
    let (owner, _, _, _) = local_app_with_memory_services();
    let visitor = generate_keys();
    let (context, dome) = hosted_dome(&owner, ChannelRef::Public, 8).await;

    let joined = snapshot(
        serve(
            &owner,
            input_request(&owner, &visitor, &dome, 1, JOIN).await,
        )
        .await,
    );
    assert!(
        has_avatar(&joined, &visitor),
        "the admission shows the visitor"
    );
    let owner_view = owner_input(&owner, &context, &dome, 1, JOIN).await;
    assert!(
        has_avatar(&owner_view, &visitor),
        "the owner sees the visitor"
    );

    sleep(Duration::from_millis(150)).await;
    let move_input = DomeSessionInputKindV1::Move {
        position: [100, 0, 100],
        rotation: [0, 0, 0],
        animation: "walk".into(),
    };
    let moved = snapshot(
        serve(
            &owner,
            input_request(&owner, &visitor, &dome, 2, move_input).await,
        )
        .await,
    );
    assert!(
        has_avatar(&moved, &owner.services.keys),
        "the visitor sees the owner"
    );
    snapshot(
        serve(
            &owner,
            input_request(
                &owner,
                &visitor,
                &dome,
                3,
                DomeSessionInputKindV1::KeepAlive,
            )
            .await,
        )
        .await,
    );

    // 30 秒 input の無い participant は host が除去する(ADR 0045)。
    let mut sessions = owner.dome_host_sessions.lock().await;
    let runtime = sessions.get_mut(&dome).expect("owner session");
    assert!(runtime.is_participant(&visitor.public_key()));
    runtime
        .advance_to(
            Utc::now().timestamp_millis() + kukuri_core::DOME_PARTICIPANT_TIMEOUT_MILLIS + 1_000,
        )
        .expect("advance the session clock");
    assert!(
        !runtime.is_participant(&visitor.public_key()),
        "the silent visitor is removed"
    );
    drop(sessions);
    owner.shutdown().await;
}

// Leave で participant から外れ、Join していない署名者の Join 以外の input は状態を変えずに拒否される(AC-1 の 2・3)。
#[tokio::test]
async fn inputs_from_a_signer_who_has_not_joined_are_rejected_without_side_effects() {
    let (owner, _, _, _) = local_app_with_memory_services();
    let visitor = generate_keys();
    let (context, dome) = hosted_dome(&owner, ChannelRef::Public, 8).await;

    // 未入室の Move は拒否され、sequence も進めない(続く小さな sequence の Join が通る)。
    let stray = DomeSessionInputKindV1::Move {
        position: [0, 0, 0],
        rotation: [0, 0, 0],
        animation: "walk".into(),
    };
    let message = rejection(
        serve(
            &owner,
            input_request(&owner, &visitor, &dome, 5, stray).await,
        )
        .await,
    );
    assert!(message.contains("DOME_SESSION_NOT_JOINED"), "{message}");
    assert!(!has_avatar(
        &owner_input(&owner, &context, &dome, 1, JOIN).await,
        &visitor
    ));
    snapshot(
        serve(
            &owner,
            input_request(&owner, &visitor, &dome, 1, JOIN).await,
        )
        .await,
    );

    snapshot(
        serve(
            &owner,
            input_request(&owner, &visitor, &dome, 2, DomeSessionInputKindV1::Leave).await,
        )
        .await,
    );
    sleep(Duration::from_millis(150)).await;
    assert!(!has_avatar(
        &owner_input(
            &owner,
            &context,
            &dome,
            2,
            DomeSessionInputKindV1::KeepAlive
        )
        .await,
        &visitor
    ));
    let message = rejection(
        serve(
            &owner,
            input_request(&owner, &visitor, &dome, 3, DomeSessionInputKindV1::Leave).await,
        )
        .await,
    );
    assert!(message.contains("DOME_SESSION_NOT_JOINED"), "{message}");
    owner.shutdown().await;
}

// owner に block された participant の Join と KeepAlive、channel に参加していない端末の Join は拒否される(AC-1 の 3)。
#[tokio::test]
async fn access_and_block_are_evaluated_by_the_owner_device() {
    let (owner, _, _, _) = local_app_with_memory_services();
    owner.switch_writer(1);
    let visitor = generate_keys();
    let (_, dome) = hosted_dome(&owner, ChannelRef::Public, 8).await;
    snapshot(
        serve(
            &owner,
            input_request(&owner, &visitor, &dome, 1, JOIN).await,
        )
        .await,
    );
    owner
        .block_author(visitor.public_key().as_str())
        .await
        .expect("block the visitor");
    let message = rejection(
        serve(
            &owner,
            input_request(
                &owner,
                &visitor,
                &dome,
                2,
                DomeSessionInputKindV1::KeepAlive,
            )
            .await,
        )
        .await,
    );
    assert_eq!(message, "DOME_TRANSITION_VISITOR_BLOCKED");
    let outsider = generate_keys();
    owner
        .block_author(outsider.public_key().as_str())
        .await
        .expect("block another visitor");
    let message = rejection(
        serve(
            &owner,
            input_request(&owner, &outsider, &dome, 1, JOIN).await,
        )
        .await,
    );
    assert_eq!(message, "DOME_TRANSITION_VISITOR_BLOCKED");

    let channel = owner
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(TOPIC),
            label: "domes".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_ref = ChannelRef::PrivateChannel {
        channel_id: ChannelId::new(channel.channel_id),
    };
    let (_, private_dome) = hosted_dome(&owner, channel_ref, 8).await;
    let stranger = generate_keys();
    let message = rejection(
        serve(
            &owner,
            input_request(&owner, &stranger, &private_dome, 1, JOIN).await,
        )
        .await,
    );
    assert_eq!(message, "DOME_TRANSITION_ACCESS_DENIED");
    let sessions = owner.dome_host_sessions.lock().await;
    assert_eq!(
        sessions
            .get(&private_dome)
            .expect("session")
            .participant_count(),
        0
    );
    drop(sessions);
    owner.shutdown().await;
}

// 定員を超える Join は、resource budget の拒否を型のまま返す(ADR 0041)。
#[tokio::test]
async fn a_full_dome_rejects_the_join_with_the_typed_budget_rejection() {
    let (owner, _, _, _) = local_app_with_memory_services();
    let (context, dome) = hosted_dome(&owner, ChannelRef::Public, 1).await;
    owner_input(&owner, &context, &dome, 1, JOIN).await;
    let visitor = generate_keys();
    match serve(
        &owner,
        input_request(&owner, &visitor, &dome, 1, JOIN).await,
    )
    .await
    {
        DomeSessionResponseV1::Rejected {
            resource_rejection: Some(MetaverseResourceRejection { .. }),
            ..
        } => {}
        other => panic!("expected a typed budget rejection, got {other:?}"),
    }
    owner.shutdown().await;
}

// 入室中の participant の再同期に ring を返し、入室していない要求者と束縛の違う proof は拒否する(AC-1 の 4)。
#[tokio::test]
async fn resync_requires_a_current_participant_with_a_bound_proof() {
    let (owner, _, _, _) = local_app_with_memory_services();
    let visitor = generate_keys();
    let (context, dome) = hosted_dome(&owner, ChannelRef::Public, 8).await;
    let resync = |keys: &KukuriKeys, target_owner: Pubkey| {
        let access_proof = build_dome_spatial_access_proof(
            keys,
            context.clone(),
            target_owner,
            Utc::now().timestamp_millis(),
            None,
            None,
        )
        .expect("access proof");
        serde_json::to_vec(&DomeSessionRequestV1::ResyncSnapshots {
            instance_id: dome.clone(),
            after_sequence: 0,
            access_proof,
        })
        .expect("request")
    };
    let owner_pubkey = owner.services.keys.public_key();

    let message = rejection(serve(&owner, resync(&visitor, owner_pubkey.clone())).await);
    assert!(message.contains("DOME_SESSION_NOT_JOINED"), "{message}");

    snapshot(
        serve(
            &owner,
            input_request(&owner, &visitor, &dome, 1, JOIN).await,
        )
        .await,
    );
    match serve(&owner, resync(&visitor, owner_pubkey)).await {
        DomeSessionResponseV1::Snapshots { snapshots } => assert!(!snapshots.is_empty()),
        other => panic!("expected the ring, got {other:?}"),
    }
    let message = rejection(serve(&owner, resync(&visitor, generate_keys().public_key())).await);
    assert!(
        message.contains("another participant, context, or owner"),
        "{message}"
    );
    owner.shutdown().await;
}

// 入力ごとの処理は、受信した Dome host の heartbeat の台帳を読まない(INVAR-4。台帳の件数に比例する走査をしない)。
// 台帳の lock を握ったままでも、別の端末と所有者本人の input が処理される。
#[tokio::test]
async fn inputs_do_not_read_the_received_heartbeat_ledger() {
    let (owner, _, _, _) = local_app_with_memory_services();
    let visitor = generate_keys();
    let (context, dome) = hosted_dome(&owner, ChannelRef::Public, 8).await;

    let ledger = owner.dome_host_heartbeats.lock().await;
    timeout(Duration::from_secs(10), async {
        let keep_alive = DomeSessionInputKindV1::KeepAlive;
        for (sequence, input) in [(1, JOIN), (2, keep_alive)] {
            snapshot(
                serve(
                    &owner,
                    input_request(&owner, &visitor, &dome, sequence, input).await,
                )
                .await,
            );
        }
        owner_input(&owner, &context, &dome, 1, JOIN).await;
    })
    .await
    .expect("the inputs never wait for the heartbeat ledger");
    drop(ledger);
    owner.shutdown().await;
}

fn accepted(response: DomeSessionResponseV1) {
    assert!(
        matches!(response, DomeSessionResponseV1::Accepted),
        "expected acceptance, got {response:?}"
    );
}

// 遷移先が別の端末の所有者の端末で稼働中の Dome のとき、prepare / commit / abort が同じ経路で成り立つ(AC-2)。
// proof の束縛が違う prepare、visitor の block、owner 間の block の prepare は拒否される。
#[tokio::test]
async fn a_visitor_transitions_into_a_dome_hosted_on_another_device() {
    let docs = Arc::new(MemoryDocsSync::default());
    let blob = Arc::new(MemoryBlobService::default());
    let visitor_keys = generate_keys();
    let owner = app_with_shared_dome_services(docs.clone(), blob.clone(), generate_keys());
    let visitor = app_with_shared_dome_services(docs, blob, visitor_keys.clone());
    let topic = "kukuri:topic:dome-session-transition";
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    let room = |title: &str| CreateMetaverseRoomInput {
        title: title.into(),
        description: String::new(),
        max_peers: Some(8),
    };
    let target = owner
        .create_metaverse_room(topic, room("Target"))
        .await
        .expect("target Dome");
    let source = visitor
        .create_metaverse_room(topic, room("Source"))
        .await
        .expect("source Dome");
    show_domes(&owner, topic).await;
    owner
        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
            proposal_id: "transition".into(),
            spatial_context: context.clone(),
            proposer_instance_id: target.clone(),
            receiver_instance_id: source.clone(),
            proposer_direction: DomeDirection::East,
        })
        .await
        .expect("propose");
    visitor
        .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
            spatial_context: context.clone(),
            proposal_id: "transition".into(),
        })
        .await
        .expect("accept");
    for (app, dome) in [(&owner, &target), (&visitor, &source)] {
        app.start_owner_dome_hosting(StartOwnerDomeHostingInput {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: dome.clone(),
            endpoint_id: "endpoint".into(),
            lease_duration_millis: 60_000,
        })
        .await
        .expect("host");
    }
    owner_input(&visitor, &context, &source, 1, JOIN).await;
    let topology = owner
        .list_dome_connection_topology(context.clone())
        .await
        .expect("topology")
        .resolution
        .topology;
    let request =
        |transition_id: &str, participant: &KukuriKeys| DomeTransitionAdmissionRequestV1 {
            transition_id: transition_id.into(),
            connection_id: topology.active_connection_ids[0].clone(),
            topology_digest: topology.topology_digest.clone(),
            spatial_context: context.clone(),
            source_instance_id: source.clone(),
            source_instance_generation: 1,
            target_instance_id: target.clone(),
            target_instance_generation: 1,
            participant_pubkey: participant.public_key(),
            direction: DomeDirection::West,
            requested_at: Utc::now().timestamp_millis(),
        };
    let prepare = |request: DomeTransitionAdmissionRequestV1,
                   signer: &KukuriKeys,
                   proof_context: &SpatialContextV1,
                   proof_owner: &Pubkey| {
        let now = Utc::now().timestamp_millis();
        let access_proof = build_dome_spatial_access_proof(
            signer,
            proof_context.clone(),
            proof_owner.clone(),
            now,
            None,
            None,
        )
        .expect("access proof");
        let request = DomeSessionRequestV1::PrepareTransition {
            request,
            access_proof,
        };
        serde_json::to_vec(&request).expect("request")
    };
    let owner_pubkey = owner.services.keys.public_key();
    let reservations = || async {
        let sessions = owner.dome_host_sessions.lock().await;
        let runtime = sessions.get(&target).expect("target session");
        runtime.transition_reservation_count()
    };
    let ticket = |response: DomeSessionResponseV1| match response {
        DomeSessionResponseV1::Ticket { ticket } => *ticket,
        other => panic!("expected a ticket, got {other:?}"),
    };

    // proof の participant・Spatial Context・owner のどれかが要求と違えば、予約を作らずに拒否する。
    let impostor = generate_keys();
    let elsewhere = SpatialContextV1::Topic {
        topic_id: TopicId::new("kukuri:topic:elsewhere"),
    };
    let other_owner = generate_keys().public_key();
    for (signer, proof_context, proof_owner) in [
        (&impostor, &context, &owner_pubkey),
        (&visitor_keys, &elsewhere, &owner_pubkey),
        (&visitor_keys, &context, &other_owner),
    ] {
        let request = prepare(
            request("t0", &visitor_keys),
            signer,
            proof_context,
            proof_owner,
        );
        let message = rejection(serve(&owner, request).await);
        assert!(message.contains("bound to another"), "{message}");
    }
    assert_eq!(reservations().await, 0);

    let prepared = |id: &str| {
        prepare(
            request(id, &visitor_keys),
            &visitor_keys,
            &context,
            &owner_pubkey,
        )
    };
    // 予約・取消・確定は topology を組み立てず、受信した heartbeat の台帳も読まない(要求ごとの処理を件数に依存させない)。
    let ledger = owner.dome_host_heartbeats.lock().await;
    timeout(Duration::from_secs(10), async {
        let aborted = ticket(serve(&owner, prepared("t1")).await);
        assert_eq!(reservations().await, 1);
        let abort = DomeSessionRequestV1::AbortTransition { ticket: aborted };
        accepted(serve(&owner, serde_json::to_vec(&abort).expect("abort")).await);
        assert_eq!(reservations().await, 0, "the reservation is withdrawn");
        let commit = DomeSessionRequestV1::CommitTransition {
            ticket: ticket(serve(&owner, prepared("t2")).await),
            position: [-1_500, 90, 0],
            rotation: [0, 0, 0],
        };
        accepted(serve(&owner, serde_json::to_vec(&commit).expect("commit")).await);
    })
    .await
    .expect("the transition never builds the topology");
    drop(ledger);
    let arrived = owner_input(&owner, &context, &target, 1, JOIN).await;
    let avatar = format!("avatar:{}", visitor_keys.public_key().as_str());
    let body = arrived
        .bodies
        .iter()
        .find(|body| body.entity_id == avatar)
        .expect("the visitor appears in the owner's Dome");
    assert_eq!(body.position, [-1_500, 90, 0], "at the arrival position");

    // owner が block した visitor と、遷移元と遷移先の owner の間に block がある prepare は拒否され、予約を作らない。
    let blocked_visitor = generate_keys();
    let third = generate_keys();
    for (blocked, participant, code) in [
        (
            &blocked_visitor,
            &blocked_visitor,
            "DOME_TRANSITION_VISITOR_BLOCKED",
        ),
        (&visitor_keys, &third, "DOME_TRANSITION_OWNERS_BLOCKED"),
    ] {
        owner
            .block_author(blocked.public_key().as_str())
            .await
            .expect("block");
        let request = prepare(
            request("t3", participant),
            participant,
            &context,
            &owner_pubkey,
        );
        assert_eq!(rejection(serve(&owner, request).await), code);
    }
    assert_eq!(reservations().await, 0);
    owner.shutdown().await;
    visitor.shutdown().await;
}
