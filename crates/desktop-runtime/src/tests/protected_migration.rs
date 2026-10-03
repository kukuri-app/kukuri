//! #1221 R5-G: 旧 `iroh-data` の本人データを保護所有先へ移し、旧領域を含めない backup で復元する(実 Iroh)。

use super::*;

use std::collections::BTreeMap;
use std::path::PathBuf;

use kukuri_blob_service::BlobService;
use kukuri_core::{
    ChannelId, DirectMessageAttachmentKind, DirectMessageAttachmentManifestV1,
    DirectMessageEncryptedBlobRefV1, DirectMessagePayloadV1, EnvelopeId, TopicId,
    build_post_envelope, direct_message_id_for_participants, encrypt_direct_message_attachment,
    encrypt_direct_message_frame,
};
use kukuri_store::{
    BookmarkedCustomReactionRow, DirectMessageMessageRow, DirectMessageOutboxRow,
    DirectMessageStore, LiveGameProjectionStore, ObjectProjectionStore, ReactionBookmarkStore,
    SqliteStore, Store,
};

use super::legacy_store_retirement::{create_empty_legacy_store, into_legacy_layout};
use crate::accounts::ensure_accounts_initialized;
use crate::backup::{
    CreateDeviceBackupRequest, DeviceBackupCancellation, RestoreDeviceBackupRequest,
    commit_device_restore, create_device_backup, finalize_device_restore,
    install_prepared_device_restore, mark_device_restore_activated,
    mark_device_restore_awaiting_consent, prepare_device_restore,
};

pub(super) const TOPIC: &str = "kukuri:topic:protected-migration";

pub(super) async fn open_runtime(db: &Path) -> DesktopRuntime {
    DesktopRuntime::new_with_config_and_identity(
        db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime")
}

/// runtime が読むだけに開いた旧 store の blob(#1221 R5-I: node は新しい store)。旧 store が無ければ `None`。
async fn legacy_blob(runtime: &DesktopRuntime, hash: &str) -> Option<Vec<u8>> {
    let legacy = runtime.legacy_store.lock().await.clone()?;
    legacy.read_blob(hash).await.expect("legacy blob read")
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

/// backup を作り、`target` の新しい account として復元する。archive の entry 名と、復元した db の path を返す。
pub(super) async fn backup_and_restore(
    source: &Path,
    db: &Path,
    target: &Path,
) -> (Vec<String>, PathBuf) {
    let archive_dir = tempdir().expect("archive dir");
    let archive = archive_dir.path().join("account.kukuri-backup");
    create_device_backup(
        source,
        db,
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
    ensure_accounts_initialized(target, IdentityStorageMode::FileOnly)
        .await
        .expect("target account");
    let prepared = prepare_device_restore(
        target,
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
    let installed = install_prepared_device_restore(target, prepared).expect("install");
    let restored_db = installed.db_path();
    commit_device_restore(&installed).expect("commit");
    mark_device_restore_awaiting_consent(target).expect("awaiting consent");
    mark_device_restore_activated(target).expect("activated");
    finalize_device_restore(installed).expect("finalize");
    (names, restored_db)
}

/// 旧領域に置いた本人データ。hash は旧 blob store(`iroh-data`)にある。
pub(super) struct Fixture {
    pub(super) post_id: String,
    pub(super) attachment: (String, Vec<u8>),
    pub(super) body_hash: String,
    pub(super) avatar: (String, String),
    pub(super) reaction_asset: String,
    pub(super) reaction_bookmark: String,
    pub(super) frame_hash: String,
    pub(super) encrypted_attachment: String,
    pub(super) plain_attachment: String,
    pub(super) dome_pin: String,
    pub(super) live_manifest: String,
    pub(super) channel_id: String,
}

pub(super) async fn seed_legacy_data(runtime: &DesktopRuntime) -> Fixture {
    // 索引の先頭 128 行を他人の envelope にし、本人の投稿を 2 ページ目に置く。
    let remote = KukuriKeys::generate();
    for index in 0..200 {
        runtime
            .sqlite
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
        .sqlite
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
        .sqlite
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
        .sqlite
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
        .sqlite
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
        .sqlite
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
        .await
        .expect("source account");
    // #1221 R5-I: 更新前の端末の保存状態(旧 store に本人のデータ、保護所有先は空)を作る。
    create_empty_legacy_store(&db).await;
    let runtime = open_runtime(&db).await;
    let fixture = seed_legacy_data(&runtime).await;
    runtime.shutdown().await;
    drop(runtime);
    into_legacy_layout(&db).await;
    let runtime = open_runtime(&db).await;
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
            .sqlite
            .get_remote_records(&topic_replica, &own_record, None, 8, false)
            .await
            .expect("records")
            .is_empty()
    );
    assert!(
        is_protected(&runtime.sqlite, &fixture.body_hash).await,
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
            .sqlite
            .protected_migration_caught_up_at()
            .await
            .expect("caught up")
            .is_some()
    );
    assert!(
        !runtime
            .sqlite
            .get_remote_records(&topic_replica, &own_record, None, 8, false)
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
    let channels = runtime
        .app_service
        .joined_private_channel_replicas("", 16)
        .await
        .unwrap();
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
                    .sqlite
                    .get_remote_records(channel_replica.as_str(), key, None, 8, false)
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
            .sqlite
            .get_remote_content("blob", hash)
            .await
            .expect("copied bytes")
            .unwrap_or_else(|| panic!("{hash} was not moved"));
        assert_eq!(copied, legacy);
        assert_eq!(blake3::hash(&copied).to_hex().as_str(), hash.as_str());
        assert!(
            is_protected(&runtime.sqlite, hash).await,
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
    let target = tempdir().expect("target dir");
    let (names, restored_db) = backup_and_restore(source.path(), &db, target.path()).await;
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
                cursor: None,
            })
            .await
            .expect("restored channels")
            .items
            .iter()
            .any(|channel| channel.channel_id == fixture.channel_id)
    );
    let outbox = restored
        .sqlite
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
    .fetch_one(restored.sqlite.pool())
    .await
    .expect("protected private records");
    assert_eq!(protected_metadata, 2);
    for hash in [
        &fixture.attachment.0,
        &fixture.live_manifest,
        &fixture.frame_hash,
    ] {
        assert!(
            is_protected(&restored.sqlite, hash).await,
            "{hash} lost protection"
        );
    }
    restored.shutdown().await;
}

/// #1221 R5-I: pin した asset は、pin したときに `dome_pin:` の保護参照で保護所有先に置く(保護移行の手順を使わない)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pinned_asset_is_protected_when_pinned() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().expect("dir");
    let db = ensure_accounts_initialized(dir.path(), IdentityStorageMode::FileOnly)
        .await
        .expect("account");
    let runtime = open_runtime(&db).await;
    let blobs = runtime.iroh_stack.blob_service.clone();
    let asset = blobs
        .put_remote_blob(b"visited dome asset".to_vec(), "model/gltf-binary")
        .await
        .expect("cached asset");
    assert!(!is_protected(&runtime.sqlite, asset.hash.as_str()).await);
    blobs.pin_blob(&asset.hash).await.expect("pin");
    assert!(is_protected(&runtime.sqlite, asset.hash.as_str()).await);
    let refs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM remote_content_cache_protected_ref WHERE ref_id = ?1",
    )
    .bind(format!("dome_pin:{}", asset.hash.as_str()))
    .fetch_one(runtime.sqlite.pool())
    .await
    .expect("refs");
    assert_eq!(refs, 1);
    runtime.shutdown().await;
}

// 本人の envelope 行は docs の record より先に入る。record がまだ無い新しい行の手前で止まり、揃ってから写す(監査の指摘)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_envelope_waits_for_its_docs_records() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().expect("dir");
    let db = ensure_accounts_initialized(dir.path(), IdentityStorageMode::FileOnly)
        .await
        .expect("account");
    create_empty_legacy_store(&db).await;
    let runtime = open_runtime(&db).await;
    let envelope = build_post_envelope(
        runtime.author_keys.as_ref(),
        &TopicId::new(TOPIC),
        "records are not written yet",
        None,
    )
    .expect("own envelope");
    runtime
        .sqlite
        .put_envelope(envelope.clone())
        .await
        .expect("envelope row before its records");
    for _ in 0..3 {
        assert!(
            !runtime.protected_migration_step().await.expect("step"),
            "a new own row without records keeps the migration from catching up"
        );
    }
    let reference = format!("own:{}", envelope.id.as_str());
    let refs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM remote_content_cache_protected_ref WHERE ref_id = ?1",
    )
    .bind(&reference)
    .fetch_one(runtime.sqlite.pool())
    .await
    .expect("refs");
    assert_eq!(refs, 0, "the row is not treated as migrated");
    runtime.shutdown().await;
}

/// #1221 R5-H AC-1: 移行が終端へ達するまで旧 writer のまま。達したら 1 回だけ切り替わり、再起動しても同じ時刻を保つ。
#[tokio::test]
async fn the_writer_switches_once_after_the_migration_and_keeps_it_across_restarts() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().expect("dir");
    let db = ensure_accounts_initialized(dir.path(), IdentityStorageMode::FileOnly)
        .await
        .expect("account");
    create_empty_legacy_store(&db).await;
    let runtime = open_runtime(&db).await;
    assert_eq!(runtime.app_service.writer_switched_at(), None);
    runtime
        .finish_protected_migration()
        .await
        .expect("finish migration");
    let switched = runtime
        .app_service
        .writer_switched_at()
        .expect("switched after the migration");
    runtime.shutdown().await;
    drop(runtime);

    let runtime = open_runtime(&db).await;
    assert_eq!(runtime.app_service.writer_switched_at(), Some(switched));
    runtime.shutdown().await;
}

/// #1221 R5-H: 切替後に書いた本人の投稿も、bucket の record・author bucket のプロフィールの行・本文の blob が
/// 保護所有先へ入り、旧領域を含めない backup → restore で戻る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_posts_written_after_the_switch_are_protected_and_restored() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let source = tempdir().expect("source dir");
    let db = ensure_accounts_initialized(source.path(), IdentityStorageMode::FileOnly)
        .await
        .expect("source account");
    let runtime = open_runtime(&db).await;
    let channel = runtime
        .create_private_channel(CreatePrivateChannelRequest {
            topic: TOPIC.into(),
            label: "after the switch".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("private channel");
    runtime
        .finish_protected_migration()
        .await
        .expect("finish migration");
    assert!(runtime.app_service.writer_switched_at().is_some());
    let mut posts = Vec::new();
    for (content, channel_ref) in [
        ("public after the switch", ChannelRef::Public),
        (
            "private after the switch",
            ChannelRef::PrivateChannel {
                channel_id: ChannelId::new(channel.channel_id.clone()),
            },
        ),
    ] {
        let post_id = runtime
            .create_post(CreatePostRequest {
                topic: TOPIC.into(),
                content: content.into(),
                reply_to: None,
                channel_ref,
                attachments: Vec::new(),
                content_labels: Vec::new(),
            })
            .await
            .expect("own post after the switch");
        let projection = runtime
            .sqlite
            .get_object_projection(&EnvelopeId::from(post_id.as_str()))
            .await
            .expect("projection")
            .expect("own projection");
        assert!(
            projection
                .source_replica_id
                .as_str()
                .starts_with("bucket::")
        );
        posts.push((post_id, projection));
    }
    tokio::time::timeout(
        Duration::from_secs(30),
        runtime.finish_protected_migration(),
    )
    .await
    .expect("posts after the switch do not hold the migration back")
    .expect("migrate posts after the switch");
    let local = runtime.author_keys.public_key_hex();
    let profile_replica = kukuri_docs_sync::BucketReplica::new(
        kukuri_docs_sync::BucketScope::Author {
            author_pubkey: local.clone(),
        },
        kukuri_docs_sync::TimeBucket::from_unix_seconds(posts[0].1.created_at).expect("bucket"),
    )
    .expect("author bucket")
    .replica_id();
    let mut protected_records = vec![(
        profile_replica.clone(),
        kukuri_docs_sync::stable_key("profile/posts", &posts[0].0),
    )];
    for (post_id, projection) in &posts {
        protected_records.push((
            projection.source_replica_id.clone(),
            format!("objects/{post_id}/envelope"),
        ));
    }
    for (replica, key) in &protected_records {
        assert!(
            !runtime
                .sqlite
                .get_remote_records(replica.as_str(), key, None, 8, false)
                .await
                .expect("records")
                .is_empty(),
            "{replica:?} {key} was not moved"
        );
    }
    runtime.shutdown().await;
    drop(runtime);

    let target = tempdir().expect("target dir");
    let (_, restored_db) = backup_and_restore(source.path(), &db, target.path()).await;
    let restored = open_runtime(&restored_db).await;
    for (replica, key) in &protected_records {
        assert!(
            !restored
                .sqlite
                .get_remote_records(replica.as_str(), key, None, 8, false)
                .await
                .expect("restored records")
                .is_empty(),
            "{replica:?} {key} was not restored"
        );
    }
    for (scope, content) in [
        (TimelineScope::Public, "public after the switch"),
        (
            TimelineScope::Channel {
                channel_id: ChannelId::new(channel.channel_id.clone()),
            },
            "private after the switch",
        ),
    ] {
        let timeline = restored
            .list_timeline(ListTimelineRequest {
                topic: TOPIC.into(),
                scope,
                cursor: None,
                limit: Some(20),
            })
            .await
            .expect("restored timeline");
        assert!(
            timeline.items.iter().any(|post| post.content == content),
            "{content} was not restored"
        );
    }
    restored.shutdown().await;
}
