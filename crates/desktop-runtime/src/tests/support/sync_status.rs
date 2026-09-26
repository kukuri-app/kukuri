use std::convert::Infallible;

use super::super::*;
use kukuri_test_support::{PollError, PollState, SyncSnapshot, TopicSyncSnapshot, poll_until};

fn snapshot_from_status(status: &SyncStatus) -> SyncSnapshot {
    SyncSnapshot {
        connected: status.connected,
        peer_count: status.peer_count,
        status_detail: status.status_detail.clone(),
        last_error: status.last_error.clone(),
        discovery_connected_peers: status.discovery.connected_peer_ids.clone(),
        topics: status
            .topic_diagnostics
            .iter()
            .map(|entry| TopicSyncSnapshot {
                topic: entry.topic.clone(),
                peer_count: entry.peer_count,
                connected_peers: entry.connected_peers.clone(),
                docs_assist_peer_ids: entry.docs_assist_peer_ids.clone(),
                configured_peer_ids: entry.configured_peer_ids.clone(),
                missing_peer_ids: Some(entry.missing_peer_ids.clone()),
                delivery_state: format!("{:?}", entry.delivery_state),
                status_detail: entry.status_detail.clone(),
            })
            .collect(),
    }
}

pub(crate) fn format_sync_snapshot(status: &SyncStatus, topic: &str) -> String {
    kukuri_test_support::format_sync_snapshot(&snapshot_from_status(status), topic)
}

pub(crate) async fn wait_for_connected_topic_peer_count(
    runtime: &DesktopRuntime,
    topic: &str,
    expected: usize,
    timeout_label: &str,
) {
    // 届くのは購読している topic だけ。待つ topic の列を開く(#1221 R2-C)。
    open_topic_column(runtime, topic, TimelineScope::Public)
        .await
        .expect("open the topic column");
    let result = poll_until(
        runtime_replication_timeout(),
        Duration::from_millis(100),
        3,
        || async {
            let status = runtime.get_sync_status().await.expect("sync status");
            Ok::<_, Infallible>(if topic_has_direct_peer(&status, topic, expected) {
                PollState::Ready(())
            } else {
                PollState::Pending
            })
        },
    )
    .await;
    if result.is_err() {
        let status = runtime.get_sync_status().await.expect("sync status");
        panic!("{timeout_label}: {}", format_sync_snapshot(&status, topic));
    }
}

pub(crate) async fn wait_for_topic_delivery(
    runtime: &DesktopRuntime,
    topic: &str,
    expected: usize,
    timeout_label: &str,
) {
    open_topic_column(runtime, topic, TimelineScope::Public)
        .await
        .expect("open the topic column");
    let result = poll_until(
        runtime_replication_timeout(),
        Duration::from_millis(100),
        3,
        || async {
            let status = runtime.get_sync_status().await.expect("sync status");
            Ok::<_, Infallible>(if topic_has_delivery(&status, topic, expected) {
                PollState::Ready(())
            } else {
                PollState::Pending
            })
        },
    )
    .await;
    if result.is_err() {
        let status = runtime.get_sync_status().await.expect("sync status");
        panic!("{timeout_label}: {}", format_sync_snapshot(&status, topic));
    }
}

pub(crate) async fn wait_for_topic_delivery_result(
    runtime: &DesktopRuntime,
    topic: &str,
    expected: usize,
    step_timeout: Duration,
) -> Result<()> {
    open_topic_column(runtime, topic, TimelineScope::Public).await?;
    match poll_until(step_timeout, Duration::from_millis(100), 3, || async {
        let status = runtime.get_sync_status().await.context("sync status")?;
        Ok::<_, anyhow::Error>(if topic_has_delivery(&status, topic, expected) {
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
                .unwrap_or_else(|| "failed to read sync status".to_string());
            bail!("topic delivery timeout; {status}");
        }
    }
}

pub(crate) fn topic_has_direct_peer(status: &SyncStatus, topic: &str, expected: usize) -> bool {
    status.topic_diagnostics.iter().any(|topic_status| {
        topic_status.topic == topic
            && topic_status.connected_peers.len() >= expected.min(1)
            && topic_status.peer_count >= expected
            && (topic_status.joined
                || matches!(
                    topic_status.delivery_state,
                    kukuri_app_api::DeliveryState::Live
                ))
    })
}

pub(crate) fn topic_has_delivery(status: &SyncStatus, topic: &str, expected: usize) -> bool {
    topic_has_direct_peer(status, topic, expected) || topic_has_durable_delivery(status, topic)
}

pub(crate) fn should_swap_shared_identity_public_replication_direction(
    publisher_status: &SyncStatus,
    subscriber_status: &SyncStatus,
    topic: &str,
    expected: usize,
) -> bool {
    !topic_has_direct_peer(publisher_status, topic, expected)
        && topic_has_direct_peer(subscriber_status, topic, expected)
}

pub(crate) async fn wait_for_direct_topic_peer_count_result(
    runtime: &DesktopRuntime,
    topic: &str,
    expected: usize,
    step_timeout: Duration,
) -> Result<()> {
    match poll_until(step_timeout, Duration::from_millis(100), 3, || async {
        let status = runtime.get_sync_status().await.context("sync status")?;
        Ok::<_, anyhow::Error>(if topic_has_direct_peer(&status, topic, expected) {
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
                .unwrap_or_else(|| "failed to read sync status".to_string());
            bail!("direct topic readiness timeout; {status}");
        }
    }
}
pub(crate) async fn wait_for_topic_doc_index_entry_result(
    runtime: &DesktopRuntime,
    topic: &str,
    object_id: &str,
    step_timeout: Duration,
) -> Result<()> {
    match timeout(step_timeout, async {
        loop {
            if runtime
                .has_topic_timeline_doc_index_entry(topic, object_id)
                .await
                .context("failed to query topic docs index")?
            {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let status = runtime
                .get_sync_status()
                .await
                .ok()
                .map(|value| format_sync_snapshot(&value, topic))
                .unwrap_or_else(|| "failed to read sync status".to_string());
            bail!("topic docs index timeout; {status}");
        }
    }
}

pub(crate) async fn wait_for_timeline_post(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: &TimelineScope,
    object_id: &str,
    timeout_label: &str,
) {
    match timeout(runtime_replication_timeout(), async {
        loop {
            let timeline = runtime
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .expect("timeline");
            if timeline
                .items
                .iter()
                .any(|post| post.object_id == object_id)
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    {
        Ok(()) => {}
        Err(_) => {
            let status = runtime.get_sync_status().await.expect("sync status");
            let private_items = runtime
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
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
            panic!(
                "{timeout_label}: {}; private_items={private_items:?}",
                format_sync_snapshot(&status, topic)
            );
        }
    }
}

pub(crate) async fn wait_for_timeline_post_result(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: &TimelineScope,
    object_id: &str,
    step_timeout: Duration,
) -> Result<()> {
    match timeout(step_timeout, async {
        loop {
            let timeline = runtime
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .context("timeline query failed")?;
            if timeline
                .items
                .iter()
                .any(|post| post.object_id == object_id)
            {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let status = runtime
                .get_sync_status()
                .await
                .ok()
                .map(|value| format_sync_snapshot(&value, topic))
                .unwrap_or_else(|| "failed to read sync status".to_string());
            bail!("timeline visibility timeout; {status}");
        }
    }
}

pub(crate) fn topic_has_durable_delivery(status: &SyncStatus, topic: &str) -> bool {
    status.topic_diagnostics.iter().any(|topic_status| {
        topic_status.topic == topic
            && !topic_status.docs_assist_peer_ids.is_empty()
            && matches!(
                topic_status.delivery_state,
                kukuri_app_api::DeliveryState::DurableRecovering
                    | kukuri_app_api::DeliveryState::DurableReady
            )
    })
}
pub(crate) async fn wait_for_profile_timeline_posts_result(
    runtime: &DesktopRuntime,
    author_pubkey: &str,
    object_ids: &[String],
    timeout_label: &str,
) -> Result<TimelineView> {
    // author の購読は profile を開いている間だけ(#1221 R2-C)。
    open_profile_column(runtime, author_pubkey).await?;
    match timeout(runtime_replication_timeout(), async {
        loop {
            let timeline = runtime
                .list_profile_timeline(ListProfileTimelineRequest {
                    pubkey: author_pubkey.to_string(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .expect("profile timeline");
            if object_ids.iter().all(|object_id| {
                timeline
                    .items
                    .iter()
                    .any(|post| post.object_id == *object_id)
            }) {
                return timeline;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    {
        Ok(timeline) => Ok(timeline),
        Err(_) => {
            let status = runtime.get_sync_status().await.expect("sync status");
            let visible_items = runtime
                .list_profile_timeline(ListProfileTimelineRequest {
                    pubkey: author_pubkey.to_string(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .ok()
                .map(|timeline| {
                    timeline
                        .items
                        .into_iter()
                        .map(|post| format!("{}@{:?}", post.object_id, post.origin_topic_id))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            bail!(
                "{timeout_label}: {}; visible_items={visible_items:?}",
                format_sync_snapshot(&status, "")
            );
        }
    }
}
pub(crate) async fn topic_timeline_doc_index_rows(
    runtime: &DesktopRuntime,
    topic: &str,
) -> Vec<String> {
    let replica = kukuri_docs_sync::topic_replica_id(topic);
    let current = runtime.iroh_stack.current.lock().await;
    let docs_sync = current.as_ref().expect("current stack").docs_sync.clone();
    drop(current);
    docs_sync
        .query_replica(&replica, DocQuery::Prefix("indexes/timeline/".into()))
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| row.key)
        .collect()
}
pub(crate) fn sync_status_with_topic(
    topic: &str,
    connected_peers: &[&str],
    docs_assist_peer_ids: &[&str],
) -> SyncStatus {
    let connected = !connected_peers.is_empty();
    let delivery_state = if connected {
        kukuri_app_api::DeliveryState::Live
    } else if !docs_assist_peer_ids.is_empty() {
        kukuri_app_api::DeliveryState::DurableRecovering
    } else {
        kukuri_app_api::DeliveryState::Offline
    };
    SyncStatus {
        connected,
        delivery_state,
        last_sync_ts: None,
        peer_count: connected_peers.len(),
        pending_events: 0,
        status_detail: "test".to_string(),
        last_error: None,
        configured_peers: Vec::new(),
        subscribed_topics: vec![topic.to_string()],
        active_path: Default::default(),
        fallback_peer_ids: Vec::new(),
        topic_diagnostics: vec![kukuri_app_api::TopicSyncStatus {
            topic: topic.to_string(),
            joined: connected,
            delivery_state,
            peer_count: connected_peers.len(),
            connected_peers: connected_peers
                .iter()
                .map(|peer| peer.to_string())
                .collect(),
            docs_assist_peer_ids: docs_assist_peer_ids
                .iter()
                .map(|peer| peer.to_string())
                .collect(),
            configured_peer_ids: Vec::new(),
            missing_peer_ids: Vec::new(),
            active_path: Default::default(),
            rendezvous_peer_ids: Vec::new(),
            fallback_peer_ids: Vec::new(),
            last_received_at: None,
            last_docs_activity_at: None,
            status_detail: "test".to_string(),
            last_error: None,
        }],
        local_author_pubkey: "author".to_string(),
        discovery: Default::default(),
        gossip_disabled_topics: Vec::new(),
        gossip_disabled_channels: Vec::new(),
    }
}
