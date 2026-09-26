use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_discovery_seeds_reapplies_runtime_without_restart() {
    let _resource = lock_test_resource(TestResource::DhtTestnet).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("seeded-a.db");
    let db_b = dir.path().join("seeded-b.db");
    let testnet = Testnet::new(5).await.expect("testnet");
    let runtime_a = new_seeded_dht_runtime(&db_a, &testnet).await;
    let runtime_b = new_seeded_dht_runtime(&db_b, &testnet).await;
    let endpoint_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;

    runtime_a
        .set_discovery_seeds(SetDiscoverySeedsRequest {
            seed_entries: vec![endpoint_b.clone()],
        })
        .await
        .expect("set seeds a");
    runtime_b
        .set_discovery_seeds(SetDiscoverySeedsRequest {
            seed_entries: vec![endpoint_a.clone()],
        })
        .await
        .expect("set seeds b");

    let config_a = runtime_a
        .get_discovery_config()
        .await
        .expect("discovery config a");
    let config_b = runtime_b
        .get_discovery_config()
        .await
        .expect("discovery config b");
    assert_eq!(config_a.seed_peers[0].endpoint_id, endpoint_b);
    assert_eq!(config_b.seed_peers[0].endpoint_id, endpoint_a);
    let topic = "kukuri:topic:runtime-seeded-dht";
    open_topic_column(&runtime_a, topic, TimelineScope::Public)
        .await
        .expect("subscribe a");
    open_topic_column(&runtime_b, topic, TimelineScope::Public)
        .await
        .expect("subscribe b");
    wait_for_seeded_dht_topic_ready(&runtime_a, &runtime_b, topic).await;
    let status_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a after seeds");
    let status_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b after seeds");
    assert!(
        status_a
            .subscribed_topics
            .iter()
            .any(|entry| entry == topic)
    );
    assert!(
        status_b
            .subscribed_topics
            .iter()
            .any(|entry| entry == topic)
    );
    assert!(
        topic_has_direct_peer(&status_a, topic, 1) || topic_has_durable_delivery(&status_a, topic)
    );
    assert!(
        topic_has_direct_peer(&status_b, topic, 1) || topic_has_durable_delivery(&status_b, topic)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_restores_seeded_dht_config_and_endpoint_identity() {
    let _resource = lock_test_resource(TestResource::DhtTestnet).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("restart-seeded-a.db");
    let db_b = dir.path().join("restart-seeded-b.db");
    let testnet = Testnet::new(5).await.expect("testnet");
    let runtime_a = new_seeded_dht_runtime(&db_a, &testnet).await;
    let runtime_b = new_seeded_dht_runtime(&db_b, &testnet).await;
    let endpoint_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;

    runtime_a
        .set_discovery_seeds(SetDiscoverySeedsRequest {
            seed_entries: vec![endpoint_b.clone()],
        })
        .await
        .expect("set seeds a");
    runtime_b
        .set_discovery_seeds(SetDiscoverySeedsRequest {
            seed_entries: vec![endpoint_a.clone()],
        })
        .await
        .expect("set seeds b");

    timeout(Duration::from_secs(15), runtime_a.shutdown())
        .await
        .expect("shutdown a");
    timeout(Duration::from_secs(15), runtime_b.shutdown())
        .await
        .expect("shutdown b");
    drop(runtime_a);
    drop(runtime_b);

    let restored_a = resolve_discovery_config_from_env(&db_a).expect("restored discovery config a");
    let restored_b = resolve_discovery_config_from_env(&db_b).expect("restored discovery config b");
    let restarted_a = new_seeded_dht_runtime_with_config(&db_a, &testnet, restored_a.clone()).await;
    let restarted_b = new_seeded_dht_runtime_with_config(&db_b, &testnet, restored_b.clone()).await;
    let restarted_endpoint_a = restarted_a
        .get_sync_status()
        .await
        .expect("restarted status a")
        .discovery
        .local_endpoint_id;
    let restarted_endpoint_b = restarted_b
        .get_sync_status()
        .await
        .expect("restarted status b")
        .discovery
        .local_endpoint_id;

    assert_eq!(restored_a.mode, DiscoveryMode::SeededDht);
    assert_eq!(restored_b.mode, DiscoveryMode::SeededDht);
    assert_eq!(restored_a.seed_peers[0].endpoint_id, endpoint_b);
    assert_eq!(restored_b.seed_peers[0].endpoint_id, endpoint_a);
    assert_eq!(restarted_endpoint_a, endpoint_a);
    assert_eq!(restarted_endpoint_b, endpoint_b);
    let topic = "kukuri:topic:runtime-seeded-restart";
    open_topic_column(&restarted_a, topic, TimelineScope::Public)
        .await
        .expect("subscribe restarted a");
    open_topic_column(&restarted_b, topic, TimelineScope::Public)
        .await
        .expect("subscribe restarted b");
    let status_a = restarted_a.get_sync_status().await.expect("sync status a");
    let status_b = restarted_b.get_sync_status().await.expect("sync status b");
    assert_eq!(status_a.discovery.mode, DiscoveryMode::SeededDht);
    assert_eq!(status_b.discovery.mode, DiscoveryMode::SeededDht);
    assert_eq!(status_a.discovery.local_endpoint_id, endpoint_a);
    assert_eq!(status_b.discovery.local_endpoint_id, endpoint_b);
    assert_eq!(
        status_a.discovery.configured_seed_peer_ids,
        vec![endpoint_b]
    );
    assert_eq!(
        status_b.discovery.configured_seed_peer_ids,
        vec![endpoint_a]
    );
    assert!(status_a.subscribed_topics.iter().any(|item| item == topic));
    assert!(status_b.subscribed_topics.iter().any(|item| item == topic));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_seed_entry_rejected_without_mutating_runtime() {
    let _resource = lock_test_resource(TestResource::DhtTestnet).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("invalid-seed.db");
    let testnet = Testnet::new(5).await.expect("testnet");
    let runtime = new_seeded_dht_runtime(&db_path, &testnet).await;

    let error = runtime
        .set_discovery_seeds(SetDiscoverySeedsRequest {
            seed_entries: vec!["not-a-node-id".into()],
        })
        .await
        .expect_err("invalid seed should fail");
    assert!(error.to_string().contains("invalid seed endpoint id"));

    let config = runtime
        .get_discovery_config()
        .await
        .expect("discovery config");
    assert!(config.seed_peers.is_empty());
    assert!(!discovery_config_path(&db_path).exists());
}
