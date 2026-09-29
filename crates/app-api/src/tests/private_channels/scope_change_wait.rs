//! #1423: 読み取りが参加状態の lock を待っている間に scope の変更を受けても、`until_content_invalid` が止まらない。

use super::super::*;

// current_thread runtime で、読み取り → 世代の確認の順に lock の列へ並べる。
#[tokio::test]
async fn scope_change_while_the_read_waits_for_the_joined_lock_does_not_stall() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:scope-change-wait";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "scope change".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("channel");
    let channel_id = channel.channel_id.as_str().to_string();
    let services = app.services.clone();
    let generation = services
        .active_content_scope_generation(topic, &channel_id)
        .await
        .expect("joined channel generation");

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
    let joined = services.joined_private_channels.clone();
    let read = async move {
        let _ = started_tx.send(());
        let _ = gate_rx.await;
        joined.lock().await.len()
    };
    let waiter = tokio::spawn({
        let services = services.clone();
        async move {
            services
                .until_content_invalid(topic, &channel_id, generation, read)
                .await
        }
    });
    started_rx.await.expect("read started");
    let held = services.joined_private_channels.lock().await;
    gate_tx.send(()).expect("open gate");
    // 読み取りが lock の列に並ぶ。
    tokio::task::yield_now().await;
    // 世代は変えずに変更だけを知らせ、世代の確認を読み取りの後ろに並ばせる。
    services.content_scope_changes.send_modify(|_| {});
    tokio::task::yield_now().await;
    drop(held);

    let result = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("until_content_invalid stalled")
        .expect("waiter");
    assert_eq!(result, Some(1));
}
