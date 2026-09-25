use super::super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_channel_leave_removes_local_access_and_syncs_participant_exit() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("leave-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("leave-b")).await;
    let app_a = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_a);
    let app_b = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_b);
    stack_a.bind_account(&app_a).await;
    stack_b.bind_account(&app_b).await;
    let topic = "kukuri:topic:private-channel-leave";

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a.import_peer_ticket(&ticket_b).await.expect("import b");
    app_b.import_peer_ticket(&ticket_a).await.expect("import a");
    let _ = app_a.list_timeline(topic, None, 20).await;
    let _ = app_b.list_timeline(topic, None, 20).await;
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;

    let channel = app_a
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "core".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let invite = app_a
        .export_private_channel_invite(topic, channel.channel_id.as_str(), None)
        .await
        .expect("export invite");
    app_b
        .import_private_channel_invite(invite.as_str())
        .await
        .expect("import invite");

    timeout(p2p_replication_timeout(), async {
        loop {
            let joined = app_a
                .list_joined_private_channels(topic)
                .await
                .expect("owner joined channels");
            if joined
                .iter()
                .any(|item| item.channel_id == channel.channel_id && item.participant_count == 2)
            {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("participant join propagation timeout");

    app_b
        .leave_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("leave private channel");
    assert!(
        app_b
            .list_joined_private_channels(topic)
            .await
            .expect("left joined channels")
            .is_empty()
    );
    let private_ref = ChannelRef::PrivateChannel {
        channel_id: ChannelId::new(channel.channel_id.clone()),
    };
    let write_error = app_b
        .create_post_in_channel(topic, private_ref, "after leave", None)
        .await
        .expect_err("left participant cannot write");
    assert!(
        write_error
            .to_string()
            .contains("private channel is not joined")
    );

    timeout(p2p_replication_timeout(), async {
        loop {
            let joined = app_a
                .list_joined_private_channels(topic)
                .await
                .expect("owner joined channels after leave");
            if joined
                .iter()
                .any(|item| item.channel_id == channel.channel_id && item.participant_count == 1)
            {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("participant leave propagation timeout");
}
