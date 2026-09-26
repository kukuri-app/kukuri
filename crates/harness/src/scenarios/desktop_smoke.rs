use crate::*;
use kukuri_core::MetaverseColliderV1;

pub(crate) async fn run_desktop_smoke_scenario(
    root: &Path,
    scenario: &ScenarioSpec,
    artifacts_dir: &Path,
) -> Result<HarnessResult> {
    let db_path = artifacts_dir.join("scenario.db");
    if db_path.exists() {
        std::fs::remove_file(&db_path)
            .with_context(|| format!("failed to remove stale db {}", db_path.display()))?;
    }

    let mut runtime = ScenarioRuntime {
        db_path,
        network: FakeNetwork::default(),
        app: None,
        current_topic: None,
        current_channel_id: None,
        private_channels: BTreeMap::new(),
        docs_sync: Arc::new(MemoryDocsSync::default()),
        blob_service: Arc::new(MemoryBlobService::default()),
        keys: generate_keys(),
        private_channel_capabilities: Arc::new(StdMutex::new(Vec::new())),
    };
    let overall_timeout = Duration::from_millis(scenario.timeouts.overall_ms);
    let step_timeout = Duration::from_millis(scenario.timeouts.step_ms);

    timeout(overall_timeout, async {
        let mut steps = Vec::new();

        for (step_index, step) in scenario.steps.iter().enumerate() {
            let started_at = Instant::now();
            let step_result = async {
                match step {
                ScenarioStep::LaunchDesktop => runtime.launch().await?,
                ScenarioStep::SelectTopic { topic } => {
                    runtime.current_topic = Some(topic.clone());
                    runtime.current_channel_id = None;
                    let _ = runtime
                        .app()?
                        .list_timeline_scoped(topic, TimelineScope::Public, None, 50)
                        .await?;
                }
                ScenarioStep::SelectPublicTimeline => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    runtime.current_channel_id = None;
                    let _ = runtime
                        .app()?
                        .list_timeline_scoped(&topic, TimelineScope::Public, None, 50)
                        .await?;
                }
                ScenarioStep::CreatePrivateChannel { label } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let channel = runtime
                        .app()?
                        .create_private_channel(CreatePrivateChannelInput {
                            topic_id: TopicId::new(topic.clone()),
                            label: label.clone(),
                            audience_kind: ChannelAudienceKind::InviteOnly,
                        })
                        .await?;
                    runtime
                        .private_channels
                        .insert(label.clone(), channel.channel_id.clone());
                }
                ScenarioStep::SelectPrivateChannel { label } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let channel_id = runtime
                        .private_channels
                        .get(label.as_str())
                        .cloned()
                        .with_context(|| format!("private channel not found for label: {label}"))?;
                    runtime.current_channel_id = Some(channel_id.clone());
                    let _ = runtime
                        .app()?
                        .list_timeline_scoped(
                            &topic,
                            TimelineScope::Channel {
                                channel_id: ChannelId::new(channel_id),
                            },
                            None,
                            50,
                        )
                        .await?;
                }
                ScenarioStep::CreatePost { content } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    match runtime.current_channel_id.clone() {
                        Some(channel_id) => {
                            runtime
                                .app()?
                                .create_post_in_channel(
                                    &topic,
                                    ChannelRef::PrivateChannel {
                                        channel_id: ChannelId::new(channel_id),
                                    },
                                    content,
                                    None,
                                )
                                .await?;
                        }
                        None => {
                            runtime.app()?.create_post(&topic, content, None).await?;
                        }
                    }
                }
                ScenarioStep::AssertTimelineContains { text } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let scope = runtime.current_scope();
                    let assertion = timeout(step_timeout, async {
                        loop {
                            let timeline = runtime
                                .app()?
                                .list_timeline_scoped(&topic, scope.clone(), None, 50)
                                .await?;
                            if timeline.items.iter().any(|item| item.content == *text) {
                                return Ok::<(), anyhow::Error>(());
                            }
                            sleep(Duration::from_millis(50)).await;
                        }
                    });
                    assertion.await.context("assertion timeout")??;
                }
                ScenarioStep::BookmarkPost { content } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let timeline = runtime
                        .app()?
                        .list_timeline_scoped(&topic, runtime.current_scope(), None, 50)
                        .await?;
                    let post = timeline
                        .items
                        .iter()
                        .find(|item| item.content == *content)
                        .with_context(|| format!("bookmark target not found in timeline: {content}"))?;
                    runtime
                        .app()?
                        .bookmark_post(&topic, post.object_id.as_str())
                        .await?;
                }
                ScenarioStep::AssertBookmarkListContains { text } => {
                    let expected = text.clone();
                    let assertion = timeout(step_timeout, async {
                        loop {
                            let bookmarks = runtime.bookmarks().await?;
                            if bookmarks
                                .iter()
                                .any(|item| item.post.content == expected)
                            {
                                return Ok::<(), anyhow::Error>(());
                            }
                            sleep(Duration::from_millis(50)).await;
                        }
                    });
                    assertion.await.context("assertion timeout")??;
                }
                ScenarioStep::AssertBookmarkListMissing { text } => {
                    let bookmarks = runtime.bookmarks().await?;
                    if bookmarks.iter().any(|item| item.post.content == *text) {
                        anyhow::bail!("bookmark still present: {text}");
                    }
                }
                ScenarioStep::RemoveBookmark { text } => {
                    let bookmarks = runtime.bookmarks().await?;
                    let bookmarked = bookmarks
                        .iter()
                        .find(|item| item.post.content == *text)
                        .with_context(|| format!("bookmarked post not found: {text}"))?;
                    runtime
                        .app()?
                        .remove_bookmarked_post(bookmarked.post.object_id.as_str())
                        .await?;
                }
                ScenarioStep::CreateLiveSession { title, description } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    runtime
                        .app()?
                        .create_live_session(
                            &topic,
                            CreateLiveSessionInput {
                                title: title.clone(),
                                description: description.clone(),
                            },
                        )
                        .await?;
                }
                ScenarioStep::JoinLiveSession { title } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let session = runtime
                        .app()?
                        .list_live_sessions(&topic)
                        .await?
                        .into_iter()
                        .find(|session| session.title == *title)
                        .with_context(|| format!("live session not found: {title}"))?;
                    runtime
                        .app()?
                        .join_live_session(&topic, session.session_id.as_str())
                        .await?;
                }
                ScenarioStep::AssertLiveViewerCount { title, viewer_count } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let expected = *viewer_count;
                    let target = title.clone();
                    let assertion = timeout(step_timeout, async {
                        loop {
                            let sessions = runtime.app()?.list_live_sessions(&topic).await?;
                            if sessions
                                .iter()
                                .any(|session| session.title == target && session.viewer_count == expected)
                            {
                                return Ok::<(), anyhow::Error>(());
                            }
                            sleep(Duration::from_millis(50)).await;
                        }
                    });
                    match assertion.await {
                        Ok(result) => result?,
                        Err(_) => {
                            let sessions = runtime.app()?.list_live_sessions(&topic).await?;
                            let observed = sessions
                                .iter()
                                .map(|session| {
                                    format!(
                                        "{}:{}:{}",
                                        session.title, session.viewer_count, session.joined_by_me
                                    )
                                })
                                .collect::<Vec<_>>();
                            anyhow::bail!(
                                "assertion timeout for live viewer count title={target} expected={expected} observed={observed:?}"
                            );
                        }
                    }
                }
                ScenarioStep::EndLiveSession { title } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let session = runtime
                        .app()?
                        .list_live_sessions(&topic)
                        .await?
                        .into_iter()
                        .find(|session| session.title == *title)
                        .with_context(|| format!("live session not found: {title}"))?;
                    runtime
                        .app()?
                        .end_live_session(&topic, session.session_id.as_str())
                        .await?;
                }
                ScenarioStep::CreateGameRoom {
                    title,
                    description,
                    participants,
                } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    runtime
                        .app()?
                        .create_game_room(
                            &topic,
                            CreateGameRoomInput {
                                title: title.clone(),
                                description: description.clone(),
                                participants: participants.clone(),
                            },
                        )
                        .await?;
                }
                ScenarioStep::UpdateGameRoom {
                    title,
                    status,
                    phase_label,
                    scores,
                } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let room = runtime
                        .app()?
                        .list_game_rooms(&topic)
                        .await?
                        .into_iter()
                        .find(|room| room.title == *title)
                        .with_context(|| format!("game room not found: {title}"))?;
                    let next_scores = room
                        .scores
                        .iter()
                        .map(|score| {
                            let next = scores
                                .iter()
                                .find(|update| update.label == score.label)
                                .map(|update| update.score)
                                .unwrap_or(score.score);
                            GameScoreView {
                                participant_id: score.participant_id.clone(),
                                label: score.label.clone(),
                                score: next,
                            }
                        })
                        .collect::<Vec<_>>();
                    runtime
                        .app()?
                        .update_game_room(
                            &topic,
                            room.room_id.as_str(),
                            UpdateGameRoomInput {
                                status: parse_game_status(status.as_str())?,
                                phase_label: phase_label.clone(),
                                scores: next_scores,
                            },
                        )
                        .await?;
                }
                ScenarioStep::AssertGameScore { title, label, score } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let expected_title = title.clone();
                    let expected_label = label.clone();
                    let expected_score = *score;
                    let assertion = timeout(step_timeout, async {
                        loop {
                            let rooms = runtime.app()?.list_game_rooms(&topic).await?;
                            if rooms.iter().any(|room| {
                                room.title == expected_title
                                    && room.scores.iter().any(|entry| {
                                        entry.label == expected_label && entry.score == expected_score
                                    })
                            }) {
                                return Ok::<(), anyhow::Error>(());
                            }
                            sleep(Duration::from_millis(50)).await;
                        }
                    });
                    assertion.await.context("assertion timeout")??;
                }
                ScenarioStep::DeleteMetaverseDome { title } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let room = runtime.app()?.list_game_rooms(&topic).await?.into_iter().find(|r| &r.title == title).context("Dome to delete not found")?;
                    let dome = room.metaverse.context("Dome state missing")?;
                    let deleted = runtime.app()?.delete_dome(kukuri_app_api::DeleteDomeInput {
                        spatial_context: dome.spatial_context, instance_id: dome.instance_id,
                        expected_generation: dome.instance_generation, operation_id: format!("scenario-delete-{}", dome.instance_generation),
                    }).await?;
                    anyhow::ensure!(deleted.deleted);
                    anyhow::ensure!(runtime.app()?.list_game_rooms(&topic).await?.iter().all(|r| &r.title != title));
                }
                ScenarioStep::CreateMetaverseDome {
                    title,
                    description,
                    max_peers,
                } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let input = CreateMetaverseRoomInput {
                        title: title.clone(),
                        description: description.clone(),
                        max_peers: *max_peers,
                    };
                    match runtime.current_channel_id.clone() {
                        Some(channel_id) => {
                            runtime
                                .app()?
                                .create_metaverse_room_in_channel(
                                    &topic,
                                    ChannelRef::PrivateChannel {
                                        channel_id: ChannelId::new(channel_id),
                                    },
                                    input,
                                )
                                .await?;
                        }
                        None => {
                            runtime.app()?.create_metaverse_room(&topic, input).await?;
                        }
                    }
                }
                ScenarioStep::CustomizeMetaverseDome {
                    title,
                    gravity_milli,
                    wall_material,
                    prop_position,
                } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let room = runtime
                        .app()?
                        .list_game_rooms_scoped(&topic, runtime.current_scope())
                        .await?
                        .into_iter()
                        .find(|room| room.title == *title)
                        .with_context(|| format!("metaverse Dome not found: {title}"))?;
                    let mut customization = room
                        .metaverse
                        .as_ref()
                        .context("metaverse Dome state missing")?
                        .dome
                        .customization
                        .clone();
                    customization.environment.gravity_milli = *gravity_milli;
                    customization.surface.wall_material = parse_dome_material(wall_material)?;
                    customization.persistent_props[0].position = *prop_position;
                    runtime
                        .app()?
                        .update_metaverse_room(
                            &topic,
                            room.room_id.as_str(),
                            UpdateMetaverseRoomInput {
                                status: GameRoomStatus::Running,
                                customization,
                            },
                        )
                        .await?;
                }
                ScenarioStep::AssertMetaverseDome {
                    title,
                    gravity_milli,
                    wall_material,
                    prop_position,
                } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let expected_material = parse_dome_material(wall_material)?;
                    let assertion = timeout(step_timeout, async {
                        loop {
                            let room = runtime
                                .app()?
                                .list_game_rooms_scoped(&topic, runtime.current_scope())
                                .await?
                                .into_iter()
                                .find(|room| room.title == *title)
                                .with_context(|| format!("metaverse Dome not found: {title}"))?;
                            let dome = &room
                                .metaverse
                                .as_ref()
                                .context("metaverse Dome state missing")?
                                .dome;
                            if dome.spec_id == "fixed_dome_v1"
                                && dome.customization.environment.gravity_milli == *gravity_milli
                                && dome.customization.surface.wall_material == expected_material
                                && dome.customization.persistent_props[0].position == *prop_position
                            {
                                return Ok::<(), anyhow::Error>(());
                            }
                            sleep(Duration::from_millis(50)).await;
                        }
                    });
                    assertion.await.context("metaverse Dome assertion timeout")??;
                }
                ScenarioStep::AssertMetaverseDomeRejectsInvalid { title } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let before = runtime
                        .app()?
                        .list_game_rooms_scoped(&topic, runtime.current_scope())
                        .await?
                        .into_iter()
                        .find(|room| room.title == *title)
                        .with_context(|| format!("metaverse Dome not found: {title}"))?;
                    let mut customization = before
                        .metaverse
                        .as_ref()
                        .context("metaverse Dome state missing")?
                        .dome
                        .customization
                        .clone();
                    customization.environment.gravity_milli = 999;
                    if runtime
                        .app()?
                        .update_metaverse_room(
                            &topic,
                            before.room_id.as_str(),
                            UpdateMetaverseRoomInput {
                                status: GameRoomStatus::Running,
                                customization,
                            },
                        )
                        .await
                        .is_ok()
                    {
                        anyhow::bail!("invalid metaverse Dome customization was accepted");
                    }
                    let after = runtime
                        .app()?
                        .list_game_rooms_scoped(&topic, runtime.current_scope())
                        .await?
                        .into_iter()
                        .find(|room| room.title == *title)
                        .context("metaverse Dome disappeared after invalid update")?;
                    let after_gravity = after
                        .metaverse
                        .as_ref()
                        .context("metaverse Dome state missing after invalid update")?
                        .dome
                        .customization
                        .environment
                        .gravity_milli;
                    if after_gravity == 999 {
                        anyhow::bail!("invalid metaverse Dome customization was persisted");
                    }
                }
                ScenarioStep::AssertMetaverseDomeCreateRejected { title } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let input = CreateMetaverseRoomInput {
                        title: title.clone(),
                        description: "duplicate owner slot".into(),
                        max_peers: Some(8),
                    };
                    let result = match runtime.current_channel_id.clone() {
                        Some(channel_id) => {
                            runtime
                                .app()?
                                .create_metaverse_room_in_channel(
                                    &topic,
                                    ChannelRef::PrivateChannel {
                                        channel_id: ChannelId::new(channel_id),
                                    },
                                    input,
                                )
                                .await
                        }
                        None => runtime.app()?.create_metaverse_room(&topic, input).await,
                    };
                    if result.is_ok() {
                        anyhow::bail!("duplicate owner Dome was accepted in current Context");
                    }
                }
                ScenarioStep::MoveMetaverseDome {
                    title,
                    move_id,
                    target_topic,
                    target_channel_label,
                } => {
                    let source_topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let room = runtime
                        .app()?
                        .list_game_rooms_scoped(&source_topic, runtime.current_scope())
                        .await?
                        .into_iter()
                        .find(|room| room.title == *title)
                        .with_context(|| format!("metaverse Dome not found: {title}"))?;
                    let target_context = match target_channel_label {
                        Some(label) => {
                            let channel_id = runtime
                                .private_channels
                                .get(label.as_str())
                                .cloned()
                                .with_context(|| {
                                    format!("private channel not found for label: {label}")
                                })?;
                            SpatialContextV1::Channel {
                                topic_id: TopicId::new(target_topic.clone()),
                                channel_id: ChannelId::new(channel_id),
                            }
                        }
                        None => SpatialContextV1::Topic {
                            topic_id: TopicId::new(target_topic.clone()),
                        },
                    };
                    runtime
                        .app()?
                        .move_dome(
                            &source_topic,
                            MoveDomeInput {
                                move_id: move_id.clone(),
                                source_instance_id: room.room_id,
                                target_context,
                            },
                        )
                        .await?;
                }
                ScenarioStep::ExerciseDomeEntry { local_title } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    if runtime.current_channel_id.is_some() {
                        anyhow::bail!("Dome entry smoke currently requires a topic Context");
                    }
                    let context = SpatialContextV1::Topic {
                        topic_id: TopicId::new(topic.clone()),
                    };
                    let room = runtime
                        .app()?
                        .list_game_rooms_scoped(&topic, TimelineScope::Public)
                        .await?
                        .into_iter()
                        .find(|room| room.title == *local_title)
                        .with_context(|| format!("entry Dome not found: {local_title}"))?;
                    let metaverse = room.metaverse.context("entry Dome metaverse state")?;
                    runtime
                        .app()?
                        .start_owner_dome_hosting(StartOwnerDomeHostingInput { expected_generation: None,
                            spatial_context: context.clone(),
                            instance_id: room.room_id.clone(),
                            endpoint_id: "harness-entry-owner".into(),
                            lease_duration_millis: 60_000,
                        })
                        .await?;
                    let first = runtime
                        .app()?
                        .submit_dome_session_input(SubmitDomeSessionInput {
                            expected_generation: None,
                            spatial_context: context.clone(),
                            instance_id: room.room_id.clone(),
                            sequence: 1,
                            input: DomeSessionInputKindV1::Join { avatar_collider: None },
                        })
                        .await?;
                    let avatar_id = format!("avatar:{}", runtime.keys.public_key().as_str());
                    let first_position = first
                        .snapshot
                        .bodies
                        .iter()
                        .find(|body| body.entity_id == avatar_id)
                        .context("authoritative entry avatar missing")?
                        .position;
                    anyhow::ensure!(first_position == metaverse.default_spawn.position);

                    runtime
                        .app()?
                        .submit_dome_session_input(SubmitDomeSessionInput {
                            expected_generation: None,
                            spatial_context: context.clone(),
                            instance_id: room.room_id.clone(),
                            sequence: 2,
                            input: DomeSessionInputKindV1::Leave,
                        })
                        .await?;
                    let evacuated = runtime
                        .app()?
                        .submit_dome_session_input(SubmitDomeSessionInput {
                            expected_generation: None,
                            spatial_context: context,
                            instance_id: room.room_id,
                            sequence: 3,
                            input: DomeSessionInputKindV1::Join {
                                avatar_collider: Some(MetaverseColliderV1::Capsule {
                                    center: [1_950, 90, 0],
                                    radius: 25,
                                    half_height: 65,
                                }),
                            },
                        })
                        .await?;
                    let evacuated_position = evacuated
                        .snapshot
                        .bodies
                        .iter()
                        .find(|body| body.entity_id == avatar_id)
                        .context("evacuated entry avatar missing")?
                        .position;
                    anyhow::ensure!(
                        evacuated_position != first_position,
                        "safe spawn did not evacuate: first={first_position:?}, evacuated={evacuated_position:?}"
                    );
                }
                ScenarioStep::ExerciseDomeConnections { local_title } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    if runtime.current_channel_id.is_some() {
                        anyhow::bail!("Dome Connection smoke currently requires a topic Context");
                    }
                    let context = SpatialContextV1::Topic {
                        topic_id: TopicId::new(topic.clone()),
                    };
                    let local_instance_id = runtime
                        .app()?
                        .list_game_rooms_scoped(&topic, TimelineScope::Public)
                        .await?
                        .into_iter()
                        .find(|room| room.title == *local_title)
                        .with_context(|| format!("local metaverse Dome not found: {local_title}"))?
                        .room_id;
                    let peer_b = dome_connection_peer(&runtime, "dome-peer-b").await?;
                    let peer_c = dome_connection_peer(&runtime, "dome-peer-c").await?;
                    let peer_b_instance_id = peer_b
                        .create_metaverse_room(
                            &topic,
                            CreateMetaverseRoomInput {
                                title: "peer-b-dome".into(),
                                description: "Dome Connection scenario peer B".into(),
                                max_peers: Some(8),
                            },
                        )
                        .await?;
                    let peer_c_instance_id = peer_c
                        .create_metaverse_room(
                            &topic,
                            CreateMetaverseRoomInput {
                                title: "peer-c-dome".into(),
                                description: "Dome Connection scenario peer C".into(),
                                max_peers: Some(8),
                            },
                        )
                        .await?;

                    runtime
                        .app()?
                        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
                            proposal_id: "scenario-a-b".into(),
                            spatial_context: context.clone(),
                            proposer_instance_id: local_instance_id.clone(),
                            receiver_instance_id: peer_b_instance_id.clone(),
                            proposer_direction: DomeDirection::East,
                        })
                        .await?;
                    peer_b
                        .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
                            spatial_context: context.clone(),
                            proposal_id: "scenario-a-b".into(),
                        })
                        .await?;
                    peer_c
                        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
                            proposal_id: "scenario-c-b".into(),
                            spatial_context: context.clone(),
                            proposer_instance_id: peer_c_instance_id.clone(),
                            receiver_instance_id: peer_b_instance_id,
                            proposer_direction: DomeDirection::South,
                        })
                        .await?;
                    peer_b
                        .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
                            spatial_context: context.clone(),
                            proposal_id: "scenario-c-b".into(),
                        })
                        .await?;

                    // A-C はすでに同じ component に属する。空いている slot でも cycle になるため、
                    // proposal は保持したまま activation だけを拒否する。
                    runtime
                        .app()?
                        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
                            proposal_id: "scenario-cycle".into(),
                            spatial_context: context.clone(),
                            proposer_instance_id: local_instance_id,
                            receiver_instance_id: peer_c_instance_id,
                            proposer_direction: DomeDirection::South,
                        })
                        .await?;
                    if peer_c
                        .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
                            spatial_context: context,
                            proposal_id: "scenario-cycle".into(),
                        })
                        .await
                        .is_ok()
                    {
                        anyhow::bail!("cycle-forming Dome Connection was accepted");
                    }
                }
                ScenarioStep::ExerciseDomeTransition { local_title } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    if runtime.current_channel_id.is_some() {
                        anyhow::bail!("Dome transition smoke currently requires a topic Context");
                    }
                    let context = SpatialContextV1::Topic {
                        topic_id: TopicId::new(topic.clone()),
                    };
                    let source = runtime
                        .app()?
                        .list_game_rooms_scoped(&topic, TimelineScope::Public)
                        .await?
                        .into_iter()
                        .find(|room| room.title == *local_title)
                        .with_context(|| format!("source metaverse Dome not found: {local_title}"))?;
                    let source_metaverse = source.metaverse.context("source metaverse state")?;
                    let target_host = dome_connection_peer(&runtime, "dome-transition-target").await?;
                    let target_instance_id = target_host
                        .create_metaverse_room(
                            &topic,
                            CreateMetaverseRoomInput {
                                title: "transition-target".into(),
                                description: "seamless transition target".into(),
                                max_peers: Some(8),
                            },
                        )
                        .await?;
                    runtime
                        .app()?
                        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
                            proposal_id: "scenario-transition".into(),
                            spatial_context: context.clone(),
                            proposer_instance_id: source.room_id.clone(),
                            receiver_instance_id: target_instance_id.clone(),
                            proposer_direction: DomeDirection::East,
                        })
                        .await?;
                    target_host
                        .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
                            spatial_context: context.clone(),
                            proposal_id: "scenario-transition".into(),
                        })
                        .await?;
                    runtime
                        .app()?
                        .start_owner_dome_hosting(StartOwnerDomeHostingInput { expected_generation: None,
                            spatial_context: context.clone(),
                            instance_id: source.room_id.clone(),
                            endpoint_id: "harness-transition-source".into(),
                            lease_duration_millis: 60_000,
                        })
                        .await?;
                    target_host
                        .start_owner_dome_hosting(StartOwnerDomeHostingInput { expected_generation: None,
                            spatial_context: context.clone(),
                            instance_id: target_instance_id.clone(),
                            endpoint_id: "harness-transition-target".into(),
                            lease_duration_millis: 60_000,
                        })
                        .await?;
                    runtime
                        .app()?
                        .submit_dome_session_input(SubmitDomeSessionInput {
                            expected_generation: None,
                            spatial_context: context.clone(),
                            instance_id: source.room_id.clone(),
                            sequence: 1,
                            input: DomeSessionInputKindV1::Join { avatar_collider: None },
                        })
                        .await?;
                    runtime
                        .app()?
                        .submit_dome_session_input(SubmitDomeSessionInput {
                            expected_generation: None,
                            spatial_context: context.clone(),
                            instance_id: source.room_id.clone(),
                            sequence: 2,
                            input: DomeSessionInputKindV1::PrepareTransition {
                                transition_id: "scenario-transition-handoff".into(),
                                direction: DomeDirection::East,
                            },
                        })
                        .await?;
                    let topology = runtime
                        .app()?
                        .list_dome_connection_topology(context.clone())
                        .await?;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_millis()
                        .try_into()?;
                    let ticket = target_host
                        .prepare_dome_transition(PrepareDomeTransitionInput {
                            request: DomeTransitionAdmissionRequestV1 {
                                transition_id: "scenario-transition-handoff".into(),
                                connection_id: "connection-scenario-transition".into(),
                                topology_digest: topology.resolution.topology.topology_digest,
                                spatial_context: context.clone(),
                                source_instance_id: source.room_id.clone(),
                                source_instance_generation: source_metaverse.instance_generation,
                                target_instance_id: target_instance_id.clone(),
                                target_instance_generation: 1,
                                participant_pubkey: runtime.keys.public_key(),
                                direction: DomeDirection::East,
                                requested_at: now,
                            },
                        })
                        .await?;
                    target_host
                        .commit_dome_transition(CommitDomeTransitionInput {
                            ticket,
                            position: [-2_830, 90, 0],
                            rotation: [0, 0, 0],
                        })
                        .await?;
                    runtime
                        .app()?
                        .submit_dome_session_input(SubmitDomeSessionInput {
                            expected_generation: None,
                            spatial_context: context.clone(),
                            instance_id: source.room_id.clone(),
                            sequence: 3,
                            input: DomeSessionInputKindV1::CompleteTransition {
                                transition_id: "scenario-transition-handoff".into(),
                            },
                        })
                        .await?;
                    let source_hosting = runtime
                        .app()?
                        .get_dome_hosting(context.clone(), &source.room_id)
                        .await?;
                    let target_hosting = target_host
                        .get_dome_hosting(context, &target_instance_id)
                        .await?;
                    anyhow::ensure!(source_hosting.participants == 0);
                    anyhow::ensure!(target_hosting.participants == 1);
                }
                ScenarioStep::AssertDomeConnectionTopology {
                    component_count,
                    active_connection_count,
                } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let context = match runtime.current_channel_id.clone() {
                        Some(channel_id) => SpatialContextV1::Channel {
                            topic_id: TopicId::new(topic),
                            channel_id: ChannelId::new(channel_id),
                        },
                        None => SpatialContextV1::Topic {
                            topic_id: TopicId::new(topic),
                        },
                    };
                    let topology = runtime.app()?.list_dome_connection_topology(context).await?;
                    if topology.resolution.topology.components.len() != *component_count
                        || topology.resolution.topology.active_connection_ids.len()
                            != *active_connection_count
                    {
                        anyhow::bail!(
                            "Dome Connection topology mismatch: expected components={} active={} observed components={} active={}",
                            component_count,
                            active_connection_count,
                            topology.resolution.topology.components.len(),
                            topology.resolution.topology.active_connection_ids.len()
                        );
                    }
                }
                ScenarioStep::RevokeLocalDomeConnection => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let context = match runtime.current_channel_id.clone() {
                        Some(channel_id) => SpatialContextV1::Channel {
                            topic_id: TopicId::new(topic),
                            channel_id: ChannelId::new(channel_id),
                        },
                        None => SpatialContextV1::Topic {
                            topic_id: TopicId::new(topic),
                        },
                    };
                    let rooms = runtime
                        .app()?
                        .list_game_rooms_scoped(context.topic_id().as_str(), runtime.current_scope())
                        .await?;
                    let local_pubkey = runtime.keys.public_key_hex();
                    let local_instance_id = rooms
                        .into_iter()
                        .find(|room| {
                            room.metaverse.is_some() && room.host_pubkey == local_pubkey
                        })
                        .context("local metaverse Dome not found")?
                        .room_id;
                    let topology = runtime
                        .app()?
                        .list_dome_connection_topology(context.clone())
                        .await?;
                    let connection_id = topology
                        .connections
                        .into_iter()
                        .find(|connection| {
                            connection.record.status == kukuri_core::DomeConnectionStatusV1::Active
                                && (connection.record.agreement.proposer.instance_id
                                    == local_instance_id
                                    || connection.record.agreement.receiver.instance_id
                                        == local_instance_id)
                        })
                        .context("active local Dome Connection not found")?
                        .record
                        .agreement
                        .connection_id;
                    runtime
                        .app()?
                        .revoke_dome_connection(RevokeDomeConnectionInput {
                            spatial_context: context,
                            connection_id,
                        })
                        .await?;
                }
                ScenarioStep::AssertMetaverseDomeMissing { title } => {
                    let topic = runtime.topic_or_default(&scenario.fixtures.topic);
                    let rooms = runtime
                        .app()?
                        .list_game_rooms_scoped(&topic, runtime.current_scope())
                        .await?;
                    if rooms.iter().any(|room| room.title == *title) {
                        anyhow::bail!("metaverse Dome is still visible: {title}");
                    }
                }
                ScenarioStep::RestartDesktop => {
                    runtime.app.take();
                    runtime.launch().await?;
                }
                    other => anyhow::bail!(
                        "unsupported step for desktop smoke scenario: {}",
                        step_name(other)
                    ),
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            step_result.with_context(|| {
                format!(
                    "scenario step {} ({}) failed: {step:?}",
                    step_index + 1,
                    step_name(step)
                )
            })?;

            steps.push(StepResult {
                action: step_name(step).to_string(),
                duration_ms: started_at.elapsed().as_millis(),
            });
        }

        let metrics_snapshot = if scenario.artifacts.metrics_snapshot {
            Some(runtime.app()?.get_sync_status().await?)
        } else {
            None
        };
        let result = HarnessResult {
            status: HarnessStatus::Pass,
            scenario: scenario.name.clone(),
            steps,
            artifacts: vec![artifacts_dir.join("result.json").display().to_string()],
            metrics_snapshot,
        };

        write_result_artifact(root, artifacts_dir, &result)?;
        Ok::<HarnessResult, anyhow::Error>(result)
    })
    .await
    .context("scenario exceeded overall timeout")?
}

async fn dome_connection_peer(runtime: &ScenarioRuntime, label: &str) -> Result<AppService> {
    let store = Arc::new(SqliteStore::connect_memory().await?);
    let transport = Arc::new(FakeTransport::new(label, runtime.network.clone()));
    Ok(AppService::from_handles(ServiceHandles::new(
        store.clone(),
        store,
        transport.clone(),
        transport,
        runtime.docs_sync.clone(),
        runtime.blob_service.clone(),
        generate_keys(),
    )))
}

fn parse_dome_material(value: &str) -> Result<DomeMaterialPreset> {
    match value {
        "concrete" => Ok(DomeMaterialPreset::Concrete),
        "stone" => Ok(DomeMaterialPreset::Stone),
        "metal" => Ok(DomeMaterialPreset::Metal),
        "wood" => Ok(DomeMaterialPreset::Wood),
        _ => anyhow::bail!("unsupported Dome material: {value}"),
    }
}
