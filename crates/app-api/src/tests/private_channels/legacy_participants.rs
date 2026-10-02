//! #1221 R5-H 決定 3: 更新前からの参加者を、owner の手元の旧 docs の参加 record から参加者の表へ 1 回だけ移す。

use super::super::*;

fn legacy_participant(
    channel_id: &str,
    topic: &str,
    epoch_id: &str,
    keys: &KukuriKeys,
) -> PrivateChannelParticipantDocV1 {
    PrivateChannelParticipantDocV1 {
        channel_id: ChannelId::new(channel_id.to_string()),
        topic_id: TopicId::new(topic),
        epoch_id: epoch_id.to_string(),
        participant_pubkey: keys.public_key(),
        joined_at: 1_700_000_000,
        is_owner: false,
        join_mode: None,
        sponsor_pubkey: None,
        share_token_id: None,
        left_at: None,
    }
}

/// 移行を終端まで進め、ステップの数を返す。各ステップは保存した位置から再開する。
async fn migrate_to_the_end(app: &AppService) -> usize {
    let mut cursor = String::new();
    for step in 1.. {
        let (next, done) = app
            .migrate_legacy_private_channel_participants(&cursor)
            .await
            .expect("migrate legacy participants");
        cursor = next;
        if done {
            return step;
        }
    }
    unreachable!()
}

// 旧 docs の参加 record 300 件を、key の窓 1 つ(128 件まで)ずつ移す。窓で埋まる接頭辞は細かくして読み、全員が
// 1 回ずつ表へ入る。終端の後のステップは何も読まずに終わる。
#[tokio::test]
async fn legacy_participants_move_to_the_table_one_bounded_window_at_a_time() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("owner", FakeNetwork::default()));
    let app = AppService::new(store.clone(), transport);
    let topic = "kukuri:topic:legacy-participants";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "legacy".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let epoch_id = channel.current_epoch_id.clone();
    let replica = private_channel_replica_for_epoch(&channel.channel_id, &epoch_id);
    for _ in 0..300 {
        let keys = generate_keys();
        persist_private_channel_participant(
            app.docs_sync(),
            &keys,
            &legacy_participant(&channel.channel_id, topic, &epoch_id, &keys),
            &replica,
        )
        .await
        .expect("legacy participant record");
    }
    assert_eq!(
        store
            .private_channel_participant_counts(&channel.channel_id, &epoch_id)
            .await
            .unwrap()
            .0,
        1,
        "only the owner is in the table before the migration"
    );

    let steps = migrate_to_the_end(&app).await;
    assert!(
        steps > 3,
        "the records are read in several windows: {steps}"
    );
    assert_eq!(
        store
            .private_channel_participant_counts(&channel.channel_id, &epoch_id)
            .await
            .unwrap()
            .0,
        301
    );
}

// 更新前に参加した相手は、owner の表が更新後に届いた record しか持たなくても、移行の後の最初の rotation で grant を
// 受け取って新しい epoch へ移る。
#[cfg(feature = "iroh-integration-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_participant_from_before_the_update_receives_the_first_rotation_grant() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let keys_a = generate_keys();
    let keys_b = generate_keys();
    let app = |store: Arc<MemoryStore>, stack: &TestIrohStack, keys: &KukuriKeys| {
        app_service_from_dependencies(
            store.clone(),
            store,
            stack.transport.clone(),
            stack.transport.clone(),
            stack.docs_sync.clone(),
            stack.blob_service.clone(),
            keys.clone(),
        )
    };
    let stack_a = TestIrohStack::new(&dir.path().join("legacy-owner")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("legacy-member")).await;
    let app_a = app(Arc::new(MemoryStore::default()), &stack_a, &keys_a);
    let store_b = Arc::new(MemoryStore::default());
    let app_b = app(store_b.clone(), &stack_b, &keys_b);
    stack_a.bind_account(&app_a).await;
    stack_b.bind_account(&app_b).await;
    let topic = "kukuri:topic:private-channel-legacy-participant";
    let ticket_a = app_a.peer_ticket().await.unwrap().unwrap();
    let ticket_b = app_b.peer_ticket().await.unwrap().unwrap();
    app_a.import_peer_ticket(&ticket_b).await.expect("import b");
    app_b.import_peer_ticket(&ticket_a).await.expect("import a");
    let channel = app_a
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "legacy member".into(),
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
    let capability = app_a
        .get_private_channel_capability(topic, &channel.channel_id)
        .await
        .expect("capability")
        .expect("owner capability");
    let epoch_id = capability.current_epoch_id.clone();
    // 参加 record が届くのを待ってから、owner を表の無い状態(更新前の owner)で起動し直す。
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
    .expect("the join record is acknowledged");
    app_a.shutdown().await;
    drop(app_a);
    stack_a
        ._node
        .clone()
        .shutdown()
        .await
        .expect("stop owner node");
    drop(stack_a);
    let stack_a = TestIrohStack::new(&dir.path().join("legacy-owner")).await;
    let store_a = Arc::new(MemoryStore::default());
    let app_a = app(store_a.clone(), &stack_a, &keys_a);
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
    app_a.import_peer_ticket(&ticket_b).await.expect("import b");
    // 旧版の同期が owner の手元へ写していた b の参加 record。
    persist_private_channel_participant(
        app_a.docs_sync(),
        &keys_b,
        &legacy_participant(&channel.channel_id, topic, &epoch_id, &keys_b),
        &private_channel_replica_for_epoch(&channel.channel_id, &epoch_id),
    )
    .await
    .expect("legacy participant record of b");
    let b_pubkey = keys_b.public_key_hex();
    let listed = |store: Arc<MemoryStore>| {
        let channel_id = channel.channel_id.clone();
        async move {
            store
                .list_private_channel_participants(&channel_id, "", 8)
                .await
                .unwrap()
        }
    };
    assert!(!listed(store_a.clone()).await.contains(&b_pubkey));

    migrate_to_the_end(&app_a).await;
    assert!(listed(store_a.clone()).await.contains(&b_pubkey));
    let rotated = app_a
        .rotate_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("rotate");
    timeout(p2p_replication_timeout(), async {
        loop {
            let joined = app_b
                .list_joined_private_channels(topic, None)
                .await
                .expect("joined channels on b")
                .items;
            if joined.iter().any(|item| {
                item.channel_id == channel.channel_id
                    && item.current_epoch_id == rotated.current_epoch_id
            }) {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("b redeems the first rotation grant");
}
