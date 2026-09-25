//! #1221 R4-C: private の通知の参照は、参加中の channel の現在の epoch で照合できたときだけ取得・反映する。

use super::super::*;
use super::receive_offer::{offer_app, offer_for};
use super::receive_offer_doubles::OfferBlobService;
use crate::service::public_notification_offer_support::encode_public_notification_manifest;
use kukuri_core::{ReceiveOfferScopeV1, seal_private_receive_payload};
use kukuri_transport::ReceiveOfferEnvelope;

const EPOCH: &str = "epoch-1";
const SECRET: [u8; 32] = [7; 32];

fn joined(topic: &TopicId, channel: &ChannelId, secret: [u8; 32]) -> JoinedPrivateChannelState {
    JoinedPrivateChannelState {
        generation: 1,
        topic_id: topic.as_str().into(),
        channel_id: channel.clone(),
        label: "room".into(),
        creator_pubkey: String::new(),
        owner_pubkey: String::new(),
        joined_via_pubkey: None,
        audience_kind: ChannelAudienceKind::InviteOnly,
        current_epoch_id: EPOCH.into(),
        current_epoch_secret_hex: hex::encode(secret),
        archived_epochs: Vec::new(),
    }
}

async fn join(app: &AppService, state: JoinedPrivateChannelState) {
    app.joined_private_channels.lock().await.insert(
        joined_private_channel_key(&state.topic_id, state.channel_id.as_str()),
        state,
    );
}

fn private_post(
    keys: &KukuriKeys,
    topic: &TopicId,
    channel: &ChannelId,
    text: &str,
    reply_to: Option<&KukuriEnvelope>,
) -> KukuriEnvelope {
    build_post_envelope_with_docs_author(
        keys,
        topic,
        PayloadRef::InlineText { text: text.into() },
        Vec::new(),
        Vec::new(),
        reply_to,
        ObjectVisibility::Private,
        Some(channel),
        Vec::new(),
        None,
    )
    .unwrap()
}

async fn deliver_private(
    app: &AppService,
    sender: &KukuriKeys,
    recipient: &KukuriKeys,
    memory_blob: &MemoryBlobService,
    channel: &ChannelId,
    source: PublicNotificationSource,
) -> Result<bool> {
    let manifest = encode_public_notification_manifest(source).unwrap();
    let sealed = seal_private_receive_payload(&SECRET, channel.as_str(), EPOCH, &manifest).unwrap();
    let stored = memory_blob
        .put_blob(sealed.encode().unwrap(), "application/octet-stream")
        .await
        .unwrap();
    let (_, offer) = offer_for(
        sender,
        recipient,
        ReceiveOfferScopeV1::PrivateSource {
            epoch_key_id: sealed.epoch_key_id,
        },
        stored.hash,
        stored.bytes as u32,
    );
    AppService::ingest_account_receive_offer(
        &app.services,
        ReceiveOfferEnvelope {
            offer,
            received_at: Utc::now().timestamp_millis(),
            source_peer: "offscreen".into(),
        },
    )
    .await
}

struct Fixture {
    app: AppService,
    blob: Arc<OfferBlobService>,
    memory_blob: Arc<MemoryBlobService>,
    sender: KukuriKeys,
    recipient: KukuriKeys,
    topic: TopicId,
    channel: ChannelId,
}

fn fixture() -> Fixture {
    let recipient = generate_keys();
    let memory_blob = Arc::new(MemoryBlobService::default());
    let blob = Arc::new(OfferBlobService::new(memory_blob.clone()));
    let app = offer_app(
        recipient.clone(),
        Arc::new(MemoryStore::default()),
        Arc::new(FakeTransport::new("recipient", FakeNetwork::default())),
        blob.clone(),
    );
    Fixture {
        app,
        blob,
        memory_blob,
        sender: generate_keys(),
        recipient,
        topic: TopicId::new("private-offer-topic"),
        channel: ChannelId::new("private-offer-channel"),
    }
}

impl Fixture {
    fn source(
        &self,
        envelope: KukuriEnvelope,
        content: String,
        reply: Option<KukuriEnvelope>,
    ) -> PublicNotificationSource {
        PublicNotificationSource::Post {
            replica: private_channel_epoch_replica_id(self.channel.as_str(), EPOCH),
            envelope,
            content,
            reply_target: reply,
        }
    }

    async fn deliver(&self, source: PublicNotificationSource) -> Result<bool> {
        deliver_private(
            &self.app,
            &self.sender,
            &self.recipient,
            self.memory_blob.as_ref(),
            &self.channel,
            source,
        )
        .await
    }
}

#[tokio::test]
async fn private_offers_create_one_mention_and_one_reply_notification() {
    let f = fixture();
    join(&f.app, joined(&f.topic, &f.channel, SECRET)).await;
    let mention_text = format!("private hello @{}", f.recipient.public_key_hex());
    let mention = private_post(&f.sender, &f.topic, &f.channel, &mention_text, None);
    let parent = private_post(&f.recipient, &f.topic, &f.channel, "my private post", None);
    let reply = private_post(
        &f.sender,
        &f.topic,
        &f.channel,
        "private reply",
        Some(&parent),
    );
    for expected in [true, false] {
        assert_eq!(
            f.deliver(f.source(mention.clone(), mention_text.clone(), None))
                .await
                .unwrap(),
            expected
        );
        assert_eq!(
            f.deliver(f.source(reply.clone(), "private reply".into(), Some(parent.clone())))
                .await
                .unwrap(),
            expected
        );
    }
    let notifications = f.app.list_notifications().await.unwrap();
    assert_eq!(notifications.len(), 2);
    let kinds = notifications
        .iter()
        .map(|item| item.kind.clone())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&NotificationKind::Mention));
    assert!(kinds.contains(&NotificationKind::Reply));
    assert!(
        notifications
            .iter()
            .all(|item| item.channel_id.as_deref() == Some(f.channel.as_str()))
    );
}

#[tokio::test]
async fn a_private_offer_without_the_current_epoch_is_never_fetched() {
    for joined_secret in [None, Some([8; 32])] {
        let f = fixture();
        if let Some(secret) = joined_secret {
            join(&f.app, joined(&f.topic, &f.channel, secret)).await;
        }
        let text = format!("hello @{}", f.recipient.public_key_hex());
        let post = private_post(&f.sender, &f.topic, &f.channel, &text, None);
        assert!(!f.deliver(f.source(post, text, None)).await.unwrap());
        assert_eq!(f.blob.fetches.load(Ordering::SeqCst), 0, "no provider I/O");
        assert!(f.app.list_notifications().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn a_private_offer_for_another_channel_or_sender_is_not_saved() {
    let f = fixture();
    join(&f.app, joined(&f.topic, &f.channel, SECRET)).await;
    let text = format!("hello @{}", f.recipient.public_key_hex());
    let other_channel = ChannelId::new("another-channel");
    let foreign = private_post(&f.sender, &f.topic, &other_channel, &text, None);
    assert!(
        f.deliver(f.source(foreign, text.clone(), None))
            .await
            .is_err()
    );
    let impostor = private_post(&generate_keys(), &f.topic, &f.channel, &text, None);
    assert!(
        f.deliver(f.source(impostor, text.clone(), None))
            .await
            .is_err()
    );
    let genuine = private_post(&f.sender, &f.topic, &f.channel, &text, None);
    assert!(
        f.deliver(f.source(genuine, "forged preview".into(), None))
            .await
            .is_err()
    );
    assert!(f.app.list_notifications().await.unwrap().is_empty());
}
