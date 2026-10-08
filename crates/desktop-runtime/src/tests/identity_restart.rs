use super::*;

#[test]
fn resolve_db_path_ignores_legacy_runtime_artifacts() {
    let dir = tempdir().expect("tempdir");
    let legacy_db_path = dir.path().join("kukuri-next.db");
    let legacy_data_dir = dir.path().join("kukuri-next.iroh-data");
    fs::write(&legacy_db_path, b"sqlite").expect("legacy db");
    fs::create_dir_all(&legacy_data_dir).expect("legacy data dir");
    fs::write(legacy_data_dir.join("blob.bin"), b"blob").expect("legacy blob");

    let resolved = resolve_db_path_from_env(dir.path()).expect("resolved db path");

    assert_eq!(resolved, dir.path().join("kukuri.db"));
    assert!(!resolved.exists());
    assert!(!resolved.with_extension("iroh-data").exists());
    assert!(legacy_db_path.exists());
    assert!(legacy_data_dir.join("blob.bin").exists());
}

#[tokio::test]
async fn desktop_runtime_persists_posts_and_author_identity_after_restart() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("kukuri.db");
    let runtime = timeout(
        Duration::from_secs(15),
        DesktopRuntime::new_with_config_and_identity(
            &db_path,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        ),
    )
    .await
    .expect("runtime creation timeout")
    .expect("runtime");
    assert_eq!(
        runtime
            .get_sync_status()
            .await
            .expect("sync status")
            .peer_count,
        0
    );
    let object_id = runtime
        .create_post(CreatePostRequest {
            topic: "kukuri:topic:runtime".into(),
            content: "persist me".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create post");
    let author_pubkey = runtime
        .get_my_profile()
        .await
        .expect("local profile")
        .pubkey;
    let profile_before_restart = runtime
        .list_profile_timeline(ListProfileTimelineRequest {
            pubkey: author_pubkey.as_str().to_string(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("profile timeline without a peer");
    assert_eq!(profile_before_restart.items.len(), 1);
    assert_eq!(profile_before_restart.items[0].object_id, object_id);
    timeout(Duration::from_secs(15), runtime.shutdown())
        .await
        .expect("runtime shutdown timeout");
    drop(runtime);

    let restarted = timeout(
        Duration::from_secs(15),
        DesktopRuntime::new_with_config_and_identity(
            &db_path,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        ),
    )
    .await
    .expect("runtime restart timeout")
    .expect("runtime restart");
    assert_eq!(
        restarted
            .get_sync_status()
            .await
            .expect("restarted sync status")
            .peer_count,
        0
    );
    let restarted_object_id = restarted
        .create_post(CreatePostRequest {
            topic: "kukuri:topic:runtime".into(),
            content: "persist me again".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create post after restart");
    let timeline = restarted
        .list_timeline(ListTimelineRequest {
            topic: "kukuri:topic:runtime".into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("timeline");

    assert!(
        timeline
            .items
            .iter()
            .any(|post| post.object_id == object_id)
    );
    assert!(
        timeline
            .items
            .iter()
            .any(|post| post.object_id == restarted_object_id)
    );
    let original_post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("original post");
    let restarted_post = timeline
        .items
        .iter()
        .find(|post| post.object_id == restarted_object_id)
        .expect("restarted post");
    assert_eq!(original_post.author_pubkey, restarted_post.author_pubkey);
    let profile_after_restart = restarted
        .list_profile_timeline(ListProfileTimelineRequest {
            pubkey: author_pubkey.as_str().to_string(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("profile timeline after restart without a peer");
    assert_eq!(profile_after_restart.items.len(), 2);
    for expected in [&object_id, &restarted_object_id] {
        assert_eq!(
            profile_after_restart
                .items
                .iter()
                .filter(|post| &post.object_id == expected)
                .count(),
            1,
        );
    }
    assert_eq!(restarted.db_path(), db_path.as_path());
    timeout(Duration::from_secs(15), restarted.shutdown())
        .await
        .expect("restarted shutdown timeout");
}

#[tokio::test]
async fn desktop_runtime_restores_profile_avatar_blob_after_restart() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("profile-avatar-restart.db");
    let avatar_bytes = b"runtime-profile-avatar".to_vec();
    let expected_payload = BASE64_STANDARD.encode(&avatar_bytes);
    let runtime = timeout(
        Duration::from_secs(15),
        DesktopRuntime::new_with_config_and_identity(
            &db_path,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        ),
    )
    .await
    .expect("runtime creation timeout")
    .expect("runtime");

    let updated = runtime
        .set_my_profile(SetMyProfileRequest {
            name: Some("runtime-avatar-owner".into()),
            display_name: Some("Runtime Avatar Owner".into()),
            about: Some("profile avatar restart".into()),
            picture_upload: Some(profile_avatar_attachment_request(
                "avatar.png",
                "image/png",
                &avatar_bytes,
            )),
            clear_picture: false,
            nip05: None,
        })
        .await
        .expect("set profile");
    let asset = updated.picture_asset.clone().expect("profile avatar");
    let author_pubkey = updated.pubkey.as_str().to_string();
    let payload_before_restart = runtime
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: asset.hash.as_str().to_string(),
            mime: asset.mime.clone(),
            source_object_id: None,
        })
        .await
        .expect("avatar payload before restart")
        .expect("avatar payload before restart value");
    let author_before_restart = runtime
        .get_author_social_view(AuthorRequest {
            pubkey: author_pubkey.clone(),
        })
        .await
        .expect("author social view before restart");

    assert_eq!(payload_before_restart.mime, "image/png");
    assert_eq!(payload_before_restart.bytes_base64, expected_payload);
    assert_eq!(
        author_before_restart
            .picture_asset
            .as_ref()
            .map(|value| value.hash.as_str()),
        Some(asset.hash.as_str())
    );
    assert_eq!(
        author_before_restart
            .picture_asset
            .as_ref()
            .map(|value| value.role.as_str()),
        Some("profile_avatar")
    );

    timeout(Duration::from_secs(15), runtime.shutdown())
        .await
        .expect("runtime shutdown timeout");
    drop(runtime);

    let restarted = timeout(
        Duration::from_secs(15),
        DesktopRuntime::new_with_config_and_identity(
            &db_path,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        ),
    )
    .await
    .expect("runtime restart timeout")
    .expect("runtime restart");
    let my_profile = restarted.get_my_profile().await.expect("my profile");
    let author_after_restart = restarted
        .get_author_social_view(AuthorRequest {
            pubkey: author_pubkey,
        })
        .await
        .expect("author social view after restart");
    let payload_after_restart = restarted
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: asset.hash.as_str().to_string(),
            mime: asset.mime.clone(),
            source_object_id: None,
        })
        .await
        .expect("avatar payload after restart")
        .expect("avatar payload after restart value");

    assert_eq!(
        my_profile
            .picture_asset
            .as_ref()
            .map(|value| value.hash.as_str()),
        Some(asset.hash.as_str())
    );
    assert_eq!(
        my_profile
            .picture_asset
            .as_ref()
            .map(|value| value.role.clone()),
        Some(AssetRole::ProfileAvatar)
    );
    assert_eq!(
        author_after_restart
            .picture_asset
            .as_ref()
            .map(|value| value.hash.as_str()),
        Some(asset.hash.as_str())
    );
    assert_eq!(
        author_after_restart
            .picture_asset
            .as_ref()
            .map(|value| value.role.as_str()),
        Some("profile_avatar")
    );
    assert_eq!(payload_after_restart.mime, "image/png");
    assert_eq!(payload_after_restart.bytes_base64, expected_payload);

    timeout(Duration::from_secs(15), restarted.shutdown())
        .await
        .expect("restarted shutdown timeout");
}
