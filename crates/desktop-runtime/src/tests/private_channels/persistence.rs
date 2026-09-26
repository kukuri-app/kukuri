use super::super::*;

/// WP-C2 (E-2): owner の export は auto-rotate で in-memory capability を新 epoch に置換する。
/// rotate 後の capability が再起動を跨いで残ることを固定する failing test 群。
///
/// 注意: export と再起動の間に persist 付きラッパー(list_joined_private_channels 等)や
/// それを内部で呼ぶテストヘルパーを挟むと、rotate 後 epoch が偶発的に persist されて
/// バグがマスクされる。期待 epoch の取得は非変異の preview_channel_access_token で行う。

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_invite_export_auto_rotate_survives_restart() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db = dir.path().join("private-export-persist-invite.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let topic = "kukuri:topic:desktop-export-persist-invite";

    open_topic_column(&runtime, topic, TimelineScope::Public)
        .await
        .expect("subscribe");

    let channel = runtime
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "export-persist-invite".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let created_epoch = channel.current_epoch_id.clone();

    let invite = runtime
        .export_private_channel_invite(ExportPrivateChannelInviteRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export invite");

    let preview = runtime
        .preview_channel_access_token(PreviewChannelAccessTokenRequest { token: invite })
        .await
        .expect("preview invite");
    let rotated_epoch = preview.epoch_id.clone();
    assert_ne!(
        rotated_epoch, created_epoch,
        "owner invite export must auto-rotate the epoch"
    );

    timeout(runtime_shutdown_timeout(), runtime.shutdown())
        .await
        .expect("runtime shutdown timeout");
    drop(runtime);

    let restarted = DesktopRuntime::new_with_config_and_identity(
        &db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart runtime");
    let joined = restarted
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("list joined after restart");
    assert_eq!(joined.len(), 1);
    assert_eq!(
        joined[0].current_epoch_id, rotated_epoch,
        "rotated epoch must survive owner restart"
    );
    assert!(
        joined[0].archived_epoch_ids.contains(&created_epoch),
        "pre-rotate epoch must be archived after restart (archived: {:?})",
        joined[0].archived_epoch_ids
    );

    timeout(runtime_shutdown_timeout(), restarted.shutdown())
        .await
        .expect("restarted runtime shutdown timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_access_token_export_auto_rotate_survives_restart() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db = dir.path().join("private-export-persist-access-token.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let topic = "kukuri:topic:desktop-export-persist-access-token";

    open_topic_column(&runtime, topic, TimelineScope::Public)
        .await
        .expect("subscribe");

    // FriendPlus も owner の Share(export)で毎回 auto-rotate する。access token 経由の
    // export は app-api 内部で export_friend_plus_share へ委譲されるディスパッチャ経路。
    let channel = runtime
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "export-persist-access".into(),
            audience_kind: ChannelAudienceKind::FriendPlus,
        })
        .await
        .expect("create private channel");
    let created_epoch = channel.current_epoch_id.clone();

    let export = runtime
        .export_channel_access_token(ExportChannelAccessTokenRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export access token");

    let preview = runtime
        .preview_channel_access_token(PreviewChannelAccessTokenRequest {
            token: export.token,
        })
        .await
        .expect("preview access token");
    let rotated_epoch = preview.epoch_id.clone();
    assert_ne!(
        rotated_epoch, created_epoch,
        "owner access-token export must auto-rotate the epoch"
    );

    timeout(runtime_shutdown_timeout(), runtime.shutdown())
        .await
        .expect("runtime shutdown timeout");
    drop(runtime);

    let restarted = DesktopRuntime::new_with_config_and_identity(
        &db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart runtime");
    let joined = restarted
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("list joined after restart");
    assert_eq!(joined.len(), 1);
    assert_eq!(
        joined[0].current_epoch_id, rotated_epoch,
        "rotated epoch must survive owner restart"
    );
    assert!(
        joined[0].archived_epoch_ids.contains(&created_epoch),
        "pre-rotate epoch must be archived after restart (archived: {:?})",
        joined[0].archived_epoch_ids
    );

    timeout(runtime_shutdown_timeout(), restarted.shutdown())
        .await
        .expect("restarted runtime shutdown timeout");
}
