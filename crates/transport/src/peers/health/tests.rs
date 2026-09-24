use super::*;

fn peer(index: u64) -> EndpointId {
    let mut key = [0; 32];
    key[..8].copy_from_slice(&index.to_be_bytes());
    iroh::SecretKey::from_bytes(&key).public()
}

#[tokio::test(start_paused = true)]
async fn blob_health_retains_active_attempts_and_bounds_history() {
    let health = Arc::new(BlobPeerHealth::default());
    let active_peer = peer(0);
    let old = health.begin(active_peer).await.unwrap();
    let current = health.begin(active_peer).await.unwrap();
    current.connection(PeerConnectionStatus::Connected).await;
    for index in 1..2_049 {
        let attempt = health.begin(peer(index)).await.unwrap();
        attempt.failure(PeerFetchFailure::NotFound).await;
    }
    assert_eq!(
        health.records.lock().await.entries.len(),
        MAX_BLOB_PEER_RECORDS
    );
    old.failure(PeerFetchFailure::ConnectTimeout).await;
    let state = health.snapshot(active_peer).await.unwrap();
    assert_eq!(state.connection_generation, current.generation());
    assert_eq!(state.connection_status, PeerConnectionStatus::Connected);
    assert_eq!(state.consecutive_fetch_failures, 0);
    assert!(health.snapshot(peer(1)).await.is_none());
    assert_eq!(health.snapshot(peer(2_048)).await.unwrap().fetch_misses, 1);
}

#[tokio::test(start_paused = true)]
async fn blob_rate_cache_never_evicts_a_live_window_to_admit_another_subject() {
    let health = BlobPeerHealth::default();
    for index in 0..MAX_BLOB_PEER_RECORDS as u64 {
        assert_eq!(
            health.record_request(peer(index)).await,
            RequestRateDecision::Allowed
        );
    }
    assert!(matches!(
        health.record_request(peer(2_000)).await,
        RequestRateDecision::Limited { .. }
    ));
    let first = peer(0);
    for _ in 1..PEER_FETCH_REQUEST_LIMIT {
        assert_eq!(
            health.record_request(first).await,
            RequestRateDecision::Allowed
        );
    }
    assert!(matches!(
        health.record_request(first).await,
        RequestRateDecision::Limited { .. }
    ));
    assert_eq!(
        health.rates.lock().await.windows.len(),
        MAX_BLOB_PEER_RECORDS
    );
    tokio::time::advance(PEER_FETCH_REQUEST_WINDOW).await;
    assert_eq!(
        health.record_request(peer(2_000)).await,
        RequestRateDecision::Allowed
    );
    assert_eq!(health.rates.lock().await.windows.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn full_pinned_health_cache_defers_without_growing_or_forgetting_attempts() {
    let health = Arc::new(BlobPeerHealth::default());
    let mut active = Vec::new();
    for index in 0..MAX_BLOB_PEER_RECORDS as u64 {
        active.push(health.begin(peer(index)).await.unwrap());
    }
    assert!(health.begin(peer(5_000)).await.is_none());
    assert_eq!(
        health.records.lock().await.entries.len(),
        MAX_BLOB_PEER_RECORDS
    );
    drop(active.pop());
    let new = health.begin(peer(5_000)).await.unwrap();
    assert_eq!(
        health.records.lock().await.entries.len(),
        MAX_BLOB_PEER_RECORDS
    );
    active[0].success(Duration::from_millis(1)).await;
    assert_eq!(health.snapshot(peer(0)).await.unwrap().fetch_successes, 1);
    drop(new);
}

#[tokio::test]
async fn source_books_keep_change_notifications_but_share_blob_observations() {
    let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .bind()
        .await
        .unwrap();
    let health = Arc::new(BlobPeerHealth::default());
    let first = PeerAddrBook::with_fetch_health(
        endpoint.clone(),
        Arc::new(MemoryLookup::new()),
        health.clone(),
    );
    let second = PeerAddrBook::with_fetch_health(
        endpoint.clone(),
        Arc::new(MemoryLookup::new()),
        health.clone(),
    );
    let target = peer(1);
    assert!(
        first
            .insert_learned_peer_addr(EndpointAddr::new(target))
            .await
            .unwrap()
    );
    assert!(
        second
            .insert_learned_peer_addr(EndpointAddr::new(target))
            .await
            .unwrap()
    );
    let attempt = first.begin_fetch_attempt(target).await.unwrap();
    attempt.success(Duration::from_millis(1)).await;
    assert_eq!(
        second
            .peer_state_snapshot(target)
            .await
            .unwrap()
            .fetch_successes,
        1
    );
    for _ in 0..8 {
        assert_eq!(
            first.record_peer_fetch_request(target).await,
            RequestRateDecision::Allowed
        );
        assert_eq!(
            second.record_peer_fetch_request(target).await,
            RequestRateDecision::Allowed
        );
    }
    assert!(matches!(
        first.record_peer_fetch_request(target).await,
        RequestRateDecision::Limited { .. }
    ));
    endpoint.close().await;
}
