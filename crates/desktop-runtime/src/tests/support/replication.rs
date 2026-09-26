use std::cell::Cell;

use super::super::*;
use kukuri_test_support::{PollError, PollState, poll_until};

/// topic の列を開いて購読する(#1221 R2-C。読み込みは購読を開始しない)。
pub(crate) async fn open_topic_column(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: TimelineScope,
) -> Result<()> {
    runtime
        .set_scope_display(crate::ScopeDisplayRequest {
            observer: format!("test-column:{topic}:{scope:?}"),
            target: crate::ScopeDisplayTarget::Timeline {
                topic: topic.to_string(),
                scope,
            },
            visible: true,
        })
        .await
}

/// author の profile の列を開いて購読する(#1221 R2-C)。
pub(crate) async fn open_profile_column(runtime: &DesktopRuntime, pubkey: &str) -> Result<()> {
    runtime
        .set_scope_display(crate::ScopeDisplayRequest {
            observer: format!("test-profile:{pubkey}"),
            target: crate::ScopeDisplayTarget::Author {
                pubkey: pubkey.to_string(),
            },
            visible: true,
        })
        .await
}

pub(crate) async fn refresh_public_runtime_for_retry(
    runtime: &DesktopRuntime,
    topic: &str,
) -> Result<()> {
    open_topic_column(runtime, topic, TimelineScope::Public).await?;
    let _ = runtime
        .list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await;
    let _ = runtime
        .list_live_sessions(ListLiveSessionsRequest {
            topic: topic.to_string(),
            scope: TimelineScope::Public,
        })
        .await;
    let _ = runtime
        .list_game_rooms(ListGameRoomsRequest {
            topic: topic.to_string(),
            scope: TimelineScope::Public,
        })
        .await;
    Ok(())
}
pub(crate) async fn force_public_runtime_connectivity_retry(
    runtime: &DesktopRuntime,
) -> Result<()> {
    runtime
        .reapply_community_node_connectivity()
        .await
        .context("reapply community-node connectivity during public retry")?;
    Ok(())
}

pub(crate) async fn wait_for_public_runtime_delivery_with_refresh(
    runtime: &DesktopRuntime,
    topic: &str,
    expected: usize,
    step_timeout: Duration,
) -> Result<()> {
    let refresh_interval = Duration::from_secs(5);
    let reapply_interval = public_connectivity_reapply_interval();
    // poll_until のクロージャは FnMut のため、時限アクションの締切は Cell 経由の
    // 共有参照で持ち回る(値の意味・実行順序は従来の手書きループと同一)。
    let next_refresh_at = Cell::new(tokio::time::Instant::now());
    let next_reapply_at = Cell::new(tokio::time::Instant::now() + reapply_interval);
    match poll_until(step_timeout, Duration::from_millis(100), 3, || async {
        if tokio::time::Instant::now() >= next_refresh_at.get() {
            refresh_public_runtime_for_retry(runtime, topic).await?;
            next_refresh_at.set(tokio::time::Instant::now() + refresh_interval);
        }
        if tokio::time::Instant::now() >= next_reapply_at.get() {
            force_public_runtime_connectivity_retry(runtime).await?;
            next_reapply_at.set(tokio::time::Instant::now() + reapply_interval);
        }

        let status = runtime
            .get_sync_status()
            .await
            .context("runtime sync status")?;
        let ready = topic_has_direct_peer(&status, topic, expected)
            || topic_has_durable_delivery(&status, topic);
        Ok::<_, anyhow::Error>(if ready {
            PollState::Ready(())
        } else {
            PollState::Pending
        })
    })
    .await
    {
        Ok(()) => Ok(()),
        Err(PollError::Operation(error)) => Err(error),
        Err(PollError::Timeout) => {
            let status = runtime
                .get_sync_status()
                .await
                .ok()
                .map(|value| format_sync_snapshot(&value, topic))
                .unwrap_or_else(|| "failed to read runtime sync status".to_string());
            bail!("public runtime delivery timeout; {status}");
        }
    }
}

pub(crate) async fn wait_for_public_pair_delivery_with_refresh(
    runtime_a: &DesktopRuntime,
    runtime_b: &DesktopRuntime,
    topic: &str,
    expected: usize,
    step_timeout: Duration,
) -> Result<()> {
    let refresh_interval = Duration::from_secs(5);
    let reapply_interval = public_connectivity_reapply_interval();
    let next_refresh_at = Cell::new(tokio::time::Instant::now());
    let next_reapply_at = Cell::new(tokio::time::Instant::now() + reapply_interval);
    match poll_until(step_timeout, Duration::from_millis(100), 3, || async {
        if tokio::time::Instant::now() >= next_refresh_at.get() {
            refresh_public_runtime_for_retry(runtime_a, topic).await?;
            refresh_public_runtime_for_retry(runtime_b, topic).await?;
            next_refresh_at.set(tokio::time::Instant::now() + refresh_interval);
        }
        if tokio::time::Instant::now() >= next_reapply_at.get() {
            force_public_runtime_connectivity_retry(runtime_a).await?;
            force_public_runtime_connectivity_retry(runtime_b).await?;
            next_reapply_at.set(tokio::time::Instant::now() + reapply_interval);
        }

        let status_a = runtime_a
            .get_sync_status()
            .await
            .context("runtime a sync status")?;
        let status_b = runtime_b
            .get_sync_status()
            .await
            .context("runtime b sync status")?;
        let ready_a = topic_has_direct_peer(&status_a, topic, expected)
            || topic_has_durable_delivery(&status_a, topic);
        let ready_b = topic_has_direct_peer(&status_b, topic, expected)
            || topic_has_durable_delivery(&status_b, topic);
        Ok::<_, anyhow::Error>(if ready_a && ready_b {
            PollState::Ready(())
        } else {
            PollState::Pending
        })
    })
    .await
    {
        Ok(()) => Ok(()),
        Err(PollError::Operation(error)) => Err(error),
        Err(PollError::Timeout) => {
            let status_a = runtime_a
                .get_sync_status()
                .await
                .ok()
                .map(|value| format_sync_snapshot(&value, topic))
                .unwrap_or_else(|| "failed to read runtime a sync status".to_string());
            let status_b = runtime_b
                .get_sync_status()
                .await
                .ok()
                .map(|value| format_sync_snapshot(&value, topic))
                .unwrap_or_else(|| "failed to read runtime b sync status".to_string());
            bail!("public pair delivery timeout; runtime_a=({status_a}); runtime_b=({status_b})");
        }
    }
}
pub(crate) fn public_replication_retry_schedule(
    step_timeout: Duration,
    same_author_shared_identity: bool,
) -> (usize, Duration) {
    let attempts = if std::env::var_os("GITHUB_ACTIONS").is_some() || same_author_shared_identity {
        3
    } else {
        1
    };
    let per_attempt_timeout = if attempts > 1 {
        Duration::from_millis(
            (step_timeout.as_millis() / attempts as u128)
                .max(1)
                .try_into()
                .expect("public replication timeout fits in u64"),
        )
    } else {
        step_timeout
    };
    (attempts, per_attempt_timeout)
}
pub(crate) async fn replicate_public_post_with_retry(
    publisher: &DesktopRuntime,
    subscriber: &DesktopRuntime,
    topic: &str,
    content_prefix: &str,
    timeout_label: &str,
) -> String {
    replicate_public_post_with_retry_inner(
        publisher,
        subscriber,
        topic,
        content_prefix,
        timeout_label,
        true,
    )
    .await
}

pub(crate) async fn replicate_public_post_with_retry_inner(
    publisher: &DesktopRuntime,
    subscriber: &DesktopRuntime,
    topic: &str,
    content_prefix: &str,
    timeout_label: &str,
    allow_shared_identity_swap: bool,
) -> String {
    let same_author_shared_identity = publisher
        .get_sync_status()
        .await
        .ok()
        .zip(subscriber.get_sync_status().await.ok())
        .is_some_and(|(publisher_status, subscriber_status)| {
            publisher_status.local_author_pubkey == subscriber_status.local_author_pubkey
        });
    let (attempts, attempt_timeout) = public_replication_retry_schedule(
        runtime_replication_timeout(),
        same_author_shared_identity,
    );
    let scope = TimelineScope::Public;
    let mut last_error = None;

    for attempt in 1..=attempts {
        let attempt_result = async {
            let _ = publisher
                .list_timeline(ListTimelineRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .context("failed to resubscribe publisher to public topic")?;
            let _ = subscriber
                .list_timeline(ListTimelineRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .context("failed to resubscribe subscriber to public topic")?;
            let publisher_status = publisher
                .get_sync_status()
                .await
                .context("publisher sync status")?;
            let subscriber_status = subscriber
                .get_sync_status()
                .await
                .context("subscriber sync status")?;
            let publish_from_subscriber = allow_shared_identity_swap
                && same_author_shared_identity
                && should_swap_shared_identity_public_replication_direction(
                    &publisher_status,
                    &subscriber_status,
                    topic,
                    1,
                );
            let (active_publisher, active_subscriber) = if publish_from_subscriber {
                (subscriber, publisher)
            } else {
                (publisher, subscriber)
            };
            if publish_from_subscriber {
                wait_for_direct_topic_peer_count_result(
                    active_publisher,
                    topic,
                    1,
                    attempt_timeout,
                )
                .await
                .context("publishing runtime did not observe direct public topic connectivity")?;
            }
            let object_id = active_publisher
                .create_post(CreatePostRequest {
                    topic: topic.to_string(),
                    content: format!("{content_prefix} #{attempt}"),
                    reply_to: None,
                    channel_ref: ChannelRef::Public,
                    attachments: Vec::new(),
                    content_labels: Vec::new(),
                })
                .await
                .context("failed to create public post")?;
            wait_for_topic_doc_index_entry_result(
                active_publisher,
                topic,
                object_id.as_str(),
                attempt_timeout,
            )
            .await
            .context("publisher did not persist public post into docs index")?;
            wait_for_timeline_post_result(
                active_subscriber,
                topic,
                &scope,
                object_id.as_str(),
                attempt_timeout,
            )
            .await
            .context("subscriber did not observe replicated public post")?;
            Ok::<String, anyhow::Error>(object_id)
        }
        .await;

        match attempt_result {
            Ok(object_id) => return object_id,
            Err(error) if attempt < attempts => {
                last_error = Some(format!("{error:#}"));
                if let Err(refresh_error) = wait_for_public_pair_delivery_with_refresh(
                    publisher,
                    subscriber,
                    topic,
                    1,
                    attempt_timeout,
                )
                .await
                {
                    last_error = Some(format!(
                        "{:#}; public topic refresh failed after replication timeout: {refresh_error:#}",
                        error
                    ));
                    break;
                }
                sleep(Duration::from_millis(250)).await;
            }
            Err(error) => {
                last_error = Some(format!("{error:#}"));
                break;
            }
        }
    }

    let publisher_status = publisher
        .get_sync_status()
        .await
        .expect("publisher sync status");
    let subscriber_status = subscriber
        .get_sync_status()
        .await
        .expect("subscriber sync status");
    let publisher_docs_rows = topic_timeline_doc_index_rows(publisher, topic).await;
    let subscriber_docs_rows = topic_timeline_doc_index_rows(subscriber, topic).await;
    panic!(
        "{timeout_label}; last_error={last_error:?}; publisher=({}); subscriber=({}); publisher_docs_rows={publisher_docs_rows:?}; subscriber_docs_rows={subscriber_docs_rows:?}",
        format_sync_snapshot(&publisher_status, topic),
        format_sync_snapshot(&subscriber_status, topic),
    );
}

pub(crate) async fn replicate_private_post_with_retry(
    publisher: &DesktopRuntime,
    subscribers: &[&DesktopRuntime],
    topic: &str,
    scope: &TimelineScope,
    channel_ref: &ChannelRef,
    content_prefix: &str,
    timeout_label: &str,
) -> String {
    let (attempts, attempt_timeout) =
        public_replication_retry_schedule(runtime_replication_timeout(), false);
    let channel_id = match scope {
        TimelineScope::Channel { channel_id } => channel_id.as_str().to_string(),
        TimelineScope::Public => {
            panic!("replicate_private_post_with_retry requires a private channel scope")
        }
    };
    let mut last_error = None;

    for attempt in 1..=attempts {
        let attempt_result = async {
            let _ = publisher
                .list_timeline(ListTimelineRequest {
                    topic: topic.to_string(),
                    scope: TimelineScope::Public,
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .context("failed to resubscribe publisher to public topic")?;
            let _ = publisher
                .list_timeline(ListTimelineRequest {
                    topic: topic.to_string(),
                    scope: scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .context("failed to resubscribe publisher to private topic")?;
            let _ = publisher
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.to_string(),
                })
                .await
                .context("failed to refresh publisher joined private channels")?;
            wait_for_topic_delivery_result(publisher, topic, 1, attempt_timeout)
                .await
                .context("publisher did not observe private topic delivery readiness")?;
            for subscriber in subscribers {
                let _ = subscriber
                    .list_timeline(ListTimelineRequest {
                        topic: topic.to_string(),
                        scope: TimelineScope::Public,
                        cursor: None,
                        limit: Some(20),
                    })
                    .await
                    .context("failed to resubscribe subscriber to public topic")?;
                let _ = subscriber
                    .list_timeline(ListTimelineRequest {
                        topic: topic.to_string(),
                        scope: scope.clone(),
                        cursor: None,
                        limit: Some(20),
                    })
                    .await
                    .context("failed to resubscribe subscriber to private topic")?;
                let _ = subscriber
                    .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                        topic: topic.to_string(),
                    })
                    .await
                    .context("failed to refresh subscriber joined private channels")?;
                wait_for_topic_delivery_result(subscriber, topic, 1, attempt_timeout)
                    .await
                    .context("subscriber did not observe private topic delivery readiness")?;
            }
            let pre_write_epoch =
                joined_private_channel_epoch_result(publisher, topic, channel_id.as_str())
                    .await
                    .context("failed to read publisher private channel state before write")?
                    .map(|entry| entry.current_epoch_id);
            let object_id = publisher
                .create_post(CreatePostRequest {
                    topic: topic.to_string(),
                    content: format!("{content_prefix} #{attempt}"),
                    reply_to: None,
                    channel_ref: channel_ref.clone(),
                    attachments: Vec::new(),
                    content_labels: Vec::new(),
                })
                .await
                .context("failed to create private post")?;
            wait_for_timeline_post_result(
                publisher,
                topic,
                scope,
                object_id.as_str(),
                attempt_timeout,
            )
            .await
            .context("publisher did not observe private post locally")?;
            let post_write_epoch =
                joined_private_channel_epoch_result(publisher, topic, channel_id.as_str())
                    .await
                    .context("failed to read publisher private channel state after write")?
                    .ok_or_else(|| anyhow::anyhow!("publisher lost private channel after write"))?
                    .current_epoch_id;
            if pre_write_epoch.as_deref() != Some(post_write_epoch.as_str()) {
                let mut runtimes = Vec::with_capacity(subscribers.len() + 1);
                runtimes.push(publisher);
                runtimes.extend(subscribers.iter().copied());
                refresh_runtime_peer_tickets(&runtimes)
                    .await
                    .context("failed to refresh peer tickets after private channel rotation")?;
                for runtime in &runtimes {
                    let _ = runtime
                        .list_timeline(ListTimelineRequest {
                            topic: topic.to_string(),
                            scope: TimelineScope::Public,
                            cursor: None,
                            limit: Some(20),
                        })
                        .await
                        .context("failed to refresh public topic after private channel rotation")?;
                    let _ = runtime
                        .list_timeline(ListTimelineRequest {
                            topic: topic.to_string(),
                            scope: scope.clone(),
                            cursor: None,
                            limit: Some(20),
                        })
                        .await
                        .context(
                            "failed to refresh private topic after private channel rotation",
                        )?;
                    let _ = runtime
                        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                            topic: topic.to_string(),
                        })
                        .await
                        .context(
                            "failed to refresh joined private channels after private channel rotation",
                        )?;
                }
                for subscriber in subscribers {
                    wait_for_joined_private_channel_epoch_result(
                        subscriber,
                        topic,
                        channel_id.as_str(),
                        post_write_epoch.as_str(),
                        1,
                        attempt_timeout,
                    )
                    .await
                    .context("subscriber did not redeem private channel rotation after write")?;
                }
            }
            for subscriber in subscribers {
                wait_for_timeline_post_result(
                    subscriber,
                    topic,
                    scope,
                    object_id.as_str(),
                    attempt_timeout,
                )
                .await
                .context("subscriber did not observe replicated private post")?;
            }
            Ok::<String, anyhow::Error>(object_id)
        }
        .await;

        match attempt_result {
            Ok(object_id) => return object_id,
            Err(error) if attempt < attempts => {
                last_error = Some(format!("{error:#}"));
                sleep(Duration::from_millis(250)).await;
            }
            Err(error) => {
                last_error = Some(format!("{error:#}"));
                break;
            }
        }
    }

    let publisher_status = publisher
        .get_sync_status()
        .await
        .expect("publisher sync status");
    let mut subscriber_details = Vec::with_capacity(subscribers.len());
    for (index, subscriber) in subscribers.iter().enumerate() {
        let status = subscriber
            .get_sync_status()
            .await
            .expect("subscriber sync status");
        let visible_items = subscriber
            .list_timeline(ListTimelineRequest {
                topic: topic.to_string(),
                scope: scope.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .ok()
            .map(|timeline| {
                timeline
                    .items
                    .into_iter()
                    .map(|post| post.object_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        subscriber_details.push(format!(
            "subscriber[{index}] {} visible_items={visible_items:?}",
            format_sync_snapshot(&status, topic)
        ));
    }
    panic!(
        "{timeout_label}; last_error={last_error:?}; publisher=({}); {}",
        format_sync_snapshot(&publisher_status, topic),
        subscriber_details.join(" | "),
    );
}

pub(crate) async fn refresh_runtime_peer_tickets(runtimes: &[&DesktopRuntime]) -> Result<()> {
    let mut tickets = Vec::with_capacity(runtimes.len());
    for (index, runtime) in runtimes.iter().enumerate() {
        let ticket = runtime
            .local_peer_ticket()
            .await
            .with_context(|| format!("failed to load local peer ticket for runtime[{index}]"))?
            .ok_or_else(|| {
                anyhow::anyhow!("runtime[{index}] did not expose a local peer ticket")
            })?;
        tickets.push(ticket);
    }
    for (runtime_index, runtime) in runtimes.iter().enumerate() {
        for (ticket_index, ticket) in tickets.iter().enumerate() {
            if runtime_index == ticket_index {
                continue;
            }
            runtime
                .import_peer_ticket(ImportPeerTicketRequest {
                    ticket: ticket.clone(),
                })
                .await
                .with_context(|| {
                    format!(
                        "failed to import peer ticket from runtime[{ticket_index}] into runtime[{runtime_index}]"
                    )
                })?;
        }
    }
    Ok(())
}
