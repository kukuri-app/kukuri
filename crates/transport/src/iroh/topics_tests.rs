use super::*;

#[test]
fn topic_reader_rotates_four_peers_without_using_another_scope() {
    let neighbors = (0..6).map(|i| format!("peer-{i}")).collect::<BTreeSet<_>>();
    let mut cursor = None;
    let first = topic_read_window(&neighbors, &mut cursor);
    let second = topic_read_window(&neighbors, &mut cursor);
    assert_eq!(first, vec!["peer-0", "peer-1", "peer-2", "peer-3"]);
    assert_eq!(second, vec!["peer-4", "peer-5", "peer-0", "peer-1"]);
    assert_eq!(
        topic_read_window(&BTreeSet::new(), &mut cursor),
        Vec::<String>::new()
    );
}

struct NotifyOnDrop(Arc<Notify>);

impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[tokio::test]
async fn unsubscribing_during_initial_join_stops_its_warmup_task() {
    let mut transport = IrohGossipTransport::bind_local().await.unwrap();
    let peer = iroh::SecretKey::from_bytes(&[47; 32]).public();
    transport
        .insert_imported_peer_addr(EndpointAddr::new(peer))
        .await
        .unwrap();
    let topic = TopicId::new("kukuri:topic:owned-initial-warmup");
    let _stream = transport.subscribe_hints(&topic).await.unwrap();
    timeout(Duration::from_secs(2), async {
        while transport
            .topic_warmups
            .initial_warmup_tasks
            .load(Ordering::SeqCst)
            == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("initial warmup task must start");
    transport.unsubscribe_hints(&topic).await.unwrap();
    timeout(Duration::from_millis(300), async {
        while transport
            .topic_warmups
            .initial_warmup_tasks
            .load(Ordering::SeqCst)
            != 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("unsubscription must stop the initial warmup task");
    transport.shutdown().await;
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn unsubscribing_stops_a_registered_peer_update_warmup() {
    let mut transport = IrohGossipTransport::bind_local().await.unwrap();
    let topic = TopicId::new("kukuri:topic:owned-update-warmup");
    let _stream = transport.subscribe_hints(&topic).await.unwrap();
    let dropped = Arc::new(Notify::new());
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let update = tokio::spawn({
        let dropped = Arc::clone(&dropped);
        async move {
            let _guard = NotifyOnDrop(dropped);
            let _ = ready_tx.send(());
            std::future::pending::<()>().await;
        }
    });
    ready_rx.await.unwrap();
    let hint_topic = kukuri_core::wire::hint_topic_id(&topic);
    transport
        .topic_states
        .lock()
        .await
        .get_mut(hint_topic.as_str())
        .unwrap()
        .update_warmup_task = Some(update);
    transport.unsubscribe_hints(&topic).await.unwrap();
    timeout(Duration::from_secs(2), dropped.notified())
        .await
        .expect("removing the topic must abort its update warmup");
    transport.shutdown().await;
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn hint_subscribe_waiting_for_registration_cannot_revive_after_shutdown() {
    let transport = Arc::new(IrohGossipTransport::bind_local().await.unwrap());
    let topic = TopicId::new("kukuri:topic:hint-shutdown-registration");
    let registration_guard = transport.subscribed_topics.lock().await;
    let subscribe = tokio::spawn({
        let transport = Arc::clone(&transport);
        async move { transport.subscribe_hints(&topic).await }
    });
    timeout(Duration::from_secs(2), async {
        while transport.topic_states.try_lock().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let shutdown = tokio::spawn({
        let transport = Arc::clone(&transport);
        async move { transport.shutdown().await }
    });
    timeout(Duration::from_secs(2), async {
        while !transport.hint_closed.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(registration_guard);
    assert!(subscribe.await.unwrap().is_err());
    shutdown.await.unwrap();
    assert!(transport.topic_states.lock().await.is_empty());
    let mut transport = Arc::try_unwrap(transport).ok().unwrap();
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn stale_rejoin_decision_cannot_remove_a_new_topic_generation() {
    let transport = Arc::new(IrohGossipTransport::bind_local().await.unwrap());
    let topic = TopicId::new("kukuri:topic:hint-stale-rejoin");
    let _first = transport.subscribe_hints(&topic).await.unwrap();
    let hint_topic = kukuri_core::wire::hint_topic_id(&topic);
    let old_error = {
        let states = transport.topic_states.lock().await;
        let state = states.get(hint_topic.as_str()).unwrap();
        *state.last_error.lock().await =
            Some("timed out waiting for initial topic join".to_string());
        Arc::clone(&state.last_error)
    };
    let held_error = old_error.lock().await;
    let stale = tokio::spawn({
        let transport = Arc::clone(&transport);
        let topic = topic.clone();
        async move { transport.subscribe_hints(&topic).await }
    });
    timeout(
        Duration::from_secs(2),
        transport.hint_existing_snapshot_observed.notified(),
    )
    .await
    .expect("stale caller must snapshot the old generation");
    transport.unsubscribe_hints(&topic).await.unwrap();
    let _new_stream = transport.subscribe_hints(&topic).await.unwrap();
    let new_generation = {
        let states = transport.topic_states.lock().await;
        Arc::clone(&states.get(hint_topic.as_str()).unwrap().closed)
    };
    drop(held_error);
    let _stale_stream = stale.await.unwrap().unwrap();
    let states = transport.topic_states.lock().await;
    assert!(
        Arc::ptr_eq(
            &states.get(hint_topic.as_str()).unwrap().closed,
            &new_generation
        ),
        "an old timeout decision must not remove a replacement receiver"
    );
    drop(states);
    transport.shutdown().await;
    let mut transport = Arc::try_unwrap(transport).ok().unwrap();
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_hint_shutdown_aborts_all_topic_tasks_before_waiting() {
    let transport = Arc::new(IrohGossipTransport::bind_local().await.unwrap());
    let first_topic = TopicId::new("kukuri:topic:a-blocked-hint-shutdown");
    let second_topic = TopicId::new("kukuri:topic:b-pending-hint-shutdown");
    let _first_stream = transport.subscribe_hints(&first_topic).await.unwrap();
    let _second_stream = transport.subscribe_hints(&second_topic).await.unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let blocked = tokio::task::spawn_blocking(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let dropped = Arc::new(Notify::new());
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let pending = tokio::spawn({
        let dropped = Arc::clone(&dropped);
        async move {
            let _guard = NotifyOnDrop(dropped);
            let _ = ready_tx.send(());
            std::future::pending::<()>().await;
        }
    });
    ready_rx.await.unwrap();
    let first_key = kukuri_core::wire::hint_topic_id(&first_topic);
    let second_key = kukuri_core::wire::hint_topic_id(&second_topic);
    {
        let mut states = transport.topic_states.lock().await;
        let first = states.get_mut(first_key.as_str()).unwrap();
        let old = std::mem::replace(&mut first._receiver_task, blocked);
        old.abort();
        let second = states.get_mut(second_key.as_str()).unwrap();
        let old = std::mem::replace(&mut second._receiver_task, pending);
        old.abort();
    }
    let shutdown = tokio::spawn({
        let transport = Arc::clone(&transport);
        async move { transport.shutdown().await }
    });
    timeout(Duration::from_secs(2), async {
        while !transport.topic_states.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    shutdown.abort();
    let _ = shutdown.await;
    release_tx.send(()).unwrap();
    timeout(Duration::from_secs(2), dropped.notified())
        .await
        .expect("all topic tasks must be aborted before shutdown awaits one");
    transport.shutdown().await;
    let mut transport = Arc::try_unwrap(transport).ok().unwrap();
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[test]
fn warmup_samples_a_moving_four_peer_window_from_large_history() {
    for history in [100_usize, 1_000] {
        let peers = (0..history)
            .map(|index| {
                let mut secret = [0_u8; 32];
                secret[..8].copy_from_slice(&(index as u64 + 1).to_be_bytes());
                EndpointAddr::new(iroh::SecretKey::from_bytes(&secret).public())
            })
            .collect::<Vec<_>>();
        let coordinator = TopicWarmupCoordinator::default();
        let first = coordinator.warmup_window(&peers);
        let second = coordinator.warmup_window(&peers);
        assert_eq!(first.len(), 4, "history={history}");
        assert_eq!(second.len(), 4, "history={history}");
        assert_eq!(first[0].id, peers[0].id);
        assert_eq!(second[0].id, peers[4].id);
        assert_eq!(second[3].id, peers[7].id);
    }
}

#[tokio::test]
async fn warmup_does_not_queue_peer_state_when_shared_dial_slots_are_full() {
    let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
    let gossip = Gossip::builder().spawn(endpoint.clone());
    let coordinator = TopicWarmupCoordinator::default();
    let first = coordinator.permits.try_acquire().unwrap();
    let second = coordinator.permits.try_acquire().unwrap();
    let peer = iroh::SecretKey::from_bytes(&[3; 32]).public();
    timeout(
        Duration::from_millis(100),
        coordinator.warmup_peer(endpoint.clone(), gossip.clone(), EndpointAddr::new(peer)),
    )
    .await
    .expect("full dial capacity must defer without a waiting future");
    assert!(coordinator.in_flight_peers.read().unwrap().is_empty());
    drop(first);
    drop(second);
    let _ = gossip.shutdown().await;
    endpoint.close().await;
}

#[tokio::test]
async fn active_other_protocol_does_not_suppress_gossip_warmup() {
    const OTHER_ALPN: &[u8] = b"kukuri-test-other-protocol";
    let local = Endpoint::bind(presets::Minimal)
        .await
        .expect("local endpoint");
    let remote = Endpoint::builder(presets::Minimal)
        .alpns(vec![OTHER_ALPN.to_vec(), GOSSIP_ALPN.to_vec()])
        .bind()
        .await
        .expect("remote endpoint");
    let (outbound, inbound) = timeout(Duration::from_secs(5), async {
        tokio::join!(local.connect(remote.addr(), OTHER_ALPN), async {
            remote.accept().await.expect("other incoming").await
        })
    })
    .await
    .expect("other protocol connection deadline");
    let outbound = outbound.expect("other outbound");
    let inbound = inbound.expect("other inbound");
    assert!(local.remote_info(remote.id()).await.is_some_and(|info| {
        info.addrs()
            .any(|addr| matches!(addr.usage(), TransportAddrUsage::Active))
    }));
    assert_eq!(outbound.alpn(), OTHER_ALPN);
    assert_eq!(inbound.alpn(), OTHER_ALPN);

    let gossip = Gossip::builder().spawn(local.clone());
    let warmup = tokio::spawn({
        let local = local.clone();
        let gossip = gossip.clone();
        let peer = remote.addr();
        async move {
            TopicWarmupCoordinator::default()
                .warmup_peer(local, gossip, peer)
                .await;
        }
    });
    let incoming = timeout(Duration::from_secs(3), async {
        remote.accept().await.expect("gossip incoming").await
    })
    .await;
    warmup.abort();
    let _ = warmup.await;
    gossip.shutdown().await.expect("gossip shutdown");
    local.close().await;
    remote.close().await;
    let connection = incoming
        .expect("an active other protocol must not suppress gossip dialing")
        .expect("gossip connection");
    assert_eq!(connection.alpn(), GOSSIP_ALPN);
}

// gossip topic id 派生の golden(WP-S3 T4)。blake3(topic 文字列)が
// on-wire の gossip 識別子そのもの。変更はネットワーク分断になる。
#[test]
fn topic_to_gossip_id_matches_golden() {
    let id = topic_to_gossip_id(&kukuri_core::TopicId::new("kukuri:topic:golden"));
    let expected =
        blake3::Hash::from_hex("65994b46e778ead0264f20707efc571bfc4bdf510f97add9ebd81c8ad507dc80")
            .expect("expected hex parses");
    assert_eq!(id.as_bytes(), expected.as_bytes());
}

#[test]
fn topic_warmup_retry_delay_backs_off_and_caps() {
    assert_eq!(
        topic_warmup_retry_delay(0, false),
        Duration::from_millis(250)
    );
    assert_eq!(
        topic_warmup_retry_delay(1, false),
        Duration::from_millis(500)
    );
    assert_eq!(topic_warmup_retry_delay(2, false), Duration::from_secs(1));
    assert_eq!(topic_warmup_retry_delay(3, false), Duration::from_secs(2));
    assert_eq!(topic_warmup_retry_delay(4, false), Duration::from_secs(5));
    assert_eq!(topic_warmup_retry_delay(12, false), Duration::from_secs(5));
}

#[test]
fn relay_topic_warmup_retry_delay_backs_off_more_slowly() {
    assert_eq!(topic_warmup_retry_delay(0, true), Duration::from_secs(1));
    assert_eq!(topic_warmup_retry_delay(1, true), Duration::from_secs(2));
    assert_eq!(topic_warmup_retry_delay(2, true), Duration::from_secs(4));
    assert_eq!(topic_warmup_retry_delay(3, true), Duration::from_secs(8));
    assert_eq!(topic_warmup_retry_delay(4, true), Duration::from_secs(10));
    assert_eq!(topic_warmup_retry_delay(12, true), Duration::from_secs(10));
}

#[test]
fn warmup_coordinator_coalesces_same_peer_dials() {
    let coordinator = TopicWarmupCoordinator::default();

    assert!(coordinator.try_mark_peer_in_flight_for_test("peer"));
    assert!(!coordinator.try_mark_peer_in_flight_for_test("peer"));

    coordinator.clear_in_flight_for_test("peer");
    assert!(coordinator.try_mark_peer_in_flight_for_test("peer"));
}

#[test]
fn warmup_in_flight_guard_clears_on_drop() {
    let coordinator = TopicWarmupCoordinator::default();
    let guard = coordinator
        .try_mark_peer_in_flight("peer".to_string())
        .expect("first warmup marks peer");

    assert!(!coordinator.try_mark_peer_in_flight_for_test("peer"));
    drop(guard);
    assert!(coordinator.try_mark_peer_in_flight_for_test("peer"));
}
