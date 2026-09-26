use super::*;

fn build_app(hint_transport: Arc<TrackingHintTransport>) -> AppService {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        hint_transport,
        Arc::new(TrackingDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    )
}

#[tokio::test]
async fn disabled_topic_blocks_topic_subscription() {
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let app = build_app(hint_transport.clone());
    let topic = "kukuri:topic:gossip-disabled";

    app.set_topic_gossip_enabled(topic, false)
        .await
        .expect("disable topic gossip");

    // 列を開いても、gossip を止めた topic は購読しない(lease だけを持つ)。
    display_topic(&app, topic).await.expect("display topic");

    assert!(!app.has_topic_subscription(topic).await);
    assert_eq!(*hint_transport.subscribe_count.lock().await, 0);

    let status = app.get_sync_status().await.expect("sync status");
    assert_eq!(status.gossip_disabled_topics, vec![topic.to_string()]);
}

#[tokio::test]
async fn enabling_topic_resubscribes() {
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let app = build_app(hint_transport.clone());
    let topic = "kukuri:topic:gossip-reenable";

    display_topic(&app, topic).await.expect("display topic");
    assert!(app.has_topic_subscription(topic).await);

    app.set_topic_gossip_enabled(topic, false)
        .await
        .expect("disable topic gossip");
    assert!(!app.has_topic_subscription(topic).await);

    app.set_topic_gossip_enabled(topic, true)
        .await
        .expect("enable topic gossip");
    assert!(app.has_topic_subscription(topic).await);

    let status = app.get_sync_status().await.expect("sync status");
    assert!(status.gossip_disabled_topics.is_empty());
}

#[tokio::test]
async fn get_sync_status_reports_disabled_channels() {
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let app = build_app(hint_transport);
    let topic = "kukuri:topic:gossip-channel";
    let channel = "channel-1";

    app.set_channel_gossip_enabled(topic, channel, false)
        .await
        .expect("disable channel gossip");

    let status = app.get_sync_status().await.expect("sync status");
    assert_eq!(
        status.gossip_disabled_channels,
        vec![format!("{topic}::{channel}")]
    );

    app.set_channel_gossip_enabled(topic, channel, true)
        .await
        .expect("enable channel gossip");
    let status = app.get_sync_status().await.expect("sync status");
    assert!(status.gossip_disabled_channels.is_empty());
}
