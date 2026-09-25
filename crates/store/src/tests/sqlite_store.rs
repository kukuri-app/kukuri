use super::*;

#[tokio::test]
async fn store_thread_materialization() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic = TopicId::new("kukuri:topic:thread");
    let keys = generate_keys();

    let root = build_post_envelope(&keys, &topic, "root", None).expect("root");
    let reply = build_post_envelope(&keys, &topic, "reply", Some(&root)).expect("reply");
    store.put_envelope(root.clone()).await.expect("insert root");
    store
        .put_envelope(reply.clone())
        .await
        .expect("insert reply");

    let thread = Store::list_thread(&store, topic.as_str(), &root.id, None, 10)
        .await
        .expect("thread");

    assert_eq!(thread.items.len(), 2);
    assert_eq!(thread.items[0].id, root.id);
    assert_eq!(thread.items[1].id, reply.id);
}
#[tokio::test]
async fn store_timeline_cursor_stable() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic = TopicId::new("kukuri:topic:timeline");
    let keys = generate_keys();

    let first = build_post_envelope(&keys, &topic, "one", None).expect("first");
    let second = build_post_envelope(&keys, &topic, "two", None).expect("second");
    let third = build_post_envelope(&keys, &topic, "three", None).expect("third");
    store
        .put_envelope(first.clone())
        .await
        .expect("insert first");
    store
        .put_envelope(second.clone())
        .await
        .expect("insert second");
    store
        .put_envelope(third.clone())
        .await
        .expect("insert third");

    let first_page = Store::list_topic_timeline(&store, topic.as_str(), None, 2)
        .await
        .expect("timeline page");
    let cursor = first_page.next_cursor.clone().expect("cursor");
    let second_page = Store::list_topic_timeline(&store, topic.as_str(), Some(cursor), 2)
        .await
        .expect("second page");

    assert_eq!(first_page.items.len(), 2);
    assert!(first_page.items[0].created_at >= first_page.items[1].created_at);
    assert!(second_page.items.len() <= 1);
    assert!(second_page.items.iter().all(|event| {
        !first_page
            .items
            .iter()
            .any(|existing| existing.id == event.id)
    }));
}
#[tokio::test]
async fn store_profile_upsert_latest_wins() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let pubkey = "f".repeat(64);

    store
        .upsert_profile(Profile {
            pubkey: pubkey.as_str().into(),
            name: Some("older".into()),
            display_name: Some("older".into()),
            about: None,
            picture_asset: None,
            updated_at: 10,
        })
        .await
        .expect("insert older");
    store
        .upsert_profile(Profile {
            pubkey: pubkey.as_str().into(),
            name: Some("newer".into()),
            display_name: Some("newer".into()),
            about: None,
            picture_asset: Some(kukuri_core::AssetRef {
                hash: kukuri_core::BlobHash::new("avatar-newer"),
                mime: "image/png".into(),
                bytes: 128,
                role: kukuri_core::AssetRole::ProfileAvatar,
            }),
            updated_at: 20,
        })
        .await
        .expect("insert newer");

    let profile = store
        .get_profile(pubkey.as_str())
        .await
        .expect("load profile")
        .expect("profile");
    assert_eq!(profile.name.as_deref(), Some("newer"));
    assert_eq!(profile.display_name.as_deref(), Some("newer"));
    assert_eq!(
        profile
            .picture_asset
            .as_ref()
            .map(|asset| asset.hash.as_str()),
        Some("avatar-newer")
    );
}
#[tokio::test]
async fn store_follow_edge_latest_wins() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let subject_keys = generate_keys();
    let target_keys = generate_keys();
    let active = build_follow_edge_envelope(
        &subject_keys,
        &target_keys.public_key(),
        FollowEdgeStatus::Active,
    )
    .expect("active edge");
    let mut revoked = build_follow_edge_envelope(
        &subject_keys,
        &target_keys.public_key(),
        FollowEdgeStatus::Revoked,
    )
    .expect("revoked edge");
    revoked.created_at = active.created_at + 1;

    store
        .put_envelope(active.clone())
        .await
        .expect("insert active edge");
    store
        .put_envelope(revoked.clone())
        .await
        .expect("insert revoked edge");
    store
        .put_envelope(active)
        .await
        .expect("reinsert older edge");

    let edges = store
        .list_follow_edges_by_subject(subject_keys.public_key_hex().as_str())
        .await
        .expect("list edges");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].status, FollowEdgeStatus::Revoked);
}

#[tokio::test]
async fn close_leaves_no_open_connection_after_connect_file() {
    let dir = tempdir().expect("tempdir");
    // connect_file の migration が返却中の接続を残したまま close へ進む競合は、1 回では起きないことがある。
    // 修正前は 100 回中 7-52 回、close 後も WAL を開いた接続が残った。
    for attempt in 0..30 {
        let db_path = dir.path().join(format!("store-{attempt}.db"));
        let store = SqliteStore::connect_file(&db_path)
            .await
            .expect("sqlite store");
        store.close().await;
        assert!(
            !db_path.with_extension("db-shm").exists(),
            "close must not leave an open WAL connection (attempt {attempt})"
        );
    }
}
