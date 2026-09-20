use super::*;
use std::collections::BTreeSet;

fn projection_row(
    topic: &str,
    channel_id: &str,
    object_id: &str,
    created_at: i64,
) -> ObjectProjectionRow {
    let hash = BlobHash::new(format!("{created_at:064x}"));
    ObjectProjectionRow {
        object_id: EnvelopeId::from(object_id),
        topic_id: topic.to_string(),
        channel_id: channel_id.to_string(),
        author_pubkey: format!("{:064x}", created_at + 100),
        created_at,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::BlobText {
            hash: hash.clone(),
            mime: "text/plain".into(),
            bytes: object_id.len() as u64,
        },
        content: Some(object_id.to_string()),
        attachments: Vec::new(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new(format!("topic::{topic}")),
        source_key: format!("objects/{object_id}/header"),
        source_envelope_id: EnvelopeId::from(object_id),
        source_blob_hash: Some(hash),
        source_docs_author: None,
        derived_at: created_at,
        projection_version: 2,
    }
}

#[tokio::test]
async fn recent_reaction_cache_query_returns_latest_rows_for_author() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let author_pubkey = "a".repeat(64);
    let target_object_id = EnvelopeId::from("target-object");
    let source_replica_id = ReplicaId::new("topic::recent-reactions");
    for row in [
        ReactionProjectionRow {
            source_replica_id: source_replica_id.clone(),
            target_object_id: target_object_id.clone(),
            reaction_id: EnvelopeId::from("reaction-1"),
            author_pubkey: author_pubkey.clone(),
            created_at: 10,
            updated_at: 10,
            reaction_key_kind: ReactionKeyKind::Emoji,
            normalized_reaction_key: "emoji:🔥".into(),
            emoji: Some("🔥".into()),
            custom_asset_id: None,
            custom_asset_snapshot: None,
            status: ObjectStatus::Active,
            source_key: "reactions/1".into(),
            source_envelope_id: EnvelopeId::from("reaction-1"),
            derived_at: 10,
            projection_version: 1,
        },
        ReactionProjectionRow {
            source_replica_id: source_replica_id.clone(),
            target_object_id: target_object_id.clone(),
            reaction_id: EnvelopeId::from("reaction-2"),
            author_pubkey: author_pubkey.clone(),
            created_at: 12,
            updated_at: 25,
            reaction_key_kind: ReactionKeyKind::Emoji,
            normalized_reaction_key: "emoji:😂".into(),
            emoji: Some("😂".into()),
            custom_asset_id: None,
            custom_asset_snapshot: None,
            status: ObjectStatus::Deleted,
            source_key: "reactions/2".into(),
            source_envelope_id: EnvelopeId::from("reaction-2"),
            derived_at: 12,
            projection_version: 1,
        },
        ReactionProjectionRow {
            source_replica_id,
            target_object_id: target_object_id.clone(),
            reaction_id: EnvelopeId::from("reaction-3"),
            author_pubkey: "b".repeat(64),
            created_at: 15,
            updated_at: 30,
            reaction_key_kind: ReactionKeyKind::Emoji,
            normalized_reaction_key: "emoji:🎉".into(),
            emoji: Some("🎉".into()),
            custom_asset_id: None,
            custom_asset_snapshot: None,
            status: ObjectStatus::Active,
            source_key: "reactions/3".into(),
            source_envelope_id: EnvelopeId::from("reaction-3"),
            derived_at: 15,
            projection_version: 1,
        },
    ] {
        ReactionBookmarkStore::upsert_reaction_cache(&store, row)
            .await
            .expect("upsert reaction cache");
    }

    let recent =
        ReactionBookmarkStore::list_recent_reaction_cache_by_author(&store, author_pubkey.as_str())
            .await
            .expect("list recent reaction cache");

    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].normalized_reaction_key, "emoji:😂");
    assert_eq!(recent[1].normalized_reaction_key, "emoji:🔥");
}
#[tokio::test]
async fn author_relationship_projection_rebuild_roundtrip() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let local_author = "a".repeat(64);
    let target_author = "b".repeat(64);

    SocialProjectionStore::rebuild_author_relationships(
        &store,
        local_author.as_str(),
        vec![AuthorRelationshipProjectionRow {
            local_author_pubkey: local_author.clone(),
            author_pubkey: target_author.clone(),
            following: false,
            followed_by: true,
            mutual: false,
            friend_of_friend: true,
            friend_of_friend_via_pubkeys: vec!["c".repeat(64)],
            derived_at: 12,
        }],
    )
    .await
    .expect("rebuild relationships");

    let relationship = SocialProjectionStore::get_author_relationship(
        &store,
        local_author.as_str(),
        target_author.as_str(),
    )
    .await
    .expect("get relationship")
    .expect("relationship");
    assert!(relationship.friend_of_friend);
    assert_eq!(
        relationship.friend_of_friend_via_pubkeys,
        vec!["c".repeat(64)]
    );
    assert!(relationship.followed_by);
}
#[tokio::test]
async fn muted_authors_restore_after_restart() {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("store.db");
    let author_pubkey = "b".repeat(64);

    {
        let store = SqliteStore::connect_file(&db_path)
            .await
            .expect("open sqlite store");
        SocialProjectionStore::put_muted_author(
            &store,
            MutedAuthorRow {
                author_pubkey: author_pubkey.clone(),
                muted_at: 42,
            },
        )
        .await
        .expect("store muted author");
        store.close().await;
    }

    let reopened = SqliteStore::connect_file(&db_path)
        .await
        .expect("reopen sqlite store");
    let muted = SocialProjectionStore::get_muted_author(&reopened, author_pubkey.as_str())
        .await
        .expect("get muted author")
        .expect("muted author exists");
    assert_eq!(muted.author_pubkey, author_pubkey);
    assert_eq!(muted.muted_at, 42);
    assert_eq!(
        SocialProjectionStore::list_muted_authors(&reopened)
            .await
            .expect("list muted authors"),
        vec![muted.clone()]
    );

    SocialProjectionStore::remove_muted_author(&reopened, author_pubkey.as_str())
        .await
        .expect("remove muted author");
    assert!(
        SocialProjectionStore::get_muted_author(&reopened, author_pubkey.as_str())
            .await
            .expect("get muted author after delete")
            .is_none()
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn author_relationship_rebuild_stays_visible_to_concurrent_readers() {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("relationship-cache.db");
    let writer = SqliteStore::connect_file(&db_path)
        .await
        .expect("writer store");
    let reader = SqliteStore::connect_file(&db_path)
        .await
        .expect("reader store");
    let local_author = "a".repeat(64);
    let target_author = "b".repeat(64);

    let mut rows = (0..512)
        .map(|index| AuthorRelationshipProjectionRow {
            local_author_pubkey: local_author.clone(),
            author_pubkey: format!("{index:064x}"),
            following: true,
            followed_by: true,
            mutual: true,
            friend_of_friend: false,
            friend_of_friend_via_pubkeys: Vec::new(),
            derived_at: index,
        })
        .collect::<Vec<_>>();
    rows.push(AuthorRelationshipProjectionRow {
        local_author_pubkey: local_author.clone(),
        author_pubkey: target_author.clone(),
        following: true,
        followed_by: true,
        mutual: true,
        friend_of_friend: false,
        friend_of_friend_via_pubkeys: Vec::new(),
        derived_at: 999,
    });

    SocialProjectionStore::rebuild_author_relationships(
        &writer,
        local_author.as_str(),
        rows.clone(),
    )
    .await
    .expect("seed relationships");

    let keep_running = Arc::new(AtomicBool::new(true));
    let saw_gap = Arc::new(AtomicBool::new(false));
    let keep_running_for_task = Arc::clone(&keep_running);
    let saw_gap_for_task = Arc::clone(&saw_gap);
    let local_author_for_task = local_author.clone();
    let target_author_for_task = target_author.clone();

    let reader_task = tokio::spawn(async move {
        while keep_running_for_task.load(Ordering::SeqCst) {
            let relationship = SocialProjectionStore::get_author_relationship(
                &reader,
                local_author_for_task.as_str(),
                target_author_for_task.as_str(),
            )
            .await
            .expect("read relationship");
            if relationship.is_none() {
                saw_gap_for_task.store(true, Ordering::SeqCst);
                break;
            }
            tokio::task::yield_now().await;
        }
    });

    for _ in 0..32 {
        SocialProjectionStore::rebuild_author_relationships(
            &writer,
            local_author.as_str(),
            rows.clone(),
        )
        .await
        .expect("rebuild relationships");
        if saw_gap.load(Ordering::SeqCst) {
            break;
        }
    }

    keep_running.store(false, Ordering::SeqCst);
    reader_task.await.expect("reader task");
    assert!(
        !saw_gap.load(Ordering::SeqCst),
        "concurrent readers should never observe a missing relationship during rebuild",
    );
}
#[tokio::test]
async fn projection_rebuild_from_docs_blobs_only() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic = "kukuri:topic:projection";
    let root_id = EnvelopeId::from("object-root");
    let reply_id = EnvelopeId::from("object-reply");
    let rows = vec![
        ObjectProjectionRow {
            object_id: root_id.clone(),
            topic_id: topic.to_string(),
            channel_id: "public".into(),
            author_pubkey: "a".repeat(64),
            created_at: 10,
            object_kind: "post".into(),
            root_object_id: None,
            reply_to_object_id: None,
            payload_ref: PayloadRef::BlobText {
                hash: BlobHash::new("1".repeat(64)),
                mime: "text/plain".into(),
                bytes: 4,
            },
            content: Some("root".into()),
            attachments: Vec::new(),
            repost_of: None,
            content_labels: Vec::new(),
            source_replica_id: ReplicaId::new(format!("topic::{topic}")),
            source_key: "objects/object-root/header".into(),
            source_envelope_id: root_id.clone(),
            source_blob_hash: Some(BlobHash::new("1".repeat(64))),
            source_docs_author: None,
            derived_at: 10,
            projection_version: 2,
        },
        ObjectProjectionRow {
            object_id: reply_id.clone(),
            topic_id: topic.to_string(),
            channel_id: "public".into(),
            author_pubkey: "b".repeat(64),
            created_at: 11,
            object_kind: "comment".into(),
            root_object_id: Some(root_id.clone()),
            reply_to_object_id: Some(root_id.clone()),
            payload_ref: PayloadRef::BlobText {
                hash: BlobHash::new("2".repeat(64)),
                mime: "text/plain".into(),
                bytes: 5,
            },
            content: Some("reply".into()),
            attachments: Vec::new(),
            repost_of: None,
            content_labels: Vec::new(),
            source_replica_id: ReplicaId::new(format!("topic::{topic}")),
            source_key: "objects/object-reply/header".into(),
            source_envelope_id: reply_id.clone(),
            source_blob_hash: Some(BlobHash::new("2".repeat(64))),
            source_docs_author: None,
            derived_at: 11,
            projection_version: 2,
        },
    ];

    ObjectProjectionStore::rebuild_object_projections(&store, rows)
        .await
        .expect("rebuild projection");

    let timeline = ObjectProjectionStore::list_topic_timeline(&store, topic, None, 10)
        .await
        .expect("timeline");
    let thread = ObjectProjectionStore::list_thread(&store, topic, &root_id, None, 10)
        .await
        .expect("thread");

    assert_eq!(timeline.items.len(), 2);
    assert_eq!(timeline.items[0].object_id, reply_id);
    assert_eq!(thread.items.len(), 2);
    assert_eq!(thread.items[0].object_id, root_id);
}

#[tokio::test]
async fn filtered_timeline_query_preserves_cursor_across_channel_pages() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic = "kukuri:topic:filtered-timeline";
    let rows = vec![
        projection_row(topic, "private:friends", "post-4", 40),
        projection_row(topic, "public", "post-3", 30),
        projection_row(topic, "private:friends", "post-2", 20),
        projection_row(topic, "private:friends", "post-1", 10),
    ];
    ObjectProjectionStore::put_object_projections(&store, rows)
        .await
        .expect("put projections");

    let allowed_channels = BTreeSet::from(["private:friends".to_string()]);
    let first_page = ObjectProjectionStore::list_topic_timeline_filtered(
        &store,
        topic,
        &allowed_channels,
        None,
        2,
    )
    .await
    .expect("first filtered timeline page");
    assert_eq!(
        first_page
            .items
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["post-4".to_string(), "post-2".to_string()]
    );
    assert!(first_page.next_cursor.is_some());

    let second_page = ObjectProjectionStore::list_topic_timeline_filtered(
        &store,
        topic,
        &allowed_channels,
        first_page.next_cursor.clone(),
        2,
    )
    .await
    .expect("second filtered timeline page");
    assert_eq!(
        second_page
            .items
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["post-1".to_string()]
    );
    assert!(second_page.next_cursor.is_none());
}

#[tokio::test]
async fn filtered_thread_query_preserves_cursor_across_channel_pages() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic = "kukuri:topic:filtered-thread";
    let root_id = EnvelopeId::from("thread-root");
    let mut root = projection_row(topic, "private:friends", root_id.as_str(), 10);
    root.object_id = root_id.clone();
    root.source_envelope_id = root_id.clone();

    let mut public_reply = projection_row(topic, "public", "reply-public", 20);
    public_reply.root_object_id = Some(root_id.clone());
    public_reply.reply_to_object_id = Some(root_id.clone());
    public_reply.object_kind = "comment".into();

    let mut private_reply_a = projection_row(topic, "private:friends", "reply-a", 30);
    private_reply_a.root_object_id = Some(root_id.clone());
    private_reply_a.reply_to_object_id = Some(root_id.clone());
    private_reply_a.object_kind = "comment".into();

    let mut private_reply_b = projection_row(topic, "private:friends", "reply-b", 40);
    private_reply_b.root_object_id = Some(root_id.clone());
    private_reply_b.reply_to_object_id = Some(root_id.clone());
    private_reply_b.object_kind = "comment".into();

    ObjectProjectionStore::put_object_projections(
        &store,
        vec![root, public_reply, private_reply_a, private_reply_b],
    )
    .await
    .expect("put projections");

    let first_page = ObjectProjectionStore::list_thread_filtered(
        &store,
        topic,
        &root_id,
        Some("private:friends"),
        None,
        2,
    )
    .await
    .expect("first filtered thread page");
    assert_eq!(
        first_page
            .items
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["thread-root".to_string(), "reply-a".to_string()]
    );
    assert!(first_page.next_cursor.is_some());

    let second_page = ObjectProjectionStore::list_thread_filtered(
        &store,
        topic,
        &root_id,
        Some("private:friends"),
        first_page.next_cursor.clone(),
        2,
    )
    .await
    .expect("second filtered thread page");
    assert_eq!(
        second_page
            .items
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["reply-b".to_string()]
    );
    assert!(second_page.next_cursor.is_none());
}

#[tokio::test]
async fn batch_projection_insert_matches_single_insert_and_preserves_attachments() {
    let batch_store = SqliteStore::connect_memory().await.expect("batch store");
    let single_store = SqliteStore::connect_memory().await.expect("single store");
    let topic = "kukuri:topic:batch-projection";

    let mut root = projection_row(topic, "public", "batch-root", 10);
    root.attachments = vec![kukuri_core::AssetRef {
        hash: BlobHash::new("f".repeat(64)),
        mime: "image/png".into(),
        bytes: 42,
        role: kukuri_core::AssetRole::ImageOriginal,
    }];
    let mut reply = projection_row(topic, "public", "batch-reply", 11);
    reply.object_kind = "comment".into();
    reply.root_object_id = Some(root.object_id.clone());
    reply.reply_to_object_id = Some(root.object_id.clone());

    ObjectProjectionStore::put_object_projections(&batch_store, vec![root.clone(), reply.clone()])
        .await
        .expect("batch insert");
    ObjectProjectionStore::put_object_projection(&single_store, root.clone())
        .await
        .expect("single insert root");
    ObjectProjectionStore::put_object_projection(&single_store, reply.clone())
        .await
        .expect("single insert reply");

    let batch_timeline = ObjectProjectionStore::list_topic_timeline(&batch_store, topic, None, 10)
        .await
        .expect("batch timeline");
    let single_timeline =
        ObjectProjectionStore::list_topic_timeline(&single_store, topic, None, 10)
            .await
            .expect("single timeline");
    assert_eq!(
        batch_timeline
            .items
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<Vec<_>>(),
        single_timeline
            .items
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<Vec<_>>()
    );

    let batch_thread =
        ObjectProjectionStore::list_thread(&batch_store, topic, &root.object_id, None, 10)
            .await
            .expect("batch thread");
    let single_thread =
        ObjectProjectionStore::list_thread(&single_store, topic, &root.object_id, None, 10)
            .await
            .expect("single thread");
    assert_eq!(
        batch_thread
            .items
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<Vec<_>>(),
        single_thread
            .items
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<Vec<_>>()
    );

    let stored = ObjectProjectionStore::get_object_projection(&batch_store, &root.object_id)
        .await
        .expect("get stored projection")
        .expect("stored projection");
    assert_eq!(stored.attachments, root.attachments);
}
