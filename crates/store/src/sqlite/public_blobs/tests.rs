use super::*;
use crate::parity::public_refs::{bookmark, hash, post, profile, public, reaction, withdrawal};
use kukuri_core::ObjectStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

async fn hold(store: &SqliteStore, hash: &str) {
    assert!(
        store
            .put_remote_content("blob", hash, "blob", b"bytes")
            .await
            .unwrap()
    );
}

async fn scheduled(store: &SqliteStore) -> Vec<String> {
    sqlx::query_scalar("SELECT blob_hash FROM public_blob_announcements ORDER BY blob_hash")
        .fetch_all(store.pool())
        .await
        .unwrap()
}

/// 投稿・リンクプレビュー・profile・reaction・自作と保存済みの custom reaction の公開参照は、Web（IndexedDB）と同じ
/// 操作列で確かめる。
#[tokio::test]
async fn public_refs_follow_the_shared_scenario() {
    let store = SqliteStore::connect_memory().await.unwrap();
    crate::parity::public_refs::check_public_blob_refs(&store).await;
}

/// 公開参照のある blob は、手元に保持がある間だけ告知の予定に載る。
#[tokio::test]
async fn a_public_blob_is_announced_only_while_it_is_held() {
    let store = SqliteStore::connect_memory().await.unwrap();
    hold(&store, &hash(9)).await;
    store
        .put_object_projection(post("p1", "public", &hash(1), &[&hash(9)]))
        .await
        .unwrap();
    // 保持のある添付は公開参照に載ったときに、本文は保持したときに載る。
    assert_eq!(scheduled(&store).await, [hash(9)]);
    hold(&store, &hash(1)).await;
    assert_eq!(scheduled(&store).await, [hash(1), hash(9)]);
    // 保持していても公開参照の無い blob は載らない。
    hold(&store, &hash(2)).await;
    assert_eq!(scheduled(&store).await, [hash(1), hash(9)]);

    store.remove_remote_content("blob", &hash(9)).await.unwrap();
    assert_eq!(scheduled(&store).await, [hash(1)]);
}

/// 取り下げと projection の回収で、投稿の本文・添付・リンクプレビューの画像は公開参照から外れる。
/// 同じ hash を別の公開記録が参照している間は公開のまま。
#[tokio::test]
async fn withdrawal_and_reclaim_forget_only_the_posts_refs() {
    let store = SqliteStore::connect_memory().await.unwrap();
    for seed in 1..=4 {
        hold(&store, &hash(seed)).await;
    }
    store
        .put_object_projection(post("p1", "public", &hash(1), &[&hash(2)]))
        .await
        .unwrap();
    store.note_link_preview_image("p1", &hash(3)).await.unwrap();
    store
        .put_remote_object_projection(post("p2", "public", &hash(4), &[&hash(2)]))
        .await
        .unwrap();
    assert_eq!(
        scheduled(&store).await,
        [hash(1), hash(2), hash(3), hash(4)]
    );

    store.put_post_withdrawal(withdrawal("p1")).await.unwrap();
    assert_eq!(
        public(&store, &[1, 2, 3, 4]).await,
        [false, true, false, true]
    );
    assert_eq!(scheduled(&store).await, [hash(2), hash(4)]);

    store
        .remove_remote_content("projection", "p2")
        .await
        .unwrap();
    assert_eq!(public(&store, &[2, 4]).await, [false, false]);
    assert!(scheduled(&store).await.is_empty());
}

/// 予定は上限まで。満杯なら本人の blob を先に残し、次に cache の利用が新しいものを残す。
#[tokio::test]
async fn the_schedule_keeps_own_and_recently_used_blobs_within_its_cap() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let now = now_ms().unwrap();
    for seed in 0..PUBLIC_BLOB_ANNOUNCEMENT_CAP as u32 {
        hold(&store, &hash(seed)).await;
        sqlx::query("UPDATE remote_content_cache SET last_used_at = ?1 WHERE cache_key = ?2")
            .bind(now - 100_000 + i64::from(seed))
            .bind(hash(seed))
            .execute(store.pool())
            .await
            .unwrap();
    }
    let mut rows = Vec::new();
    for seed in 0..PUBLIC_BLOB_ANNOUNCEMENT_CAP as u32 + 2 {
        rows.push(post(&format!("p{seed}"), "public", &hash(seed), &[]));
    }
    // 上限を超える 2 件はまだ保持していない。
    store.put_object_projections(rows).await.unwrap();
    assert_eq!(
        scheduled(&store).await.len(),
        PUBLIC_BLOB_ANNOUNCEMENT_CAP as usize
    );

    // 利用の古い blob は、満杯の予定に入らない。
    let older = hash(PUBLIC_BLOB_ANNOUNCEMENT_CAP as u32);
    sqlx::query(
        "INSERT INTO remote_content_cache (kind, cache_key, scope_key, payload, charged_bytes,          is_protected, last_used_at) VALUES ('blob', ?1, 'blob', x'00', 1, 0, ?2)",
    )
    .bind(&older)
    .bind(now - 200_000)
    .execute(store.pool())
    .await
    .unwrap();
    let mut tx = begin_public_blob_write(&store).await.unwrap();
    refresh_public_blob_announcement(&mut tx, &older, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(!scheduled(&store).await.contains(&older));

    // 本人の blob は、利用の最も古い他人の blob と入れ替わる。
    let own = hash(PUBLIC_BLOB_ANNOUNCEMENT_CAP as u32 + 1);
    store
        .put_owned_blob(&format!("own_blob:{own}"), &own, b"bytes")
        .await
        .unwrap();
    let schedule = scheduled(&store).await;
    assert!(schedule.contains(&own));
    assert!(!schedule.contains(&hash(0)));
    assert_eq!(schedule.len(), PUBLIC_BLOB_ANNOUNCEMENT_CAP as usize);
}

/// 時刻の来た予定を時刻の順に返し、来ていなければ次の時刻を返す。始め直すと、すべてがすぐ告知の対象になる。
#[tokio::test]
async fn due_announcements_come_in_order_and_restart_makes_all_due() {
    let store = SqliteStore::connect_memory().await.unwrap();
    for seed in 1..=3 {
        hold(&store, &hash(seed)).await;
        store
            .put_object_projection(post(&format!("p{seed}"), "public", &hash(seed), &[]))
            .await
            .unwrap();
    }
    for (seed, next_at) in [(1, 300), (2, 100), (3, 200)] {
        store
            .reschedule_public_blob_announcement(&hash(seed), Some(next_at))
            .await
            .unwrap();
    }
    assert_eq!(
        store.due_public_blob_announcements(250, 128).await.unwrap(),
        (vec![hash(2), hash(3)], None)
    );
    assert_eq!(
        store.due_public_blob_announcements(50, 128).await.unwrap(),
        (Vec::new(), Some(100))
    );
    store
        .reschedule_public_blob_announcement(&hash(2), None)
        .await
        .unwrap();
    assert_eq!(scheduled(&store).await, [hash(1), hash(3)]);

    store
        .restart_public_blob_announcements(10_000)
        .await
        .unwrap();
    assert_eq!(
        store
            .due_public_blob_announcements(10_000, 128)
            .await
            .unwrap(),
        (vec![hash(1), hash(3)], None)
    );
}

/// 公開参照と告知の予定は記録と同じ transaction で書くので、記録の保存が確定しなければ残らない。
#[tokio::test]
async fn refs_and_schedule_roll_back_with_the_record() {
    let store = SqliteStore::connect_memory().await.unwrap();
    hold(&store, &hash(1)).await;
    let mut tx = begin_public_blob_write(&store).await.unwrap();
    replace_public_blob_refs(&mut tx, "post", "p1", vec![hash(1)])
        .await
        .unwrap();
    drop(tx);
    assert_eq!(public(&store, &[1]).await, [false]);
    assert!(scheduled(&store).await.is_empty());
}

/// 読み出しの期限（7 日の未使用）が回収より先に切れた保持は、告知の直前に予定から外す。利用時刻は変えない。
#[tokio::test]
async fn an_expired_hold_leaves_the_schedule_before_it_is_reclaimed() {
    let store = SqliteStore::connect_memory().await.unwrap();
    for seed in 1..=2 {
        hold(&store, &hash(seed)).await;
        store
            .put_object_projection(post(&format!("p{seed}"), "public", &hash(seed), &[]))
            .await
            .unwrap();
    }
    let expired = now_ms().unwrap() - REMOTE_CACHE_UNUSED_MS - 1;
    sqlx::query("UPDATE remote_content_cache SET last_used_at = ?1 WHERE cache_key = ?2")
        .bind(expired)
        .bind(hash(1))
        .execute(store.pool())
        .await
        .unwrap();
    let now = now_ms().unwrap();
    assert_eq!(
        store.due_public_blob_announcements(now, 1).await.unwrap(),
        (Vec::new(), Some(now))
    );
    assert_eq!(
        store.due_public_blob_announcements(now, 1).await.unwrap(),
        (vec![hash(2)], None)
    );
    assert_eq!(scheduled(&store).await, [hash(2)]);
    let used: i64 =
        sqlx::query_scalar("SELECT last_used_at FROM remote_content_cache WHERE cache_key = ?1")
            .bind(hash(1))
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(used, expired);
}

/// 導入前の公開投稿・profile・reaction を、種類ごとに上限の行数ずつ取り込み、終えたら `true` を返す。
#[tokio::test]
async fn backfill_takes_old_records_a_page_at_a_time() {
    let store = SqliteStore::connect_memory().await.unwrap();
    for seed in 1..=3 {
        store
            .put_object_projection(post(&format!("p{seed}"), "public", &hash(seed), &[]))
            .await
            .unwrap();
    }
    store
        .put_object_projection(post("private", "channel-x", &hash(4), &[]))
        .await
        .unwrap();
    store
        .upsert_profile(profile(Some(&hash(5)), 1))
        .await
        .unwrap();
    store
        .upsert_reaction_cache(reaction("r1", "p1", &hash(6), ObjectStatus::Active))
        .await
        .unwrap();
    // 導入前の行と同じく、索引に無い状態にする。
    sqlx::query("DELETE FROM public_blob_refs")
        .execute(store.pool())
        .await
        .unwrap();

    assert!(!store.backfill_public_blob_refs_step(2).await.unwrap());
    assert_eq!(
        public(&store, &[1, 2, 3, 5, 6]).await,
        [true, true, false, true, true]
    );
    while !store.backfill_public_blob_refs_step(2).await.unwrap() {}
    assert_eq!(
        public(&store, &[1, 2, 3, 4, 5, 6]).await,
        [true, true, true, false, true, true]
    );
}

/// #1232 AC-3: 導入前の自作の custom reaction の asset（envelope の行）と保存済みの行も、rowid の窓ごとに取り込む。
/// asset でない envelope の行は、窓を進めるだけで参照にしない。
#[tokio::test]
async fn backfill_takes_own_reaction_assets_and_saved_reactions() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let keys = kukuri_core::generate_keys();
    let follow = kukuri_core::build_follow_edge_envelope(
        &keys,
        &kukuri_core::generate_keys().public_key(),
        kukuri_core::FollowEdgeStatus::Active,
    )
    .unwrap();
    store.put_envelope(follow).await.unwrap();
    for seed in [1, 2] {
        let asset = kukuri_core::build_custom_reaction_asset_envelope(
            &keys,
            kukuri_core::BlobHash::new(hash(seed)),
            format!("asset-{seed}"),
            "image/png".into(),
            1,
            1,
            1,
        )
        .unwrap();
        store.put_envelope(asset).await.unwrap();
    }
    store
        .put_bookmarked_custom_reaction(bookmark(&hash(3)))
        .await
        .unwrap();
    // 導入前の行と同じく、索引に無い状態にする。
    sqlx::query("DELETE FROM public_blob_refs")
        .execute(store.pool())
        .await
        .unwrap();

    assert!(!store.backfill_public_blob_refs_step(2).await.unwrap());
    assert_eq!(public(&store, &[1, 2, 3]).await, [true, false, true]);
    while !store.backfill_public_blob_refs_step(2).await.unwrap() {}
    assert_eq!(public(&store, &[1, 2, 3]).await, [true, true, true]);
}

/// 取込みは、空や読めない値を参照なしとして扱い、止まらない（行の読取りと同じく空文字列は無い扱い）。
#[tokio::test]
async fn backfill_treats_values_it_cannot_read_as_no_refs() {
    let store = SqliteStore::connect_memory().await.unwrap();
    store
        .put_object_projection(post("p1", "public", &hash(1), &[&hash(2)]))
        .await
        .unwrap();
    store
        .upsert_reaction_cache(reaction("r1", "p1", &hash(3), ObjectStatus::Active))
        .await
        .unwrap();
    for sql in [
        "UPDATE object_index_cache SET attachments_json = '', repost_of_json = '{'",
        "UPDATE reaction_cache SET custom_asset_snapshot_json = '{'",
        "DELETE FROM public_blob_refs",
    ] {
        sqlx::query(sql).execute(store.pool()).await.unwrap();
    }
    while !store.backfill_public_blob_refs_step(2).await.unwrap() {}
    assert_eq!(public(&store, &[1, 2, 3]).await, [true, false, false]);
}

/// 1 回の取込みと、満杯の予定への出し入れの命令の数は、表の件数によらない。
#[tokio::test]
async fn backfill_and_schedule_work_does_not_grow_with_the_number_of_records() {
    let mut backfill_steps = Vec::new();
    let mut refresh_steps = Vec::new();
    for records in [20_i64, 200, 2000] {
        let counter = Arc::new(AtomicU64::new(0));
        let store = SqliteStore::connect_memory_counting_vm_steps(counter.clone())
            .await
            .unwrap();
        store
            .put_object_projection(post("template", "public", &hash(0), &[]))
            .await
            .unwrap();
        sqlx::query(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < ?1) \
             INSERT INTO object_index_cache (object_id, topic_id, channel_id, author_pubkey, created_at, \
             object_kind, root_object_id, reply_to_object_id, payload_ref_json, content, attachments_json, \
             repost_of_json, content_labels_json, source_replica_id, source_key, source_envelope_id, \
             source_blob_hash, derived_at, projection_version, source_docs_author) \
             SELECT 'bulk-' || n, topic_id, channel_id, author_pubkey, created_at, object_kind, \
             root_object_id, reply_to_object_id, payload_ref_json, content, attachments_json, repost_of_json, \
             content_labels_json, source_replica_id, source_key, source_envelope_id, printf('%064x', n), \
             derived_at, projection_version, source_docs_author \
             FROM seq, object_index_cache WHERE object_id = 'template'",
        )
        .bind(records)
        .execute(store.pool())
        .await
        .unwrap();
        // 予定を満杯にし、保持を表の件数だけ置く（予定に載っていない分も含む）。
        sqlx::query(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < ?1) \
             INSERT INTO remote_content_cache (kind, cache_key, scope_key, payload, charged_bytes, \
             is_protected, last_used_at) \
             SELECT 'blob', printf('%064x', n), 'blob', x'00', 1, 0, ?2 + n FROM seq",
        )
        .bind(records.max(PUBLIC_BLOB_ANNOUNCEMENT_CAP))
        .bind(now_ms().unwrap() - 1_000_000)
        .execute(store.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO public_blob_announcements (blob_hash, own, next_at) \
             SELECT cache_key, 0, 0 FROM remote_content_cache WHERE kind = 'blob' \
             ORDER BY cache_key LIMIT ?1",
        )
        .bind(PUBLIC_BLOB_ANNOUNCEMENT_CAP)
        .execute(store.pool())
        .await
        .unwrap();
        sqlx::query("DELETE FROM public_blob_refs")
            .execute(store.pool())
            .await
            .unwrap();

        counter.store(0, Ordering::Relaxed);
        assert!(!store.backfill_public_blob_refs_step(16).await.unwrap());
        backfill_steps.push(counter.load(Ordering::Relaxed));

        // 満杯の予定へ、利用の新しい公開 blob を 1 件入れる。
        let newest = hash(9_999_999);
        sqlx::query(
            "INSERT INTO remote_content_cache (kind, cache_key, scope_key, payload, charged_bytes, \
             is_protected, last_used_at) VALUES ('blob', ?1, 'blob', x'00', 1, 0, ?2)",
        )
        .bind(&newest)
        .bind(now_ms().unwrap())
        .execute(store.pool())
        .await
        .unwrap();
        counter.store(0, Ordering::Relaxed);
        store
            .note_link_preview_image("template", &newest)
            .await
            .unwrap();
        refresh_steps.push(counter.load(Ordering::Relaxed));
        assert!(scheduled(&store).await.contains(&newest));
    }
    assert!(
        backfill_steps.windows(2).all(|pair| pair[0] == pair[1]),
        "backfill steps {backfill_steps:?}"
    );
    assert!(
        refresh_steps.windows(2).all(|pair| pair[0] == pair[1]),
        "refresh steps {refresh_steps:?}"
    );
}

/// 公開参照と予定の読取りは索引を使い、表を走査・並べ替えしない。
#[tokio::test]
async fn public_blob_queries_use_their_indexes() {
    let store = SqliteStore::connect_memory().await.unwrap();
    for (sql, expected) in [
        (
            "SELECT EXISTS(SELECT 1 FROM public_blob_refs WHERE blob_hash = ?1)",
            "idx_public_blob_refs_hash",
        ),
        (
            "SELECT blob_hash FROM public_blob_refs WHERE source_kind = ?1 AND source_id = ?1",
            "PRIMARY KEY",
        ),
        (
            "SELECT a.blob_hash, EXISTS(SELECT 1 FROM remote_content_cache c \
             WHERE c.kind = 'blob' AND c.cache_key = a.blob_hash \
             AND (c.is_protected = 1 OR c.last_used_at > ?1)) \
             FROM public_blob_announcements a WHERE a.next_at <= ?1 \
             ORDER BY a.next_at, a.blob_hash LIMIT ?1",
            "idx_public_blob_announcements_next",
        ),
    ] {
        let plan = sqlx::query(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
            .bind("x")
            .fetch_all(store.pool())
            .await
            .unwrap();
        let details = plan
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>();
        assert!(
            details.iter().any(|detail| detail.contains(expected))
                && !details
                    .iter()
                    .any(|detail| detail.starts_with("SCAN public_blob")
                        || detail.contains("TEMP B-TREE")),
            "{sql}: {details:?}"
        );
    }
}
