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
            cursor: None,
        })
        .await
        .expect("list joined after restart")
        .items;
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
            cursor: None,
        })
        .await
        .expect("list joined after restart")
        .items;
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

/// #1218 AC-4b: 旧 registry(全件の 1 つの JSON。旧 backup の restore も同じ値を戻す)は、起動時に 1 回だけ参加の
/// 行と世代の鍵の行へ移り、registry は消える(ADR 0061 §9)。移した後の参加状態と担当の移行の結果は移す前と同じで、
/// 次の起動からは行から戻る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_registry_moves_into_rows_once() {
    use crate::identity::{load_optional_secret, persist_optional_secret};
    use crate::runtime::{PRIVATE_CHANNEL_CAPABILITIES_KEY, PRIVATE_CHANNEL_CAPABILITIES_PURPOSE};

    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db = dir.path().join("private-legacy-registry.db");
    let start = || {
        DesktopRuntime::new_with_config_and_identity(
            &db,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        )
    };
    let runtime = start().await.expect("runtime");
    let local = runtime.author_keys.public_key_hex();
    timeout(runtime_shutdown_timeout(), runtime.shutdown())
        .await
        .expect("runtime shutdown timeout");
    drop(runtime);

    let topic = "kukuri:topic:legacy-registry";
    let epoch = |day: u64| format!("epoch-{}-x", day * 86_400_000);
    let other = "11".repeat(32);
    // 担当の欄の無い自分の channel と、過去の世代を持つ他人の channel(凍結した registry の形)。
    let registry = serde_json::json!([
        {
            "topic_id": topic, "channel_id": "own", "label": "own", "creator_pubkey": local,
            "owner_pubkey": local, "audience_kind": "invite_only",
            "current_epoch_id": epoch(1), "current_epoch_secret_hex": "22".repeat(32),
        },
        {
            "topic_id": topic, "channel_id": "joined", "label": "joined", "creator_pubkey": other,
            "owner_pubkey": other, "joined_via_pubkey": other, "audience_kind": "friend_only",
            "current_epoch_id": epoch(3), "current_epoch_secret_hex": "33".repeat(32),
            "archived_epochs": [
                {"epoch_id": epoch(1), "namespace_secret_hex": "44".repeat(32)},
                {"epoch_id": epoch(2), "namespace_secret_hex": "55".repeat(32)},
            ],
        },
    ]);
    persist_optional_secret(
        &db,
        IdentityStorageMode::FileOnly,
        PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
        PRIVATE_CHANNEL_CAPABILITIES_KEY,
        &registry.to_string(),
    )
    .await
    .expect("legacy registry");

    for restart in 0..2 {
        let runtime = start().await.expect("restart runtime");
        assert_eq!(
            load_optional_secret(
                &db,
                IdentityStorageMode::FileOnly,
                PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
                PRIVATE_CHANNEL_CAPABILITIES_KEY,
            )
            .await
            .expect("read registry"),
            None,
            "the legacy registry is removed after the move"
        );
        let mut joined = runtime
            .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                topic: topic.into(),
                cursor: None,
            })
            .await
            .expect("list joined channels")
            .items;
        joined.sort_by(|left, right| left.channel_id.cmp(&right.channel_id));
        let summary = joined
            .iter()
            .map(|view| {
                (
                    view.channel_id.as_str(),
                    view.current_epoch_id.clone(),
                    view.archived_epoch_ids.clone(),
                    view.is_owner,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            vec![
                ("joined", epoch(3), vec![epoch(2), epoch(1)], false),
                ("own", epoch(1), Vec::new(), true),
            ],
            "restart {restart}"
        );
        // 担当の欄の無い自分の channel は、移した端末が担当になる(ADR 0018 §8)。担当でなければ鍵更新は保留になる。
        if restart == 1 {
            runtime
                .rotate_private_channel(RotatePrivateChannelRequest {
                    topic: topic.into(),
                    channel_id: "own".into(),
                })
                .await
                .expect("the moved own channel is controlled by this device");
        }
        timeout(runtime_shutdown_timeout(), runtime.shutdown())
            .await
            .expect("runtime shutdown timeout");
    }
}
