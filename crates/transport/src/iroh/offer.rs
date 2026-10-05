//! One account receive route per transport. The wire contains only a sealed,
//! bounded offer; scope and provider authorization remain with the recipient.

use super::*;
use kukuri_core::receive_route_for_account;

const OFFER_BOOTSTRAP_PER_SOURCE: usize = 4;
const MAX_OUTBOUND_OFFER_HOLDS: usize = 32;
const OUTBOUND_OFFER_HOLD: Duration = Duration::from_secs(30);

#[cfg(test)]
struct CountedOfferTask(Arc<AtomicUsize>);

#[cfg(test)]
impl CountedOfferTask {
    fn new(count: Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}

#[cfg(test)]
impl Drop for CountedOfferTask {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IrohGossipTransport {
    pub(crate) async fn offer_bootstrap_window(&self) -> Vec<EndpointAddr> {
        let mut selected = Vec::with_capacity(3 * OFFER_BOOTSTRAP_PER_SOURCE);
        let mut seen = BTreeSet::new();
        for source in [
            &self.configured_seed_peers,
            &self.bootstrap_seed_peers,
            &self.imported_peers,
        ] {
            let guard = source.lock().await;
            for peer in guard.values().take(OFFER_BOOTSTRAP_PER_SOURCE) {
                if seen.insert(peer.id) {
                    selected.push(peer.clone());
                }
            }
        }
        selected
    }

    pub(super) async fn subscribe_receive_offers_impl(
        &self,
        recipient: &Pubkey,
        expected: Option<ReceiveOfferLease>,
        if_vacant: bool,
    ) -> Result<Option<ReceiveOfferSubscription>> {
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        let route = receive_route_for_account(recipient)?;
        let mut current = self.receive_offer_topic.lock().await;
        if if_vacant && current.is_some() {
            return Ok(None);
        }
        if let Some(expected) = expected
            && !current
                .as_ref()
                .is_some_and(|state| state.route == route.as_str() && state.lease == expected)
        {
            return Ok(None);
        }
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        if let Some(state) = current.as_mut()
            && state.route == route.as_str()
            && !state.closing
            && !state.receiver_task.is_finished()
        {
            let _ = state.stop.send(ReceiveOfferStop::Superseded);
            let (stop, _) = watch::channel(ReceiveOfferStop::Active);
            state.stop = stop;
            state.lease = ReceiveOfferLease::for_instance(self.receive_offer_instance);
            return Ok(Some((
                state.lease,
                stream_from_offer_sender(&state.broadcaster, &state.stop),
                state.stop.subscribe(),
            )));
        }
        if current.is_some() {
            let old = current.as_mut().expect("offer route exists");
            old.closing = true;
            let _ = old.stop.send(ReceiveOfferStop::Superseded);
            old.receiver_task.abort();
            let _ = (&mut old.receiver_task).await;
            current.take();
        }

        let peers = self.offer_bootstrap_window().await;
        for peer in &peers {
            if !peer.is_empty() {
                self.discovery.add_endpoint_info(peer.clone());
            }
        }
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        let topic = self
            .gossip
            .subscribe(
                topics::topic_to_gossip_id(&route),
                peers.into_iter().map(|peer| peer.id).collect(),
            )
            .await?;
        let (sender, mut receiver) = topic.split();
        let (broadcaster, _) = broadcast::channel(64);
        let (stop, _) = watch::channel(ReceiveOfferStop::Active);
        let forward = broadcaster.clone();
        // Registration cannot suspend after the receiver task is spawned.
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        #[cfg(test)]
        let task_guard = CountedOfferTask::new(Arc::clone(&self.offer_receiver_tasks));
        let task = n0_future::task::spawn(async move {
            #[cfg(test)]
            let _task_guard = task_guard;
            while let Some(event) = receiver.next().await {
                let Ok(GossipEvent::Received(message)) = event else {
                    continue;
                };
                if let Ok(offer) = SealedReceiveOfferV1::decode(&message.content) {
                    let _ = forward.send(ReceiveOfferEnvelope {
                        offer,
                        received_at: Utc::now().timestamp_millis(),
                        source_peer: message.delivered_from.to_string(),
                    });
                }
            }
        });
        let lease = ReceiveOfferLease::for_instance(self.receive_offer_instance);
        *current = Some(ReceiveOfferTopicState {
            route: route.as_str().to_string(),
            lease,
            closing: false,
            broadcaster: broadcaster.clone(),
            stop: stop.clone(),
            _sender: sender,
            receiver_task: task,
        });
        Ok(Some((
            lease,
            stream_from_offer_sender(&broadcaster, &stop),
            stop.subscribe(),
        )))
    }

    pub(super) async fn unsubscribe_receive_offers_impl(
        &self,
        recipient: &Pubkey,
        lease: ReceiveOfferLease,
    ) -> Result<()> {
        let route = receive_route_for_account(recipient)?;
        let mut current = self.receive_offer_topic.lock().await;
        if current
            .as_ref()
            .is_some_and(|state| state.route == route.as_str() && state.lease == lease)
        {
            let old = current.as_mut().expect("matching offer route");
            old.closing = true;
            let _ = old.stop.send(ReceiveOfferStop::Closed);
            old.receiver_task.abort();
            let _ = (&mut old.receiver_task).await;
            current.take();
        }
        Ok(())
    }

    pub(super) async fn publish_receive_offer_impl(
        &self,
        recipient: &Pubkey,
        destination: EndpointAddr,
        offer: SealedReceiveOfferV1,
    ) -> Result<()> {
        let payload = offer.encode()?;
        let route = receive_route_for_account(recipient)?;
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        let mut peer_ids = vec![destination.id];
        if !destination.is_empty() {
            self.discovery.add_endpoint_info(destination);
        }
        for peer in self.offer_bootstrap_window().await {
            if !peer_ids.contains(&peer.id) {
                if !peer.is_empty() {
                    self.discovery.add_endpoint_info(peer.clone());
                }
                peer_ids.push(peer.id);
            }
        }
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        let mut topic = self
            .gossip
            .subscribe(topics::topic_to_gossip_id(&route), peer_ids)
            .await?;
        let stopped = self.offer_shutdown_notify.notified();
        tokio::pin!(stopped);
        stopped.as_mut().enable();
        #[cfg(test)]
        self.offer_publish_join_started.notify_one();
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        tokio::select! {
            result = timeout(Duration::from_secs(10), topic.joined()) => {
                result.context("account receive route join timed out")??;
            }
            _ = &mut stopped => anyhow::bail!("account receive offer transport is closed"),
        }
        #[cfg(test)]
        self.offer_publish_joined.notify_one();
        let mut holds = self.outbound_offer_holds.lock().await;
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );
        topic.broadcast(payload.into()).await?;
        anyhow::ensure!(
            !self.offer_closed.load(Ordering::Acquire),
            "account receive offer transport is closed"
        );

        // Keep the sending subscription alive briefly after the gossip actor
        // accepts the message. Eviction and transport shutdown abort every hold.
        let now = n0_future::time::Instant::now();
        #[cfg(test)]
        let task_guard = CountedOfferTask::new(Arc::clone(&self.offer_hold_tasks));
        let task = n0_future::task::spawn(async move {
            #[cfg(test)]
            let _task_guard = task_guard;
            sleep(OUTBOUND_OFFER_HOLD).await;
            drop(topic);
        });
        while holds.front().is_some_and(|hold| hold.expires_at <= now) {
            if let Some(old) = holds.pop_front() {
                old.task.abort();
            }
        }
        holds.push_back(OutboundOfferHold {
            expires_at: now + OUTBOUND_OFFER_HOLD,
            task,
        });
        while holds.len() > MAX_OUTBOUND_OFFER_HOLDS {
            if let Some(old) = holds.pop_front() {
                old.task.abort();
            }
        }
        Ok(())
    }

    pub(super) async fn shutdown_receive_offers(&self) {
        self.offer_closed.store(true, Ordering::Release);
        self.offer_shutdown_notify.notify_waiters();
        let mut current = self.receive_offer_topic.lock().await;
        if current.is_some() {
            let state = current.as_mut().expect("offer route exists");
            state.closing = true;
            let _ = state.stop.send(ReceiveOfferStop::TransportClosed);
            state.receiver_task.abort();
            let _ = (&mut state.receiver_task).await;
            current.take();
        }
        let holds = self
            .outbound_offer_holds
            .lock()
            .await
            .drain(..)
            .collect::<Vec<_>>();
        for hold in &holds {
            hold.task.abort();
        }
        for hold in holds {
            let _ = hold.task.await;
        }
    }
}

fn stream_from_offer_sender(
    sender: &broadcast::Sender<ReceiveOfferEnvelope>,
    stop: &watch::Sender<ReceiveOfferStop>,
) -> ReceiveOfferStream {
    let stream = stream::unfold(
        (sender.subscribe(), stop.subscribe()),
        |(mut receiver, mut stop)| async move {
            loop {
                if *stop.borrow() != ReceiveOfferStop::Active {
                    return None;
                }
                tokio::select! {
                    biased;
                    changed = stop.changed() => {
                        let _ = changed;
                        return None;
                    }
                    event = receiver.recv() => match event {
                        Ok(envelope) => return Some((envelope, (receiver, stop))),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    },
                }
            }
        },
    );
    Box::pin(stream)
}
