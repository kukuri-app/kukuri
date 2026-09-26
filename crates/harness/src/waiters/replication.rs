use std::cell::Cell;

use super::*;
use crate::*;
use kukuri_test_support::{PollError, PollState, poll_until};

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

pub(crate) struct PublicReplicationLabels<'a> {
    pub(crate) failure: &'a str,
    pub(crate) publisher: &'a str,
    pub(crate) subscriber: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublicReplicationDirection {
    PreferOriginalPublisher,
    PreferDirectConnectedSubscriber,
}

pub(crate) async fn replicate_public_post_with_retry(
    publisher: &DesktopRuntime,
    subscriber: &DesktopRuntime,
    topic: &str,
    content_prefix: &str,
    step_timeout: Duration,
    direction: PublicReplicationDirection,
    labels: PublicReplicationLabels<'_>,
) -> Result<String> {
    let same_author_shared_identity = publisher
        .get_sync_status()
        .await
        .ok()
        .zip(subscriber.get_sync_status().await.ok())
        .is_some_and(|(publisher_status, subscriber_status)| {
            publisher_status.local_author_pubkey == subscriber_status.local_author_pubkey
        });
    let (attempts, attempt_timeout) =
        public_replication_retry_schedule(step_timeout, same_author_shared_identity);
    let mut last_error = None;
    let mut last_direction = None;

    for attempt in 1..=attempts {
        let attempt_result = async {
            open_topic_column(publisher, topic, &TimelineScope::Public)
                .await
                .context("failed to subscribe publisher to public topic")?;
            open_topic_column(subscriber, topic, &TimelineScope::Public)
                .await
                .context("failed to subscribe subscriber to public topic")?;
            wait_for_topic_delivery(publisher, topic, 1, attempt_timeout)
                .await
                .context("publisher did not observe public topic delivery readiness")?;
            wait_for_topic_delivery(subscriber, topic, 1, attempt_timeout)
                .await
                .context("subscriber did not observe public topic delivery readiness")?;
            let publisher_status = publisher
                .get_sync_status()
                .await
                .context("publisher sync status")?;
            let subscriber_status = subscriber
                .get_sync_status()
                .await
                .context("subscriber sync status")?;
            let publish_from_subscriber = should_retry_public_replication_from_subscriber(
                &publisher_status,
                &subscriber_status,
                topic,
                1,
                direction,
                attempt,
            );
            let (active_publisher, active_subscriber, publisher_label, subscriber_label) =
                if publish_from_subscriber {
                    (subscriber, publisher, labels.subscriber, labels.publisher)
                } else {
                    (publisher, subscriber, labels.publisher, labels.subscriber)
                };
            if publish_from_subscriber {
                wait_for_direct_topic_peer_count(active_publisher, topic, 1, attempt_timeout)
                    .await
                    .with_context(|| {
                        format!(
                            "{} did not observe direct public topic connectivity",
                            publisher_label
                        )
                    })?;
            }
            last_direction = Some(format!("publish {publisher_label}->{subscriber_label}"));
            let post_id = active_publisher
                .create_post(CreatePostRequest {
                    topic: topic.to_string(),
                    content: format!("{content_prefix} #{attempt}"),
                    reply_to: None,
                    channel_ref: ChannelRef::Public,
                    attachments: Vec::new(),
                    content_labels: Vec::new(),
                })
                .await
                .with_context(|| format!("failed to create public post on {publisher_label}"))?;
            wait_for_topic_doc_index_entry(
                active_publisher,
                topic,
                post_id.as_str(),
                attempt_timeout,
            )
            .await
            .context("publisher did not persist public post into docs index")?;
            wait_for_timeline_object(active_subscriber, topic, post_id.as_str(), attempt_timeout)
                .await
                .context("timeline assertion timeout")?;
            Ok::<String, anyhow::Error>(post_id)
        }
        .await;

        match attempt_result {
            Ok(post_id) => return Ok(post_id),
            Err(error) if attempt < attempts => {
                last_error = Some(format!("{error:#}"));
                refresh_public_pair(publisher, subscriber, topic, attempt_timeout)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to refresh public pair after {} replication timeout",
                            labels.failure
                        )
                    })?;
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
        .ok()
        .map(|status| format_sync_snapshot(&status, topic))
        .unwrap_or_else(|| format!("failed to read {} sync status", labels.publisher));
    let subscriber_status = subscriber
        .get_sync_status()
        .await
        .ok()
        .map(|status| format_sync_snapshot(&status, topic))
        .unwrap_or_else(|| format!("failed to read {} sync status", labels.subscriber));
    Err(anyhow::anyhow!(
        "{}",
        last_error
            .unwrap_or_else(|| { format!("unknown replication failure for {}", labels.failure) })
    )
    .context(format!(
        "{} did not receive the {}; direction={}; {}=({publisher_status}); {}=({subscriber_status})",
        labels.subscriber,
        labels.failure,
        last_direction.unwrap_or_else(|| "unknown".to_string()),
        labels.publisher,
        labels.subscriber
    )))
}

pub(crate) async fn refresh_public_pair(
    runtime_a: &DesktopRuntime,
    runtime_b: &DesktopRuntime,
    topic: &str,
    step_timeout: Duration,
) -> Result<()> {
    async fn refresh_public_runtime(runtime: &DesktopRuntime, topic: &str) {
        let _ = open_topic_column(runtime, topic, &TimelineScope::Public).await;
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
        if let Ok(statuses) = runtime.get_community_node_statuses().await {
            for node in statuses {
                if node.auth_state.authenticated {
                    let _ = runtime
                        .refresh_community_node_metadata(CommunityNodeTargetRequest {
                            base_url: node.base_url,
                        })
                        .await;
                }
            }
        }
    }

    fn public_connectivity_reapply_interval() -> Duration {
        if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
            Duration::from_secs(20)
        } else {
            Duration::from_secs(10)
        }
    }

    async fn force_public_runtime_connectivity(runtime: &DesktopRuntime) {
        let has_active_node_consent =
            runtime
                .get_community_node_statuses()
                .await
                .is_ok_and(|statuses| {
                    statuses
                        .iter()
                        .any(|status| status.local_consent.has_active_consent())
                });
        if has_active_node_consent {
            let _ = runtime.reapply_community_node_connectivity().await;
        }
    }

    let refresh_interval = Duration::from_secs(5);
    let reapply_interval = public_connectivity_reapply_interval();
    // poll_until のクロージャは FnMut のため、時限アクションの締切は Cell 経由の
    // 共有参照で持ち回る(値の意味・実行順序は従来の手書きループと同一)。
    let next_refresh_at = Cell::new(Instant::now());
    let next_reapply_at = Cell::new(Instant::now() + reapply_interval);
    match poll_until(step_timeout, Duration::from_millis(100), 3, || async {
        if Instant::now() >= next_refresh_at.get() {
            refresh_public_runtime(runtime_a, topic).await;
            refresh_public_runtime(runtime_b, topic).await;
            next_refresh_at.set(Instant::now() + refresh_interval);
        }
        if Instant::now() >= next_reapply_at.get() {
            force_public_runtime_connectivity(runtime_a).await;
            force_public_runtime_connectivity(runtime_b).await;
            next_reapply_at.set(Instant::now() + reapply_interval);
        }

        let status_a = runtime_a
            .get_sync_status()
            .await
            .context("desktop a public sync status")?;
        let status_b = runtime_b
            .get_sync_status()
            .await
            .context("desktop b public sync status")?;
        let ready_a = topic_has_direct_peer(&status_a, topic, 1)
            || topic_has_durable_delivery(&status_a, topic);
        let ready_b = topic_has_direct_peer(&status_b, topic, 1)
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
            let snapshot_a = runtime_a
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, topic))
                .unwrap_or_else(|| "failed to read desktop a sync status".to_string());
            let snapshot_b = runtime_b
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, topic))
                .unwrap_or_else(|| "failed to read desktop b sync status".to_string());
            anyhow::bail!(
                "public pair refresh timeout; desktop_a=({snapshot_a}); desktop_b=({snapshot_b})"
            );
        }
    }
}

pub(crate) async fn select_public_feature_pair<'a>(
    runtime_a: &'a DesktopRuntime,
    runtime_b: &'a DesktopRuntime,
    topic: &str,
    step_timeout: Duration,
    attempt: usize,
) -> Result<(
    &'a DesktopRuntime,
    &'a DesktopRuntime,
    &'static str,
    &'static str,
)> {
    refresh_public_pair(runtime_a, runtime_b, topic, step_timeout).await?;
    let direct_pair_timeout = ci_timeout_floor(step_timeout, Duration::from_secs(60));
    let _ =
        timeout(direct_pair_timeout, async {
            loop {
                let publisher_status = runtime_a.get_sync_status().await.context(
                    "desktop a sync status while waiting for public feature connectivity",
                )?;
                let subscriber_status = runtime_b.get_sync_status().await.context(
                    "desktop b sync status while waiting for public feature connectivity",
                )?;
                if topic_has_direct_peer(&publisher_status, topic, 1)
                    && topic_has_direct_peer(&subscriber_status, topic, 1)
                {
                    return Ok::<(), anyhow::Error>(());
                }
                refresh_public_pair(runtime_a, runtime_b, topic, direct_pair_timeout).await?;
                sleep(Duration::from_millis(250)).await;
            }
        })
        .await;
    let publisher_status = runtime_a
        .get_sync_status()
        .await
        .context("desktop a sync status for public feature selection")?;
    let subscriber_status = runtime_b
        .get_sync_status()
        .await
        .context("desktop b sync status for public feature selection")?;
    let strategy =
        select_public_feature_strategy(&publisher_status, &subscriber_status, topic, 1, attempt);
    if strategy.select_subscriber {
        if strategy.require_direct_subscriber {
            wait_for_direct_topic_peer_count(runtime_b, topic, 1, step_timeout)
                .await
                .context("desktop b did not observe direct public topic connectivity")?;
        }
        Ok((runtime_b, runtime_a, "desktop b", "desktop a"))
    } else {
        if topic_has_direct_peer(&publisher_status, topic, 1) {
            wait_for_direct_topic_peer_count(runtime_a, topic, 1, step_timeout)
                .await
                .context("desktop a did not observe direct public topic connectivity")?;
        }
        Ok((runtime_a, runtime_b, "desktop a", "desktop b"))
    }
}
