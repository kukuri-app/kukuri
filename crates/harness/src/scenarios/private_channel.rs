use crate::*;

pub(crate) async fn run_private_channel_invite_connectivity(
    scenario: &ScenarioSpec,
    artifacts_dir: &Path,
) -> Result<HarnessResult> {
    unsafe { std::env::set_var("KUKURI_DISABLE_KEYRING", "1") };

    let db_a = artifacts_dir.join("private-channel-a.db");
    let db_b = artifacts_dir.join("private-channel-b.db");
    let db_c = artifacts_dir.join("private-channel-c.db");
    cleanup_runtime_artifacts(&db_a)?;
    cleanup_runtime_artifacts(&db_b)?;
    cleanup_runtime_artifacts(&db_c)?;

    let runtime_a = DesktopRuntime::new_with_config(&db_a, TransportNetworkConfig::loopback())
        .await
        .context("failed to launch desktop a for private-channel scenario")?;
    let runtime_b = DesktopRuntime::new_with_config(&db_b, TransportNetworkConfig::loopback())
        .await
        .context("failed to launch desktop b for private-channel scenario")?;
    let runtime_c = DesktopRuntime::new_with_config(&db_c, TransportNetworkConfig::loopback())
        .await
        .context("failed to launch desktop c for private-channel scenario")?;
    let overall_timeout =
        if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
            Duration::from_millis(scenario.timeouts.overall_ms).max(Duration::from_secs(600))
        } else {
            Duration::from_millis(scenario.timeouts.overall_ms)
        };
    let step_timeout =
        if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
            Duration::from_millis(scenario.timeouts.step_ms).max(Duration::from_secs(180))
        } else {
            Duration::from_millis(scenario.timeouts.step_ms)
        };

    timeout(overall_timeout, async move {
        let mut steps = Vec::new();
        let topic = scenario.fixtures.topic.as_str();
        let mut runtime_b = runtime_b;

        let started_at = Instant::now();
        let ticket_a = runtime_a
            .local_peer_ticket()
            .await
            .context("failed to export ticket for desktop a")?
            .context("missing ticket for desktop a")?;
        let ticket_b = runtime_b
            .local_peer_ticket()
            .await
            .context("failed to export ticket for desktop b")?
            .context("missing ticket for desktop b")?;
        let ticket_c = runtime_c
            .local_peer_ticket()
            .await
            .context("failed to export ticket for desktop c")?
            .context("missing ticket for desktop c")?;
        runtime_a
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_b.clone(),
            })
            .await
            .context("failed to import desktop b ticket into desktop a")?;
        runtime_b
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_a.clone(),
            })
            .await
            .context("failed to import desktop a ticket into desktop b")?;
        push_named_step(&mut steps, "connect", started_at);

        let started_at = Instant::now();
        let public_scope = TimelineScope::Public;
        // 参加していない利用者(desktop c)は public の scope で確かめる。複数の channel をまたぐ scope は、
        // API から閉じた(#1280)。参加していない利用者にとっては、どちらも public だけを指す。
        let outsider_scope = TimelineScope::Public;
        let _ = runtime_a
            .list_timeline(ListTimelineRequest {
                topic: topic.to_string(),
                scope: public_scope.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("failed to subscribe desktop a to public topic")?;
        let _ = runtime_b
            .list_timeline(ListTimelineRequest {
                topic: topic.to_string(),
                scope: public_scope.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("failed to subscribe desktop b to public topic")?;
        runtime_a
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_b.clone(),
            })
            .await
            .context("failed to rebuild desktop b ticket into desktop a after subscribe")?;
        runtime_b
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_a.clone(),
            })
            .await
            .context("failed to rebuild desktop a ticket into desktop b after subscribe")?;
        let public_sync_attempts =
            if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
                3
            } else {
                1
            };
        let public_sync_timeout = if public_sync_attempts > 1 {
            Duration::from_millis(
                (step_timeout.as_millis() / public_sync_attempts as u128)
                    .max(1)
                    .try_into()
                    .expect("public sync timeout fits in u64"),
            )
        } else {
            step_timeout
        };
        let mut public_sync_error = None;
        for attempt in 1..=public_sync_attempts {
            let attempt_result = async {
                wait_for_topic_peer_count(&runtime_a, topic, 1, public_sync_timeout)
                    .await
                    .context("desktop a did not observe public topic connectivity")?;
                wait_for_topic_peer_count(&runtime_b, topic, 1, public_sync_timeout)
                    .await
                    .context("desktop b did not observe public topic connectivity")
            }
            .await;
            match attempt_result {
                Ok(()) => {
                    public_sync_error = None;
                    break;
                }
                Err(error) if attempt < public_sync_attempts => {
                    public_sync_error = Some(format!("{error:#}"));
                    runtime_a
                        .import_peer_ticket(ImportPeerTicketRequest {
                            ticket: ticket_b.clone(),
                        })
                        .await
                        .context("failed to refresh desktop b ticket into desktop a after public sync timeout")?;
                    runtime_b
                        .import_peer_ticket(ImportPeerTicketRequest {
                            ticket: ticket_a.clone(),
                        })
                        .await
                        .context("failed to refresh desktop a ticket into desktop b after public sync timeout")?;
                    let _ = runtime_a
                        .list_timeline(ListTimelineRequest {
                            topic: topic.to_string(),
                            scope: public_scope.clone(),
                            cursor: None,
                            limit: Some(20),
                        })
                        .await;
                    let _ = runtime_b
                        .list_timeline(ListTimelineRequest {
                            topic: topic.to_string(),
                            scope: public_scope.clone(),
                            cursor: None,
                            limit: Some(20),
                        })
                        .await;
                    sleep(Duration::from_millis(250)).await;
                }
                Err(error) => {
                    public_sync_error = Some(format!("{error:#}"));
                    break;
                }
            }
        }
        if let Some(error) = public_sync_error {
            let status_a = runtime_a
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, topic))
                .unwrap_or_else(|| "failed to read desktop a sync status".to_string());
            let status_b = runtime_b
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, topic))
                .unwrap_or_else(|| "failed to read desktop b sync status".to_string());
            anyhow::bail!(
                "{error}; desktop_a=({status_a}); desktop_b=({status_b})"
            );
        }
        push_named_step(&mut steps, "public_sync", started_at);

        let started_at = Instant::now();
        let channel = runtime_a
            .create_private_channel(CreatePrivateChannelRequest {
                topic: topic.to_string(),
                label: "core".to_string(),
                audience_kind: kukuri_core::ChannelAudienceKind::InviteOnly,
            })
            .await
            .context("failed to create private channel")?;
        push_named_step(&mut steps, "create_channel", started_at);

        let started_at = Instant::now();
        let invite = runtime_a
            .export_private_channel_invite(ExportPrivateChannelInviteRequest {
                topic: topic.to_string(),
                channel_id: channel.channel_id.clone(),
                expires_at: None,
            })
            .await
            .context("failed to export private channel invite")?;
        push_named_step(&mut steps, "create_invite", started_at);

        let started_at = Instant::now();
        let preview = runtime_b
            .import_private_channel_invite(ImportPrivateChannelInviteRequest { token: invite })
            .await
            .context("failed to import private channel invite")?;
        assert_eq!(preview.topic_id.as_str(), topic);
        assert_eq!(preview.channel_id.as_str(), channel.channel_id);
        wait_for_joined_private_channel(
            &runtime_b,
            topic,
            channel.channel_id.as_str(),
            step_timeout,
        )
        .await
        .context("desktop b did not join private channel after invite import")?;
        let joined_channels = runtime_b
            .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                topic: topic.to_string(),
            })
            .await
            .context("failed to list joined private channels after invite import")?;
        assert!(
            joined_channels
                .iter()
                .any(|entry| entry.channel_id == channel.channel_id && entry.label == "core")
        );
        push_named_step(&mut steps, "import_invite", started_at);

        let private_channel_id = kukuri_core::ChannelId::new(channel.channel_id.clone());
        let private_scope = TimelineScope::Channel {
            channel_id: private_channel_id.clone(),
        };
        let private_ref = ChannelRef::PrivateChannel {
            channel_id: private_channel_id.clone(),
        };
        let _ = runtime_a
            .list_timeline(ListTimelineRequest {
                topic: topic.to_string(),
                scope: private_scope.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("failed to subscribe desktop a to private channel")?;
        let _ = runtime_b
            .list_timeline(ListTimelineRequest {
                topic: topic.to_string(),
                scope: private_scope.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("failed to subscribe desktop b to private channel")?;
        runtime_a
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_b.clone(),
            })
            .await
            .context("failed to refresh desktop b ticket into desktop a for private channel")?;
        runtime_b
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_a.clone(),
            })
            .await
            .context("failed to refresh desktop a ticket into desktop b for private channel")?;
        let started_at = Instant::now();
        let private_post_id = runtime_b
            .create_post(CreatePostRequest {
                topic: topic.to_string(),
                content: "private post".to_string(),
                reply_to: None,
                channel_ref: private_ref.clone(),
                attachments: Vec::new(),
                content_labels: Vec::new(),
            })
            .await
            .context("failed to create private post")?;
        let private_post_attempts =
            if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
                3
            } else {
                1
            };
        let private_post_timeout = if private_post_attempts > 1 {
            Duration::from_millis(
                (step_timeout.as_millis() / private_post_attempts as u128)
                    .max(1)
                    .try_into()
                    .expect("private post timeout fits in u64"),
            )
        } else {
            step_timeout
        };
        let mut private_post_error = None;
        for attempt in 1..=private_post_attempts {
            match wait_for_timeline_object_in_scope(
                &runtime_a,
                topic,
                private_scope.clone(),
                private_post_id.as_str(),
                private_post_timeout,
            )
            .await
            {
                Ok(_) => {
                    private_post_error = None;
                    break;
                }
                Err(error) if attempt < private_post_attempts => {
                    private_post_error = Some(format!("{error:#}"));
                    runtime_a
                        .import_peer_ticket(ImportPeerTicketRequest {
                            ticket: ticket_b.clone(),
                        })
                        .await
                        .context("failed to refresh desktop b ticket into desktop a after private post timeout")?;
                    runtime_b
                        .import_peer_ticket(ImportPeerTicketRequest {
                            ticket: ticket_a.clone(),
                        })
                        .await
                        .context("failed to refresh desktop a ticket into desktop b after private post timeout")?;
                    let _ = runtime_a
                        .list_timeline(ListTimelineRequest {
                            topic: topic.to_string(),
                            scope: private_scope.clone(),
                            cursor: None,
                            limit: Some(20),
                        })
                        .await;
                    let _ = runtime_b
                        .list_timeline(ListTimelineRequest {
                            topic: topic.to_string(),
                            scope: private_scope.clone(),
                            cursor: None,
                            limit: Some(20),
                        })
                        .await;
                    sleep(Duration::from_millis(250)).await;
                }
                Err(error) => {
                    private_post_error = Some(format!("{error:#}"));
                    break;
                }
            }
        }
        if let Some(error) = private_post_error {
            let status_a = runtime_a
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, topic))
                .unwrap_or_else(|| "failed to read desktop a sync status".to_string());
            let status_b = runtime_b
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, topic))
                .unwrap_or_else(|| "failed to read desktop b sync status".to_string());
            let joined_a = runtime_a
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.to_string(),
                })
                .await
                .unwrap_or_default();
            let joined_b = runtime_b
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.to_string(),
                })
                .await
                .unwrap_or_default();
            return Err(anyhow::anyhow!(error).context(format!(
                "desktop a did not receive private post; desktop_a=({status_a}); desktop_b=({status_b}); joined_a={joined_a:?}; joined_b={joined_b:?}"
            )));
        }
        assert_timeline_scope_excludes_object(
            &runtime_b,
            topic,
            public_scope.clone(),
            private_post_id.as_str(),
            Duration::from_millis(500),
        )
        .await
        .context("desktop b public scope leaked private post")?;
        push_named_step(&mut steps, "private_post", started_at);

        let started_at = Instant::now();
        let private_reply_id = runtime_b
            .create_post(CreatePostRequest {
                topic: topic.to_string(),
                content: "private reply".to_string(),
                reply_to: Some(private_post_id.clone()),
                channel_ref: ChannelRef::Public,
                attachments: Vec::new(),
                content_labels: Vec::new(),
            })
            .await
            .context("failed to create private reply")?;
        let private_reply_attempts =
            if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
                3
            } else {
                1
            };
        let private_reply_timeout = if private_reply_attempts > 1 {
            Duration::from_millis(
                (step_timeout.as_millis() / private_reply_attempts as u128)
                    .max(1)
                    .try_into()
                    .expect("private reply timeout fits in u64"),
            )
        } else {
            step_timeout
        };
        let mut private_reply_error = None;
        for attempt in 1..=private_reply_attempts {
            match wait_for_thread_object(
                &runtime_a,
                topic,
                private_post_id.as_str(),
                private_reply_id.as_str(),
                private_reply_timeout,
            )
            .await
            {
                Ok(_) => {
                    private_reply_error = None;
                    break;
                }
                Err(error) if attempt < private_reply_attempts => {
                    private_reply_error = Some(format!("{error:#}"));
                    runtime_a
                        .import_peer_ticket(ImportPeerTicketRequest {
                            ticket: ticket_b.clone(),
                        })
                        .await
                        .context("failed to refresh desktop b ticket into desktop a after private reply timeout")?;
                    runtime_b
                        .import_peer_ticket(ImportPeerTicketRequest {
                            ticket: ticket_a.clone(),
                        })
                        .await
                        .context("failed to refresh desktop a ticket into desktop b after private reply timeout")?;
                    let _ = runtime_a
                        .list_timeline(ListTimelineRequest {
                            topic: topic.to_string(),
                            scope: private_scope.clone(),
                            cursor: None,
                            limit: Some(20),
                        })
                        .await;
                    let _ = runtime_b
                        .list_timeline(ListTimelineRequest {
                            topic: topic.to_string(),
                            scope: private_scope.clone(),
                            cursor: None,
                            limit: Some(20),
                        })
                        .await;
                    let _ = runtime_a
                        .list_thread(ListThreadRequest {
                            topic: topic.to_string(),
                            thread_id: private_post_id.clone(),
                            cursor: None,
                            limit: Some(20),
                        })
                        .await;
                    sleep(Duration::from_millis(250)).await;
                }
                Err(error) => {
                    private_reply_error = Some(format!("{error:#}"));
                    break;
                }
            }
        }
        if let Some(error) = private_reply_error {
            anyhow::bail!("desktop a did not receive private reply in thread: {error}");
        }
        let private_thread = runtime_a
            .list_thread(ListThreadRequest {
                topic: topic.to_string(),
                thread_id: private_post_id.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("failed to read private thread on desktop a")?;
        assert!(private_thread.items.iter().any(|post| {
            post.object_id == private_reply_id
                && post.channel_id.as_deref() == Some(channel.channel_id.as_str())
        }));
        push_named_step(&mut steps, "private_reply_thread", started_at);

        let (private_replication_attempts, private_replication_timeout) =
            private_replication_retry_schedule(step_timeout);

        let started_at = Instant::now();
        let session_id = runtime_b
            .create_live_session(CreateLiveSessionRequest {
                topic: topic.to_string(),
                channel_ref: private_ref.clone(),
                title: "private live".to_string(),
                description: "core stream".to_string(),
            })
            .await
            .context("failed to create private live session")?;
        let mut live_session_error = None;
        for attempt in 1..=private_replication_attempts {
            match wait_for_live_session_in_scope(
                &runtime_a,
                topic,
                private_scope.clone(),
                session_id.as_str(),
                private_replication_timeout,
            )
            .await
            {
                Ok(_) => {
                    live_session_error = None;
                    break;
                }
                Err(error) if attempt < private_replication_attempts => {
                    live_session_error = Some(format!("{error:#}"));
                    refresh_private_channel_pair(
                        &runtime_a,
                        &runtime_b,
                        ticket_a.as_str(),
                        ticket_b.as_str(),
                        topic,
                        &private_scope,
                    )
                    .await
                    .context("failed to refresh private channel after live-session timeout")?;
                    sleep(Duration::from_millis(250)).await;
                }
                Err(error) => {
                    live_session_error = Some(format!("{error:#}"));
                    break;
                }
            }
        }
        if let Some(error) = live_session_error {
            anyhow::bail!("desktop a did not receive private live session: {error}");
        }
        runtime_b
            .end_live_session(LiveSessionCommandRequest {
                topic: topic.to_string(),
                session_id: session_id.clone(),
            })
            .await
            .context("failed to end private live session on desktop b")?;
        let mut live_ended_error = None;
        for attempt in 1..=private_replication_attempts {
            match wait_for_live_ended_in_scope(
                &runtime_a,
                topic,
                private_scope.clone(),
                session_id.as_str(),
                private_replication_timeout,
            )
            .await
            {
                Ok(_) => {
                    live_ended_error = None;
                    break;
                }
                Err(error) if attempt < private_replication_attempts => {
                    live_ended_error = Some(format!("{error:#}"));
                    refresh_private_channel_pair(
                        &runtime_a,
                        &runtime_b,
                        ticket_a.as_str(),
                        ticket_b.as_str(),
                        topic,
                        &private_scope,
                    )
                    .await
                    .context("failed to refresh private channel after live-ended timeout")?;
                    sleep(Duration::from_millis(250)).await;
                }
                Err(error) => {
                    live_ended_error = Some(format!("{error:#}"));
                    break;
                }
            }
        }
        if let Some(error) = live_ended_error {
            anyhow::bail!("desktop a did not observe ended private live session: {error}");
        }
        push_named_step(&mut steps, "private_live", started_at);

        let started_at = Instant::now();
        let room_id = runtime_b
            .create_game_room(CreateGameRoomRequest {
                topic: topic.to_string(),
                channel_ref: private_ref.clone(),
                title: "private finals".to_string(),
                description: "core bracket".to_string(),
                participants: vec!["Alice".to_string(), "Bob".to_string()],
            })
            .await
            .context("failed to create private game room")?;
        let mut room_a = None;
        let mut game_room_error = None;
        for attempt in 1..=private_replication_attempts {
            match wait_for_game_room_in_scope(
                &runtime_a,
                topic,
                private_scope.clone(),
                room_id.as_str(),
                private_replication_timeout,
            )
            .await
            {
                Ok(room) => {
                    room_a = Some(room);
                    game_room_error = None;
                    break;
                }
                Err(error) if attempt < private_replication_attempts => {
                    game_room_error = Some(format!("{error:#}"));
                    refresh_private_channel_pair(
                        &runtime_a,
                        &runtime_b,
                        ticket_a.as_str(),
                        ticket_b.as_str(),
                        topic,
                        &private_scope,
                    )
                    .await
                    .context("failed to refresh private channel after game-room timeout")?;
                    sleep(Duration::from_millis(250)).await;
                }
                Err(error) => {
                    game_room_error = Some(format!("{error:#}"));
                    break;
                }
            }
        }
        if let Some(error) = game_room_error {
            anyhow::bail!("desktop a did not receive private game room: {error}");
        }
        let room_a = room_a.expect("private game room should be available after successful wait");
        assert_eq!(room_a.title, "private finals");
        push_named_step(&mut steps, "private_game", started_at);

        let started_at = Instant::now();
        shutdown_runtime(runtime_b, "desktop b private-channel restart pre-shutdown")
            .await
            .context("failed to shut down desktop b before restart")?;
        remove_sqlite_runtime_db(&db_b)
            .with_context(|| format!("failed to remove {} before restart", db_b.display()))?;
        runtime_b = DesktopRuntime::new_with_config(&db_b, TransportNetworkConfig::loopback())
            .await
            .context("failed to restart desktop b for private-channel scenario")?;
        let joined_after_restart = runtime_b
            .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                topic: topic.to_string(),
            })
            .await
            .context("failed to list joined private channels after restart")?;
        assert!(
            joined_after_restart
                .iter()
                .any(|entry| entry.channel_id == channel.channel_id && entry.label == "core")
        );
        wait_for_timeline_object_in_scope(
            &runtime_b,
            topic,
            private_scope.clone(),
            private_post_id.as_str(),
            step_timeout,
        )
        .await
        .context("desktop b did not restore private post after restart")?;
        let private_thread_after_restart = runtime_b
            .list_thread(ListThreadRequest {
                topic: topic.to_string(),
                thread_id: private_post_id.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("failed to read private thread after restart")?;
        assert!(
            private_thread_after_restart
                .items
                .iter()
                .any(|post| post.object_id == private_reply_id)
        );
        wait_for_live_ended_in_scope(
            &runtime_b,
            topic,
            private_scope.clone(),
            session_id.as_str(),
            step_timeout,
        )
        .await
        .context("desktop b did not restore private live session after restart")?;
        let restored_room = wait_for_game_room_in_scope(
            &runtime_b,
            topic,
            private_scope.clone(),
            room_id.as_str(),
            step_timeout,
        )
        .await
        .context("desktop b did not restore private game room after restart")?;
        assert_eq!(restored_room.title, "private finals");
        let fresh_invite = runtime_b
            .export_private_channel_invite(ExportPrivateChannelInviteRequest {
                topic: topic.to_string(),
                channel_id: channel.channel_id.clone(),
                expires_at: None,
            })
            .await
            .context("failed to re-export private invite after restart")?;
        assert!(fresh_invite.contains(topic));
        assert!(fresh_invite.contains(channel.channel_id.as_str()));
        push_named_step(&mut steps, "restart_rehydrate", started_at);

        let started_at = Instant::now();
        let ticket_b_after_restart = runtime_b
            .local_peer_ticket()
            .await
            .context("failed to export restarted desktop b ticket")?
            .context("missing restarted desktop b ticket")?;
        runtime_a
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_c.clone(),
            })
            .await
            .context("failed to import desktop c ticket into desktop a for outsider check")?;
        runtime_c
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_a.clone(),
            })
            .await
            .context("failed to import desktop a ticket into desktop c for outsider check")?;
        runtime_b
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_c.clone(),
            })
            .await
            .context("failed to import desktop c ticket into desktop b for outsider check")?;
        runtime_c
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_b_after_restart,
            })
            .await
            .context("failed to import restarted desktop b ticket into desktop c")?;
        let _ = runtime_c
            .list_timeline(ListTimelineRequest {
                topic: topic.to_string(),
                scope: public_scope.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("failed to subscribe desktop c to public topic")?;
        let _ = runtime_c
            .list_timeline(ListTimelineRequest {
                topic: topic.to_string(),
                scope: outsider_scope.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("failed to subscribe desktop c to public topic")?;
        wait_for_topic_peer_count(&runtime_c, topic, 1, step_timeout)
            .await
            .context("desktop c did not connect as outsider")?;
        let joined_channels_c = runtime_c
            .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                topic: topic.to_string(),
            })
            .await
            .context("failed to list desktop c joined private channels")?;
        assert!(
            joined_channels_c
                .iter()
                .all(|entry| entry.channel_id != channel.channel_id)
        );
        assert_timeline_scope_excludes_object(
            &runtime_c,
            topic,
            public_scope.clone(),
            private_post_id.as_str(),
            Duration::from_millis(500),
        )
        .await
        .context("desktop c public scope leaked private post")?;
        assert_timeline_scope_excludes_object(
            &runtime_c,
            topic,
            outsider_scope.clone(),
            private_post_id.as_str(),
            Duration::from_millis(500),
        )
        .await
        .context("desktop c public scope leaked private post")?;
        assert_timeline_scope_excludes_object(
            &runtime_c,
            topic,
            outsider_scope.clone(),
            private_reply_id.as_str(),
            Duration::from_millis(500),
        )
        .await
        .context("desktop c public scope leaked private reply")?;
        assert_live_session_absent_in_scope(
            &runtime_c,
            topic,
            outsider_scope.clone(),
            session_id.as_str(),
            Duration::from_millis(500),
        )
        .await
        .context("desktop c leaked private live session")?;
        assert_game_room_absent_in_scope(
            &runtime_c,
            topic,
            outsider_scope.clone(),
            room_id.as_str(),
            Duration::from_millis(500),
        )
        .await
        .context("desktop c leaked private game room")?;
        push_named_step(&mut steps, "outsider_isolation", started_at);

        let metrics_snapshot = if scenario.artifacts.metrics_snapshot {
            Some(
                runtime_b
                    .get_sync_status()
                    .await
                    .context("failed to collect final private-channel sync status")?,
            )
        } else {
            None
        };
        shutdown_runtime(runtime_a, "desktop a final shutdown")
            .await
            .context("desktop a final shutdown timed out")?;
        shutdown_runtime(runtime_b, "desktop b final shutdown")
            .await
            .context("desktop b final shutdown timed out")?;
        shutdown_runtime(runtime_c, "desktop c final shutdown")
            .await
            .context("desktop c final shutdown timed out")?;

        let result = HarnessResult {
            status: HarnessStatus::Pass,
            scenario: scenario.name.clone(),
            steps,
            artifacts: vec![artifacts_dir.join("result.json").display().to_string()],
            metrics_snapshot,
        };
        write_result_artifact(Path::new("."), artifacts_dir, &result)?;
        Ok::<HarnessResult, anyhow::Error>(result)
    })
    .await
    .context("scenario exceeded overall timeout")?
}
