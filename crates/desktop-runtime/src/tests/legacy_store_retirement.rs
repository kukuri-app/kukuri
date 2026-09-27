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

async fn open_runtime(db: &Path) -> DesktopRuntime {
    DesktopRuntime::new_with_config_and_identity(
        db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime")
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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_writes_go_to_the_protected_owner_when_written() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().expect("dir");
    let db =
        ensure_accounts_initialized(dir.path(), IdentityStorageMode::FileOnly).expect("account");
    let runtime = open_runtime(&db).await;
    let attachment = vec![7u8; 1024 * 1024 + 5];
    let post_id = runtime
        .create_post(CreatePostRequest {
            topic: TOPIC.into(),
            content: "written straight to the protected owner".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![image_attachment_request(
                "large.png",
                "image/png",
                &attachment,
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
    let body = match &projection.payload_ref {
        kukuri_core::PayloadRef::BlobText { hash, .. } => hash.as_str().to_string(),
        other => panic!("unexpected payload {other:?}"),
    };
    let attached = projection.attachments[0].hash.as_str().to_string();
    for hash in [&body, &attached] {
        assert!(
            protected(&runtime.store, "blob", None, hash).await,
            "{hash}"
        );
        assert!(
            node_blob(&runtime, hash).await.is_none(),
            "own blobs are not kept in the iroh store"
        );
    }
    assert!(db.with_extension("remote-blobs").join(&attached).is_file());
    assert!(
        protected(
            &runtime.store,
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
        })
        .await
        .expect("profile");
    let local = runtime.author_keys.public_key_hex();
    assert!(
        protected(
            &runtime.store,
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
            .fetch_one(runtime.store.pool())
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
        .store
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
    assert!(protected(&runtime.store, "blob", None, frame.hash.as_str()).await);
    runtime
        .store
        .remove_direct_message_outbox("dm-1", "message-1")
        .await
        .expect("ack");
    assert!(!protected(&runtime.store, "blob", None, frame.hash.as_str()).await);
    runtime.shutdown().await;
}
