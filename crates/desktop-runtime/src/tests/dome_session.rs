//! #1527: 所有者の端末で稼働中の Dome へ、別の端末の参加者が P2P の session 経路で入る(ADR 0038)。

use super::*;
use kukuri_core::{
    DomeHostingStateKindV1, DomePhysicsSnapshotV1, DomeSessionInputKindV1, SpatialContextV1,
    TopicId,
};
use kukuri_test_support::{PollError, PollState, poll_until};

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

// 所有者の端末の iroh stack を作り直しても受け口は付いたまま応答し(AC-1 の 6)、所有者の端末が止まると入室は
// `DOME_HOST_UNREACHABLE` で失敗する(AC-1 の 5。接続の打ち切りは 10 秒)。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owner_keeps_answering_after_a_stack_rebuild_and_a_stopped_owner_is_unreachable()
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
        .rebuild(&discovery, &[], kukuri_transport::TransportRelayConfig::default())
        .await?;
    refresh_runtime_peer_tickets(&[&owner, &visitor]).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut sequence = 2;
    loop {
        match submit(&visitor, &context, &dome, sequence, DomeSessionInputKindV1::KeepAlive).await {
            Ok(_) => break,
            Err(error) if tokio::time::Instant::now() >= deadline => {
                return Err(error.context("the rebuilt owner stack never answered"));
            }
            Err(_) => sleep(Duration::from_millis(500)).await,
        }
        sequence += 1;
    }

    owner.shutdown().await;
    let started = tokio::time::Instant::now();
    let error = submit(&visitor, &context, &dome, sequence + 1, join())
        .await
        .expect_err("the stopped owner cannot admit the visitor");
    assert_eq!(
        error
            .downcast_ref::<crate::DomeHostingRequestError>()
            .map(|error| error.code.as_str()),
        Some("DOME_HOST_UNREACHABLE"),
        "{error:#}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the failure is bounded by the connect timeout: {:?}",
        started.elapsed()
    );
    visitor.shutdown().await;
    Ok(())
}
