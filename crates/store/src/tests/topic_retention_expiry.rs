use super::*;
use crate::TopicRetentionStore;

/// #2053: 日の選択のメタデータ更新も、小窓の回収量に収まる。
#[tokio::test]
async fn topic_retention_reclaim_bounds_expired_day_updates() {
    let mut counts = Vec::new();
    let mut instructions = Vec::new();
    for days in [8_u64, 64] {
        let vm = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let store = SqliteStore::connect_memory_counting_vm_steps(vm.clone())
            .await
            .expect("sqlite store");
        store.retain_topic("topic-a", 1).await.expect("retain");
        store
            .open_topic_retention_days("topic-a", 1, days)
            .await
            .expect("days");
        for bucket in 1..=days {
            store
                .protect_topic_retention_day("topic-a", bucket, &format!("scope-{bucket}"))
                .await
                .expect("scope");
        }
        for query in [
            "CREATE TABLE retention_changes (n INTEGER NOT NULL)",
            "INSERT INTO retention_changes VALUES (0)",
            "CREATE TRIGGER count_day_update AFTER UPDATE ON topic_retention_days BEGIN UPDATE retention_changes SET n = n + 1; END",
            "CREATE TRIGGER count_scope_delete AFTER DELETE ON topic_retention_scopes BEGIN UPDATE retention_changes SET n = n + 1; END",
            "CREATE TRIGGER count_expiry_insert AFTER INSERT ON topic_retention_expiry BEGIN UPDATE retention_changes SET n = n + 1; END",
            "CREATE TRIGGER count_expiry_update AFTER UPDATE ON topic_retention_expiry BEGIN UPDATE retention_changes SET n = n + 1; END",
            "CREATE TRIGGER count_expiry_delete AFTER DELETE ON topic_retention_expiry BEGIN UPDATE retention_changes SET n = n + 1; END",
            "CREATE TRIGGER count_limit_update AFTER UPDATE ON topic_retention_limits BEGIN UPDATE retention_changes SET n = n + 1; END",
        ] {
            sqlx::query(query)
                .execute(store.pool())
                .await
                .expect("observe writes");
        }
        vm.store(0, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            store
                .reclaim_topic_retention_step(100, 1)
                .await
                .expect("step"),
            1
        );
        instructions.push(vm.load(std::sync::atomic::Ordering::Relaxed));
        let changed: i64 = sqlx::query_scalar("SELECT n FROM retention_changes")
            .fetch_one(store.pool())
            .await
            .expect("changed rows");
        assert!(changed <= i64::from(crate::MAX_RETENTION_DAYS) * 2 + 3);
        counts.push(changed);
        // 続行すれば全日を外し、参照のない日も一日一件として数える。
        let mut spent = 1;
        loop {
            let n = store
                .reclaim_topic_retention_step(100, 1)
                .await
                .expect("continue");
            if n == 0 {
                break;
            }
            spent += n;
        }
        assert_eq!(spent as u64, days);
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM topic_retention_scopes")
            .fetch_one(store.pool())
            .await
            .expect("remaining scopes");
        assert_eq!(remaining, 0);
    }
    assert_eq!(
        instructions[0], instructions[1],
        "SQL reads must not grow with topic/day rows"
    );
    assert_eq!(
        counts[0], counts[1],
        "metadata work must not grow with topic/day rows"
    );
}

/// 回収の窓に入らなかった期限切れ日も、retryから保護し直さない。
#[tokio::test]
async fn topic_retention_expired_pending_day_cannot_be_protected_again() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    store.retain_topic("topic-a", 1).await.expect("retain");
    store
        .open_topic_retention_days("topic-a", 1, 8)
        .await
        .expect("days");
    for bucket in 1..=8 {
        store
            .protect_topic_retention_day("topic-a", bucket, &format!("scope-{bucket}"))
            .await
            .expect("scope");
    }
    assert_eq!(
        store
            .reclaim_topic_retention_step(9, 1)
            .await
            .expect("step"),
        1
    );
    store
        .claim_topic_retention_blob("topic-a", 8, "retry-blob")
        .await
        .expect("retry claim");
    let refs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM remote_content_cache_protected_ref WHERE cache_key = 'retry-blob'",
    )
    .fetch_one(store.pool())
    .await
    .expect("references");
    assert_eq!(
        refs, 0,
        "expiry must also reject a retry for an uncollected day"
    );
}

#[tokio::test]
async fn topic_retention_expiry_survives_restart_reopen_and_downgrade() {
    let dir = tempdir().expect("directory");
    let path = dir.path().join("retention.db");
    super::migrations::materialize_sqlite_fixture(&path, 20261031000000)
        .await
        .expect("old schema");
    let store = SqliteStore::connect_file(&path).await.expect("upgrade");
    store.retain_topic("topic-a", 1).await.expect("retain");
    store
        .open_topic_retention_days("topic-a", 1, 8)
        .await
        .expect("days");
    for bucket in 1..=8 {
        store
            .protect_topic_retention_day("topic-a", bucket, &format!("scope-{bucket}"))
            .await
            .expect("scope");
    }
    store
        .reclaim_topic_retention_step(9, 1)
        .await
        .expect("expire");
    store.close().await;
    let store = SqliteStore::connect_file(&path).await.expect("restart");
    // 期間を延ばしても、失効済みの未回収行は復活しない。
    store
        .reclaim_topic_retention_step(2, 1)
        .await
        .expect("longer period");
    assert!(
        !store
            .protect_topic_retention_day("topic-a", 8, "scope-8")
            .await
            .expect("expired")
    );
    assert_eq!(
        store
            .next_topic_retention_backfill("topic-a", 8, 8)
            .await
            .expect("backfill"),
        None
    );
    // 通常のrecord書込みと、登録前recordへの後付けでも失効を守る。
    store
        .put_remote_record("scope-8", "late", "author", &[1])
        .await
        .expect("late record");
    store
        .protect_topic_retention_scopes_step(128)
        .await
        .expect("pending scope");
    let key = SqliteStore::remote_record_cache_key("scope-8", "late", "author");
    let protected: i64 =
        sqlx::query_scalar("SELECT is_protected FROM remote_content_cache WHERE cache_key = ?1")
            .bind(key)
            .fetch_one(store.pool())
            .await
            .expect("cache");
    assert_eq!(protected, 0);
    // 回収後に同じ日を作り直した場合は、現在の世代で開く。
    store
        .reclaim_topic_retention_step(9, 128)
        .await
        .expect("finish release");
    store
        .open_topic_retention_days("topic-a", 5, 8)
        .await
        .expect("new days");
    assert!(
        store
            .protect_topic_retention_day("topic-a", 8, "scope-8")
            .await
            .expect("new generation")
    );
    store
        .reclaim_topic_retention_step(7, 1)
        .await
        .expect("partial expiry");
    assert!(
        store
            .protect_topic_retention_day("topic-a", 8, "scope-8")
            .await
            .expect("still in period")
    );
    // 保持をやめて再開する場合も開いた世代へ戻り、同じcutoffの次passでは再失効する。
    store
        .reclaim_topic_retention_step(9, 1)
        .await
        .expect("expire again");
    store.release_topic("topic-a").await.expect("release topic");
    store
        .retain_topic("topic-a", 2)
        .await
        .expect("retain again");
    assert!(
        store
            .protect_topic_retention_day("topic-a", 8, "scope-8")
            .await
            .expect("reopened")
    );
    store
        .reclaim_topic_retention_step(9, 1)
        .await
        .expect("same cutoff");
    assert!(
        !store
            .protect_topic_retention_day("topic-a", 8, "scope-8")
            .await
            .expect("expired again")
    );
    STORE_MIGRATOR
        .undo(store.pool(), 20261031000000)
        .await
        .expect("downgrade");
    let releasing: i64 = sqlx::query_scalar(
        "SELECT releasing FROM topic_retention_days WHERE topic_id = 'topic-a' AND bucket = 8",
    )
    .fetch_one(store.pool())
    .await
    .expect("legacy flag");
    assert_eq!(releasing, 1);
    STORE_MIGRATOR
        .run(store.pool())
        .await
        .expect("upgrade again");
    assert!(
        !store
            .protect_topic_retention_day("topic-a", 8, "scope-8")
            .await
            .expect("migration preserves expiry")
    );
    store
        .reclaim_topic_retention_step(500, 1)
        .await
        .expect("old dates");
    let buckets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM topic_retention_expiry")
        .fetch_one(store.pool())
        .await
        .expect("bounded bucket table");
    assert_eq!(buckets, i64::from(crate::MAX_RETENTION_DAYS));
}
