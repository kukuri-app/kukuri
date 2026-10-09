//! #1221 R5-I: 本人の書込みを保護所有先へ直接入れ、旧 iroh store を新しい store へ有界に移してから退役させる。

use super::*;

use kukuri_blob_service::BlobService;
use kukuri_core::EnvelopeId;
use kukuri_docs_sync::{DocOp, author_replica_id};
use kukuri_store::{
    DirectMessageOutboxRow, DirectMessageStore, ObjectProjectionStore, SqliteStore,
};

use crate::accounts::ensure_accounts_initialized;

const TOPIC: &str = "kukuri:topic:legacy-store-retirement";
use super::protected_migration::TOPIC as FIXTURE_TOPIC;

async fn open_runtime(db: &Path) -> DesktopRuntime {
    DesktopRuntime::new_with_config_and_identity(
        db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime")
}

/// 更新前の端末の旧 store(空)を置く。runtime は旧 store があると移行を済んだものとせず、旧形式の writer のまま書く。
pub(super) async fn create_empty_legacy_store(db: &Path) {
    let node = kukuri_iroh_node::IrohDocsNode::persistent(db.with_extension("iroh-data"))
        .await
        .expect("legacy node");
    node.shutdown().await.expect("legacy node shutdown");
}

/// 止めた runtime が書いた内容を、更新前(R5-I より前)の端末の保存状態へ変える。新しい store を旧 root へ移し、
/// 保護所有先の blob を旧 blob store へ戻し、保護参照と保護移行・writer の切替・退役の台帳を消す。
pub(super) async fn into_legacy_layout(db: &Path) {
    let legacy = db.with_extension("iroh-data");
    if legacy.exists() {
        std::fs::remove_dir_all(&legacy).expect("remove the empty legacy store");
    }
    std::fs::rename(db.with_extension("iroh-store"), &legacy)
        .expect("move the store to the legacy root");
    let store = SqliteStore::connect_file(db).await.expect("store");
    let node = kukuri_iroh_node::IrohDocsNode::persistent(&legacy)
        .await
        .expect("legacy node");
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT cache_key, file_name FROM remote_content_cache WHERE kind = 'blob' AND is_protected = 1",
    )
    .fetch_all(store.pool())
    .await
    .expect("protected blobs");
    for (hash, file_name) in rows {
        let bytes = store
            .get_remote_content("blob", &hash)
            .await
            .expect("protected bytes")
            .expect("protected blob");
        node.blobs()
            .blobs()
            .add_bytes(bytes)
            .await
            .expect("legacy blob");
        if let Some(name) = file_name {
            std::fs::remove_file(db.with_extension("remote-blobs").join(name))
                .expect("remove the protected file");
        }
    }
    node.shutdown().await.expect("legacy node shutdown");
    for statement in [
        "DELETE FROM remote_content_cache WHERE is_protected = 1",
        "DELETE FROM remote_content_cache_protected_ref",
        "DELETE FROM protected_migration",
        "DELETE FROM writer_cutover",
        "DELETE FROM legacy_store_retirement",
    ] {
        sqlx::query(statement)
            .execute(store.pool())
            .await
            .expect(statement);
    }
    store.close().await;
}

async fn protected(store: &SqliteStore, kind: &str, scope: Option<&str>, key: &str) -> bool {
    let flag: Option<i64> = match scope {
        Some(scope) => sqlx::query_scalar(
            "SELECT is_protected FROM remote_content_cache \
             WHERE kind = ?1 AND scope_key = ?2 AND record_key = ?3",
        )
        .bind(kind)
        .bind(scope)
        .bind(key),
        None => sqlx::query_scalar(
            "SELECT is_protected FROM remote_content_cache WHERE kind = ?1 AND cache_key = ?2",
        )
        .bind(kind)
        .bind(key),
    }
    .fetch_optional(store.pool())
    .await
    .expect("protected flag");
    flag == Some(1)
}

async fn node_blob(runtime: &DesktopRuntime, hash: &str) -> Option<Vec<u8>> {
    let node = runtime
        .iroh_stack
        .current
        .lock()
        .await
        .as_ref()
        .expect("stack")
        .node
        .clone();
    node.read_local_blob(hash).await.expect("node blob read")
}

/// AC-2: 本人の書込みは、書いたときに保護参照つきで保護所有先へ入る(保護移行の手順を使わない)。
/// 他人の author の領域へ置いた行は入れない。送信待ちの frame は ACK で保護が外れる。
/// #1690: 複数枚の画像と動画も、投稿したときにすべて入る(閲覧者が取得しない 5 枚目以降も投稿者の端末に残る)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_writes_go_to_the_protected_owner_when_written() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().expect("dir");
    let db = ensure_accounts_initialized(dir.path(), IdentityStorageMode::FileOnly)
        .await
        .expect("account");
    let runtime = open_runtime(&db).await;
    let large = vec![7u8; 1024 * 1024 + 5];
    let mut attachments = vec![image_attachment_request("large.png", "image/png", &large)];
    attachments.extend((1..5u8).map(|n| image_attachment_request("small.png", "image/png", &[n])));
    for (mime, role) in [
        ("video/mp4", "video_manifest"),
        ("image/jpeg", "video_poster"),
    ] {
        attachments.push(video_attachment_request(role, mime, role.as_bytes(), role));
    }
    let post_id = runtime
        .create_post(CreatePostRequest {
            topic: TOPIC.into(),
            content: "written straight to the protected owner".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments,
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
    let body = match &projection.payload_ref {
        kukuri_core::PayloadRef::BlobText { hash, .. } => hash.as_str().to_string(),
        other => panic!("unexpected payload {other:?}"),
    };
    assert_eq!(projection.attachments.len(), 7);
    let attached = projection.attachments[0].hash.as_str().to_string();
    let attachment_hashes = projection.attachments.iter().map(|a| a.hash.as_str());
    for hash in std::iter::once(body.as_str()).chain(attachment_hashes) {
        assert!(
            protected(&runtime.sqlite, "blob", None, hash).await,
            "{hash}"
        );
        assert!(
            node_blob(&runtime, hash).await.is_some(),
            "own blobs stay in the iroh store for a restore without SQLite"
        );
    }
    assert!(db.with_extension("remote-blobs").join(&attached).is_file());
    assert!(
        protected(
            &runtime.sqlite,
            "record",
            Some(projection.source_replica_id.as_str()),
            &format!("objects/{post_id}/envelope"),
        )
        .await
    );
    runtime
        .set_my_profile(SetMyProfileRequest {
            name: Some("owner".into()),
            display_name: None,
            about: None,
            picture_upload: None,
            clear_picture: false,
            nip05: None,
        })
        .await
        .expect("profile");
    let local = runtime.author_keys.public_key_hex();
    assert!(
        protected(
            &runtime.sqlite,
            "record",
            Some(author_replica_id(&local).as_str()),
            "profile/latest",
        )
        .await,
        "the author control value is protected when written"
    );
    // 他人の author の領域へ手元で置いた行(読み直し・hydration)は、保護所有先へ入れない。
    let other = author_replica_id(&KukuriKeys::generate().public_key_hex());
    runtime
        .iroh_stack
        .docs_sync
        .apply_doc_op(
            &other,
            DocOp::SetJson {
                key: "profile/latest".into(),
                value: serde_json::json!({"placed": true}),
            },
        )
        .await
        .expect("place another author's row");
    let others: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM remote_content_cache WHERE scope_key = ?1")
            .bind(other.as_str())
            .fetch_one(runtime.sqlite.pool())
            .await
            .expect("count");
    assert_eq!(others, 0);

    // 送信待ちの frame は送信待ちの参照で守り、ACK で外れる。
    let frame = runtime
        .iroh_stack
        .blob_service
        .put_owned_blob(
            b"pending frame".to_vec(),
            "application/json",
            "dm_outbox:dm-1/message-1",
        )
        .await
        .expect("frame");
    runtime
        .sqlite
        .put_direct_message_outbox(DirectMessageOutboxRow {
            dm_id: "dm-1".into(),
            message_id: "message-1".into(),
            peer_pubkey: KukuriKeys::generate().public_key_hex(),
            frame_blob_hash: frame.hash.clone(),
            created_at: 1,
            last_attempt_at: None,
        })
        .await
        .expect("outbox");
    assert!(protected(&runtime.sqlite, "blob", None, frame.hash.as_str()).await);
    runtime
        .sqlite
        .remove_direct_message_outbox("dm-1", "message-1")
        .await
        .expect("ack");
    assert!(!protected(&runtime.sqlite, "blob", None, frame.hash.as_str()).await);
    runtime.shutdown().await;
}

fn remote_projection(
    author: &KukuriKeys,
    index: usize,
    body: &kukuri_core::BlobHash,
    derived_at: i64,
) -> kukuri_store::ObjectProjectionRow {
    let object_id = format!("remote-post-{index:04}");
    kukuri_store::ObjectProjectionRow {
        object_id: EnvelopeId::from(object_id.clone()),
        topic_id: TOPIC.into(),
        channel_id: "public".into(),
        author_pubkey: author.public_key_hex(),
        created_at: 1_000 + index as i64,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: kukuri_core::PayloadRef::BlobText {
            hash: body.clone(),
            mime: "text/plain".into(),
            bytes: 1,
        },
        content: Some(format!("remote {index}")),
        attachments: vec![kukuri_core::AssetRef {
            hash: kukuri_core::BlobHash::new(format!("{:064x}", index + 1)),
            mime: "image/png".into(),
            bytes: 1,
            role: kukuri_core::AssetRole::ImageOriginal,
        }],
        repost_of: None,
        content_labels: vec![kukuri_core::ADULT_CONTENT_LABEL.into()],
        source_replica_id: kukuri_docs_sync::topic_replica_id(TOPIC),
        source_key: format!("objects/{object_id}/state"),
        source_envelope_id: EnvelopeId::from(object_id),
        source_blob_hash: Some(body.clone()),
        source_docs_author: None,
        derived_at,
        projection_version: 2,
    }
}

fn entries_under(path: &Path) -> usize {
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .map(|entry| {
                    let entry = entry.expect("entry");
                    1 + if entry.file_type().expect("type").is_dir() {
                        entries_under(&entry.path())
                    } else {
                        0
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

async fn count(store: &SqliteStore, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
        .fetch_one(store.pool())
        .await
        .expect("count")
}

/// AC-3・AC-4・AC-6: 更新前の端末の旧 store を、1 回 128 対象以内・再起動をまたいで新しい store と cache へ移し、
/// 名前を変えてから file を上限つきで消す。退役の後も本人のデータと保護種別を再表示でき、最近の他人の内容は cache から
/// 出て古いものは消える。endpoint ID は変わらず、相手へ本人の投稿を再提供し、受入済みの行を再発行しない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_store_moves_in_bounded_steps_and_is_retired() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let source = tempdir().expect("source dir");
    let db = ensure_accounts_initialized(source.path(), IdentityStorageMode::FileOnly)
        .await
        .expect("account");
    create_empty_legacy_store(&db).await;
    let runtime = open_runtime(&db).await;
    let endpoint_id = runtime.iroh_stack.endpoint().await.id();
    let fixture = super::protected_migration::seed_legacy_data(&runtime).await;
    let own_adult = runtime
        .create_post(CreatePostRequest {
            topic: TOPIC.into(),
            content: "own adult post".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![image_attachment_request(
                "adult.png",
                "image/png",
                &png_source_bytes(),
            )],
            content_labels: vec![kukuri_core::ADULT_CONTENT_LABEL.into()],
        })
        .await
        .expect("own adult post");
    let own_adult_hash = runtime
        .sqlite
        .get_object_projection(&EnvelopeId::from(own_adult.as_str()))
        .await
        .expect("projection")
        .expect("own adult projection")
        .attachments[0]
        .hash
        .clone();
    // 旧同期で入った他人の投稿(課金の外の行)。最近のものは本文が旧 store にある。
    let remote = KukuriKeys::generate();
    let recent_body = runtime
        .iroh_stack
        .blob_service
        .put_blob(b"recent remote body".to_vec(), "text/plain")
        .await
        .expect("recent body")
        .hash;
    let now = chrono::Utc::now().timestamp_millis();
    let mut rows = (0..200)
        .map(|index| remote_projection(&remote, index, &recent_body, now - 1_000))
        .collect::<Vec<_>>();
    let old = remote_projection(&remote, 200, &recent_body, now - 30 * 24 * 60 * 60 * 1000);
    rows.push(old.clone());
    runtime
        .sqlite
        .put_object_projections(rows)
        .await
        .expect("legacy projections");
    runtime.shutdown().await;
    drop(runtime);
    into_legacy_layout(&db).await;

    let runtime = open_runtime(&db).await;
    let envelopes = count(&runtime.sqlite, "envelopes").await;
    let outbox = count(&runtime.sqlite, "dm_outbox").await;
    assert!(matches!(
        runtime.legacy_store_step().await.expect("first step"),
        crate::runtime::LegacyStoreProgress::More
    ));
    let (cursor, done) = runtime
        .sqlite
        .legacy_store_position("legacy_projection")
        .await
        .expect("position");
    assert!(!done && !cursor.is_empty(), "the first page is saved");
    runtime.shutdown().await;
    drop(runtime);

    // 再起動しても保存した位置から続け、最後に file を上限つきで消す。
    let runtime = open_runtime(&db).await;
    let retiring = db.with_extension("iroh-data.retiring");
    let mut steps = 0;
    loop {
        let before = entries_under(&retiring);
        let progress = runtime.legacy_store_step().await.expect("step");
        if before > 0 {
            assert!(
                before - entries_under(&retiring) <= 128,
                "one step removes at most 128 files"
            );
        }
        steps += 1;
        match progress {
            crate::runtime::LegacyStoreProgress::Retired => break,
            crate::runtime::LegacyStoreProgress::Waiting => {
                // 保護移行の終端が writer の切替の後になるまで待つ(本番は 60 秒後の次のステップ)。
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            crate::runtime::LegacyStoreProgress::More => {}
        }
        assert!(steps < 200, "retirement does not converge");
    }
    assert!(!db.with_extension("iroh-data").exists());
    assert!(!retiring.exists(), "no file of the legacy store is left");
    assert!(runtime.legacy_store.lock().await.is_none());
    assert_eq!(runtime.iroh_stack.endpoint().await.id(), endpoint_id);
    assert_eq!(count(&runtime.sqlite, "envelopes").await, envelopes);
    assert_eq!(count(&runtime.sqlite, "dm_outbox").await, outbox);

    // 本人のデータと保護種別の再表示。
    let timeline = runtime
        .list_timeline(ListTimelineRequest {
            topic: FIXTURE_TOPIC.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(50),
        })
        .await
        .expect("timeline");
    assert!(
        timeline
            .items
            .iter()
            .any(|post| post.object_id == fixture.post_id && post.content == "protected own post"),
        "{:?}",
        timeline
            .items
            .iter()
            .map(|post| (post.object_id.clone(), post.content.clone()))
            .collect::<Vec<_>>()
    );
    let attachment = runtime
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: fixture.attachment.0.clone(),
            mime: "image/png".into(),
            source_object_id: None,
        })
        .await
        .expect("attachment")
        .expect("attachment bytes");
    assert_eq!(
        BASE64_STANDARD.decode(attachment.bytes_base64).unwrap(),
        fixture.attachment.1
    );
    let blobs = runtime.iroh_stack.blob_service.clone();
    for hash in [
        &fixture.avatar.0,
        &fixture.reaction_asset,
        &fixture.reaction_bookmark,
        &fixture.frame_hash,
        &fixture.plain_attachment,
        &fixture.dome_pin,
        &fixture.live_manifest,
    ] {
        let bytes = blobs
            .fetch_local_blob(&kukuri_core::BlobHash::new(hash.clone()))
            .await
            .expect("local blob")
            .unwrap_or_else(|| panic!("{hash} is not readable after retirement"));
        assert_eq!(blake3::hash(&bytes).to_hex().as_str(), hash.as_str());
    }
    assert_eq!(
        blobs
            .blob_status(&kukuri_core::BlobHash::new(fixture.dome_pin.clone()))
            .await
            .expect("pin status"),
        kukuri_blob_service::BlobStatus::Pinned
    );
    assert!(
        runtime
            .list_bookmarked_posts_page(ListBookmarkedPostsRequest {
                cursor: None,
                before: false,
            })
            .await
            .expect("bookmarks")
            .items
            .iter()
            .any(|item| item.post.object_id == fixture.post_id)
    );
    assert!(
        runtime
            .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                topic: FIXTURE_TOPIC.into(),
                cursor: None,
            })
            .await
            .expect("channels")
            .items
            .iter()
            .any(|channel| channel.channel_id == fixture.channel_id)
    );
    // `author::` の制御領域は新しい store へ移った(保護所有先ではなく新しい store から読む)。
    let local = runtime.author_keys.public_key_hex();
    assert!(
        !runtime
            .iroh_stack
            .docs_sync
            .query_local_source(&author_replica_id(&local), "profile/latest", None, 8)
            .await
            .expect("author control")
            .is_empty()
    );
    // 最近の他人の内容は cache から出て、古いものは消える。成人向けの hash は行の参照がある分だけ残る。
    let recent = runtime
        .sqlite
        .get_object_projection(&EnvelopeId::from("remote-post-0000"))
        .await
        .expect("recent")
        .expect("recent remote projection");
    assert_eq!(recent.content.as_deref(), Some("remote 0"));
    assert!(
        blobs
            .fetch_local_blob(&recent_body)
            .await
            .expect("recent body")
            .is_some()
    );
    assert!(
        runtime
            .sqlite
            .get_object_projection(&old.object_id)
            .await
            .expect("old")
            .is_none()
    );
    assert!(
        !runtime
            .sqlite
            .is_adult_media_hash(&old.attachments[0].hash)
            .await
            .expect("old marker")
    );
    assert!(
        runtime
            .sqlite
            .is_adult_media_hash(&own_adult_hash)
            .await
            .expect("own marker")
    );
    // 退役の後は、参加状態が変わっても保護移行の台帳を読み直さない。
    runtime
        .private_migration_dirty
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        runtime
            .protected_migration_step()
            .await
            .expect("no-op step")
    );
    assert!(
        runtime
            .sqlite
            .protected_migration_caught_up_at()
            .await
            .expect("caught up")
            .is_some()
    );

    // 実 Iroh: 相手が、退役した旧 store の代わりに新しい store と保護所有先から本人の投稿を読む。
    let peer_dir = tempdir().expect("peer dir");
    let peer = kukuri_iroh_node::IrohDocsNode::persistent(peer_dir.path())
        .await
        .expect("peer node");
    let replica = kukuri_docs_sync::topic_replica_id(FIXTURE_TOPIC);
    let secret = iroh_docs::NamespaceSecret::from_bytes(
        blake3::hash(format!("kukuri-docs:{}", replica.as_str()).as_bytes()).as_bytes(),
    );
    let provider = runtime.iroh_stack.endpoint().await.addr();
    let exact = peer
        .query_remote_docs(
            provider.clone(),
            &replica,
            &secret,
            kukuri_iroh_node::DocReadQuery::Exact {
                key: format!("objects/{}/envelope", fixture.post_id),
                limit: 1,
                author: None,
            },
        )
        .await
        .expect("exact read");
    assert!(
        matches!(exact, kukuri_iroh_node::DocReadResponse::Records(records) if records.len() == 1)
    );
    let keys = peer
        .query_remote_docs(
            provider,
            &replica,
            &secret,
            kukuri_iroh_node::DocReadQuery::Keys {
                prefix: format!("objects/{}/", fixture.post_id),
                descending: false,
                limit: 8,
                author: None,
            },
        )
        .await
        .expect("page read");
    assert!(
        matches!(keys, kukuri_iroh_node::DocReadResponse::Keys { entries, .. } if !entries.is_empty()),
        "the copied own entries are served as a page"
    );
    let peer_blobs = kukuri_blob_service::IrohBlobService::new(peer.clone());
    peer_blobs
        .import_peer_ticket(
            &runtime
                .local_peer_ticket()
                .await
                .expect("ticket")
                .expect("ticket value"),
        )
        .await
        .expect("import ticket");
    // 本人の blob は iroh の store に無く、`RemoteBlobProtocol` が保護所有先から渡す(相手の表示の取得と同じ経路)。
    let body = peer_blobs
        .fetch_blob_ephemeral(&kukuri_core::BlobHash::new(fixture.body_hash.clone()))
        .await
        .expect("fetch body")
        .expect("own body is provided from the protected owner");
    assert_eq!(blake3::hash(&body).to_hex().as_str(), fixture.body_hash);
    peer.shutdown().await.expect("peer shutdown");
    runtime.shutdown().await;
    drop(runtime);

    // backup → restore でも戻る。
    let target = tempdir().expect("target dir");
    let (names, restored_db) =
        super::protected_migration::backup_and_restore(source.path(), &db, target.path()).await;
    assert!(
        names.iter().all(|name| !name.contains("iroh-")),
        "{names:?}"
    );
    let restored = open_runtime(&restored_db).await;
    let payload = restored
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: fixture.attachment.0.clone(),
            mime: "image/png".into(),
            source_object_id: None,
        })
        .await
        .expect("restored attachment")
        .expect("restored attachment bytes");
    assert_eq!(
        BASE64_STANDARD.decode(payload.bytes_base64).unwrap(),
        fixture.attachment.1
    );
    for hash in [
        &fixture.dome_pin,
        &fixture.reaction_asset,
        &fixture.plain_attachment,
    ] {
        assert!(
            restored
                .iroh_stack
                .blob_service
                .fetch_local_blob(&kukuri_core::BlobHash::new(hash.clone()))
                .await
                .expect("restored blob")
                .is_some(),
            "{hash} was not restored"
        );
    }
    assert_eq!(count(&restored.sqlite, "dm_outbox").await, outbox);
    restored.shutdown().await;
}
