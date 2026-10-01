use super::*;
use kukuri_core::BlobHash;

#[test]
fn sync_status_changed_event_wire_shape_is_stable() {
    let event = RuntimeEvent::SyncStatusChanged {
        sync_status: None,
        removed_topics: vec!["kukuri:topic:gone".into()],
        community_node_statuses: Vec::new(),
        removed_community_nodes: Vec::new(),
    };

    assert_eq!(
        serde_json::to_value(event).expect("serialize runtime event"),
        serde_json::json!({
            "type": "sync_status_changed",
            "sync_status": null,
            "removed_topics": ["kukuri:topic:gone"],
            "community_node_statuses": [],
            "removed_community_nodes": [],
        })
    );
}

#[test]
fn adult_label_eviction_event_identifies_the_hash() {
    assert_eq!(
        serde_json::to_value(RuntimeEvent::AdultMediaLabelEvicted {
            hash: Some("hash-1".into()),
        })
        .unwrap(),
        serde_json::json!({"type": "adult_media_label_evicted", "hash": "hash-1"})
    );
}

#[tokio::test]
async fn reclaimed_adult_label_reaches_runtime_event_subscribers() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().unwrap();
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("adult-label-event.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .unwrap();
    let mut events = runtime.subscribe_events();
    let hash = BlobHash::new("e".repeat(64));
    kukuri_store::ObjectProjectionStore::mark_adult_media_hashes(
        runtime.sqlite.as_ref(),
        std::slice::from_ref(&hash),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE remote_content_cache SET last_used_at = 0 \
         WHERE kind = 'adult_marker' AND cache_key = ?1",
    )
    .bind(hash.as_str())
    .execute(runtime.sqlite.pool())
    .await
    .unwrap();
    runtime.sqlite.reclaim_remote_cache_step().await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event,
        RuntimeEvent::AdultMediaLabelEvicted {
            hash: Some(hash.as_str().to_string())
        }
    );
    runtime.shutdown_checked().await.unwrap();
}

type PushedDelta = (
    Option<Box<SyncStatus>>,
    Vec<String>,
    Vec<CommunityNodeNodeStatus>,
    Vec<String>,
);

/// 差分の event を、`quiet` の間 event が無くなるまで集める。
async fn drain_sync_events(
    events: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>,
    quiet: Duration,
) -> Vec<PushedDelta> {
    let mut pushed = Vec::new();
    while let Ok(event) = timeout(quiet, events.recv()).await {
        if let Ok(RuntimeEvent::SyncStatusChanged {
            sync_status,
            removed_topics,
            community_node_statuses,
            removed_community_nodes,
        }) = event
        {
            pushed.push((
                sync_status,
                removed_topics,
                community_node_statuses,
                removed_community_nodes,
            ));
        }
    }
    pushed
}

async fn observed_runtime(dir: &std::path::Path) -> Arc<DesktopRuntime> {
    let runtime = Arc::new(
        DesktopRuntime::new_with_config_and_identity(
            dir.join("runtime-events.db"),
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        )
        .await
        .expect("runtime"),
    );
    runtime.start_sync_status_observer().await;
    runtime.start_sync_status_observer().await;
    runtime
}

async fn close_topic_column(runtime: &DesktopRuntime, topic: &str) {
    runtime
        .set_scope_display(crate::ScopeDisplayRequest {
            observer: format!("test-column:{topic}:{:?}", TimelineScope::Public),
            target: crate::ScopeDisplayTarget::Timeline {
                topic: topic.to_string(),
                scope: TimelineScope::Public,
            },
            visible: false,
        })
        .await
        .expect("close column");
}

fn delta_reads(runtime: &DesktopRuntime) -> usize {
    runtime
        .sync_status_delta_reads
        .load(std::sync::atomic::Ordering::SeqCst)
}

fn endpoint_id(index: u64) -> String {
    let mut secret = [0; 32];
    secret[..8].copy_from_slice(&index.to_be_bytes());
    iroh::SecretKey::from_bytes(&secret).public().to_string()
}

/// #1221 R2-D AC-4: 変化の無い間は event も読取りも無い。列を開く・閉じる(lease の変化)は、その topic だけの
/// 差分を 1 件送る。CN の session の書込みは、その node だけを送る。再試行中の node は Ready として送らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_status_events_carry_only_the_changed_parts_and_stop_on_shutdown() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let runtime = observed_runtime(dir.path()).await;
    let mut events = runtime.subscribe_events();
    let quiet = Duration::from_millis(1_500);

    assert!(drain_sync_events(&mut events, quiet).await.is_empty());
    assert_eq!(delta_reads(&runtime), 0, "no read while nothing changes");

    let topic = "kukuri:topic:observer-delta";
    open_topic_column(&runtime, topic, TimelineScope::Public)
        .await
        .expect("open column");
    let opened = drain_sync_events(&mut events, quiet).await;
    assert_eq!(opened.len(), 1, "one delta for one change");
    let status = opened[0].0.as_ref().expect("status delta");
    assert_eq!(
        status
            .topic_diagnostics
            .iter()
            .map(|topic| topic.topic.as_str())
            .collect::<Vec<_>>(),
        vec![topic]
    );
    assert!(status.subscribed_topics.contains(&topic.to_string()));
    assert!(opened[0].2.is_empty() && opened[0].3.is_empty());

    close_topic_column(&runtime, topic).await;
    let closed = drain_sync_events(&mut events, quiet).await;
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].1, vec![topic.to_string()]);
    assert!(
        closed[0]
            .0
            .as_ref()
            .expect("status")
            .topic_diagnostics
            .is_empty()
    );

    let base_url = "http://127.0.0.1:9";
    seed_local_community_node_consents(&runtime, base_url, 1);
    runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            nodes: vec![SetCommunityNodeConfigNode::new(base_url.to_string())],
            trust_node_priority: None,
        })
        .await
        .expect("config");
    let node_events = drain_sync_events(&mut events, quiet).await;
    let pushed_nodes = node_events
        .iter()
        .flat_map(|event| event.2.iter())
        .collect::<Vec<_>>();
    assert!(!pushed_nodes.is_empty(), "the session write is pushed");
    assert!(pushed_nodes.iter().all(|node| node.base_url == base_url));
    assert!(
        pushed_nodes
            .iter()
            .all(|node| node.session_phase != CommunityNodeSessionPhase::Ready),
        "an unreachable node is not shown as ready: {pushed_nodes:?}"
    );

    runtime.shutdown().await;
    assert!(runtime.sync_status_observer_task.lock().await.is_none());
}

/// #1221 R2-D AC-4・AC-6: 同じ種類の頻繁な変化は 1 秒に 1 回までにまとめ、最後の値を送る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frequent_changes_are_pushed_at_most_once_per_second() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let runtime = observed_runtime(dir.path()).await;
    let mut events = runtime.subscribe_events();
    let started = tokio::time::Instant::now();
    for count in 1..=20 {
        runtime
            .set_discovery_seeds(SetDiscoverySeedsRequest {
                seed_entries: (0..count).map(endpoint_id).collect(),
            })
            .await
            .expect("seeds");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let elapsed = started.elapsed();
    let pushed = drain_sync_events(&mut events, Duration::from_millis(1_500)).await;
    let limit = elapsed.as_secs() as usize + 2;
    assert!(
        !pushed.is_empty() && pushed.len() <= limit,
        "{} deltas in {elapsed:?}",
        pushed.len()
    );
    assert_eq!(
        pushed
            .last()
            .and_then(|event| event.0.as_ref())
            .expect("last status")
            .discovery
            .configured_seed_peer_count,
        20
    );
    runtime.shutdown().await;
}

/// #1221 R2-D AC-6(V2): 休止した topic・取り込んだ peer・seed・gossip の停止の設定・CN の node の履歴を
/// 10 倍にしても、変化の無い間の仕事(event 0・読取り 0)、1 件の変化で送る差分の数と大きさ、状態の 1 回の
/// 読取りの仕事は変わらない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_push_work_does_not_grow_with_dormant_history() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let runtime = observed_runtime(dir.path()).await;
    let mut events = runtime.subscribe_events();
    open_topic_column(&runtime, "kukuri:topic:push-leased", TimelineScope::Public)
        .await
        .expect("leased");
    let mut history = 0_u64;
    let quiet = Duration::from_millis(1_500);
    let mut measure = async |runtime: &DesktopRuntime, count: u64| {
        for _ in 0..count {
            let topic = format!("kukuri:topic:push-dormant-{history}");
            open_topic_column(runtime, &topic, TimelineScope::Public)
                .await
                .expect("dormant");
            close_topic_column(runtime, &topic).await;
            runtime
                .set_topic_gossip_enabled(SetTopicGossipEnabledRequest {
                    topic: topic.clone(),
                    enabled: false,
                })
                .await
                .expect("disable dormant");
            // 一覧から削除した topic(停止設定の履歴は topic とともに消える)。
            runtime
                .unsubscribe_topic(UnsubscribeTopicRequest { topic })
                .await
                .expect("remove dormant");
            for index in 0..10 {
                let id = endpoint_id(1_000 + history * 10 + index);
                let addr = iroh::EndpointAddr::new(id.parse().expect("id"));
                runtime
                    .sqlite
                    .put_peer_candidate(
                        "gossip",
                        "imported",
                        &id,
                        &serde_json::to_vec(&addr).expect("addr"),
                        Utc::now().timestamp_millis(),
                    )
                    .await
                    .expect("imported history");
            }
            let removed = format!("http://127.0.0.1:9/removed-{history}");
            for nodes in [vec![removed], Vec::new()] {
                runtime
                    .set_community_node_config(SetCommunityNodeConfigRequest {
                        nodes: nodes
                            .into_iter()
                            .map(SetCommunityNodeConfigNode::new)
                            .collect(),
                        trust_node_priority: None,
                    })
                    .await
                    .expect("node history");
            }
            history += 1;
        }
        runtime
            .set_discovery_seeds(SetDiscoverySeedsRequest {
                seed_entries: (0..history * 10).map(endpoint_id).collect(),
            })
            .await
            .expect("seed history");
        drain_sync_events(&mut events, quiet).await;
        let transport = runtime.iroh_stack.transport.current().await;
        let (reads, steps) = (delta_reads(runtime), transport.status_read_steps());
        assert!(
            drain_sync_events(&mut events, quiet).await.is_empty(),
            "no event while nothing changes"
        );
        assert_eq!(delta_reads(runtime), reads, "no read while nothing changes");
        assert_eq!(transport.status_read_steps(), steps);
        let status = runtime.get_sync_status().await.expect("status");
        let read_steps = transport.status_read_steps() - steps;
        let probe = format!("kukuri:topic:push-probe-{history}");
        open_topic_column(runtime, &probe, TimelineScope::Public)
            .await
            .expect("probe");
        let pushed = drain_sync_events(&mut events, quiet).await;
        close_topic_column(runtime, &probe).await;
        drain_sync_events(&mut events, quiet).await;
        (
            read_steps,
            status.topic_diagnostics.len(),
            status.gossip_disabled_topics.len(),
            pushed
                .iter()
                .map(|(status, removed, nodes, removed_nodes)| {
                    (
                        status.as_ref().map(|status| {
                            (
                                status.topic_diagnostics.len(),
                                status.subscribed_topics.len(),
                                status.gossip_disabled_topics.len(),
                            )
                        }),
                        removed.len(),
                        nodes.len(),
                        removed_nodes.len(),
                    )
                })
                .collect::<Vec<_>>(),
        )
    };
    let once = measure(&runtime, 1).await;
    let tenfold = measure(&runtime, 9).await;
    assert_eq!(once.3.len(), 1, "one delta for one change: {once:?}");
    assert_eq!(
        once.3[0].0,
        Some((1, 2, 0)),
        "the delta carries only the opened topic of the two leased ones"
    );
    assert_eq!(once, tenfold);
    runtime.shutdown().await;
}

#[tokio::test]
async fn notification_event_forwarder_stops_when_runtime_shuts_down() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("notification-event.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .unwrap();
    let notify = runtime.app_service.notification_inserted_notify();
    let mut events = runtime.subscribe_events();

    notify.notify_one();
    assert!(matches!(
        timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap(),
        RuntimeEvent::NotificationStatusChanged
    ));
    runtime.shutdown().await;
    notify.notify_one();
    assert!(
        timeout(Duration::from_millis(150), events.recv())
            .await
            .is_err(),
        "shutdown must stop forwarding events from the former account"
    );
}

#[tokio::test]
async fn notification_event_forwarder_stops_when_runtime_is_dropped() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("notification-drop.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .unwrap();
    let notify = runtime.app_service.notification_inserted_notify();
    let mut events = runtime.subscribe_events();

    notify.notify_one();
    assert!(matches!(
        timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap(),
        RuntimeEvent::NotificationStatusChanged
    ));
    drop(runtime);
    tokio::task::yield_now().await;
    notify.notify_one();
    assert!(
        !matches!(
            timeout(Duration::from_millis(150), events.recv()).await,
            Ok(Ok(RuntimeEvent::NotificationStatusChanged))
        ),
        "dropping an account runtime must not leave its forwarder alive"
    );
}
