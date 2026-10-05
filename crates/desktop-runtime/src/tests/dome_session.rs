//! #1527: 所有者の端末で稼働中の Dome へ、別の端末の参加者が P2P の session 経路で入る(ADR 0038)。

use super::*;
use kukuri_core::{
    DomeDirection, DomeHostingStateKindV1, DomePhysicsSnapshotV1, DomeSessionInputKindV1,
    DomeTransitionAdmissionRequestV1, Pubkey, SpatialContextV1, TopicId,
};
use kukuri_test_support::{PollError, PollState, poll_until};
use std::future::Future;

const TOPIC: &str = "kukuri:topic:dome-session";

async fn new_runtime(dir: &Path, name: &str) -> Result<DesktopRuntime> {
    DesktopRuntime::new_with_config_and_identity(
        &dir.join(format!("{name}.db")),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
}

fn has_avatar(snapshot: &DomePhysicsSnapshotV1, pubkey: &str) -> bool {
    let entity_id = format!("avatar:{pubkey}");
    snapshot
        .bodies
        .iter()
        .any(|body| body.entity_id == entity_id)
}

async fn submit(
    runtime: &DesktopRuntime,
    context: &SpatialContextV1,
    dome: &str,
    sequence: u64,
    input: DomeSessionInputKindV1,
) -> Result<DomePhysicsSnapshotV1> {
    runtime
        .submit_dome_session_input(SubmitDomeSessionInputRequest {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: dome.to_string(),
            sequence,
            input,
        })
        .await
}

/// 所有者の端末が Dome を稼働し、別の端末がその Dome を「所有者の端末で稼働中」と見るまで進める。
async fn host_dome_seen_by(
    owner: &DesktopRuntime,
    visitor: &DesktopRuntime,
) -> Result<(SpatialContextV1, String)> {
    refresh_runtime_peer_tickets(&[owner, visitor]).await?;
    open_topic_column(owner, TOPIC, TimelineScope::Public).await?;
    open_topic_column(visitor, TOPIC, TimelineScope::Public).await?;
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(TOPIC),
    };
    let dome = owner
        .create_metaverse_room(CreateMetaverseRoomRequest {
            topic: TOPIC.into(),
            channel_ref: ChannelRef::Public,
            title: "Dome".into(),
            description: String::new(),
            max_peers: None,
        })
        .await?;
    owner
        .start_owner_dome_hosting(StartOwnerDomeHostingRequest {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: dome.clone(),
            endpoint_id: owner.get_sync_status().await?.discovery.local_endpoint_id,
            lease_duration_millis: 600_000,
        })
        .await?;
    visitor
        .set_session_display(SessionDisplayRequest {
            topic: TOPIC.into(),
            scope: TimelineScope::Public,
            replica_id: String::new(),
            session_id: dome.clone(),
            kind: "game".into(),
            observer: format!("dome-session:{dome}"),
            visible: true,
            retry: false,
        })
        .await?;
    let seen = poll_until(
        Duration::from_secs(120),
        Duration::from_millis(250),
        1,
        || async {
            let listed = visitor
                .list_game_rooms(ListGameRoomsRequest {
                    topic: TOPIC.into(),
                    scope: TimelineScope::Public,
                })
                .await?
                .iter()
                .any(|room| room.room_id == dome);
            if !listed {
                return Ok::<_, anyhow::Error>(PollState::Pending);
            }
            let hosting = visitor
                .get_dome_hosting(GetDomeHostingRequest {
                    spatial_context: context.clone(),
                    instance_id: dome.clone(),
                })
                .await?;
            Ok(
                if hosting.state.kind == DomeHostingStateKindV1::OwnerHosted
                    && hosting.state.session_id.is_some()
                {
                    PollState::Ready(())
                } else {
                    PollState::Pending
                },
            )
        },
    )
    .await;
    match seen {
        Ok(()) => Ok((context, dome)),
        Err(PollError::Operation(error)) => Err(error),
        Err(PollError::Timeout) => bail!("the visitor never saw the owner-hosted Dome"),
    }
}

// 別の端末の参加者の入室・移動・退室が、所有者の端末の host へ届き、互いの avatar が snapshot に現れる(AC-1 の 1・2)。
// 修正前は、別の端末の入室が `the active Dome host is not reachable from this device` で失敗した。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_device_enters_moves_and_leaves_an_owner_device_hosted_dome() -> Result<()> {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir()?;
    let owner = new_runtime(dir.path(), "owner").await?;
    let visitor = new_runtime(dir.path(), "visitor").await?;
    let owner_pubkey = owner.get_sync_status().await?.local_author_pubkey;
    let visitor_pubkey = visitor.get_sync_status().await?.local_author_pubkey;
    let (context, dome) = host_dome_seen_by(&owner, &visitor).await?;

    let joined = submit(
        &visitor,
        &context,
        &dome,
        1,
        DomeSessionInputKindV1::Join {
            avatar_collider: None,
        },
    )
    .await?;
    assert!(
        has_avatar(&joined, &visitor_pubkey),
        "admission shows the visitor"
    );
    let owner_view = submit(
        &owner,
        &context,
        &dome,
        1,
        DomeSessionInputKindV1::Join {
            avatar_collider: None,
        },
    )
    .await?;
    assert!(
        has_avatar(&owner_view, &visitor_pubkey),
        "the owner sees the visitor"
    );
    assert!(has_avatar(&owner_view, &owner_pubkey));

    sleep(Duration::from_millis(150)).await;
    let moved = submit(
        &visitor,
        &context,
        &dome,
        2,
        DomeSessionInputKindV1::Move {
            position: [100, 0, 100],
            rotation: [0, 0, 0],
            animation: "walk".into(),
        },
    )
    .await?;
    assert!(
        has_avatar(&moved, &owner_pubkey),
        "the visitor sees the owner"
    );

    submit(&visitor, &context, &dome, 3, DomeSessionInputKindV1::Leave).await?;
    sleep(Duration::from_millis(150)).await;
    let after_leave = submit(
        &owner,
        &context,
        &dome,
        2,
        DomeSessionInputKindV1::KeepAlive,
    )
    .await?;
    assert!(
        !has_avatar(&after_leave, &visitor_pubkey),
        "the visitor left"
    );
    owner.shutdown().await;
    visitor.shutdown().await;
    Ok(())
}

// 所有者の端末の iroh stack を作り直しても受け口は付いたまま応答し(AC-1 の 6)、lease の endpoint に接続できない
// Dome への入室は `DOME_HOST_UNREACHABLE` で失敗する(AC-1 の 5。接続の打ち切りは 10 秒)。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owner_keeps_answering_after_a_stack_rebuild_and_an_unreachable_host_fails_fast()
-> Result<()> {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir()?;
    let owner = new_runtime(dir.path(), "owner").await?;
    let visitor = new_runtime(dir.path(), "visitor").await?;
    let (context, dome) = host_dome_seen_by(&owner, &visitor).await?;
    let join = || DomeSessionInputKindV1::Join {
        avatar_collider: None,
    };
    submit(&visitor, &context, &dome, 1, join()).await?;

    let discovery = owner.discovery_config.lock().await.clone();
    owner
        .iroh_stack
        .rebuild(
            &discovery,
            &[],
            kukuri_transport::TransportRelayConfig::default(),
        )
        .await?;
    refresh_runtime_peer_tickets(&[&owner, &visitor]).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut sequence = 2;
    loop {
        match submit(
            &visitor,
            &context,
            &dome,
            sequence,
            DomeSessionInputKindV1::KeepAlive,
        )
        .await
        {
            Ok(_) => break,
            Err(error) if tokio::time::Instant::now() >= deadline => {
                return Err(error.context("the rebuilt owner stack never answered"));
            }
            Err(_) => sleep(Duration::from_millis(500)).await,
        }
        sequence += 1;
    }

    // 所有者が、どの端末も応じない endpoint で稼働を始め直す(この経路に応じない端末と同じ)。
    let unreachable = iroh::SecretKey::from_bytes(&[7; 32]).public().to_string();
    owner
        .start_owner_dome_hosting(StartOwnerDomeHostingRequest {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: dome.clone(),
            endpoint_id: unreachable.clone(),
            lease_duration_millis: 600_000,
        })
        .await?;
    let seen = poll_until(Duration::from_secs(60), Duration::from_millis(250), 1, || async {
        let hosting = visitor
            .get_dome_hosting(GetDomeHostingRequest {
                spatial_context: context.clone(),
                instance_id: dome.clone(),
            })
            .await?;
        let moved = matches!(
            hosting.lease.as_ref().map(|lease| &lease.host),
            Some(kukuri_core::DomeHostTargetV1::OwnerDevice { endpoint_id, .. }) if *endpoint_id == unreachable
        );
        Ok::<_, anyhow::Error>(
            if moved && hosting.state.session_id.is_some() {
                PollState::Ready(())
            } else {
                PollState::Pending
            },
        )
    })
    .await;
    if seen.is_err() {
        bail!("the visitor never saw the new lease");
    }
    let started = tokio::time::Instant::now();
    let error = submit(&visitor, &context, &dome, sequence + 1, join())
        .await
        .expect_err("an unreachable host cannot admit the visitor");
    assert_eq!(
        error
            .downcast_ref::<crate::DomeHostingRequestError>()
            .map(|error| error.code.as_str()),
        Some("DOME_HOST_UNREACHABLE"),
        "{error:#}"
    );
    assert!(
        started.elapsed() <= Duration::from_secs(10),
        "the Join fails within 10 seconds (AC-1 5): {:?}",
        started.elapsed()
    );
    owner.shutdown().await;
    visitor.shutdown().await;
    Ok(())
}

/// 別の端末の変化が届くまで繰り返す。届く前の失敗と `None` は待ち、期限を過ぎたら最後の失敗を返す。
async fn eventually<T, Fut>(label: &str, attempt: impl Fn() -> Fut) -> Result<T>
where
    Fut: Future<Output = Result<Option<T>>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let last = match attempt().await {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => anyhow::anyhow!("not yet"),
            Err(error) => error,
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(last.context(format!("{label} never happened")));
        }
        sleep(Duration::from_millis(250)).await;
    }
}

// 隣の Dome を別の端末の所有者の端末が稼働しているとき、遷移の prepare / abort / commit が P2P の session 経路で
// 成り立ち、commit の後は遷移先の位置で所有者の Dome に現れる(AC-2)。修正前は prepare が
// `the active destination Dome host is not reachable from this device` で失敗した。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_device_transitions_into_an_owner_device_hosted_dome() -> Result<()> {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir()?;
    let owner = new_runtime(dir.path(), "owner").await?;
    let visitor = new_runtime(dir.path(), "visitor").await?;
    let visitor_pubkey = visitor.get_sync_status().await?.local_author_pubkey;
    // 移行の後の書き込みの形にする(接続の記録は Dome の anchor に置かれ、別の端末が読む。#1221 R5-H)。
    for runtime in [&owner, &visitor] {
        runtime.app_service.switch_writer(1);
    }
    let (context, target) = host_dome_seen_by(&owner, &visitor).await?;
    let source = visitor
        .create_metaverse_room(CreateMetaverseRoomRequest {
            topic: TOPIC.into(),
            channel_ref: ChannelRef::Public,
            title: "Source".into(),
            description: String::new(),
            max_peers: None,
        })
        .await?;
    visitor
        .start_owner_dome_hosting(StartOwnerDomeHostingRequest {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: source.clone(),
            endpoint_id: visitor.get_sync_status().await?.discovery.local_endpoint_id,
            lease_duration_millis: 600_000,
        })
        .await?;

    // 所有者が 2 つの Dome の接続を提案し、別の端末が topology の表示で提案を読んで受け入れる。
    let proposal = CreateDomeConnectionProposalRequest {
        proposal_id: "transition".into(),
        spatial_context: context.clone(),
        proposer_instance_id: target.clone(),
        receiver_instance_id: source.clone(),
        proposer_direction: DomeDirection::East,
    };
    eventually("the proposal", || async {
        owner
            .create_dome_connection_proposal(proposal.clone())
            .await
            .map(Some)
    })
    .await?;
    let shown = ListDomeConnectionTopologyRequest {
        spatial_context: context.clone(),
    };
    eventually("the proposal on the other device", || async {
        let topology = visitor.list_dome_connection_topology(shown.clone()).await?;
        let proposed = topology
            .proposals
            .iter()
            .any(|item| item.proposal.proposal_id == "transition");
        Ok(proposed.then_some(()))
    })
    .await?;
    visitor
        .accept_dome_connection_proposal(AcceptDomeConnectionProposalRequest {
            spatial_context: context.clone(),
            proposal_id: "transition".into(),
        })
        .await?;
    let topology = eventually("the shared topology", || async {
        let mine = visitor.list_dome_connection_topology(shown.clone()).await?;
        let theirs = owner.list_dome_connection_topology(shown.clone()).await?;
        let (mine, theirs) = (mine.resolution.topology, theirs.resolution.topology);
        let shared = mine.topology_digest == theirs.topology_digest
            && !mine.active_connection_ids.is_empty();
        Ok(shared.then_some(mine))
    })
    .await?;
    let join = DomeSessionInputKindV1::Join {
        avatar_collider: None,
    };
    submit(&visitor, &context, &source, 1, join.clone()).await?;

    let request = |transition_id: &str| PrepareDomeTransitionRequest {
        request: DomeTransitionAdmissionRequestV1 {
            transition_id: transition_id.into(),
            connection_id: topology.active_connection_ids[0].clone(),
            topology_digest: topology.topology_digest.clone(),
            spatial_context: context.clone(),
            source_instance_id: source.clone(),
            source_instance_generation: 1,
            target_instance_id: target.clone(),
            target_instance_generation: 1,
            participant_pubkey: Pubkey::from(visitor_pubkey.as_str()),
            direction: DomeDirection::West,
            requested_at: chrono::Utc::now().timestamp_millis(),
        },
    };
    let ticket = visitor.prepare_dome_transition(request("aborted")).await?;
    visitor
        .abort_dome_transition(AbortDomeTransitionRequest { ticket })
        .await?;
    let ticket = visitor
        .prepare_dome_transition(request("committed"))
        .await?;
    visitor
        .commit_dome_transition(CommitDomeTransitionRequest {
            ticket,
            position: [-1_500, 90, 0],
            rotation: [0, 0, 0],
        })
        .await?;
    let arrived = submit(&owner, &context, &target, 1, join).await?;
    let avatar = format!("avatar:{visitor_pubkey}");
    let body = arrived
        .bodies
        .iter()
        .find(|body| body.entity_id == avatar)
        .context("the visitor appears in the owner's Dome")?;
    assert_eq!(body.position, [-1_500, 90, 0], "at the arrival position");
    owner.shutdown().await;
    visitor.shutdown().await;
    Ok(())
}
