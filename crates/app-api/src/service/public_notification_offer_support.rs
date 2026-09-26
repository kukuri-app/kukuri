use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use kukuri_core::{
    PrivateReceivePayloadV1, RECEIVE_PAYLOAD_MAX_BYTES, ReceiveOfferReferenceV1,
    ReceiveOfferScopeV1, VerifiedReceiveOffer, receive_epoch_key_id, seal_private_receive_payload,
    seal_receive_offer,
};
use kukuri_transport::EndpointAddr;
use serde::Deserialize;

use super::notifications_support::{
    notification_candidate_from_verified_follow, notification_candidate_from_verified_post,
    pubkey_mentions,
};
use super::post_integrity::VerifiedPost;
use super::subscription_registry::PublicNotificationOfferQueue;
use super::*;

const PUBLIC_OFFER_QUEUE_CAPACITY: usize = 64;
const PUBLIC_OFFER_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PublicNotificationSource {
    Post {
        replica: ReplicaId,
        envelope: KukuriEnvelope,
        content: String,
        reply_target: Option<KukuriEnvelope>,
    },
    Follow {
        envelope: KukuriEnvelope,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicNotificationManifest {
    version: u8,
    source: PublicNotificationSource,
}

pub(crate) fn encode_public_notification_manifest(
    source: PublicNotificationSource,
) -> Result<Vec<u8>> {
    let payload = serde_json::to_vec(&PublicNotificationManifest { version: 1, source })?;
    anyhow::ensure!(
        payload.len() <= RECEIVE_PAYLOAD_MAX_BYTES,
        "public notification manifest too large"
    );
    Ok(payload)
}

/// private の通知の参照を、投稿した epoch の鍵で暗号化する(#1221 R4-C)。offer の外側を復号できても、
/// epoch の鍵が無ければ投稿・本文・channel を読めない。
fn private_notification_payload(
    state: &JoinedPrivateChannelState,
    manifest: &[u8],
) -> Result<(Vec<u8>, ReceiveOfferScopeV1)> {
    let mut secret = [0_u8; 32];
    hex::decode_to_slice(&state.current_epoch_secret_hex, &mut secret)?;
    let sealed = seal_private_receive_payload(
        &secret,
        state.channel_id.as_str(),
        &state.current_epoch_id,
        manifest,
    )?;
    let scope = ReceiveOfferScopeV1::PrivateSource {
        epoch_key_id: sealed.epoch_key_id.clone(),
    };
    Ok((sealed.encode()?, scope))
}

impl AppService {
    /// 投稿・返信の明示された相手(mention、返信先の著者)へ通知の参照を送る。private は投稿した epoch の鍵で
    /// 暗号化した参照を `PrivateSource` の offer で送る(#1221 R4-C)。
    pub(crate) async fn queue_post_offer(
        &self,
        replica: &ReplicaId,
        envelope: &KukuriEnvelope,
        content: &str,
        parent: Option<&KukuriEnvelope>,
        private: Option<&JoinedPrivateChannelState>,
    ) {
        let reply_target = match parent {
            Some(parent) => self
                .resolve_signed_post_envelope(&parent.id)
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let recipients = public_post_notification_recipients(content, reply_target.as_ref(), None);
        let source = PublicNotificationSource::Post {
            replica: replica.clone(),
            envelope: envelope.clone(),
            content: content.to_string(),
            reply_target,
        };
        let Some(state) = private else {
            self.queue_public_notification_offer(source, recipients)
                .await;
            return;
        };
        let payload = encode_public_notification_manifest(source)
            .and_then(|manifest| private_notification_payload(state, &manifest));
        match payload {
            Ok((payload, scope)) => {
                self.queue_notification_offer(payload, recipients, scope)
                    .await
            }
            Err(error) => tracing::debug!(%error, "private notification offer skipped"),
        }
    }

    pub(crate) async fn queue_public_repost_offer(
        &self,
        topic_id: &str,
        envelope: &KukuriEnvelope,
        source: Option<&RepostSourceSnapshotV1>,
        commentary: Option<&str>,
    ) {
        let content = commentary.unwrap_or_default();
        let recipients = public_post_notification_recipients(content, None, source);
        self.queue_public_notification_offer(
            PublicNotificationSource::Post {
                replica: topic_replica_id(topic_id),
                envelope: envelope.clone(),
                content: content.to_string(),
                reply_target: None,
            },
            recipients,
        )
        .await;
    }

    pub(crate) async fn queue_public_notification_offer(
        &self,
        source: PublicNotificationSource,
        recipients: BTreeSet<String>,
    ) {
        if let Ok(payload) = encode_public_notification_manifest(source) {
            self.queue_notification_offer(payload, recipients, ReceiveOfferScopeV1::PublicSource)
                .await;
        }
    }

    async fn queue_notification_offer(
        &self,
        payload: Vec<u8>,
        mut recipients: BTreeSet<String>,
        scope: ReceiveOfferScopeV1,
    ) {
        recipients.remove(self.current_author_pubkey().as_str());
        if recipients.is_empty() {
            return;
        }
        let mut queue = self
            .subscription_registry
            .public_notification_offer_queue
            .lock()
            .await;
        if queue
            .as_ref()
            .is_none_or(|worker| worker.task.is_finished())
        {
            let (sender, mut receiver) = tokio::sync::mpsc::channel(PUBLIC_OFFER_QUEUE_CAPACITY);
            let services = self.services.clone();
            let closed = Arc::clone(&self.subscription_registry.account_receive_offer_closed);
            let task = AbortOnDropTask::new(tokio::spawn(async move {
                while let Some((payload, recipients, scope)) = receiver.recv().await {
                    if closed.load(Ordering::Acquire) {
                        break;
                    }
                    if let Err(error) = publish_public_notification_offers(
                        &services, &closed, payload, recipients, scope,
                    )
                    .await
                    {
                        tracing::debug!(%error, "public notification offers deferred");
                    }
                }
            }));
            *queue = Some(PublicNotificationOfferQueue { sender, task });
        }
        if let Some(worker) = queue.as_ref()
            && let Err(error) = worker.sender.try_send((payload, recipients, scope))
        {
            tracing::debug!(%error, "public notification offer queue is full");
        }
    }

    pub(crate) async fn ingest_public_notification_offer(
        services: &ServiceHandles,
        offer: &VerifiedReceiveOffer,
    ) -> Result<bool> {
        let provider = EndpointAddr::new(offer.reference().provider_endpoint_id.parse()?);
        let payload = services
            .blob_service
            .fetch_verified_receive_offer_payload(offer, provider)
            .await?;
        let manifest: PublicNotificationManifest = serde_json::from_slice(&payload)?;
        anyhow::ensure!(
            manifest.version == 1,
            "unsupported public notification manifest"
        );
        let local = services.keys.public_key_hex();
        let candidate = match manifest.source {
            PublicNotificationSource::Post {
                replica,
                envelope,
                content,
                reply_target,
            } => post_notification_candidate(
                &local,
                offer,
                &replica,
                envelope,
                content,
                reply_target,
                None,
            )?,
            PublicNotificationSource::Follow { envelope } => {
                envelope.verify()?;
                anyhow::ensure!(
                    envelope.pubkey == *offer.sender(),
                    "follow offer sender mismatch"
                );
                let edge = parse_follow_edge(&envelope)?.context("invalid follow offer")?;
                // #1221 R4-D: 自分を指す署名済みの edge を手元へ保存する。関係(mutual)は読むときに edge から求める。
                if edge.target_pubkey.as_str() == local {
                    services.store.put_envelope(envelope).await?;
                }
                notification_candidate_from_verified_follow(
                    &local,
                    &author_replica_id(offer.sender().as_str()),
                    &edge,
                )
            }
        };
        let Some(candidate) = candidate else {
            return Ok(false);
        };
        Self::put_notification_candidate(services.projection_store.as_ref(), &local, candidate)
            .await
    }

    /// private の通知の参照(#1221 R4-C)。参加中の channel の現在の epoch の識別子で照合し、合わなければ provider へ
    /// 要求しない。取得と保存はその channel の参加の世代が続く間だけ行う。
    pub(crate) async fn ingest_private_notification_offer(
        services: &ServiceHandles,
        offer: &VerifiedReceiveOffer,
        epoch_key_id: &str,
    ) -> Result<bool> {
        let joined = services
            .joined_private_channels
            .lock()
            .await
            .values()
            .find_map(|state| {
                let mut secret = [0_u8; 32];
                hex::decode_to_slice(&state.current_epoch_secret_hex, &mut secret).ok()?;
                (receive_epoch_key_id(&secret, state.channel_id.as_str(), &state.current_epoch_id)
                    .ok()?
                    == epoch_key_id)
                    .then(|| (state.clone(), secret))
            });
        let Some((state, secret)) = joined else {
            return Ok(false);
        };
        let (topic, channel) = (state.topic_id.as_str(), state.channel_id.as_str());
        let provider = EndpointAddr::new(offer.reference().provider_endpoint_id.parse()?);
        let Some(payload) = services
            .until_content_invalid(
                topic,
                channel,
                state.generation,
                services
                    .blob_service
                    .fetch_verified_receive_offer_payload(offer, provider),
            )
            .await
        else {
            return Ok(false);
        };
        let manifest = PrivateReceivePayloadV1::decode(&payload?)?.open(
            &secret,
            channel,
            &state.current_epoch_id,
        )?;
        let manifest: PublicNotificationManifest = serde_json::from_slice(&manifest)?;
        anyhow::ensure!(
            manifest.version == 1,
            "unsupported private notification manifest"
        );
        let PublicNotificationSource::Post {
            replica,
            envelope,
            content,
            reply_target,
        } = manifest.source
        else {
            anyhow::bail!("private notification must reference a post");
        };
        anyhow::ensure!(
            replica == current_private_channel_replica_id(&state),
            "private notification replica does not match its epoch"
        );
        let local = services.keys.public_key_hex();
        let Some(candidate) = post_notification_candidate(
            &local,
            offer,
            &replica,
            envelope,
            content,
            reply_target,
            Some(channel),
        )?
        else {
            return Ok(false);
        };
        let _save = services.content_save_access.lock().await;
        if !services
            .content_scope_is_current(topic, channel, state.generation)
            .await
        {
            return Ok(false);
        }
        Self::put_notification_candidate(services.projection_store.as_ref(), &local, candidate)
            .await
    }
}

/// 送信者が署名した投稿から通知の候補を作る。投稿の scope(公開、または `channel` の private)、本文 hash、
/// 返信先(自分の投稿で、同じ topic・channel のもの)を確かめる。
fn post_notification_candidate(
    local: &str,
    offer: &VerifiedReceiveOffer,
    replica: &ReplicaId,
    envelope: KukuriEnvelope,
    content: String,
    reply_target: Option<KukuriEnvelope>,
    channel: Option<&str>,
) -> Result<Option<NotificationCandidate>> {
    anyhow::ensure!(
        envelope.pubkey == *offer.sender(),
        "notification offer sender mismatch"
    );
    let post = VerifiedPost::verify_local(envelope, replica)
        .map_err(|reason| anyhow::anyhow!("invalid notification post: {reason:?}"))?;
    let header = post.header();
    anyhow::ensure!(
        header.channel_id.as_ref().map(ChannelId::as_str) == channel
            && (header.visibility == ObjectVisibility::Public) == channel.is_none(),
        "notification post does not belong to the offer route"
    );
    let content_matches = match &header.payload_ref {
        PayloadRef::BlobText { hash, bytes, .. } => {
            content.len() as u64 == *bytes
                && blake3::hash(content.as_bytes()).to_hex().as_str() == hash.as_str()
        }
        PayloadRef::InlineText { text } => content == *text,
    };
    anyhow::ensure!(content_matches, "notification content mismatch");
    let reply_to_local = if let (Some(parent_id), Some(parent)) = (&header.reply_to, reply_target) {
        parent.verify()?;
        let parent_post = parent.to_post_object()?.context("invalid reply target")?;
        anyhow::ensure!(parent.id == *parent_id, "reply target id mismatch");
        parent.pubkey.as_str() == local
            && parent_post.topic_id == header.topic_id
            && parent_post.channel_id == header.channel_id
            && parent_post.visibility == header.visibility
    } else {
        false
    };
    Ok(notification_candidate_from_verified_post(
        local,
        &post,
        Some(content),
        reply_to_local,
    ))
}

pub(crate) fn public_post_notification_recipients(
    content: &str,
    reply_target: Option<&KukuriEnvelope>,
    repost_of: Option<&RepostSourceSnapshotV1>,
) -> BTreeSet<String> {
    let mut recipients = pubkey_mentions(content)
        .map(str::to_ascii_lowercase)
        .collect::<BTreeSet<_>>();
    if let Some(parent) = reply_target {
        recipients.insert(parent.pubkey.as_str().to_string());
    }
    if let Some(source) = repost_of {
        recipients.insert(source.source_author_pubkey.as_str().to_string());
    }
    recipients
}

async fn publish_public_notification_offers(
    services: &ServiceHandles,
    closed: &AtomicBool,
    payload: Vec<u8>,
    recipients: BTreeSet<String>,
    scope: ReceiveOfferScopeV1,
) -> Result<()> {
    if closed.load(Ordering::Acquire) {
        return Ok(());
    }
    let stored = services
        .blob_service
        .put_remote_blob(payload, "application/vnd.kukuri.public-notification+json")
        .await?;
    let provider_endpoint_id = services.transport.discovery().await?.local_endpoint_id;
    for recipient in recipients {
        if closed.load(Ordering::Acquire) {
            break;
        }
        if let Err(error) = publish_public_notification_offer(
            services,
            &recipient,
            &provider_endpoint_id,
            &stored,
            scope.clone(),
        )
        .await
        {
            tracing::debug!(%error, recipient, "public notification offer deferred");
        }
    }
    Ok(())
}

async fn publish_public_notification_offer(
    services: &ServiceHandles,
    recipient: &str,
    provider_endpoint_id: &str,
    stored: &StoredBlob,
    scope: ReceiveOfferScopeV1,
) -> Result<()> {
    let recipient = Pubkey::from(recipient);
    let Some(destination) = services
        .hint_transport
        .resolve_receive_destination(&recipient)
        .await?
    else {
        return Ok(());
    };
    let now = Utc::now().timestamp_millis();
    let offer = seal_receive_offer(
        services.keys.as_ref(),
        &recipient,
        ReceiveOfferReferenceV1 {
            provider_endpoint_id: provider_endpoint_id.to_string(),
            payload_hash: stored.hash.clone(),
            payload_bytes: u32::try_from(stored.bytes)?,
            scope,
        },
        now,
        now + 60_000,
    )?;
    tokio::time::timeout(
        PUBLIC_OFFER_TIMEOUT,
        services
            .hint_transport
            .publish_receive_offer(&recipient, destination, offer),
    )
    .await
    .context("public notification offer timed out")??;
    Ok(())
}
