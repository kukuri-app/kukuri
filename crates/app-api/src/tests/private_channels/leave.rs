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
            if joined.iter().any(|item| {
                item.channel_id == channel.channel_id && item.participant_count == Some(2)
            }) {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("participant join propagation timeout");
    // 参加・退出 record は owner にだけ届くため、owner 以外の端末は人数を返さない(#1221 R5-H)。
    let joined_b = app_b
        .list_joined_private_channels(topic)
        .await
        .expect("participant joined channels");
    assert_eq!(joined_b.len(), 1);
    assert_eq!(joined_b[0].participant_count, None);

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
            if joined.iter().any(|item| {
                item.channel_id == channel.channel_id && item.participant_count == Some(1)
            }) {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("participant leave propagation timeout");
}

async fn wait_for_owner_participant_count(
    app: &AppService,
    topic: &str,
    channel_id: &str,
    expected: usize,
) {
    timeout(p2p_replication_timeout(), async {
        loop {
            if app
                .list_joined_private_channels(topic)
                .await
                .expect("owner joined channels")
                .iter()
                .any(|item| {
                    item.channel_id == channel_id && item.participant_count == Some(expected)
                })
            {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("owner participant count did not become {expected}"));
}

/// #1221 R5-H AC-5: owner が offline の間の退出 record は参加者の outbox に残り、owner の再起動後に account 経路で
/// 届いて人数と rotation の宛先に入る(旧 sync なし)。届いたら owner の ACK で outbox から消える。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn participant_leave_while_the_owner_is_offline_arrives_after_the_owner_restarts() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let store_a = Arc::new(MemoryStore::default());
    let keys_a = generate_keys();
    let owner = |stack: &TestIrohStack| {
        app_service_from_dependencies(
            store_a.clone(),
            store_a.clone(),
            stack.transport.clone(),
            stack.transport.clone(),
            stack.docs_sync.clone(),
            stack.blob_service.clone(),
            keys_a.clone(),
        )
    };
    let stack_a = TestIrohStack::new(&dir.path().join("offline-owner")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("offline-member")).await;
    let app_a = owner(&stack_a);
    let store_b = Arc::new(MemoryStore::default());
    let app_b = app_with_iroh_services(store_b.clone(), &stack_b);
    stack_a.bind_account(&app_a).await;
    stack_b.bind_account(&app_b).await;
    let topic = "kukuri:topic:private-channel-offline-owner";
    let ticket_a = app_a.peer_ticket().await.unwrap().unwrap();
    let ticket_b = app_b.peer_ticket().await.unwrap().unwrap();
    app_a.import_peer_ticket(&ticket_b).await.expect("import b");
    app_b.import_peer_ticket(&ticket_a).await.expect("import a");

    let channel = app_a
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "offline owner".into(),
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
    wait_for_owner_participant_count(&app_a, topic, &channel.channel_id, 2).await;
    let capability = app_a
        .get_private_channel_capability(topic, &channel.channel_id)
        .await
        .expect("capability")
        .expect("owner capability");

    // owner を止めてから退出する。退出 record は owner 宛の outbox に残る。
    app_a.shutdown().await;
    drop(app_a);
    stack_a
        ._node
        .clone()
        .shutdown()
        .await
        .expect("stop owner node");
    drop(stack_a);
    app_b
        .leave_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("leave while the owner is offline");
    sleep(Duration::from_secs(3)).await;
    let pending = store_b.list_direct_message_outbox().await.unwrap();
    assert_eq!(pending.len(), 1, "the leave record waits for the owner");
    assert!(pending[0].dm_id.starts_with(EPOCH_CONTROL_OUTBOX_PREFIX));

    // owner の再起動(同じ保存場所・同じ account)。参加者は新しい宛先を知る。
    let stack_a = TestIrohStack::new(&dir.path().join("offline-owner")).await;
    let app_a = owner(&stack_a);
    app_a
        .restore_private_channel_capability(capability)
        .await
        .expect("restore owner channel");
    stack_a.bind_account(&app_a).await;
    let ticket_a = app_a.peer_ticket().await.unwrap().unwrap();
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import restarted owner");
    wait_for_owner_participant_count(&app_a, topic, &channel.channel_id, 1).await;
    timeout(p2p_replication_timeout(), async {
        while !store_b
            .list_direct_message_outbox()
            .await
            .unwrap()
            .is_empty()
        {
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the owner's ACK removes the leave record");
    assert!(
        store_a
            .list_private_channel_participants(&channel.channel_id, None, "", 8)
            .await
            .unwrap()
            .iter()
            .all(|participant| *participant != app_b.current_author_pubkey()),
        "the member who left is not a rotation recipient"
    );
}

/// #1221 R5-H: 参加 record は account 経路で owner へ届くので、owner の回転より後に着きうる。回転の後に届いた直前の
/// epoch の参加 record にも、owner は現 epoch の handoff grant を送り、参加者は回転後の epoch へ移る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_join_record_arriving_after_the_rotation_still_gets_the_handoff_grant() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("late-join-owner")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("late-join-member")).await;
    let app_a = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_a);
    let app_b = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_b);
    stack_a.bind_account(&app_a).await;
    stack_b.bind_account(&app_b).await;
    let topic = "kukuri:topic:private-channel-late-join";
    let ticket_a = app_a.peer_ticket().await.unwrap().unwrap();
    let ticket_b = app_b.peer_ticket().await.unwrap().unwrap();
    app_a.import_peer_ticket(&ticket_b).await.expect("import b");
    app_b.import_peer_ticket(&ticket_a).await.expect("import a");
    let channel = app_a
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "late join".into(),
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
    // 参加 record が owner の outbox の周期で届く前に回転する。
    let rotated = app_a
        .rotate_private_channel(topic, &channel.channel_id)
        .await
        .expect("rotate");
    timeout(p2p_replication_timeout(), async {
        loop {
            if app_b
                .list_joined_private_channels(topic)
                .await
                .expect("member channels")
                .iter()
                .any(|item| item.current_epoch_id == rotated.current_epoch_id)
            {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the member moves to the rotated epoch");
}
