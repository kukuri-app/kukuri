use std::collections::BTreeMap;

use kukuri_desktop_runtime::{
    AuthorRequest, BookmarkPostRequest, CommunityNodeConfig, CommunityNodeNodeConfig,
    CreateDeviceBackupRequest, CreatePostRequest, CreatePrivateChannelRequest,
    DeviceBackupCancellation, GetBlobMediaRequest, ListBookmarkedPostsRequest,
    ListJoinedPrivateChannelsRequest, ListTimelineRequest, PreviewDeviceBackupRequest,
    RestoreDeviceBackupRequest, commit_device_restore, create_device_backup,
    ensure_accounts_initialized_from_env, finalize_device_restore, install_prepared_device_restore,
    list_accounts, mark_device_restore_activated, mark_device_restore_awaiting_consent,
    prepare_device_restore, preview_device_backup,
};

use crate::*;

pub(crate) async fn run_device_backup_restore(
    root: &Path,
    scenario: &ScenarioSpec,
    artifacts_dir: &Path,
) -> Result<HarnessResult> {
    unsafe { std::env::set_var("KUKURI_DISABLE_KEYRING", "1") };
    let run_dir = tempfile::Builder::new()
        .prefix("device-backup-")
        .tempdir_in(artifacts_dir)?;
    let source_dir = run_dir.path().join("backup-source");
    let target_dir = run_dir.path().join("backup-target");
    std::fs::create_dir_all(&source_dir)?;
    std::fs::create_dir_all(&target_dir)?;
    let mut steps = Vec::new();

    let started = Instant::now();
    let source_db = ensure_accounts_initialized_from_env(&source_dir)?;
    let source_runtime = DesktopRuntime::new(&source_db).await?;
    let topic = scenario.fixtures.topic.clone();
    let post_content = "offline backup post";
    let attachment_bytes = b"portable attachment bytes";
    let post_id = source_runtime
        .create_post(CreatePostRequest {
            topic: topic.clone(),
            content: post_content.to_string(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![image_attachment_request(
                "backup.png",
                "image/png",
                attachment_bytes,
            )],
            content_labels: Vec::new(),
        })
        .await?;
    source_runtime
        .bookmark_post(BookmarkPostRequest {
            topic: topic.clone(),
            object_id: post_id.clone(),
            channel_ref: ChannelRef::Public,
        })
        .await?;
    let remote_pubkey = KukuriKeys::generate().public_key_hex();
    source_runtime
        .mute_author(AuthorRequest {
            pubkey: remote_pubkey.clone(),
        })
        .await?;
    source_runtime
        .block_author(AuthorRequest {
            pubkey: remote_pubkey.clone(),
        })
        .await?;
    let private_channel = source_runtime
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.clone(),
            label: "portable private channel".to_string(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await?;
    let source_timeline = source_runtime
        .list_timeline(ListTimelineRequest {
            topic: topic.clone(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await?;
    let source_post = source_timeline
        .items
        .iter()
        .find(|post| post.object_id == post_id)
        .context("source post missing before backup")?;
    let attachment = source_post
        .attachments
        .first()
        .context("source attachment missing before backup")?;
    let attachment_hash = attachment.hash.clone();
    // #1221 R5-G: backup は保護所有先だけを含める。旧 `iroh-data` の本人データを移してから止める。
    source_runtime.finish_protected_migration().await?;
    source_runtime.shutdown().await;
    drop(source_runtime);

    let node_config = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: "https://backup-node.example".to_string(),
            resolved_urls: None,
        }],
    };
    std::fs::write(
        source_db.with_extension("community-node.json"),
        serde_json::to_vec(&node_config)?,
    )?;
    let iroh_dir = source_db.with_extension("iroh-data");
    anyhow::ensure!(
        iroh_dir.join("endpoint-secret.json").is_file(),
        "source endpoint secret was not created"
    );
    push_named_step(&mut steps, "source_ready", started);

    let started = Instant::now();
    let backup_path = run_dir.path().join("account.kukuri-backup");
    let frontend_state = BTreeMap::from([
        ("kukuri.desktop.locale".to_string(), "ja".to_string()),
        (
            "kukuri.workspace.layout".to_string(),
            "{\"version\":1,\"columns\":[\"timeline\"]}".to_string(),
        ),
        (
            "kukuri.composer.drafts".to_string(),
            "{\"public\":\"portable draft\"}".to_string(),
        ),
        ("kukuri.desktop.theme".to_string(), "dark".to_string()),
    ]);
    create_device_backup(
        &source_dir,
        &source_db,
        &CreateDeviceBackupRequest {
            path: backup_path.display().to_string(),
            passphrase: "harness backup passphrase".to_string(),
            frontend_state: frontend_state.clone(),
        },
        &DeviceBackupCancellation::default(),
        |_| {},
    )?;
    anyhow::ensure!(backup_path.is_file(), "one-file backup was not created");
    push_named_step(&mut steps, "encrypted_backup_created", started);

    let target_db = ensure_accounts_initialized_from_env(&target_dir)?;
    let target_store = SqliteStore::connect_file(&target_db).await?;
    target_store.close().await;
    let target_before = list_accounts(&target_dir)?;

    let started = Instant::now();
    let wrong_passphrase = prepare_device_restore(
        &target_dir,
        &RestoreDeviceBackupRequest {
            path: backup_path.display().to_string(),
            passphrase: "wrong passphrase".to_string(),
            replace_existing: false,
            apply_frontend_state: true,
        },
        &DeviceBackupCancellation::default(),
        |_| {},
    );
    anyhow::ensure!(wrong_passphrase.is_err(), "wrong passphrase was accepted");
    anyhow::ensure!(
        list_accounts(&target_dir)? == target_before,
        "failed restore changed the target registry"
    );
    push_named_step(&mut steps, "wrong_passphrase_preserved_target", started);

    let started = Instant::now();
    let preview = preview_device_backup(
        &target_dir,
        &PreviewDeviceBackupRequest {
            path: backup_path.display().to_string(),
            passphrase: "harness backup passphrase".to_string(),
        },
    )?;
    anyhow::ensure!(preview.existing_account_id.is_none());
    let prepared = prepare_device_restore(
        &target_dir,
        &RestoreDeviceBackupRequest {
            path: backup_path.display().to_string(),
            passphrase: "harness backup passphrase".to_string(),
            replace_existing: false,
            apply_frontend_state: true,
        },
        &DeviceBackupCancellation::default(),
        |_| {},
    )?;
    let staged_store = SqliteStore::connect_file(prepared.staging_db_path()).await?;
    staged_store.close().await;
    let installed = install_prepared_device_restore(&target_dir, prepared)?;
    let restored_db = installed.db_path();
    let result = commit_device_restore(&installed)?;
    mark_device_restore_awaiting_consent(&target_dir)?;
    mark_device_restore_activated(&target_dir)?;
    finalize_device_restore(installed)?;
    anyhow::ensure!(result.frontend_state == frontend_state);
    anyhow::ensure!(
        !restored_db
            .with_extension("iroh-data")
            .join("endpoint-secret.json")
            .exists(),
        "device-bound endpoint secret was restored"
    );
    anyhow::ensure!(list_accounts(&target_dir)?.active_account_id == result.account.id);

    let restored_runtime = DesktopRuntime::new(&restored_db).await?;
    let restored_timeline = restored_runtime
        .list_timeline(ListTimelineRequest {
            topic: topic.clone(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await?;
    let restored_post = restored_timeline
        .items
        .iter()
        .find(|post| post.object_id == post_id && post.content == post_content)
        .context("restored offline post missing")?;
    anyhow::ensure!(
        restored_post
            .attachments
            .iter()
            .any(|item| item.hash == attachment_hash),
        "restored attachment metadata missing"
    );
    let restored_payload = restored_runtime
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: attachment_hash,
            mime: "image/png".to_string(),
            source_object_id: None,
        })
        .await?
        .context("restored attachment payload missing")?;
    anyhow::ensure!(
        BASE64_STANDARD.decode(restored_payload.bytes_base64.as_bytes())? == attachment_bytes
    );
    anyhow::ensure!(
        restored_runtime
            .list_bookmarked_posts_page(ListBookmarkedPostsRequest {
                cursor: None,
                before: false,
            })
            .await?
            .items
            .iter()
            .any(|bookmark| bookmark.post.object_id == post_id),
        "restored bookmark missing"
    );
    let restored_social = restored_runtime
        .get_author_social_view(AuthorRequest {
            pubkey: remote_pubkey,
        })
        .await?;
    anyhow::ensure!(restored_social.muted, "restored mute state missing");
    anyhow::ensure!(restored_social.blocking, "restored block state missing");
    anyhow::ensure!(
        restored_runtime
            .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                topic: topic.clone(),
            })
            .await?
            .iter()
            .any(|channel| channel.channel_id == private_channel.channel_id),
        "restored private channel missing"
    );
    anyhow::ensure!(
        restored_runtime.get_community_node_config().await? == node_config,
        "restored Community Node config mismatch"
    );
    anyhow::ensure!(
        !restored_runtime
            .get_content_display_settings()
            .adult_content_enabled,
        "adult content display was restored as enabled"
    );
    restored_runtime.shutdown().await;
    drop(restored_runtime);
    push_named_step(&mut steps, "restored_and_activated", started);

    let run_dir = run_dir.keep();
    let result = HarnessResult {
        status: HarnessStatus::Pass,
        scenario: scenario.name.clone(),
        steps,
        artifacts: vec![run_dir.join("account.kukuri-backup").display().to_string()],
        metrics_snapshot: None,
    };
    write_result_artifact(root, artifacts_dir, &result)?;
    Ok(result)
}
