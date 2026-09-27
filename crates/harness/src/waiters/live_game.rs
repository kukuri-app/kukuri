use crate::*;

pub(crate) async fn wait_for_live_session(
    runtime: &DesktopRuntime,
    topic: &str,
    session_id: &str,
    step_timeout: Duration,
) -> Result<()> {
    let _ = wait_for_live_session_in_scope(
        runtime,
        topic,
        TimelineScope::Public,
        session_id,
        step_timeout,
    )
    .await?;
    Ok(())
}

pub(crate) async fn wait_for_live_session_in_scope(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: TimelineScope,
    session_id: &str,
    step_timeout: Duration,
) -> Result<kukuri_app_api::LiveSessionView> {
    runtime
        .set_session_display(kukuri_desktop_runtime::SessionDisplayRequest {
            topic: topic.into(),
            scope: scope.clone(),
            replica_id: String::new(),
            session_id: session_id.into(),
            kind: "live".into(),
            observer: format!("harness-{session_id}"),
            visible: true,
            retry: false,
        })
        .await?;
    timeout(step_timeout, async {
        loop {
            let sessions = runtime
                .list_live_sessions(ListLiveSessionsRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                })
                .await?;
            if let Some(session) = sessions
                .into_iter()
                .find(|session| session.session_id == session_id)
            {
                return Ok::<kukuri_app_api::LiveSessionView, anyhow::Error>(session);
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("live-session assertion timeout")?
}

pub(crate) async fn wait_for_live_viewer_count(
    runtime: &DesktopRuntime,
    topic: &str,
    session_id: &str,
    expected: usize,
    step_timeout: Duration,
) -> Result<()> {
    wait_for_live_viewer_count_in_scope(
        runtime,
        topic,
        TimelineScope::Public,
        session_id,
        expected,
        step_timeout,
    )
    .await
}

pub(crate) async fn wait_for_live_viewer_count_in_scope(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: TimelineScope,
    session_id: &str,
    expected: usize,
    step_timeout: Duration,
) -> Result<()> {
    timeout(step_timeout, async {
        loop {
            let sessions = runtime
                .list_live_sessions(ListLiveSessionsRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                })
                .await?;
            if sessions
                .iter()
                .any(|session| session.session_id == session_id && session.viewer_count == expected)
            {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("live-session viewer assertion timeout")?
}

pub(crate) async fn wait_for_live_ended(
    runtime: &DesktopRuntime,
    topic: &str,
    session_id: &str,
    step_timeout: Duration,
) -> Result<()> {
    wait_for_live_ended_in_scope(
        runtime,
        topic,
        TimelineScope::Public,
        session_id,
        step_timeout,
    )
    .await
}

pub(crate) async fn wait_for_live_ended_in_scope(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: TimelineScope,
    session_id: &str,
    step_timeout: Duration,
) -> Result<()> {
    timeout(step_timeout, async {
        loop {
            let sessions = runtime
                .list_live_sessions(ListLiveSessionsRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                })
                .await?;
            if sessions.iter().any(|session| {
                session.session_id == session_id
                    && session.status == kukuri_core::LiveSessionStatus::Ended
            }) {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("live-session ended assertion timeout")?
}

pub(crate) async fn assert_live_session_absent_in_scope(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: TimelineScope,
    session_id: &str,
    duration: Duration,
) -> Result<()> {
    let result = timeout(duration, async {
        loop {
            let sessions = runtime
                .list_live_sessions(ListLiveSessionsRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                })
                .await?;
            if sessions
                .iter()
                .any(|session| session.session_id == session_id)
            {
                anyhow::bail!("live session leaked into filtered scope");
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    match result {
        Err(_) => Ok(()),
        Ok(inner) => inner,
    }
}

pub(crate) async fn wait_for_game_room(
    runtime: &DesktopRuntime,
    topic: &str,
    room_id: &str,
    step_timeout: Duration,
) -> Result<kukuri_app_api::GameRoomView> {
    wait_for_game_room_in_scope(runtime, topic, TimelineScope::Public, room_id, step_timeout).await
}

pub(crate) async fn wait_for_game_room_in_scope(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: TimelineScope,
    room_id: &str,
    step_timeout: Duration,
) -> Result<kukuri_app_api::GameRoomView> {
    runtime
        .set_session_display(kukuri_desktop_runtime::SessionDisplayRequest {
            topic: topic.into(),
            scope: scope.clone(),
            replica_id: String::new(),
            session_id: room_id.into(),
            kind: "game".into(),
            observer: format!("harness-{room_id}"),
            visible: true,
            retry: false,
        })
        .await?;
    timeout(step_timeout, async {
        loop {
            let rooms = runtime
                .list_game_rooms(ListGameRoomsRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                })
                .await?;
            if let Some(room) = rooms.into_iter().find(|room| room.room_id == room_id) {
                return Ok::<kukuri_app_api::GameRoomView, anyhow::Error>(room);
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("game-room assertion timeout")?
}

pub(crate) async fn wait_for_game_score(
    runtime: &DesktopRuntime,
    topic: &str,
    room_id: &str,
    label: &str,
    expected: i64,
    step_timeout: Duration,
) -> Result<()> {
    wait_for_game_score_in_scope(
        runtime,
        topic,
        TimelineScope::Public,
        room_id,
        label,
        expected,
        step_timeout,
    )
    .await
}

pub(crate) async fn wait_for_game_score_in_scope(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: TimelineScope,
    room_id: &str,
    label: &str,
    expected: i64,
    step_timeout: Duration,
) -> Result<()> {
    timeout(step_timeout, async {
        loop {
            let rooms = runtime
                .list_game_rooms(ListGameRoomsRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                })
                .await?;
            if rooms.iter().any(|room| {
                room.room_id == room_id
                    && room
                        .scores
                        .iter()
                        .any(|score| score.label == label && score.score == expected)
            }) {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("game-score assertion timeout")?
}

pub(crate) async fn assert_game_room_absent_in_scope(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: TimelineScope,
    room_id: &str,
    duration: Duration,
) -> Result<()> {
    let result = timeout(duration, async {
        loop {
            let rooms = runtime
                .list_game_rooms(ListGameRoomsRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                })
                .await?;
            if rooms.iter().any(|room| room.room_id == room_id) {
                anyhow::bail!("game room leaked into filtered scope");
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    match result {
        Err(_) => Ok(()),
        Ok(inner) => inner,
    }
}

/// 提案する側は、相手の Dome を開いて一覧で知ってから提案する(#1221 R5-H: context の replica を走査しない)。
pub(crate) async fn wait_for_listed_domes(
    app: &AppService,
    topic: &str,
    instance_ids: &[&str],
    wait: Duration,
) -> Result<()> {
    for id in instance_ids {
        app.set_session_display(kukuri_app_api::SessionDisplayRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            replica_id: String::new(),
            session_id: (*id).into(),
            kind: "game".into(),
            observer: format!("harness-dome:{id}"),
            visible: true,
            retry: false,
        })
        .await?;
    }
    tokio::time::timeout(wait, async {
        loop {
            let rooms = app
                .list_game_rooms_scoped(topic, TimelineScope::Public)
                .await?;
            if instance_ids
                .iter()
                .all(|id| rooms.iter().any(|room| room.room_id == *id))
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("peer Domes were not listed")?
}
