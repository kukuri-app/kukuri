//! #1221 R5-G: 旧 `iroh-data` の本人データを保護所有先へ移し、旧領域を含めない backup で復元する(実 Iroh)。

use super::*;

use std::collections::BTreeMap;

use kukuri_blob_service::BlobService;
use kukuri_core::{
    DirectMessageAttachmentKind, DirectMessageAttachmentManifestV1,
    DirectMessageEncryptedBlobRefV1, DirectMessagePayloadV1, EnvelopeId, TopicId,
    build_post_envelope, direct_message_id_for_participants, encrypt_direct_message_attachment,
    encrypt_direct_message_frame,
};
use kukuri_store::{
    BookmarkedCustomReactionRow, DirectMessageMessageRow, DirectMessageOutboxRow,
    DirectMessageStore, LiveGameProjectionStore, ObjectProjectionStore, ReactionBookmarkStore,
    SqliteStore, Store,
};

use crate::accounts::ensure_accounts_initialized;
use crate::backup::{
    CreateDeviceBackupRequest, DeviceBackupCancellation, RestoreDeviceBackupRequest,
    commit_device_restore, create_device_backup, finalize_device_restore,
    install_prepared_device_restore, mark_device_restore_activated,
    mark_device_restore_awaiting_consent, prepare_device_restore,
};

const TOPIC: &str = "kukuri:topic:protected-migration";

async fn open_runtime(db: &Path) -> DesktopRuntime {
    DesktopRuntime::new_with_config_and_identity(
        db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime")
}

async fn legacy_blob(runtime: &DesktopRuntime, hash: &str) -> Option<Vec<u8>> {
    let current = runtime.iroh_stack.current.lock().await;
    let node = current.as_ref().expect("stack").node.clone();
    drop(current);
    node.read_local_blob(hash).await.expect("legacy blob read")
}

async fn is_protected(store: &SqliteStore, hash: &str) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT is_protected FROM remote_content_cache WHERE kind = 'blob' AND cache_key = ?1",
    )
    .bind(hash)
    .fetch_optional(store.pool())
    .await
    .expect("protected flag")
        == Some(1)
}

/// 旧領域に置いた本人データ。hash は旧 blob store(`iroh-data`)にある。
struct Fixture {
    post_id: String,
    attachment: (String, Vec<u8>),
    body_hash: String,
    avatar: (String, String),
    reaction_asset: String,
    reaction_bookmark: String,
    frame_hash: String,
    encrypted_attachment: String,
    plain_attachment: String,
    dome_pin: String,
    live_manifest: String,
    channel_id: String,
}

async fn seed_legacy_data(runtime: &DesktopRuntime) -> Fixture {
    // 索引の先頭 128 行を他人の envelope にし、本人の投稿を 2 ページ目に置く。
    let remote = KukuriKeys::generate();
    for index in 0..200 {
        runtime
            .store
            .put_envelope(
                build_post_envelope(
                    &remote,
                    &TopicId::new(TOPIC),
                    &format!("remote {index}"),
                    None,
                )
                .expect("remote envelope"),
            )
            .await
            .expect("store remote envelope");
    }
    let attachment_bytes = vec![9u8; 1024 * 1024 + 17];
    let post_id = runtime
        .create_post(CreatePostRequest {
            topic: TOPIC.into(),
            content: "protected own post".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![image_attachment_request(
                "large.png",
                "image/png",
                &attachment_bytes,
            )],
            content_labels: Vec::new(),
        })
        .await
        .expect("own post");
    let projection = runtime
        .store
        .get_object_projection(&EnvelopeId::from(post_id.as_str()))
        .await
        .expect("projection")
        .expect("own projection");
    let body_hash = match &projection.payload_ref {
        kukuri_core::PayloadRef::BlobText { hash, .. } => hash.as_str().to_string(),
        other => panic!("unexpected payload {other:?}"),
    };
    let attachment_hash = projection.attachments[0].hash.as_str().to_string();
    // 本人投稿と bookmark が同じ blob を指す。
    runtime
        .bookmark_post(BookmarkPostRequest {
            topic: TOPIC.into(),
            object_id: post_id.clone(),
            channel_ref: ChannelRef::Public,
        })
        .await
        .expect("bookmark own post");
    let profile = runtime
        .set_my_profile(SetMyProfileRequest {
            name: Some("migration".into()),
            display_name: None,
            about: None,
            picture_upload: Some(profile_avatar_attachment_request(
                "avatar.png",
                "image/png",
                &png_source_bytes(),
            )),
            clear_picture: false,
        })
        .await
        .expect("avatar");
    let avatar = profile.picture_asset.expect("avatar asset");
    let asset = runtime
        .create_custom_reaction_asset(CreateCustomReactionAssetRequest {
            upload: image_attachment_request("reaction.png", "image/png", &png_source_bytes()),
            crop_rect: CustomReactionCropRect {
                x: 70,
                y: 0,
                size: 180,
            },
            search_key: "migration".into(),
        })
        .await
        .expect("own reaction asset");
    // 他人の custom reaction を bookmark した(bytes は表示のときに旧領域へ入った)。
    let blobs = runtime.iroh_stack.blob_service.clone();
    let remote_asset = blobs
        .put_blob(b"remote reaction asset".to_vec(), "image/png")
        .await
        .expect("remote reaction asset");
    runtime
        .store
        .put_bookmarked_custom_reaction(BookmarkedCustomReactionRow {
            asset_id: "remote-asset".into(),
            owner_pubkey: remote.public_key_hex(),
            blob_hash: remote_asset.hash.clone(),
            search_key: "remote".into(),
            mime: "image/png".into(),
            bytes: 21,
            width: 128,
            height: 128,
            bookmarked_at: 1,
        })
        .await
        .expect("bookmark remote reaction");
    let channel = runtime
        .create_private_channel(CreatePrivateChannelRequest {
            topic: TOPIC.into(),
            label: "migration channel".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("private channel");
    let session_id = runtime
        .create_live_session(CreateLiveSessionRequest {
            topic: TOPIC.into(),
            channel_ref: ChannelRef::Public,
            title: "migration live".into(),
            description: String::new(),
        })
        .await
        .expect("live session");
    let live_manifest = runtime
        .store
        .get_live_session(TOPIC, &session_id)
        .await
        .expect("live row")
        .expect("live session row")
        .manifest_blob_hash
        .as_str()
        .to_string();

    // 未 ACK の送信待ち DM(frame と暗号化添付は旧領域、平文添付は履歴の行が指す)。
    let keys = runtime.author_keys.as_ref();
    let peer = KukuriKeys::generate().public_key();
    let dm_id = direct_message_id_for_participants(&keys.public_key(), &peer);
    let plain = blobs
        .put_blob(b"plain dm attachment".to_vec(), "image/png")
        .await
        .expect("plain dm attachment");
    let encrypted = encrypt_direct_message_attachment(
        keys,
        &peer,
        "message-1",
        "original",
        b"plain dm attachment",
    )
    .expect("encrypt attachment");
    let encrypted_blob = blobs
        .put_blob(serde_json::to_vec(&encrypted).unwrap(), "application/json")
        .await
        .expect("encrypted attachment");
    let blob_ref = |hash: &kukuri_core::BlobHash, nonce: &str| DirectMessageEncryptedBlobRefV1 {
        blob_id: "original".into(),
        hash: hash.clone(),
        mime: "image/png".into(),
        bytes: 19,
        nonce_hex: nonce.into(),
    };
    let manifest = |blob: DirectMessageEncryptedBlobRefV1| DirectMessageAttachmentManifestV1 {
        attachment_id: "attachment-1".into(),
        kind: DirectMessageAttachmentKind::Image,
        original: blob,
        poster: None,
    };
    let frame = encrypt_direct_message_frame(
        keys,
        &peer,
        &dm_id,
        "message-1",
        1,
        &DirectMessagePayloadV1 {
            text: Some("pending".into()),
            reply_to: None,
            attachment_manifest: Some(manifest(blob_ref(
                &encrypted_blob.hash,
                &encrypted.nonce_hex,
            ))),
        },
    )
    .expect("frame");
    let frame_blob = blobs
        .put_blob(serde_json::to_vec(&frame).unwrap(), "application/json")
        .await
        .expect("frame blob");
    runtime
        .store
        .put_direct_message_message(DirectMessageMessageRow {
            dm_id: dm_id.clone(),
            message_id: "message-1".into(),
            sender_pubkey: keys.public_key_hex(),
            recipient_pubkey: peer.as_str().to_string(),
            created_at: 1,
            text: Some("pending".into()),
            reply_to_message_id: None,
            attachment_manifest: Some(manifest(blob_ref(&plain.hash, ""))),
            outgoing: true,
            acked_at: None,
        })
        .await
        .expect("dm history");
    runtime
        .store
        .put_direct_message_outbox(DirectMessageOutboxRow {
            dm_id,
            message_id: "message-1".into(),
            peer_pubkey: peer.as_str().to_string(),
            frame_blob_hash: frame_blob.hash.clone(),
            created_at: 1,
            last_attempt_at: None,
        })
        .await
        .expect("unacked outbox");
    // 自作 Dome の asset(旧 blob store の pin tag が索引)。
    let dome_asset = blobs
        .put_blob(b"own dome asset".to_vec(), "model/gltf-binary")
        .await
        .expect("dome asset");
    blobs
        .pin_blob(&dome_asset.hash)
        .await
        .expect("pin dome asset");

    Fixture {
        post_id,
        attachment: (attachment_hash, attachment_bytes),
        body_hash,
        avatar: (avatar.hash.as_str().to_string(), avatar.mime),
        reaction_asset: asset.blob_hash,
        reaction_bookmark: remote_asset.hash.as_str().to_string(),
        frame_hash: frame_blob.hash.as_str().to_string(),
        encrypted_attachment: encrypted_blob.hash.as_str().to_string(),
        plain_attachment: plain.hash.as_str().to_string(),
        dome_pin: dome_asset.hash.as_str().to_string(),
        live_manifest,
        channel_id: channel.channel_id,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_protected_data_moves_in_pages_and_restores_without_the_legacy_tree() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let source = tempdir().expect("source dir");
    let db = ensure_accounts_initialized(source.path(), IdentityStorageMode::FileOnly)
        .expect("source account");
    let runtime = open_runtime(&db).await;
    let fixture = seed_legacy_data(&runtime).await;
    let own_record = format!("objects/{}/envelope", fixture.post_id);
    let topic_replica = format!("topic::{TOPIC}");

    // 1 ステップは各 kind の 1 ページ(128 行まで)。本人投稿の envelope は 2 ページ目にある。
    assert!(
        !runtime
            .protected_migration_step()
            .await
            .expect("first step")
    );
    assert!(
        runtime
            .store
            .get_remote_records(&topic_replica, &own_record, None, 8)
            .await
            .expect("records")
            .is_empty()
    );
    assert!(
        is_protected(&runtime.store, &fixture.body_hash).await,
        "bookmark page shares the body"
    );
    runtime.shutdown().await;
    drop(runtime);

    // ページを保存した後に止めても、再起動した runtime が続きから写し終える。
    let runtime = open_runtime(&db).await;
    runtime
        .finish_protected_migration()
        .await
        .expect("finish migration");
    assert!(
        runtime
            .store
            .protected_migration_caught_up_at()
            .await
            .expect("caught up")
            .is_some()
    );
    assert!(
        !runtime
            .store
            .get_remote_records(&topic_replica, &own_record, None, 8)
            .await
            .expect("records")
            .is_empty(),
        "own post envelope record was not moved"
    );
    // 追いついた後に参加状態が変わると、private を先頭から読み直して新しい channel も移す。
    runtime
        .create_private_channel(CreatePrivateChannelRequest {
            topic: TOPIC.into(),
            label: "joined after catch-up".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("second private channel");
    runtime
        .finish_protected_migration()
        .await
        .expect("migrate the new channel");
    let channels = runtime.app_service.joined_private_channel_replicas().await;
    assert_eq!(channels.len(), 2);
    assert!(
        channels
            .iter()
            .any(|(key, _)| key.contains(&fixture.channel_id))
    );
    for (_, channel_replica) in &channels {
        for key in ["channels/metadata", "channels/policy/envelope"] {
            assert!(
                !runtime
                    .store
                    .get_remote_records(channel_replica.as_str(), key, None, 8)
                    .await
                    .expect("private records")
                    .is_empty(),
                "private {key} was not moved"
            );
        }
    }
    for hash in [
        &fixture.body_hash,
        &fixture.attachment.0,
        &fixture.avatar.0,
        &fixture.reaction_asset,
        &fixture.reaction_bookmark,
        &fixture.frame_hash,
        &fixture.encrypted_attachment,
        &fixture.plain_attachment,
        &fixture.dome_pin,
        &fixture.live_manifest,
    ] {
        let legacy = legacy_blob(&runtime, hash).await.expect("legacy bytes");
        let copied = runtime
            .store
            .get_remote_content("blob", hash)
            .await
            .expect("copied bytes")
            .unwrap_or_else(|| panic!("{hash} was not moved"));
        assert_eq!(copied, legacy);
        assert_eq!(blake3::hash(&copied).to_hex().as_str(), hash.as_str());
        assert!(
            is_protected(&runtime.store, hash).await,
            "{hash} is not protected"
        );
    }
    // 1MiB を超える添付は `kukuri.remote-blobs/` の file に置く。
    assert!(
        db.with_extension("remote-blobs")
            .join(&fixture.attachment.0)
            .is_file()
    );
    runtime.shutdown().await;
    drop(runtime);

    // backup は旧 `iroh-data` を含めず、保護所有先を含める。
    let archive_dir = tempdir().expect("archive dir");
    let archive = archive_dir.path().join("account.kukuri-backup");
    create_device_backup(
        source.path(),
        &db,
        &CreateDeviceBackupRequest {
            path: archive.display().to_string(),
            passphrase: "protected migration passphrase".into(),
            frontend_state: BTreeMap::new(),
        },
        &DeviceBackupCancellation::default(),
        |_| {},
    )
    .expect("create backup");
    let reader = kukuri_core::DeviceBackupReader::open(
        std::fs::File::open(&archive).expect("open archive"),
        "protected migration passphrase",
    )
    .expect("read archive");
    let names = reader
        .manifest()
        .entries
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    assert!(
        names.iter().all(|name| !name.contains("iroh-data")),
        "{names:?}"
    );
    assert!(
        names.contains(&format!(
            "file/kukuri.remote-blobs/{}",
            fixture.attachment.0
        )),
        "{names:?}"
    );

    let target = tempdir().expect("target dir");
    ensure_accounts_initialized(target.path(), IdentityStorageMode::FileOnly)
        .expect("target account");
    let prepared = prepare_device_restore(
        target.path(),
        &RestoreDeviceBackupRequest {
            path: archive.display().to_string(),
            passphrase: "protected migration passphrase".into(),
            replace_existing: false,
            apply_frontend_state: false,
        },
        &DeviceBackupCancellation::default(),
        |_| {},
    )
    .expect("prepare restore");
    let installed = install_prepared_device_restore(target.path(), prepared).expect("install");
    let restored_db = installed.db_path();
    commit_device_restore(&installed).expect("commit");
    mark_device_restore_awaiting_consent(target.path()).expect("awaiting consent");
    mark_device_restore_activated(target.path()).expect("activated");
    finalize_device_restore(installed).expect("finalize");

    let restored = open_runtime(&restored_db).await;
    assert!(
        legacy_blob(&restored, &fixture.attachment.0)
            .await
            .is_none(),
        "the restored account must not carry the legacy blob store"
    );
    let timeline = restored
        .list_timeline(ListTimelineRequest {
            topic: TOPIC.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("restored timeline");
    assert!(
        timeline
            .items
            .iter()
            .any(|post| post.object_id == fixture.post_id && post.content == "protected own post")
    );
    for (hash, mime, bytes) in [
        (
            &fixture.attachment.0,
            "image/png",
            Some(fixture.attachment.1.clone()),
        ),
        (&fixture.avatar.0, fixture.avatar.1.as_str(), None),
    ] {
        let payload = restored
            .get_blob_media_payload(GetBlobMediaRequest {
                hash: hash.clone(),
                mime: mime.into(),
                source_object_id: None,
            })
            .await
            .expect("restored payload")
            .expect("restored payload bytes");
        if let Some(bytes) = bytes {
            assert_eq!(BASE64_STANDARD.decode(payload.bytes_base64).unwrap(), bytes);
        }
    }
    assert!(
        restored
            .list_bookmarked_posts_page(ListBookmarkedPostsRequest {
                cursor: None,
                before: false,
            })
            .await
            .expect("restored bookmarks")
            .items
            .iter()
            .any(|item| item.post.object_id == fixture.post_id)
    );
    assert!(
        restored
            .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                topic: TOPIC.into(),
            })
            .await
            .expect("restored channels")
            .iter()
            .any(|channel| channel.channel_id == fixture.channel_id)
    );
    let outbox = restored
        .store
        .list_direct_message_outbox()
        .await
        .expect("restored outbox");
    assert_eq!(outbox.len(), 1, "the unacked outbox row must survive");
    let frame = restored
        .iroh_stack
        .blob_service
        .fetch_local_blob(&outbox[0].frame_blob_hash)
        .await
        .expect("restored frame")
        .expect("restored frame bytes");
    assert_eq!(
        blake3::hash(&frame).to_hex().as_str(),
        fixture.frame_hash.as_str()
    );
    // 復元した account の旧領域は空。起動時に private を先頭から読み直しても、移行は保護を減らさない。
    restored
        .finish_protected_migration()
        .await
        .expect("migration after restore");
    let protected_metadata: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM remote_content_cache \
         WHERE kind = 'record' AND record_key = 'channels/metadata' AND is_protected = 1",
    )
    .fetch_one(restored.store.pool())
    .await
    .expect("protected private records");
    assert_eq!(protected_metadata, 2);
    for hash in [
        &fixture.attachment.0,
        &fixture.live_manifest,
        &fixture.frame_hash,
    ] {
        assert!(
            is_protected(&restored.store, hash).await,
            "{hash} lost protection"
        );
    }
    restored.shutdown().await;
}
