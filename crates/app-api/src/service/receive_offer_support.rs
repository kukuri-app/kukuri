use std::sync::atomic::Ordering;
use std::time::Duration;

use kukuri_core::{DirectMessageAckV1, ReceiveOfferScopeV1};
use kukuri_transport::{EndpointAddr, ReceiveOfferEnvelope, ReceiveOfferStop};

use super::direct_messages_delivery_support::DirectMessageHintServices;
use super::*;

const RECEIVE_OFFER_RESTART_DELAY: Duration = Duration::from_secs(3);
const RECEIVE_OFFER_MAX_IN_FLIGHT: usize = 4;

impl AppService {
    async fn unsubscribe_account_receive_offer_lease(&self, recipient: &Pubkey) -> Result<()> {
        let lease = *self
            .subscription_registry
            .account_receive_offer_lease
            .lock()
            .expect("account receive lease poisoned");
        if let Some(lease) = lease {
            self.services
                .hint_transport
                .unsubscribe_receive_offers(recipient, lease)
                .await?;
            let mut current = self
                .subscription_registry
                .account_receive_offer_lease
                .lock()
                .expect("account receive lease poisoned");
            if *current == Some(lease) {
                *current = None;
            }
        }
        Ok(())
    }

    pub(crate) async fn shutdown_account_receive_offers(&self) {
        self.subscription_registry
            .account_receive_offer_closed
            .store(true, Ordering::Release);
        self.subscription_registry
            .account_receive_offer_shutdown
            .notify_waiters();
        let task = self
            .subscription_registry
            .account_receive_offer_task
            .lock()
            .await
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = tokio::time::timeout(Duration::from_secs(2), task.wait()).await;
        }
        let public_queue = std::mem::take(
            &mut *self
                .subscription_registry
                .public_notification_offer_queue
                .lock()
                .await,
        );
        if let Some(queue) = public_queue {
            queue.task.abort();
            let _ = tokio::time::timeout(Duration::from_secs(2), queue.task.wait()).await;
        }
        if let Err(error) = self
            .unsubscribe_account_receive_offer_lease(&self.services.keys.public_key())
            .await
        {
            warn!(%error, "account receive route unsubscribe deferred");
        }
    }

    /// Keep one account route alive while the account runtime is active. The
    /// legacy topic receivers continue serving scopes not yet migrated here.
    pub async fn start_account_receive_offers(&self) -> Result<()> {
        let closed = &self.subscription_registry.account_receive_offer_closed;
        let shutdown = self
            .subscription_registry
            .account_receive_offer_shutdown
            .notified();
        tokio::pin!(shutdown);
        shutdown.as_mut().enable();
        anyhow::ensure!(
            !closed.load(Ordering::Acquire),
            "account receive route is closed"
        );
        let mut owner = self
            .subscription_registry
            .account_receive_offer_task
            .lock()
            .await;
        if owner.as_ref().is_some_and(|task| !task.is_finished()) {
            return Ok(());
        }
        let recipient = self.services.keys.public_key();
        if let Some(old) = owner.take() {
            old.abort();
            old.wait().await;
        }
        tokio::select! {
            biased;
            _ = &mut shutdown => anyhow::bail!("account receive route is closed"),
            result = self.unsubscribe_account_receive_offer_lease(&recipient) => result?,
        }
        let (lease, stream, stop) = tokio::select! {
            biased;
            _ = &mut shutdown => anyhow::bail!("account receive route is closed"),
            result = self.services.hint_transport.subscribe_receive_offers(&recipient) => result?,
        };
        *self
            .subscription_registry
            .account_receive_offer_lease
            .lock()
            .expect("account receive lease poisoned") = Some(lease);
        if closed.load(Ordering::Acquire) {
            anyhow::bail!("account receive route is closed");
        }
        let services = self.services.clone();
        let closed = Arc::clone(closed);
        let lease_slot = Arc::clone(&self.subscription_registry.account_receive_offer_lease);
        let last_sync = Arc::clone(&self.last_sync_ts);
        let notification_inserted = Arc::clone(&self.notification_inserted_notify);
        *owner = Some(AbortOnDropTask::new(tokio::spawn(async move {
            let mut stream = Some((stream, stop));
            let mut active_lease = lease;
            loop {
                let mut stop_reason = ReceiveOfferStop::Active;
                if let Some((active_stream, stop)) = stream.take() {
                    let active_services = services.clone();
                    let active_last_sync = Arc::clone(&last_sync);
                    let active_notification = Arc::clone(&notification_inserted);
                    let observed_stop = stop.clone();
                    stop_reason = tokio::select! {
                        biased;
                        reason = wait_receive_offer_stop(stop) => reason,
                        _ = active_stream.for_each_concurrent(RECEIVE_OFFER_MAX_IN_FLIGHT, move |envelope| {
                                let services = active_services.clone();
                                let last_sync = Arc::clone(&active_last_sync);
                                let notification_inserted = Arc::clone(&active_notification);
                                async move {
                                    match Self::ingest_account_receive_offer(&services, envelope).await {
                                        Ok(true) => {
                                            *last_sync.lock().await = Some(Utc::now().timestamp_millis());
                                            notification_inserted.notify_waiters();
                                        }
                                        Ok(false) => {}
                                        Err(error) => {
                                            tracing::debug!(%error, "account receive offer was not applied");
                                        }
                                    }
                                }
                            }) => *observed_stop.borrow(),
                    };
                }
                if matches!(
                    stop_reason,
                    ReceiveOfferStop::Superseded | ReceiveOfferStop::Closed
                ) {
                    return;
                }
                if closed.load(Ordering::Acquire) {
                    return;
                }
                tokio::time::sleep(RECEIVE_OFFER_RESTART_DELAY).await;
                if closed.load(Ordering::Acquire) {
                    return;
                }
                match services
                    .hint_transport
                    .resubscribe_receive_offers_if_current(&recipient, active_lease)
                    .await
                {
                    Ok(Some((next_lease, next, next_stop))) => {
                        *lease_slot.lock().expect("account receive lease poisoned") =
                            Some(next_lease);
                        active_lease = next_lease;
                        stream = Some((next, next_stop));
                    }
                    Ok(None) => {
                        match services
                            .hint_transport
                            .receive_offer_transport_instance()
                            .await
                        {
                            Ok(current_instance)
                                if current_instance != active_lease.transport_instance() =>
                            {
                                match services
                                    .hint_transport
                                    .subscribe_receive_offers_if_vacant(&recipient)
                                    .await
                                {
                                    Ok(Some((next_lease, next, next_stop))) => {
                                        *lease_slot
                                            .lock()
                                            .expect("account receive lease poisoned") =
                                            Some(next_lease);
                                        active_lease = next_lease;
                                        stream = Some((next, next_stop));
                                    }
                                    Ok(None) => return,
                                    Err(error) => {
                                        tracing::debug!(%error, "account receive route rebuild retry deferred");
                                    }
                                }
                            }
                            Ok(_) => return,
                            Err(error) => {
                                tracing::debug!(%error, "account receive transport identity unavailable");
                            }
                        }
                    }
                    Err(error) => {
                        tracing::debug!(%error, "account receive route retry deferred");
                    }
                }
            }
        })));
        Ok(())
    }

    pub(crate) async fn ingest_account_receive_offer(
        services: &ServiceHandles,
        envelope: ReceiveOfferEnvelope,
    ) -> Result<bool> {
        let verified = envelope
            .offer
            .open(services.keys.as_ref(), Utc::now().timestamp_millis())?;
        let local = services.keys.public_key_hex();
        let sender = verified.sender().as_str();
        let topic = derive_direct_message_topic(services.keys.as_ref(), verified.sender())?;
        match &verified.reference().scope {
            ReceiveOfferScopeV1::PublicSource => {
                return Self::ingest_public_notification_offer(services, &verified).await;
            }
            ReceiveOfferScopeV1::PrivateSource { epoch_key_id } => {
                return Self::ingest_private_notification_offer(services, &verified, epoch_key_id)
                    .await;
            }
            ReceiveOfferScopeV1::DirectMessageAck {
                dm_id,
                message_id,
                acked_at,
                signature,
            } => {
                if !receive_offer_dm_is_mutual(services, local.as_str(), sender).await?
                    || Utc::now().timestamp_millis() >= verified.expires_at_ms()
                {
                    return Ok(false);
                }
                let hint = GossipHint::DirectMessageAck {
                    topic_id: topic.clone(),
                    ack: DirectMessageAckV1 {
                        dm_id: dm_id.clone(),
                        message_id: message_id.clone(),
                        sender: verified.sender().clone(),
                        recipient: verified.recipient().clone(),
                        acked_at: *acked_at,
                        signature: signature.clone(),
                    },
                };
                return Self::handle_direct_message_hint(
                    DirectMessageHintServices {
                        services,
                        local_author_pubkey: local.as_str(),
                        peer_pubkey: sender,
                        topic: &topic,
                        ack_destination: None,
                    },
                    &hint,
                )
                .await;
            }
            ReceiveOfferScopeV1::DirectMessageFrame {
                dm_id,
                message_id,
                frame_hash,
            } => {
                if !receive_offer_dm_is_mutual(services, local.as_str(), sender).await? {
                    return Ok(false);
                }
                let provider =
                    EndpointAddr::new(verified.reference().provider_endpoint_id.parse()?);
                services
                    .hint_transport
                    .verify_receive_provider(verified.sender(), provider.clone())
                    .await?;
                if Utc::now().timestamp_millis() >= verified.expires_at_ms()
                    || !receive_offer_dm_is_mutual(services, local.as_str(), sender).await?
                {
                    return Ok(false);
                }
                services
                    .blob_service
                    .learn_peer(&verified.reference().provider_endpoint_id)
                    .await?;
                if Utc::now().timestamp_millis() >= verified.expires_at_ms()
                    || !receive_offer_dm_is_mutual(services, local.as_str(), sender).await?
                {
                    return Ok(false);
                }
                let hint = GossipHint::DirectMessageFrame {
                    topic_id: topic.clone(),
                    dm_id: dm_id.clone(),
                    message_id: message_id.clone(),
                    frame_hash: frame_hash.clone(),
                };
                return Self::handle_direct_message_hint(
                    DirectMessageHintServices {
                        services,
                        local_author_pubkey: local.as_str(),
                        peer_pubkey: sender,
                        topic: &topic,
                        ack_destination: Some(provider),
                    },
                    &hint,
                )
                .await;
            }
            ReceiveOfferScopeV1::DirectMessage => {}
            _ => return Ok(false),
        }
        if !receive_offer_dm_is_mutual(services, local.as_str(), sender).await? {
            return Ok(false);
        }
        let provider = EndpointAddr::new(verified.reference().provider_endpoint_id.parse()?);
        let payload = services
            .blob_service
            .fetch_verified_receive_offer_payload(&verified, provider.clone())
            .await?;
        if Utc::now().timestamp_millis() >= verified.expires_at_ms()
            || !receive_offer_dm_is_mutual(services, local.as_str(), sender).await?
        {
            return Ok(false);
        }
        services
            .blob_service
            .learn_peer(&verified.reference().provider_endpoint_id)
            .await?;
        let hint: GossipHint =
            serde_json::from_slice(&payload).context("invalid direct message receive manifest")?;
        if !matches!(
            &hint,
            GossipHint::DirectMessageFrame { topic_id, .. }
                | GossipHint::DirectMessageAck { topic_id, .. }
                if topic_id == &topic
        ) {
            return Ok(false);
        }
        let ack_destination =
            matches!(&hint, GossipHint::DirectMessageFrame { .. }).then_some(provider);
        if !receive_offer_dm_is_mutual(services, local.as_str(), sender).await? {
            return Ok(false);
        }
        Self::handle_direct_message_hint(
            DirectMessageHintServices {
                services,
                local_author_pubkey: local.as_str(),
                peer_pubkey: sender,
                topic: &topic,
                ack_destination,
            },
            &hint,
        )
        .await
    }
}

async fn wait_receive_offer_stop(
    mut stop: tokio::sync::watch::Receiver<ReceiveOfferStop>,
) -> ReceiveOfferStop {
    if *stop.borrow() != ReceiveOfferStop::Active {
        return *stop.borrow();
    }
    while stop.changed().await.is_ok() {
        if *stop.borrow() != ReceiveOfferStop::Active {
            return *stop.borrow();
        }
    }
    ReceiveOfferStop::TransportClosed
}

async fn receive_offer_dm_is_mutual(
    services: &ServiceHandles,
    local: &str,
    sender: &str,
) -> Result<bool> {
    Ok(services
        .projection_store
        .get_author_relationship(local, sender)
        .await?
        .as_ref()
        .is_some_and(|relationship| relationship.mutual))
}
