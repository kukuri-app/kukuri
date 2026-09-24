use super::*;
use crate::receive_binding::{RECEIVE_BINDING_ALPN, ReceiveBindingProtocol};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use kukuri_core::{KukuriKeys, ReceiveEndpointBindingV1};

#[derive(Debug)]
struct StalledDestinationBinding {
    requested: Arc<Notify>,
    closed: Arc<Notify>,
}

fn attempt_all(
    window: &mut DestinationWindow,
    recipient: &Pubkey,
    candidates: Vec<DestinationCandidate>,
) -> Vec<EndpointId> {
    candidates
        .into_iter()
        .map(|(candidate, _, rendezvous_cursor, known_cursor)| {
            window.attempted_candidate(
                recipient,
                candidate.id,
                &[None, None, None],
                rendezvous_cursor,
                known_cursor,
            );
            candidate.id
        })
        .collect()
}

impl ProtocolHandler for StalledDestinationBinding {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let (_send, mut recv) = connection.accept_bi().await?;
        recv.read_to_end(1).await.map_err(AcceptError::from_err)?;
        self.requested.notify_one();
        connection.closed().await;
        self.closed.notify_one();
        Ok(())
    }
}

#[test]
fn destination_window_rotates_through_large_peer_history_in_four_candidate_steps() {
    let mut peers = BTreeMap::new();
    for index in 0..1_000u32 {
        let mut secret = [0u8; 32];
        secret[..4].copy_from_slice(&index.to_le_bytes());
        let endpoint_id = SecretKey::from_bytes(&secret).public();
        peers.insert(endpoint_id.to_string(), EndpointAddr::new(endpoint_id));
    }
    let recipient = Pubkey::from("account-a");
    let mut state = DestinationWindow::default();
    let mut observed = BTreeSet::new();
    for _ in 0..250 {
        let (candidates, _) =
            state.select(&recipient, [&peers, &BTreeMap::new(), &BTreeMap::new()]);
        assert!(candidates.len() <= CANDIDATES_PER_LOOKUP);
        observed.extend(attempt_all(&mut state, &recipient, candidates));
    }
    assert_eq!(observed.len(), 1_000);
}

#[test]
fn invalidation_rejects_stale_lookup_and_state_has_a_fixed_account_cap() {
    let recipient = Pubkey::from("account-a");
    let endpoint_id = SecretKey::from_bytes(&[7; 32]).public();
    let address = EndpointAddr::new(endpoint_id);
    let mut peers = BTreeMap::new();
    peers.insert(endpoint_id.to_string(), address.clone());
    let mut state = DestinationWindow::default();
    let (_, revision) = state.select(&recipient, [&peers, &BTreeMap::new(), &BTreeMap::new()]);
    state.invalidate(&recipient, &endpoint_id.to_string());
    assert!(!state.store_verified(
        &recipient,
        revision,
        address.clone(),
        None,
        i64::MAX,
        Instant::now() + Duration::from_secs(1),
    ));
    for index in 0..MAX_DESTINATION_ACCOUNTS + 1 {
        state.touch(&Pubkey::from(format!("account-{index}")));
    }
    assert_eq!(state.entries.len(), MAX_DESTINATION_ACCOUNTS);
    assert!(!state.entries.contains_key(&recipient));
}

#[test]
fn destination_cursor_reaches_old_peer_during_new_inserts_and_deletes() {
    let recipient = Pubkey::from("account-b");
    let mut peers = BTreeMap::new();
    let mut old = None;
    for index in 0..1_000u32 {
        let mut secret = [0u8; 32];
        secret[..4].copy_from_slice(&index.to_le_bytes());
        let id = SecretKey::from_bytes(&secret).public();
        peers.insert(id.to_string(), EndpointAddr::new(id));
        if index == 999 {
            old = Some(id);
        }
    }
    let target = old.unwrap();
    let mut state = DestinationWindow::default();
    let mut reached = false;
    for index in 1_000..1_350u32 {
        let mut secret = [0u8; 32];
        secret[..4].copy_from_slice(&index.to_le_bytes());
        let id = SecretKey::from_bytes(&secret).public();
        peers.insert(id.to_string(), EndpointAddr::new(id));
        let (candidates, _) =
            state.select(&recipient, [&peers, &BTreeMap::new(), &BTreeMap::new()]);
        reached |= attempt_all(&mut state, &recipient, candidates).contains(&target);
        let first = peers.keys().next().unwrap().clone();
        if first != target.to_string() {
            peers.remove(&first);
        }
    }
    assert!(reached, "an old unprocessed peer must not starve");
}

#[test]
fn rendezvous_window_does_not_starve_known_peer_cursor() {
    let recipient = Pubkey::from("account-rendezvous");
    let mut peers = BTreeMap::new();
    for index in 0..1_000u32 {
        let mut secret = [0u8; 32];
        secret[..4].copy_from_slice(&index.to_le_bytes());
        let id = SecretKey::from_bytes(&secret).public();
        peers.insert(id.to_string(), EndpointAddr::new(id));
    }
    let cn = (1_000..1_008u32)
        .map(|index| {
            let mut secret = [0u8; 32];
            secret[..4].copy_from_slice(&index.to_le_bytes());
            EndpointAddr::new(SecretKey::from_bytes(&secret).public())
        })
        .collect::<Vec<_>>();
    let mut state = DestinationWindow::default();
    state.observe_rendezvous("cn-a", &recipient, cn);
    let mut observed_known = BTreeSet::new();
    for _ in 0..500 {
        let (candidates, _) =
            state.select(&recipient, [&peers, &BTreeMap::new(), &BTreeMap::new()]);
        assert!(candidates.len() <= CANDIDATES_PER_LOOKUP);
        observed_known.extend(
            attempt_all(&mut state, &recipient, candidates)
                .into_iter()
                .filter(|id| peers.contains_key(&id.to_string())),
        );
    }
    assert_eq!(observed_known.len(), 1_000);
}

#[test]
fn destination_window_rotates_across_known_device_sources() {
    let recipient = Pubkey::from("account-multiple-devices");
    let first = EndpointAddr::new(SecretKey::from_bytes(&[51; 32]).public());
    let second = EndpointAddr::new(SecretKey::from_bytes(&[52; 32]).public());
    let gossip = BTreeMap::from([(first.id.to_string(), first.clone())]);
    let docs = BTreeMap::from([(second.id.to_string(), second.clone())]);
    let empty = BTreeMap::new();
    let mut window = DestinationWindow::default();
    let mut first_choices = BTreeSet::new();
    for _ in 0..6 {
        let (candidates, _) =
            window.select(&recipient, [&empty, &empty, &empty, &gossip, &docs, &empty]);
        assert!(candidates.len() <= CANDIDATES_PER_LOOKUP);
        first_choices.insert(candidates[0].0.id);
    }
    assert_eq!(first_choices, BTreeSet::from([first.id, second.id]));
}

#[test]
fn learned_cursor_waits_for_a_probe_with_full_cn_and_known_windows() {
    let recipient = Pubkey::from("account-six-known-devices");
    let address = |seed| EndpointAddr::new(SecretKey::from_bytes(&[seed; 32]).public());
    let mut devices = (1..=6).map(address).collect::<Vec<_>>();
    devices.sort_by_key(|peer| peer.id.to_string());
    let source = |seed| {
        let peer = address(seed);
        BTreeMap::from([(peer.id.to_string(), peer)])
    };
    let configured = source(20);
    let bootstrap = source(21);
    let imported = source(22);
    let docs = source(23);
    let blob = source(24);
    let mut window = DestinationWindow::default();
    window.observe_rendezvous("cn", &recipient, vec![address(25), address(26)]);
    let mut attempted = BTreeSet::new();
    let mut known_first = false;
    for _ in 0..36 {
        let next = window.touch(&recipient).learned_cursors[0]
            .as_ref()
            .and_then(|(_, id)| devices.iter().position(|peer| peer.id.to_string() == *id))
            .map(|index| (index + 1) % devices.len())
            .unwrap_or(0);
        let peer = devices[next].clone();
        let gossip = BTreeMap::from([(peer.id.to_string(), peer.clone())]);
        let (selected, _) = window.select(
            &recipient,
            [&configured, &bootstrap, &imported, &gossip, &docs, &blob],
        );
        assert!(selected.len() <= CANDIDATES_PER_LOOKUP);
        known_first |= selected.first().is_some_and(|(candidate, _, _, _)| {
            candidate.id != address(25).id && candidate.id != address(26).id
        });
        let page = [Some((0, peer.id.to_string())), None, None];
        for (candidate, _, rendezvous_cursor, known_cursor) in selected {
            window.attempted_candidate(
                &recipient,
                candidate.id,
                &page,
                rendezvous_cursor,
                known_cursor,
            );
            if candidate.id == peer.id {
                attempted.insert(candidate.id);
            }
        }
    }
    assert_eq!(attempted.len(), devices.len());
    assert!(
        known_first,
        "known devices must get a first probe despite active CNs"
    );
}

#[test]
fn rendezvous_cursor_reaches_all_devices_when_first_binding_succeeds() {
    let recipient = Pubkey::from("account-cn-eight-devices");
    let peers = (30..38)
        .map(|seed| EndpointAddr::new(SecretKey::from_bytes(&[seed; 32]).public()))
        .collect::<Vec<_>>();
    let mut window = DestinationWindow::default();
    window.observe_rendezvous("cn", &recipient, peers);
    let empty = BTreeMap::new();
    let mut reached = BTreeSet::new();
    for _ in 0..8 {
        let (candidates, _) = window.select(&recipient, [&empty; 6]);
        let (first, _, cursor, known_cursor) = candidates.into_iter().next().unwrap();
        reached.insert(first.id);
        window.attempted_candidate(
            &recipient,
            first.id,
            &[None, None, None],
            cursor,
            known_cursor,
        );
    }
    assert_eq!(reached.len(), 8);
}

#[test]
fn seed_cursor_reaches_all_devices_when_first_binding_succeeds() {
    let recipient = Pubkey::from("account-seed-six-devices");
    let peers = (40..46)
        .map(|seed| EndpointAddr::new(SecretKey::from_bytes(&[seed; 32]).public()))
        .map(|peer| (peer.id.to_string(), peer))
        .collect::<BTreeMap<_, _>>();
    let mut window = DestinationWindow::default();
    let empty = BTreeMap::new();
    let mut reached = BTreeSet::new();
    for _ in 0..6 {
        let (candidates, _) =
            window.select(&recipient, [&peers, &empty, &empty, &empty, &empty, &empty]);
        let (first, _, rendezvous_cursor, known_cursor) = candidates.into_iter().next().unwrap();
        reached.insert(first.id);
        window.attempted_candidate(
            &recipient,
            first.id,
            &[None, None, None],
            rendezvous_cursor,
            known_cursor,
        );
    }
    assert_eq!(reached.len(), 6);
}

#[test]
fn cache_expires_and_invalidating_another_endpoint_preserves_current_binding() {
    let recipient = Pubkey::from("account-c");
    let id = SecretKey::from_bytes(&[8; 32]).public();
    let address = EndpointAddr::new(id);
    let mut state = DestinationWindow::default();
    let (_, revision) = state.select(
        &recipient,
        [&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new()],
    );
    assert!(state.store_verified(
        &recipient,
        revision,
        address.clone(),
        None,
        100,
        Instant::now() + Duration::from_secs(1),
    ));
    state.invalidate(
        &recipient,
        &SecretKey::from_bytes(&[9; 32]).public().to_string(),
    );
    assert_eq!(state.cached(&recipient, 99).unwrap().id, id);
    assert!(state.cached(&recipient, 100).is_none());
    let (_, revision) = state.select(
        &recipient,
        [&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new()],
    );
    assert!(state.store_verified(
        &recipient,
        revision,
        address,
        None,
        i64::MAX,
        Instant::now() - Duration::from_secs(1),
    ));
    assert!(state.cached(&recipient, 99).is_none());
}

#[test]
fn revoking_source_clears_cache_even_after_its_candidate_list_changes() {
    let recipient = Pubkey::from("account-source-revoke");
    let address = EndpointAddr::new(SecretKey::from_bytes(&[18; 32]).public());
    let mut state = DestinationWindow::default();
    state.observe_rendezvous("cn-a", &recipient, vec![address.clone()]);
    let (_, revision) = state.select(
        &recipient,
        [&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new()],
    );
    assert!(state.store_verified(
        &recipient,
        revision,
        address,
        Some("cn-a".into()),
        i64::MAX,
        Instant::now() + Duration::from_secs(10),
    ));
    state.observe_rendezvous("cn-a", &recipient, Vec::new());
    assert!(state.cached(&recipient, 0).is_some());
    state.clear_rendezvous(Some("cn-a"));
    assert!(state.cached(&recipient, 0).is_none());
    assert!(
        !state.store_verified(
            &recipient,
            revision,
            EndpointAddr::new(SecretKey::from_bytes(&[18; 32]).public()),
            Some("cn-a".into()),
            i64::MAX,
            Instant::now() + Duration::from_secs(10),
        ),
        "a probe selected before source revoke cannot repopulate verified cache"
    );
}

#[test]
fn revoking_evicted_source_fences_its_in_flight_binding_probe() {
    let recipient = Pubkey::from("account-evicted-source");
    let address = EndpointAddr::new(SecretKey::from_bytes(&[19; 32]).public());
    let mut state = DestinationWindow::default();
    state.observe_rendezvous("cn-00", &recipient, vec![address.clone()]);
    let (_, old_revision) = state.select(
        &recipient,
        [&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new()],
    );
    for index in 1..=MAX_RENDEZVOUS_SOURCES {
        state.observe_rendezvous(&format!("cn-{index:02}"), &recipient, Vec::new());
    }
    assert!(
        !state.entries[&recipient]
            .rendezvous_sources
            .contains_key("cn-00")
    );
    state.clear_rendezvous(Some("cn-00"));
    assert!(
        !state.store_verified(
            &recipient,
            old_revision,
            address,
            Some("cn-00".into()),
            i64::MAX,
            Instant::now() + Duration::from_secs(10),
        ),
        "source revoke must fence an old probe even after source eviction"
    );
}

#[test]
fn clear_then_invalidate_never_reuses_an_older_probe_revision() {
    let recipient = Pubkey::from("account-revision-aba");
    let address = EndpointAddr::new(SecretKey::from_bytes(&[20; 32]).public());
    let mut state = DestinationWindow::default();
    let _ = state.select(
        &recipient,
        [&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new()],
    );
    state.clear_rendezvous(Some("cn-a"));
    state.clear_rendezvous(Some("cn-a"));
    let (_, old_revision) = state.select(
        &recipient,
        [&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new()],
    );
    state.clear_rendezvous(Some("cn-a"));
    state.invalidate(&recipient, &address.id.to_string());
    assert!(
        !state.store_verified(
            &recipient,
            old_revision,
            address,
            None,
            i64::MAX,
            Instant::now() + Duration::from_secs(10),
        ),
        "clear and invalidate must share a non-reusable revision sequence"
    );
}

#[test]
fn invalidating_old_probe_cannot_replace_newer_verified_endpoint() {
    let recipient = Pubkey::from("account-d");
    let old_id = SecretKey::from_bytes(&[10; 32]).public();
    let new_id = SecretKey::from_bytes(&[11; 32]).public();
    let mut state = DestinationWindow::default();
    let (_, old_revision) = state.select(
        &recipient,
        [&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new()],
    );
    assert!(state.store_verified(
        &recipient,
        old_revision,
        EndpointAddr::new(new_id),
        None,
        i64::MAX,
        Instant::now() + Duration::from_secs(1),
    ));
    state.invalidate(&recipient, &old_id.to_string());
    assert!(!state.store_verified(
        &recipient,
        old_revision,
        EndpointAddr::new(old_id),
        None,
        i64::MAX,
        Instant::now() + Duration::from_secs(1),
    ));
    assert_eq!(state.cached(&recipient, 0).unwrap().id, new_id);
}

#[tokio::test]
async fn shutdown_during_cache_lock_wait_does_not_return_cached_destination() {
    let transport = Arc::new(IrohGossipTransport::bind_local().await.unwrap());
    let recipient = KukuriKeys::generate().public_key();
    let id = SecretKey::from_bytes(&[12; 32]).public();
    let mut state = transport.receive_destinations.lock().await;
    let (_, revision) = state.select(
        &recipient,
        [&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new()],
    );
    assert!(state.store_verified(
        &recipient,
        revision,
        EndpointAddr::new(id),
        None,
        i64::MAX,
        Instant::now() + Duration::from_secs(10),
    ));
    let resolving = Arc::clone(&transport);
    let account = recipient.clone();
    let mut task =
        tokio::spawn(async move { resolving.resolve_receive_destination(&account).await });
    assert!(timeout(Duration::from_millis(20), &mut task).await.is_err());
    transport.shutdown().await;
    drop(state);
    assert!(task.await.unwrap().unwrap().is_none());
    let mut transport = Arc::try_unwrap(transport).ok().unwrap();
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_cancels_active_binding_probe_and_closes_connection() {
    let transport = Arc::new(IrohGossipTransport::bind_local().await.unwrap());
    let receiver = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let requested = Arc::new(Notify::new());
    let closed = Arc::new(Notify::new());
    let router = Router::builder(receiver.clone())
        .accept(
            RECEIVE_BINDING_ALPN,
            StalledDestinationBinding {
                requested: requested.clone(),
                closed: closed.clone(),
            },
        )
        .spawn();
    transport
        .imported_peers
        .lock()
        .await
        .insert(receiver.id().to_string(), receiver.addr());
    let recipient = KukuriKeys::generate().public_key();
    let resolving = Arc::clone(&transport);
    let task = tokio::spawn(async move { resolving.resolve_receive_destination(&recipient).await });
    timeout(Duration::from_secs(5), requested.notified())
        .await
        .unwrap();
    transport.shutdown().await;
    assert!(
        timeout(Duration::from_millis(500), task)
            .await
            .expect("shutdown must cancel lookup before candidate deadline")
            .unwrap()
            .unwrap()
            .is_none()
    );
    timeout(Duration::from_secs(2), closed.notified())
        .await
        .expect("shutdown must close the active binding connection");
    router.shutdown().await.unwrap();
    let mut transport = Arc::try_unwrap(transport).ok().unwrap();
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn seed_destination_requires_live_binding_for_the_exact_account_and_invalidates_cache() {
    let mut transport = IrohGossipTransport::bind_local().await.unwrap();
    let receiver = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let recipient = KukuriKeys::generate();
    let other = KukuriKeys::generate();
    let now = Utc::now().timestamp_millis();
    let binding =
        ReceiveEndpointBindingV1::sign(&recipient, &receiver.id().to_string(), now, now + 60_000)
            .unwrap();
    let handler = ReceiveBindingProtocol::new(receiver.id(), binding).unwrap();
    let router = Router::builder(receiver.clone())
        .accept(RECEIVE_BINDING_ALPN, handler)
        .spawn();
    transport
        .configured_seed_peers
        .lock()
        .await
        .insert(receiver.id().to_string(), receiver.addr());

    assert!(
        transport
            .resolve_receive_destination(&other.public_key())
            .await
            .unwrap()
            .is_none()
    );
    let resolved = transport
        .resolve_receive_destination(&recipient.public_key())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.id, receiver.id());
    transport
        .verify_receive_provider(&recipient.public_key(), receiver.addr())
        .await
        .unwrap();
    assert!(
        transport
            .verify_receive_provider(&other.public_key(), receiver.addr())
            .await
            .is_err()
    );
    assert_eq!(
        transport
            .resolve_receive_destination(&recipient.public_key())
            .await
            .unwrap()
            .unwrap()
            .id,
        receiver.id()
    );

    transport
        .invalidate_receive_destination(&recipient.public_key(), &receiver.id().to_string())
        .await
        .unwrap();
    assert!(
        transport
            .receive_destinations
            .lock()
            .await
            .cached(&recipient.public_key(), Utc::now().timestamp_millis())
            .is_none()
    );
    router.shutdown().await.unwrap();
    transport.shutdown().await;
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn account_ticket_candidate_reaches_live_receive_binding() {
    let store = Arc::new(kukuri_store::SqliteStore::connect_memory().await.unwrap());
    let mut transport = IrohGossipTransport::bind_local()
        .await
        .unwrap()
        .with_account_store(store);
    let receiver = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let recipient = KukuriKeys::generate();
    let now = Utc::now().timestamp_millis();
    let binding =
        ReceiveEndpointBindingV1::sign(&recipient, &receiver.id().to_string(), now, now + 60_000)
            .unwrap();
    let router = Router::builder(receiver.clone())
        .accept(
            RECEIVE_BINDING_ALPN,
            ReceiveBindingProtocol::new(receiver.id(), binding).unwrap(),
        )
        .spawn();
    transport
        .insert_imported_peer_addr(receiver.addr())
        .await
        .unwrap();
    assert!(transport.imported_peers.lock().await.is_empty());
    assert_eq!(
        transport
            .resolve_receive_destination(&recipient.public_key())
            .await
            .unwrap()
            .unwrap()
            .id,
        receiver.id()
    );
    let learned_store = Arc::new(kukuri_store::SqliteStore::connect_memory().await.unwrap());
    let other_device = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let other_binding = ReceiveEndpointBindingV1::sign(
        &recipient,
        &other_device.id().to_string(),
        now,
        now + 60_000,
    )
    .unwrap();
    let other_router = Router::builder(other_device.clone())
        .accept(
            RECEIVE_BINDING_ALPN,
            ReceiveBindingProtocol::new(other_device.id(), other_binding).unwrap(),
        )
        .spawn();
    learned_store
        .put_peer_candidate(
            "docs",
            "learned",
            &receiver.id().to_string(),
            &serde_json::to_vec(&receiver.addr()).unwrap(),
            now,
        )
        .await
        .unwrap();
    learned_store
        .put_peer_candidate(
            "blob",
            "learned",
            &other_device.id().to_string(),
            &serde_json::to_vec(&other_device.addr()).unwrap(),
            now,
        )
        .await
        .unwrap();
    let mut learned_transport = IrohGossipTransport::bind_local()
        .await
        .unwrap()
        .with_account_store(learned_store);
    let mut reached = BTreeSet::new();
    for _ in 0..6 {
        let destination = learned_transport
            .resolve_receive_destination(&recipient.public_key())
            .await
            .unwrap()
            .unwrap();
        reached.insert(destination.id);
        learned_transport
            .invalidate_receive_destination(&recipient.public_key(), &destination.id.to_string())
            .await
            .unwrap();
    }
    assert_eq!(
        reached,
        BTreeSet::from([receiver.id(), other_device.id()]),
        "CN-less known peers must rotate across two live devices"
    );
    learned_transport.shutdown().await;
    learned_transport
        ._router
        .take()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    other_router.shutdown().await.unwrap();
    router.shutdown().await.unwrap();
    transport.shutdown().await;
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn saturated_probe_budget_defers_without_queuing() {
    let mut transport = IrohGossipTransport::bind_local().await.unwrap();
    let permits = transport
        .receive_destination_probes
        .acquire_many(2)
        .await
        .unwrap();
    let recipient = KukuriKeys::generate().public_key();
    assert!(
        transport
            .resolve_receive_destination(&recipient)
            .await
            .unwrap()
            .is_none()
    );
    drop(permits);
    transport.shutdown().await;
    transport._router.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn untrusted_rendezvous_candidate_requires_live_account_binding() {
    let store = Arc::new(kukuri_store::SqliteStore::connect_memory().await.unwrap());
    let mut transport = IrohGossipTransport::bind_local()
        .await
        .unwrap()
        .with_account_store(store.clone());
    let receiver = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let recipient = KukuriKeys::generate();
    assert!(
        SeedPeer {
            endpoint_id: receiver.id().to_string(),
            addr_hint: Some("example.invalid:4242".into()),
        }
        .to_endpoint_addr_with_relay_url_strings(&[])
        .is_err(),
        "untrusted CN address hints must not invoke hostname resolution"
    );
    let now = Utc::now().timestamp_millis();
    let binding =
        ReceiveEndpointBindingV1::sign(&recipient, &receiver.id().to_string(), now, now + 60_000)
            .unwrap();
    let router = Router::builder(receiver.clone())
        .accept(
            RECEIVE_BINDING_ALPN,
            ReceiveBindingProtocol::new(receiver.id(), binding).unwrap(),
        )
        .spawn();
    let fence = transport.receive_candidate_fence().await.unwrap();
    assert!(
        transport
            .offer_receive_candidates(
                "cn-a",
                &recipient.public_key(),
                vec![receiver.addr(); 9],
                fence
            )
            .await
            .is_err()
    );
    transport
        .offer_receive_candidates(
            "cn-a",
            &recipient.public_key(),
            vec![receiver.addr()],
            fence,
        )
        .await
        .unwrap();
    transport
        .offer_receive_candidates("cn-b", &recipient.public_key(), Vec::new(), fence)
        .await
        .unwrap();
    assert!(transport.imported_peers.lock().await.is_empty());
    assert!(
        transport
            .resolve_receive_destination(&KukuriKeys::generate().public_key())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        transport
            .resolve_receive_destination(&recipient.public_key())
            .await
            .unwrap()
            .unwrap()
            .id,
        receiver.id()
    );
    transport
        .clear_receive_candidates(Some("cn-b"))
        .await
        .unwrap();
    assert_eq!(
        transport
            .resolve_receive_destination(&recipient.public_key())
            .await
            .unwrap()
            .unwrap()
            .id,
        receiver.id(),
        "revoking another CN must preserve this CN's verified destination"
    );
    assert!(
        transport
            .offer_receive_candidates(
                "cn-b",
                &recipient.public_key(),
                vec![receiver.addr()],
                fence
            )
            .await
            .is_err(),
        "a response from before CN deactivation must not recreate candidates"
    );
    transport
        .clear_receive_candidates(Some("cn-a"))
        .await
        .unwrap();
    assert!(
        transport
            .resolve_receive_destination(&recipient.public_key())
            .await
            .unwrap()
            .is_none(),
        "CN consent removal must also clear a previously verified destination"
    );
    store
        .put_peer_candidate(
            "gossip",
            "learned",
            &receiver.id().to_string(),
            &serde_json::to_vec(&receiver.addr()).unwrap(),
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        transport
            .resolve_receive_destination(&recipient.public_key())
            .await
            .unwrap()
            .unwrap()
            .id,
        receiver.id(),
        "clearing one CN must preserve an independently known direct peer"
    );
    let mut replacement = IrohGossipTransport::bind_local().await.unwrap();
    assert!(
        replacement
            .offer_receive_candidates(
                "cn-a",
                &recipient.public_key(),
                vec![receiver.addr()],
                fence
            )
            .await
            .is_err(),
        "a candidate response from an old transport cannot enter its replacement"
    );
    replacement.shutdown().await;
    replacement
        ._router
        .take()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    router.shutdown().await.unwrap();
    transport.shutdown().await;
    transport._router.take().unwrap().shutdown().await.unwrap();
}
