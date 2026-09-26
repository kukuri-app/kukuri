//! #1221 R5-G: 保護所有先への移行の台帳。ページの上限、再接続後の継続、共有 blob、ACK での解放、件数に依らない読み。

use super::*;
use crate::{
    BookmarkedPostRow, DirectMessageOutboxRow, DirectMessageStore, PROTECTED_MIGRATION_KINDS,
    PROTECTED_MIGRATION_PAGE, ReactionBookmarkStore,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

fn hash(index: usize) -> String {
    blake3::hash(format!("blob-{index}").as_bytes())
        .to_hex()
        .to_string()
}

fn bookmark(index: usize) -> BookmarkedPostRow {
    BookmarkedPostRow {
        source_object_id: EnvelopeId::from(format!("post-{index:05}")),
        source_envelope_id: EnvelopeId::from(format!("post-{index:05}")),
        source_replica_id: ReplicaId::new("topic::migration"),
        topic_id: "migration".into(),
        channel_id: "public".into(),
        author_pubkey: "c".repeat(64),
        created_at: index as i64,
        object_kind: "post".into(),
        payload_ref: PayloadRef::BlobText {
            hash: BlobHash::new(hash(index)),
            mime: "text/plain".into(),
            bytes: 1,
        },
        content: Some(format!("body {index}")),
        attachments: Vec::new(),
        reply_to_object_id: None,
        root_object_id: None,
        repost_of: None,
        bookmarked_at: index as i64,
    }
}

async fn protected_refs(store: &SqliteStore, key: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT ref_id FROM remote_content_cache_protected_ref WHERE kind = 'blob' AND cache_key = ?1 \
         ORDER BY ref_id",
    )
    .bind(key)
    .fetch_all(store.pool())
    .await
    .expect("protected refs")
}

async fn cache_row(store: &SqliteStore, key: &str) -> Option<(i64, i64)> {
    sqlx::query_as(
        "SELECT is_protected, charged_bytes FROM remote_content_cache WHERE kind = 'blob' AND cache_key = ?1",
    )
    .bind(key)
    .fetch_optional(store.pool())
    .await
    .expect("cache row")
}

async fn used_bytes(store: &SqliteStore) -> i64 {
    sqlx::query_scalar("SELECT used_bytes FROM remote_content_cache_usage WHERE id = 1")
        .fetch_one(store.pool())
        .await
        .expect("usage")
}

#[tokio::test]
async fn pages_stay_within_128_rows_and_continue_after_reconnect() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("kukuri.db");
    let store = SqliteStore::connect_file(&path).await?;
    for index in 0..300 {
        store.put_bookmarked_post(bookmark(index)).await?;
    }
    let first = store.protected_migration_page("bookmark", "me").await?;
    assert_eq!(first.candidates.len(), PROTECTED_MIGRATION_PAGE);
    assert!(!first.done);
    store
        .finish_protected_migration_page("bookmark", &first.cursor, first.done)
        .await?;
    store.close().await;

    // 位置を保存した後に止めても、開き直すと続きから読む。
    let store = SqliteStore::connect_file(&path).await?;
    let mut seen = first
        .candidates
        .iter()
        .map(|candidate| candidate.reference.clone())
        .collect::<Vec<_>>();
    loop {
        let page = store.protected_migration_page("bookmark", "me").await?;
        assert!(page.candidates.len() <= PROTECTED_MIGRATION_PAGE);
        seen.extend(page.candidates.iter().map(|item| item.reference.clone()));
        store
            .finish_protected_migration_page("bookmark", &page.cursor, page.done)
            .await?;
        if page.done {
            break;
        }
    }
    let unique = seen.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!((seen.len(), unique.len()), (300, 300));
    assert_eq!(store.protected_migration_caught_up_at().await?, None);
    for kind in PROTECTED_MIGRATION_KINDS {
        store
            .finish_protected_migration_page(kind, "", true)
            .await?;
    }
    assert!(store.protected_migration_caught_up_at().await?.is_some());
    // 索引の順に並ばない kind を読み直させると、全体は未完了に戻る。
    store.reset_protected_migration("private").await?;
    assert_eq!(store.protected_migration_caught_up_at().await?, None);
    store.close().await;
    Ok(())
}

#[tokio::test]
async fn shared_blob_is_stored_once_and_stays_protected_until_the_last_reference()
-> anyhow::Result<()> {
    let store = SqliteStore::connect_memory().await?;
    let row = bookmark(1);
    let shared = hash(1);
    // 非保護の cache 行が先にある(別 peer から表示のために取得した)。
    assert!(
        store
            .put_remote_content("blob", &shared, "blob", b"shared body")
            .await?
    );
    let charged = cache_row(&store, &shared).await.expect("cached").1;
    assert_eq!(used_bytes(&store).await, charged);

    let own = vec![("blob".to_string(), shared.clone())];
    assert!(store.set_protected_refs("own:post-00001", &own).await?);
    store.put_bookmarked_post(row.clone()).await?;
    assert!(
        store
            .put_remote_content("blob", &shared, "blob", b"shared body")
            .await?
    );
    assert_eq!(
        protected_refs(&store, &shared).await,
        ["bookmark:post-00001", "own:post-00001"]
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM remote_content_cache WHERE cache_key = ?1")
            .bind(&shared)
            .fetch_one(store.pool())
            .await?;
    assert_eq!(rows, 1, "the shared blob must not be stored twice");
    assert_eq!(cache_row(&store, &shared).await.map(|row| row.0), Some(1));
    assert_eq!(
        used_bytes(&store).await,
        0,
        "protected bytes are not counted"
    );

    // bookmark を外しても本人投稿の参照が残り、回収の対象にならない。
    store.remove_bookmarked_post(&row.source_object_id).await?;
    assert_eq!(protected_refs(&store, &shared).await, ["own:post-00001"]);
    sqlx::query("UPDATE remote_content_cache SET last_used_at = 0")
        .execute(store.pool())
        .await?;
    assert_eq!(store.reclaim_remote_cache_step().await?, 0);
    assert_eq!(cache_row(&store, &shared).await.map(|row| row.0), Some(1));
    Ok(())
}

#[tokio::test]
async fn ack_and_removed_index_rows_release_protection() -> anyhow::Result<()> {
    let store = SqliteStore::connect_memory().await?;
    let frame = hash(7);
    store
        .put_direct_message_outbox(DirectMessageOutboxRow {
            dm_id: "dm-1".into(),
            message_id: "message-1".into(),
            peer_pubkey: "d".repeat(64),
            frame_blob_hash: BlobHash::new(frame.clone()),
            created_at: 1,
            last_attempt_at: None,
        })
        .await?;
    let page = store.protected_migration_page("dm_outbox", "me").await?;
    assert_eq!(page.candidates.len(), 1);
    assert_eq!(page.candidates[0].reference, "dm_outbox:dm-1/message-1");
    let desired = vec![("blob".to_string(), frame.clone())];
    assert!(
        store
            .set_protected_refs("dm_outbox:dm-1/message-1", &desired)
            .await?
    );
    assert!(
        store
            .put_remote_content("blob", &frame, "blob", b"frame")
            .await?
    );
    assert_eq!(cache_row(&store, &frame).await.map(|row| row.0), Some(1));

    // ACK で送信待ちを消すと、同じ transaction で frame の保護を外す。
    store
        .remove_direct_message_outbox("dm-1", "message-1")
        .await?;
    assert!(protected_refs(&store, &frame).await.is_empty());
    assert_eq!(cache_row(&store, &frame).await.map(|row| row.0), Some(0));

    // 移行が読んだ後に行が消えていれば、参照を付けない(競合しても消えた行を守り続けない)。
    assert!(
        !store
            .set_protected_refs("dm_outbox:dm-1/message-1", &desired)
            .await?
    );
    assert!(protected_refs(&store, &frame).await.is_empty());
    let asset = hash(8);
    assert!(
        !store
            .set_protected_refs(
                "reaction_bookmark:missing",
                &[("blob".to_string(), asset.clone())]
            )
            .await?
    );
    assert!(protected_refs(&store, &asset).await.is_empty());
    assert!(
        ReactionBookmarkStore::list_bookmarked_custom_reactions(&store)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn one_page_read_does_not_grow_with_table_size() -> anyhow::Result<()> {
    async fn measured_steps(rows: usize) -> anyhow::Result<u64> {
        let counter = Arc::new(AtomicU64::new(0));
        let store = SqliteStore::connect_memory_counting_vm_steps(counter.clone()).await?;
        let keys = generate_keys();
        for index in 0..rows {
            store
                .put_envelope(build_post_envelope(
                    &keys,
                    &TopicId::new("migration"),
                    &format!("post {index}"),
                    None,
                )?)
                .await?;
            store.put_bookmarked_post(bookmark(index)).await?;
            store
                .put_direct_message_outbox(DirectMessageOutboxRow {
                    dm_id: "dm-1".into(),
                    message_id: format!("message-{index:05}"),
                    peer_pubkey: "d".repeat(64),
                    frame_blob_hash: BlobHash::new(hash(index)),
                    created_at: index as i64,
                    last_attempt_at: None,
                })
                .await?;
            sqlx::query(
                "INSERT INTO live_session_cache (session_id, topic_id, host_pubkey, title, description, \
                 status, started_at, updated_at, source_replica_id, source_key, manifest_blob_hash, \
                 derived_at, projection_version) \
                 VALUES (?1, 'migration', 'host', '', '', 'live', 0, 0, 'topic::migration', 'key', ?2, ?3, 1)",
            )
            .bind(format!("session-{index:05}"))
            .bind(hash(index))
            .bind(index as i64)
            .execute(store.pool())
            .await?;
        }
        // 途中の位置から 1 ページを読む。
        let middle = rows / 2;
        for (kind, cursor) in [
            ("own_envelope", middle.to_string()),
            ("bookmark", middle.to_string()),
            (
                "dm_outbox",
                serde_json::to_string(&(middle as i64, format!("message-{middle:05}"), "dm-1"))?,
            ),
            (
                "live_session",
                serde_json::to_string(&(middle as i64, format!("session-{middle:05}")))?,
            ),
        ] {
            store
                .finish_protected_migration_page(kind, &cursor, false)
                .await?;
        }
        counter.store(0, Ordering::Relaxed);
        for kind in ["own_envelope", "bookmark", "dm_outbox", "live_session"] {
            let page = store
                .protected_migration_page(kind, keys.public_key_hex().as_str())
                .await?;
            assert_eq!(page.candidates.len(), PROTECTED_MIGRATION_PAGE, "{kind}");
        }
        Ok(counter.load(Ordering::Relaxed))
    }
    let short = measured_steps(400).await?;
    let long = measured_steps(2_000).await?;
    assert!(
        long <= short * 5 / 4,
        "one migration page grew with table size: {short} -> {long}"
    );
    Ok(())
}

#[tokio::test]
async fn a_reused_rowid_after_the_cursor_is_read_again() -> anyhow::Result<()> {
    let store = SqliteStore::connect_memory().await?;
    for index in 0..3 {
        store.put_bookmarked_post(bookmark(index)).await?;
    }
    let page = store.protected_migration_page("bookmark", "me").await?;
    assert!(page.done);
    store
        .finish_protected_migration_page("bookmark", &page.cursor, page.done)
        .await?;
    // 最大の行を消した直後の追加は同じ rowid を使う。移行済みの位置の手前へ戻って読み直す。
    store
        .remove_bookmarked_post(&EnvelopeId::from("post-00002"))
        .await?;
    store.put_bookmarked_post(bookmark(3)).await?;
    let page = store.protected_migration_page("bookmark", "me").await?;
    assert!(
        page.candidates
            .iter()
            .any(|candidate| candidate.reference == "bookmark:post-00003"),
        "the row that reused a migrated rowid must be migrated"
    );
    Ok(())
}
