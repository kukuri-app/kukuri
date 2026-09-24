use super::*;
use iroh::endpoint::presets;

#[tokio::test]
async fn account_candidate_history_is_indexed_and_survives_book_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.db");
    let endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let store = Arc::new(SqliteStore::connect_file(&path).await.unwrap());
    let book = PeerAddrBook::with_account_store(
        endpoint.clone(),
        Arc::new(MemoryLookup::new()),
        Arc::new(BlobPeerHealth::default()),
        store,
        "blob",
    );
    for index in 0_u64..1_000 {
        let mut key = [0; 32];
        key[..8].copy_from_slice(&index.to_be_bytes());
        book.insert_learned_peer_addr(EndpointAddr::new(
            iroh::SecretKey::from_bytes(&key).public(),
        ))
        .await
        .unwrap();
    }
    book.recent_peers.lock().await.clear();
    let mut seen = BTreeSet::new();
    for _ in 0..250 {
        let selected = book.ranked_peers().await;
        assert_eq!(selected.len(), 4);
        seen.extend(selected.into_iter().map(|peer| peer.id));
    }
    assert_eq!(seen.len(), 1_000, "old retained peers must be revisited");
    assert!(book.sampled_peer_count.load(Ordering::Relaxed) <= 12);
    drop(book);
    let reopened = PeerAddrBook::with_account_store(
        endpoint.clone(),
        Arc::new(MemoryLookup::new()),
        Arc::new(BlobPeerHealth::default()),
        Arc::new(SqliteStore::connect_file(&path).await.unwrap()),
        "blob",
    );
    assert_eq!(reopened.ranked_peers().await.len(), 4);
    drop(reopened);
    endpoint.close().await;
}

#[tokio::test]
async fn successful_peer_is_ranked_before_recently_timed_out_peer() {
    let endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let discovery = Arc::new(MemoryLookup::new());
    let book = PeerAddrBook::new(endpoint, discovery);
    let slow_endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let fast_endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let slow = slow_endpoint.id();
    let fast = fast_endpoint.id();
    book.set_seed_peers(
        vec![
            SeedPeer {
                endpoint_id: slow.to_string(),
                addr_hint: Some("192.0.2.1:4433".into()),
            },
            SeedPeer {
                endpoint_id: fast.to_string(),
                addr_hint: Some("192.0.2.2:4433".into()),
            },
        ],
        &[],
    )
    .await
    .unwrap();

    book.record_fetch_failure(slow, PeerFetchFailure::TransferTimeout)
        .await;
    book.record_fetch_success(fast, Duration::from_millis(12))
        .await;

    let ranked = book.ranked_peers().await;
    assert_eq!(ranked.first().map(|peer| peer.id), Some(fast));
    assert_eq!(ranked.last().map(|peer| peer.id), Some(slow));
}

#[tokio::test]
async fn stale_disconnect_does_not_replace_newer_connection_generation() {
    let endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let discovery = Arc::new(MemoryLookup::new());
    let book = PeerAddrBook::new(endpoint, discovery);
    let peer_endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let peer = peer_endpoint.id();

    book.record_connection_state(peer, 2, PeerConnectionStatus::Connected)
        .await;
    book.record_connection_state(peer, 1, PeerConnectionStatus::Disconnected)
        .await;

    let snapshot = book.peer_state_snapshot(peer).await.unwrap();
    assert_eq!(snapshot.connection_generation, 2);
    assert_eq!(snapshot.connection_status, PeerConnectionStatus::Connected);
}

#[test]
fn stale_connection_observation_expires_to_unknown() {
    let now = Instant::now();
    let state = PeerRuntimeRecord {
        connection_generation: 7,
        connection_status: PeerConnectionStatus::Connected,
        connection_observed_at: Some(now),
        ..PeerRuntimeRecord::default()
    };
    assert_eq!(
        state
            .snapshot(now + PEER_CONNECTION_STATE_TTL + Duration::from_secs(1))
            .connection_status,
        PeerConnectionStatus::Unknown
    );
}

#[tokio::test]
async fn request_frequency_keeps_http_peer_and_relay_subjects_separate() {
    let ledger = RequestRateLedger::default();
    let now = Instant::now();
    let policy = RequestRatePolicy {
        limit: 1,
        window: Duration::from_secs(10),
    };
    assert_eq!(
        ledger
            .check_and_record(
                RequestRateSubject::HttpIp("192.0.2.1".into()),
                RequestRateClass::HttpRequest,
                1,
                policy,
                now,
            )
            .await,
        RequestRateDecision::Allowed
    );
    assert!(matches!(
        ledger
            .check_and_record(
                RequestRateSubject::HttpIp("192.0.2.1".into()),
                RequestRateClass::HttpRequest,
                1,
                policy,
                now,
            )
            .await,
        RequestRateDecision::Limited { .. }
    ));
    assert_eq!(
        ledger
            .check_and_record(
                RequestRateSubject::PeerEndpoint("192.0.2.1".into()),
                RequestRateClass::P2pRequest,
                1,
                policy,
                now,
            )
            .await,
        RequestRateDecision::Allowed
    );
    assert_eq!(
        ledger
            .check_and_record(
                RequestRateSubject::RelayClient("192.0.2.1".into()),
                RequestRateClass::RelayIngressBytes,
                1,
                policy,
                now,
            )
            .await,
        RequestRateDecision::Allowed
    );
}

// かつて docs-sync / blob-service に同名で重複していたテストの単一版(WP-H2)。
// #1207: 実行中の同じ対象は並行させず、先行する走査へ合流させる契約に変えた。
#[test]
fn remote_fetch_retry_state_joins_active_fetches_and_cools_down_failures() {
    let now = Instant::now();
    let mut state = RemoteFetchRetryState::default();

    let RemoteFetchBegin::Lead(_sender) = state.begin("hash-a", "store:hash-a", now) else {
        panic!("first caller must lead");
    };
    assert!(matches!(
        state.begin("hash-a", "store:hash-a", now),
        RemoteFetchBegin::Join(_)
    ));
    // 保存先が違う取得は合流しない。
    let RemoteFetchBegin::Lead(_ephemeral) = state.begin("hash-a", "ephemeral:hash-a", now) else {
        panic!("a different flight key must not join");
    };
    state.finish("hash-a", "ephemeral:hash-a", false, now);

    state.finish("hash-a", "store:hash-a", false, now);
    assert_eq!(state.in_flight_len(), 0);
    assert!(matches!(
        state.begin("hash-a", "store:hash-a", now + Duration::from_secs(1)),
        RemoteFetchBegin::CoolingDown
    ));
    let RemoteFetchBegin::Lead(_retry) =
        state.begin("hash-a", "store:hash-a", now + REMOTE_FETCH_RETRY_COOLDOWN)
    else {
        panic!("cooldown expiry must allow a new attempt");
    };

    state.finish(
        "hash-a",
        "store:hash-a",
        true,
        now + REMOTE_FETCH_RETRY_COOLDOWN,
    );
    assert_eq!(state.cooldown_len(), 0);
}

#[test]
fn remote_fetch_retry_state_does_not_join_an_abandoned_reservation() {
    let now = Instant::now();
    let mut state = RemoteFetchRetryState::default();
    let RemoteFetchBegin::Lead(sender) = state.begin("hash-a", "store:hash-a", now) else {
        panic!("first caller must lead");
    };
    drop(sender);
    assert!(matches!(
        state.begin("hash-a", "store:hash-a", now),
        RemoteFetchBegin::Lead(_)
    ));
}

#[test]
fn remote_fetch_retry_state_prunes_expired_cooldowns() {
    let now = Instant::now();
    let mut state = RemoteFetchRetryState::default();
    for index in 0..4 {
        let key = format!("hash-{index}");
        let _ = state.begin(&key, &key, now);
        state.finish(&key, &key, false, now);
    }
    assert_eq!(state.cooldown_len(), 4);
    let later = now + REMOTE_FETCH_RETRY_COOLDOWN + Duration::from_secs(1);
    let _ = state.begin("hash-z", "hash-z", later);
    state.finish("hash-z", "hash-z", false, later);
    assert_eq!(state.cooldown_len(), 1);
}

#[test]
fn remote_fetch_failure_history_has_a_fixed_capacity() {
    let now = Instant::now();
    let mut state = RemoteFetchRetryState::default();
    for index in 0..10_240 {
        let key = format!("hash-{index:05}");
        state.finish(&key, &key, false, now);
    }
    assert_eq!(state.cooldown_len(), 1_024);
    assert!(state.is_cooling_down("hash-10239", now));
    state.finish("fresh", "fresh", true, now + REMOTE_FETCH_RETRY_COOLDOWN);
    assert_eq!(state.cooldown_len(), 0);
    let oversized = "x".repeat(REMOTE_FETCH_MAX_COOLDOWN_KEY_BYTES + 1);
    state.finish(&oversized, &oversized, false, now);
    assert_eq!(state.cooldown_len(), 0);
    assert!(state.retry_deadlines.is_empty());
}

#[tokio::test]
async fn fetch_candidates_read_a_fixed_window_instead_of_all_peer_history() {
    let endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    for history in [100_u64, 1_000] {
        let book = PeerAddrBook::new(endpoint.clone(), Arc::new(MemoryLookup::new()));
        for index in 0..history {
            let mut secret = [0; 32];
            secret[..8].copy_from_slice(&index.to_be_bytes());
            let peer = iroh::SecretKey::from_bytes(&secret).public();
            book.insert_learned_peer_addr(EndpointAddr::new(peer))
                .await
                .unwrap();
        }
        book.recent_peers.lock().await.clear(); // Historical rows, not fresh hint/import demand.
        let first = book.ranked_peers().await;
        assert!(
            book.sampled_peer_count.load(Ordering::Relaxed) <= 12,
            "must bound materialized candidates before ranking, history={history}"
        );
        assert_eq!(first.len(), 4);
        let second = book.ranked_peers().await;
        assert!(
            second
                .iter()
                .all(|peer| first.iter().all(|old| old.id != peer.id)),
            "cursor must advance"
        );
    }
    endpoint.close().await;
}

#[tokio::test]
async fn bounded_selection_keeps_recent_success_and_new_import_without_expanding_sources() {
    let endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let book = PeerAddrBook::new(endpoint.clone(), Arc::new(MemoryLookup::new()));
    let make_peer = |index: u64| {
        let mut key = [0; 32];
        key[..8].copy_from_slice(&index.to_be_bytes());
        iroh::SecretKey::from_bytes(&key).public()
    };
    for index in 0..100 {
        book.insert_learned_peer_addr(EndpointAddr::new(make_peer(index)))
            .await
            .unwrap();
    }
    book.recent_peers.lock().await.clear();
    let known = book
        .learned_peers
        .lock()
        .await
        .last_key_value()
        .unwrap()
        .1
        .id;
    book.record_fetch_success(known, Duration::from_millis(1))
        .await;
    let foreign = make_peer(9_999);
    book.health.success(foreign, Duration::from_millis(1)).await;
    let fresh = make_peer(10_000);
    book.insert_imported_peer_addr(EndpointAddr::new(fresh))
        .await
        .unwrap();
    let selected = book.ranked_peers().await;
    assert!(selected.iter().any(|peer| peer.id == known));
    assert!(selected.iter().any(|peer| peer.id == fresh));
    assert!(
        selected.iter().all(|peer| peer.id != foreign),
        "shared health cannot add a peer outside this source book"
    );
    assert!(selected.len() <= 4);
    assert!(book.sampled_peer_count.load(Ordering::Relaxed) <= 12);
    endpoint.close().await;
}

#[tokio::test]
async fn new_import_remains_a_candidate_for_follow_up_blob_requests() {
    let endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let book = PeerAddrBook::new(endpoint.clone(), Arc::new(MemoryLookup::new()));
    let make_peer = |index: u64| {
        let mut key = [0; 32];
        key[..8].copy_from_slice(&index.to_be_bytes());
        iroh::SecretKey::from_bytes(&key).public()
    };
    for index in 0..100 {
        book.insert_learned_peer_addr(EndpointAddr::new(make_peer(index)))
            .await
            .unwrap();
    }
    book.recent_peers.lock().await.clear();
    let newly_imported = make_peer(10_000);
    book.insert_imported_peer_addr(EndpointAddr::new(newly_imported))
        .await
        .unwrap();
    for _ in 0..2 {
        let candidates = book.ranked_peers().await;
        assert!(
            candidates.iter().any(|peer| peer.id == newly_imported),
            "successive blob hashes need the same newly imported provider"
        );
    }
    endpoint.close().await;
}

#[tokio::test]
async fn new_import_gets_a_slot_when_older_healthy_peers_fill_the_window() {
    let endpoint = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let book = PeerAddrBook::new(endpoint.clone(), Arc::new(MemoryLookup::new()));
    let make_peer = |index: u64| {
        let mut key = [0; 32];
        key[..8].copy_from_slice(&index.to_be_bytes());
        iroh::SecretKey::from_bytes(&key).public()
    };
    for index in 0..4 {
        let peer = make_peer(index);
        book.insert_learned_peer_addr(EndpointAddr::new(peer))
            .await
            .unwrap();
        book.record_fetch_success(peer, Duration::from_millis(1))
            .await;
    }
    book.recent_peers.lock().await.clear();
    let fresh = make_peer(10_000);
    book.insert_imported_peer_addr(EndpointAddr::new(fresh))
        .await
        .unwrap();
    assert!(
        book.ranked_peers()
            .await
            .iter()
            .any(|peer| peer.id == fresh),
        "a new explicit ticket must be tried alongside earlier successful peers"
    );
    endpoint.close().await;
}
