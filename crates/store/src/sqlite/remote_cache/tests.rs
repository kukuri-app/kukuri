use super::*;
use kukuri_core::{AssetRef, AssetRole, PayloadRef};

fn remote_post(object_id: &str) -> ObjectProjectionRow {
    let hash = BlobHash::new("a".repeat(64));
    ObjectProjectionRow {
        object_id: EnvelopeId::from(object_id),
        topic_id: "topic".into(),
        channel_id: "public".into(),
        author_pubkey: "b".repeat(64),
        created_at: 1,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::BlobText {
            hash: hash.clone(),
            mime: "text/plain".into(),
            bytes: 1,
        },
        content: Some("body".into()),
        attachments: Vec::new(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new("bucket::v1::topic::746f706963::1"),
        source_key: format!("objects/{object_id}/envelope"),
        source_envelope_id: EnvelopeId::from(object_id),
        source_blob_hash: Some(hash),
        source_docs_author: None,
        derived_at: 1,
        projection_version: 3,
    }
}

#[tokio::test]
async fn cached_blob_copies_to_display_file_in_bounded_chunks() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let bytes = vec![7u8; 2 * 1024 * 1024 + 13];
    let hash = blake3::hash(&bytes).to_hex().to_string();
    assert!(
        store
            .put_remote_content("blob", &hash, "blob", &bytes)
            .await
            .unwrap()
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("display.bin");
    assert_eq!(
        store
            .copy_remote_content_to_file("blob", &hash, &path)
            .await
            .unwrap(),
        Some(bytes.len() as u64)
    );
    assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
}

#[tokio::test]
async fn file_backed_remote_blob_survives_restart_and_reclaim_removes_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("store.db");
    let source = dir.path().join("source.bin");
    let bytes = vec![17u8; 2 * 1024 * 1024 + 7];
    tokio::fs::write(&source, &bytes).await.unwrap();
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let store = SqliteStore::connect_file(&database).await.unwrap();
    store.put_remote_blob_file(&hash, &source).await.unwrap();
    let cached_file = database.with_extension("remote-blobs").join(&hash);
    assert!(cached_file.exists());
    let display = dir.path().join("display.bin");
    assert_eq!(
        store
            .copy_remote_content_to_file("blob", &hash, &display)
            .await
            .unwrap(),
        Some(bytes.len() as u64)
    );
    assert_eq!(tokio::fs::read(&display).await.unwrap(), bytes);
    store.close().await;

    let reopened = SqliteStore::connect_file(&database).await.unwrap();
    assert!(reopened.has_remote_content("blob", &hash).await.unwrap());
    sqlx::query(
        "UPDATE remote_content_cache SET last_used_at = 0 WHERE kind = 'blob' AND cache_key = ?1",
    )
    .bind(&hash)
    .execute(reopened.pool())
    .await
    .unwrap();
    assert_eq!(reopened.reclaim_remote_cache_step().await.unwrap(), 1);
    assert!(!cached_file.exists());
    assert_eq!(tokio::fs::read(display).await.unwrap(), bytes);
}

#[tokio::test]
async fn rolled_back_reclaim_keeps_the_file_backed_blob_readable() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let bytes = b"cached media";
    tokio::fs::write(&source, bytes).await.unwrap();
    let hash = blake3::hash(bytes).to_hex().to_string();
    let store = SqliteStore::connect_file(dir.path().join("store.db"))
        .await
        .unwrap();
    store.put_remote_blob_file(&hash, &source).await.unwrap();
    let mut tx = store.pool.begin().await.unwrap();
    let mut labels = Vec::new();
    let mut removed_files = Vec::new();
    delete_cache_item(&mut tx, "blob", &hash, &mut labels, &mut removed_files)
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(removed_files, vec![hash.clone()]);
    assert_eq!(
        store.get_remote_content("blob", &hash).await.unwrap(),
        Some(bytes.to_vec())
    );
}

#[tokio::test]
async fn file_backed_media_over_128_mib_keeps_its_original_size() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("large-video.bin");
    let length = 129 * 1024 * 1024u64;
    tokio::fs::File::create(&source)
        .await
        .unwrap()
        .set_len(length)
        .await
        .unwrap();
    let mut hasher = blake3::Hasher::new();
    let chunk = vec![0u8; 1024 * 1024];
    for _ in 0..129 {
        hasher.update(&chunk);
    }
    let hash = hasher.finalize().to_hex().to_string();
    let store = SqliteStore::connect_file(dir.path().join("large.db"))
        .await
        .unwrap();
    store.put_remote_blob_file(&hash, &source).await.unwrap();
    let display = dir.path().join("display.mp4");
    assert_eq!(
        store
            .copy_remote_content_to_file("blob", &hash, &display)
            .await
            .unwrap(),
        Some(length)
    );
    assert_eq!(tokio::fs::metadata(display).await.unwrap().len(), length);
}

#[tokio::test]
async fn fresh_cache_reads_do_not_take_the_sqlite_writer_lock() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect_file(dir.path().join("remote-cache.db"))
        .await
        .unwrap();
    let post = remote_post("read-with-writer");
    store
        .put_remote_object_projection(post.clone())
        .await
        .unwrap();
    store
        .put_remote_content("blob", "blob-hash", "scope", b"body")
        .await
        .unwrap();
    store
        .put_remote_record("replica", "key", "author", b"record")
        .await
        .unwrap();

    let mut writer = store.pool().begin().await.unwrap();
    sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
        .execute(&mut *writer)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        assert_eq!(
            store
                .available_remote_projections(vec![post])
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store.get_remote_content("blob", "blob-hash").await.unwrap(),
            Some(b"body".to_vec())
        );
        assert_eq!(
            store.remote_content_len("blob", "blob-hash").await.unwrap(),
            Some(4)
        );
        assert_eq!(
            store
                .get_remote_records("replica", "key", Some("author"), 1)
                .await
                .unwrap(),
            vec![b"record".to_vec()]
        );
    })
    .await
    .expect("fresh cache reads must not wait for a writer");
    writer.rollback().await.unwrap();
}

#[tokio::test]
async fn local_projection_write_waits_for_a_concurrent_writer() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect_file(dir.path().join("remote-cache.db"))
        .await
        .unwrap();
    let mut writer = store.pool().begin().await.unwrap();
    sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
        .execute(&mut *writer)
        .await
        .unwrap();
    let (written, ()) = tokio::join!(
        store.put_object_projection(remote_post("local-with-writer")),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            writer.rollback().await.unwrap();
        }
    );
    written.expect("a local write waits for the writer");
}

#[tokio::test]
async fn adult_hash_marker_follows_cached_projection_refs() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let mut evictions = store.subscribe_adult_label_evictions();
    let hash = BlobHash::new("c".repeat(64));
    let mut first = remote_post("adult-1");
    first.content_labels = vec!["adult".into()];
    first.attachments.push(AssetRef {
        hash: hash.clone(),
        mime: "image/png".into(),
        bytes: 3,
        role: AssetRole::ImageOriginal,
    });
    let mut second = first.clone();
    second.object_id = EnvelopeId::from("adult-2");
    second.source_envelope_id = second.object_id.clone();
    second.source_key = "objects/adult-2/envelope".into();
    store
        .put_remote_object_projection(first.clone())
        .await
        .unwrap();
    store
        .put_remote_object_projection(second.clone())
        .await
        .unwrap();
    assert!(store.is_adult_media_hash(&hash).await.unwrap());
    store
        .remove_remote_content("projection", first.object_id.as_str())
        .await
        .unwrap();
    assert!(store.is_adult_media_hash(&hash).await.unwrap());
    assert!(matches!(
        evictions.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    store
        .remove_remote_content("projection", second.object_id.as_str())
        .await
        .unwrap();
    assert!(!store.is_adult_media_hash(&hash).await.unwrap());
    assert_eq!(evictions.try_recv().unwrap(), hash.as_str());

    let mut own = remote_post("own-labeled");
    own.content_labels = vec!["adult".into()];
    own.attachments = first.attachments;
    store.put_object_projection(own).await.unwrap();
    assert!(store.is_adult_media_hash(&hash).await.unwrap());
}

#[tokio::test]
async fn profile_label_marker_expires_with_remote_cache() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let mut evictions = store.subscribe_adult_label_evictions();
    let hash = BlobHash::new("d".repeat(64));
    store
        .mark_adult_media_hashes(std::slice::from_ref(&hash))
        .await
        .unwrap();
    assert!(store.is_adult_media_hash(&hash).await.unwrap());
    sqlx::query(
        "UPDATE remote_content_cache SET last_used_at = 0 \
         WHERE kind = 'adult_marker' AND cache_key = ?1",
    )
    .bind(hash.as_str())
    .execute(store.pool())
    .await
    .unwrap();
    assert_eq!(store.reclaim_remote_cache_step().await.unwrap(), 1);
    assert!(!store.is_adult_media_hash(&hash).await.unwrap());
    assert_eq!(evictions.try_recv().unwrap(), hash.as_str());
}

#[tokio::test]
async fn expired_remote_projection_stays_unavailable_until_revalidated() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let row = remote_post("expired-post");
    store
        .put_remote_object_projection(row.clone())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE remote_content_cache SET last_used_at = 0 \
         WHERE kind = 'projection' AND cache_key = ?1",
    )
    .bind(row.object_id.as_str())
    .execute(store.pool())
    .await
    .unwrap();
    assert!(
        store
            .get_object_projection(&row.object_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.reclaim_remote_cache_step().await.unwrap(), 1);
    assert!(
        store
            .get_object_projection(&row.object_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn remote_projection_and_its_page_index_are_reclaimed_together() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let row = remote_post("remote-1");
    store
        .put_remote_object_projection(row.clone())
        .await
        .unwrap();
    let charged = sqlx::query_scalar::<_, i64>(
        "SELECT used_bytes FROM remote_content_cache_usage WHERE id = 1",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert!(charged > 0);
    assert!(
        store
            .put_remote_content_with_budget(
                "blob",
                "x",
                "s",
                CachePayload::Bytes(b"v"),
                None,
                None,
                100,
                now_ms().unwrap()
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .get_object_projection(&row.object_id)
            .await
            .unwrap()
            .is_none()
    );
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM object_thread_cache WHERE object_id = ?1",
    )
    .bind(row.object_id.as_str())
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn bookmark_protects_shared_remote_hash_until_its_reference_is_removed() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let row = remote_post("remote-bookmark");
    let hash = match &row.payload_ref {
        PayloadRef::BlobText { hash, .. } => hash.clone(),
        _ => unreachable!(),
    };
    store
        .put_remote_object_projection(row.clone())
        .await
        .unwrap();
    assert!(
        store
            .put_remote_content("blob", hash.as_str(), "blob", b"body")
            .await
            .unwrap()
    );
    let bookmark = BookmarkedPostRow {
        source_object_id: row.object_id.clone(),
        source_envelope_id: row.source_envelope_id.clone(),
        source_replica_id: row.source_replica_id.clone(),
        topic_id: row.topic_id.clone(),
        channel_id: row.channel_id.clone(),
        author_pubkey: row.author_pubkey.clone(),
        created_at: row.created_at,
        object_kind: row.object_kind.clone(),
        payload_ref: row.payload_ref.clone(),
        content: row.content.clone(),
        attachments: row.attachments.clone(),
        reply_to_object_id: row.reply_to_object_id.clone(),
        root_object_id: row.root_object_id.clone(),
        repost_of: row.repost_of.clone(),
        bookmarked_at: 2,
    };
    store.put_bookmarked_post(bookmark.clone()).await.unwrap();
    let mut second = bookmark;
    second.source_object_id = EnvelopeId::from("second-bookmark");
    store.put_bookmarked_post(second.clone()).await.unwrap();
    let protected = sqlx::query_scalar::<_, i64>(
        "SELECT is_protected FROM remote_content_cache WHERE kind = 'blob' AND cache_key = ?1",
    )
    .bind(hash.as_str())
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(protected, 1);
    // Dome pin copies this hash into the SDK store. The bookmark must keep its
    // cache copy after the temporary Dome pin is released.
    store
        .remove_remote_content("blob", hash.as_str())
        .await
        .unwrap();
    assert_eq!(
        store
            .get_remote_content("blob", hash.as_str())
            .await
            .unwrap(),
        Some(b"body".to_vec())
    );
    store.remove_bookmarked_post(&row.object_id).await.unwrap();
    let protected = sqlx::query_scalar::<_, i64>(
        "SELECT is_protected FROM remote_content_cache WHERE kind = 'blob' AND cache_key = ?1",
    )
    .bind(hash.as_str())
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(protected, 1);
    store
        .remove_bookmarked_post(&second.source_object_id)
        .await
        .unwrap();
    let protected = sqlx::query_scalar::<_, i64>(
        "SELECT is_protected FROM remote_content_cache WHERE kind = 'blob' AND cache_key = ?1",
    )
    .bind(hash.as_str())
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(protected, 0);
    store
        .remove_remote_content("blob", hash.as_str())
        .await
        .unwrap();
    assert!(
        store
            .get_remote_content("blob", hash.as_str())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn remote_cache_reclaims_oldest_entries_with_two_entry_budget() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let now = now_ms().unwrap();
    let payload = [7u8; 100];
    let charge = 100 + "blob".len() + "a".len() + "scope".len() + 64;
    let budget = (charge * 2) as i64;
    for (key, at) in [("a", now - 3), ("b", now - 2)] {
        assert!(
            store
                .put_remote_content_with_budget(
                    "blob",
                    key,
                    "scope",
                    CachePayload::Bytes(&payload),
                    None,
                    None,
                    budget,
                    at
                )
                .await
                .unwrap()
        );
    }
    for (key, at) in [("c", now - 1), ("d", now)] {
        assert!(
            store
                .put_remote_content_with_budget(
                    "blob",
                    key,
                    "scope",
                    CachePayload::Bytes(&payload),
                    None,
                    None,
                    budget,
                    at
                )
                .await
                .unwrap()
        );
    }
    assert!(
        store
            .get_remote_content("blob", "a")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_remote_content("blob", "b")
            .await
            .unwrap()
            .is_none()
    );
    for key in ["c", "d"] {
        assert!(
            store
                .get_remote_content("blob", key)
                .await
                .unwrap()
                .is_some()
        );
    }
    let used = sqlx::query_scalar::<_, i64>(
        "SELECT used_bytes FROM remote_content_cache_usage WHERE id = 1",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(used, budget);
}

#[tokio::test]
async fn in_flight_reservation_counts_against_cache_writes_and_releases_on_drop() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let mut reservation = store.empty_remote_cache_reservation();
    assert!(
        store
            .reserve_remote_cache_bytes(&mut reservation, REMOTE_CACHE_CAPACITY_BYTES as u64 - 80,)
            .await
            .unwrap()
    );
    assert!(
        !store
            .put_remote_content("blob", "reserved", "scope", &[1; 100])
            .await
            .unwrap()
    );
    drop(reservation);
    assert!(
        store
            .put_remote_content("blob", "reserved", "scope", &[1; 100])
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn remote_cache_reclaims_at_most_128_expired_items_per_write() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let now = now_ms().unwrap();
    let old = now - REMOTE_CACHE_UNUSED_MS - 1;
    for i in 0..129 {
        store
            .put_remote_content_with_budget(
                "record",
                &format!("item-{i}"),
                "scope",
                CachePayload::Bytes(b"v"),
                None,
                None,
                REMOTE_CACHE_CAPACITY_BYTES,
                old,
            )
            .await
            .unwrap();
    }
    store
        .put_remote_content_with_budget(
            "record",
            "fresh",
            "scope",
            CachePayload::Bytes(b"v"),
            None,
            None,
            REMOTE_CACHE_CAPACITY_BYTES,
            now,
        )
        .await
        .unwrap();
    let expired = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM remote_content_cache WHERE last_used_at < ?1",
    )
    .bind(now - REMOTE_CACHE_UNUSED_MS)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(expired, 1);
    assert_eq!(store.reclaim_remote_cache_step().await.unwrap(), 1);
    assert_eq!(store.reclaim_remote_cache_step().await.unwrap(), 0);
}
