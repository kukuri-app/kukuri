use super::super::*;

use kukuri_core::{private_topic_rendezvous_key_hex_secret, wire::hint_topic_id};
use kukuri_docs_sync::private_channel_hint_topic;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_channel_rendezvous_keys_follow_the_current_epoch_secret() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:private-rendezvous";
    let _ = app.list_timeline(topic, None, 20).await;

    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "非公開ランデブー".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("非公開チャンネルを作成できる");
    let capability = app
        .get_private_channel_capability(topic, channel.channel_id.as_str())
        .await
        .expect("非公開チャンネルの権限を読み出せる")
        .expect("非公開チャンネルの権限がある");
    let rendezvous_topic = hint_topic_id(&private_channel_hint_topic(channel.channel_id.as_str()));
    let expected_before = private_topic_rendezvous_key_hex_secret(
        capability.current_epoch_secret_hex.as_str(),
        &rendezvous_topic,
    )
    .expect("世代切替前の非公開ランデブー鍵を派生できる");

    let keys_before = app.private_channel_rendezvous_keys().await;
    assert_eq!(
        keys_before.get(rendezvous_topic.as_str()),
        Some(&expected_before)
    );

    app.rotate_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("非公開チャンネルの世代を切り替えられる");
    let rotated_capability = app
        .get_private_channel_capability(topic, channel.channel_id.as_str())
        .await
        .expect("世代切替後の非公開チャンネル権限を読み出せる")
        .expect("世代切替後の非公開チャンネル権限がある");
    let expected_after = private_topic_rendezvous_key_hex_secret(
        rotated_capability.current_epoch_secret_hex.as_str(),
        &rendezvous_topic,
    )
    .expect("世代切替後の非公開ランデブー鍵を派生できる");
    let keys_after = app.private_channel_rendezvous_keys().await;

    assert_ne!(expected_before, expected_after);
    assert_eq!(
        keys_after.get(rendezvous_topic.as_str()),
        Some(&expected_after)
    );
    assert!(!keys_after.values().any(|key| key == &expected_before));
}
