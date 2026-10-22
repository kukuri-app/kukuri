//! #1909: 保持する公開 topic と、取り込みの続きの位置。SQLite と memory で同じ結果になる。

use super::*;
use crate::{TopicRetentionCursor, TopicRetentionStore};

fn cursor(
    index_prefix: &str,
    bucket: u64,
    created_at: i64,
    object_id: &str,
) -> TopicRetentionCursor {
    TopicRetentionCursor {
        index_prefix: index_prefix.into(),
        bucket,
        created_at,
        object_id: object_id.into(),
    }
}

/// 設定・続きの位置の上書き・古い bucket の削除・解除の後の状態を、読める形で返す。
async fn scenario(store: &dyn TopicRetentionStore) -> Vec<String> {
    let mut observed = Vec::new();
    store.retain_topic("topic-b", 20).await.expect("retain b");
    store.retain_topic("topic-a", 10).await.expect("retain a");
    // 既にある topic を足し直しても、保持を始めた時刻は変えない。
    store
        .retain_topic("topic-a", 99)
        .await
        .expect("retain a again");
    for (topic, cursor) in [
        ("topic-a", cursor("indexes/timeline/", 100, 5, "a5")),
        ("topic-a", cursor("indexes/timeline/", 101, 7, "a7")),
        ("topic-a", cursor("channels/activity/", 101, 8, "pc.a8")),
        ("topic-b", cursor("indexes/timeline/", 101, 9, "b9")),
        // 保持していない topic の位置は置かない。
        ("topic-c", cursor("indexes/timeline/", 101, 1, "c1")),
    ] {
        store
            .put_topic_retention_cursor(topic, &cursor)
            .await
            .expect("put cursor");
    }
    // 同じ索引と bucket の位置は上書きする。
    store
        .put_topic_retention_cursor("topic-a", &cursor("indexes/timeline/", 101, 12, "a12"))
        .await
        .expect("advance cursor");
    observed.push(format!(
        "topics {:?}",
        store
            .retained_topics(10)
            .await
            .expect("topics")
            .iter()
            .map(|row| (row.topic_id.clone(), row.retained_at))
            .collect::<Vec<_>>()
    ));
    observed.push(format!(
        "limited {:?}",
        store
            .retained_topics(1)
            .await
            .expect("limited")
            .iter()
            .map(|row| row.topic_id.clone())
            .collect::<Vec<_>>()
    ));
    for (topic, from) in [("topic-a", 0), ("topic-a", 101), ("topic-c", 0)] {
        observed.push(format!(
            "{topic} from {from} {:?}",
            store
                .topic_retention_cursors(topic, from)
                .await
                .expect("cursors")
        ));
    }
    store
        .prune_topic_retention_cursors("topic-a", 101)
        .await
        .expect("prune");
    observed.push(format!(
        "pruned {:?}",
        store
            .topic_retention_cursors("topic-a", 0)
            .await
            .expect("pruned cursors")
    ));
    store.release_topic("topic-a").await.expect("release");
    observed.push(format!(
        "released {:?} {:?} {:?}",
        store
            .retained_topics(10)
            .await
            .expect("topics after release")
            .iter()
            .map(|row| row.topic_id.clone())
            .collect::<Vec<_>>(),
        store
            .topic_retention_cursors("topic-a", 0)
            .await
            .expect("released cursors"),
        store
            .topic_retention_cursors("topic-b", 0)
            .await
            .expect("other topic cursors")
            .len()
    ));
    observed
}

#[tokio::test]
async fn topic_retention_rows_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = scenario(&sqlite).await;
    assert_eq!(from_sqlite, scenario(&memory).await);
    assert_eq!(
        from_sqlite,
        vec![
            r#"topics [("topic-a", 10), ("topic-b", 20)]"#.to_string(),
            r#"limited ["topic-a"]"#.to_string(),
            format!(
                "topic-a from 0 {:?}",
                vec![
                    cursor("channels/activity/", 101, 8, "pc.a8"),
                    cursor("indexes/timeline/", 100, 5, "a5"),
                    cursor("indexes/timeline/", 101, 12, "a12"),
                ]
            ),
            format!(
                "topic-a from 101 {:?}",
                vec![
                    cursor("channels/activity/", 101, 8, "pc.a8"),
                    cursor("indexes/timeline/", 101, 12, "a12"),
                ]
            ),
            "topic-c from 0 []".to_string(),
            format!(
                "pruned {:?}",
                vec![
                    cursor("channels/activity/", 101, 8, "pc.a8"),
                    cursor("indexes/timeline/", 101, 12, "a12"),
                ]
            ),
            r#"released ["topic-b"] [] 1"#.to_string(),
        ]
    );
}

/// #1909 AC-2: 取り直しの予定。1 回目から 5 回目までの間で予定し、5 回目の後は諦めた数を足す。予定の合計は上限つき。
async fn retry_scenario(store: &dyn TopicRetentionStore) -> Vec<String> {
    let mut observed = Vec::new();
    store.retain_topic("topic-a", 1).await.expect("retain");
    // 保持していない topic は予定しない。
    store
        .schedule_topic_retention_retry("topic-x", "post", "x", 7, 0)
        .await
        .expect("not retained");
    store
        .schedule_topic_retention_retry("topic-a", "post", "p1", 7, 1_000)
        .await
        .expect("first retry");
    store
        .schedule_topic_retention_retry("topic-a", "session", "s1", 7, 2_000)
        .await
        .expect("session retry");
    for now in [1_000, 61_000, 62_000] {
        observed.push(format!(
            "due at {now} {:?}",
            store
                .due_topic_retention_retries("topic-a", now, 10)
                .await
                .expect("due")
        ));
    }
    // 取り直しが続けて失敗すると、間を延ばして 5 回目まで予定し、その後は諦める。
    for now in [61_000, 661_000, 4_261_000, 25_861_000] {
        store
            .schedule_topic_retention_retry("topic-a", "post", "p1", 7, now)
            .await
            .expect("next retry");
        observed.push(format!(
            "after failure at {now} {:?} {:?}",
            store
                .due_topic_retention_retries("topic-a", i64::MAX, 1)
                .await
                .expect("next"),
            store
                .topic_retention_retry_counts("topic-a")
                .await
                .expect("counts")
        ));
    }
    store
        .schedule_topic_retention_retry("topic-a", "post", "p1", 7, 112_261_000)
        .await
        .expect("give up");
    observed.push(format!(
        "given up {:?}",
        store
            .topic_retention_retry_counts("topic-a")
            .await
            .expect("counts")
    ));
    store
        .finish_topic_retention_retry("topic-a", "session", "s1")
        .await
        .expect("finish");
    observed.push(format!(
        "finished {:?}",
        store
            .topic_retention_retry_counts("topic-a")
            .await
            .expect("counts")
    ));
    // 予定は合計 RETENTION_RETRY_LIMIT 件まで。超える分は諦めた数に足す。
    for index in 0..=crate::RETENTION_RETRY_LIMIT {
        store
            .schedule_topic_retention_retry("topic-a", "post", &format!("bulk-{index:05}"), 7, 0)
            .await
            .expect("bulk retry");
    }
    observed.push(format!(
        "bulk {:?}",
        store
            .topic_retention_retry_counts("topic-a")
            .await
            .expect("counts")
    ));
    store.release_topic("topic-a").await.expect("release");
    observed.push(format!(
        "released {:?}",
        store
            .topic_retention_retry_counts("topic-a")
            .await
            .expect("counts")
    ));
    observed
}

#[tokio::test]
async fn topic_retention_retries_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = retry_scenario(&sqlite).await;
    assert_eq!(from_sqlite, retry_scenario(&memory).await);
    let retry = |kind: &str, id: &str, attempts: u32| crate::TopicRetentionRetry {
        object_kind: kind.into(),
        object_id: id.into(),
        bucket: 7,
        attempts,
    };
    assert_eq!(
        from_sqlite,
        vec![
            "due at 1000 []".to_string(),
            format!("due at 61000 {:?}", vec![retry("post", "p1", 1)]),
            format!(
                "due at 62000 {:?}",
                vec![retry("post", "p1", 1), retry("session", "s1", 1)]
            ),
            format!(
                "after failure at 61000 {:?} (2, 0)",
                vec![retry("session", "s1", 1)]
            ),
            format!(
                "after failure at 661000 {:?} (2, 0)",
                vec![retry("session", "s1", 1)]
            ),
            format!(
                "after failure at 4261000 {:?} (2, 0)",
                vec![retry("session", "s1", 1)]
            ),
            format!(
                "after failure at 25861000 {:?} (2, 0)",
                vec![retry("session", "s1", 1)]
            ),
            "given up (1, 1)".to_string(),
            "finished (0, 1)".to_string(),
            format!("bulk ({}, 2)", crate::RETENTION_RETRY_LIMIT),
            "released (0, 0)".to_string(),
        ]
    );
}

/// #1909 AC-3: 日の行は新しい日から古い日へ 1 回ずつ読み、期間を過ぎた日と保持をやめた topic の日は回収で消える。
async fn days_scenario(store: &dyn TopicRetentionStore) -> Vec<String> {
    let mut observed = Vec::new();
    observed.push(format!(
        "default {:?}",
        store.topic_retention_limits().await.expect("limits")
    ));
    store
        .set_topic_retention_limits(3, 1 << 30)
        .await
        .expect("set limits");
    observed.push(format!(
        "set {:?}",
        store.topic_retention_limits().await.expect("limits")
    ));
    // 保持していない topic には日の行を作らない。
    store
        .open_topic_retention_days("topic-x", 10, 12)
        .await
        .expect("open not retained");
    observed.push(format!(
        "not retained {:?} {}",
        store
            .next_topic_retention_backfill("topic-x", 10, 12)
            .await
            .expect("backfill"),
        store
            .protect_topic_retention_day("topic-x", 12, "scope-x-12")
            .await
            .expect("protect")
    ));
    store.retain_topic("topic-a", 1).await.expect("retain");
    store
        .open_topic_retention_days("topic-a", 10, 12)
        .await
        .expect("open");
    store
        .put_topic_retention_cursor("topic-a", &cursor("indexes/timeline/", 11, 5, "a5"))
        .await
        .expect("cursor of a day");
    observed.push(format!("first {:?}", read_days(store, 10, 12).await));
    // 読み終えた日の続きの位置は消える。
    observed.push(format!(
        "cursors after reading {:?}",
        store
            .topic_retention_cursors("topic-a", 0)
            .await
            .expect("cursors")
    ));
    // 次に起動したとき、停止していた間の日を足す。読み終えた日は読み直さない。
    store
        .open_topic_retention_days("topic-a", 11, 14)
        .await
        .expect("open after restart");
    observed.push(format!("restart {:?}", read_days(store, 11, 14).await));
    // 保持している topic を設定し直しても、読み終えた日は読み直さない。
    store
        .retain_topic("topic-a", 2)
        .await
        .expect("retain again");
    observed.push(format!("retain again {:?}", read_days(store, 10, 14).await));
    // 期間を延ばすと、既にある日より古い側の日も作り、新しい日から読む。
    store
        .open_topic_retention_days("topic-a", 7, 14)
        .await
        .expect("open a longer period");
    observed.push(format!("longer {:?}", read_days(store, 7, 14).await));
    // 期間を過ぎた日は回収で消え、保護しない。
    store
        .reclaim_topic_retention_step(13, 128)
        .await
        .expect("reclaim expired");
    observed.push(format!(
        "expired {:?}",
        protected_days(store, 12..=14).await
    ));
    store.release_topic("topic-a").await.expect("release");
    store
        .reclaim_topic_retention_step(13, 128)
        .await
        .expect("reclaim released");
    observed.push(format!(
        "released {:?}",
        protected_days(store, 12..=14).await
    ));
    // 保持し直すと、期間の内の日をまた読む。
    store.retain_topic("topic-a", 3).await.expect("retain anew");
    store
        .open_topic_retention_days("topic-a", 13, 14)
        .await
        .expect("open after retaining anew");
    observed.push(format!(
        "retained anew {:?}",
        read_days(store, 13, 14).await
    ));
    observed
}

/// まだ読んでいない日を、読む順に最後まで読んだことにする。
async fn read_days(store: &dyn TopicRetentionStore, from: u64, to: u64) -> Vec<u64> {
    let mut order = Vec::new();
    while let Some(bucket) = store
        .next_topic_retention_backfill("topic-a", from, to)
        .await
        .expect("backfill")
    {
        order.push(bucket);
        store
            .finish_topic_retention_day("topic-a", bucket)
            .await
            .expect("finish");
    }
    order
}

async fn protected_days(
    store: &dyn TopicRetentionStore,
    buckets: std::ops::RangeInclusive<u64>,
) -> Vec<u64> {
    let mut protected = Vec::new();
    for bucket in buckets {
        if store
            .protect_topic_retention_day("topic-a", bucket, &format!("scope-a-{bucket}"))
            .await
            .expect("protect")
        {
            protected.push(bucket);
        }
    }
    protected
}

#[tokio::test]
async fn topic_retention_days_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = days_scenario(&sqlite).await;
    assert_eq!(from_sqlite, days_scenario(&memory).await);
    let limits = |retention_days, capacity_bytes| crate::TopicRetentionLimits {
        retention_days,
        capacity_bytes,
        used_bytes: 0,
    };
    assert_eq!(
        from_sqlite,
        vec![
            format!("default {:?}", limits(30, 10 << 30)),
            format!("set {:?}", limits(3, 1 << 30)),
            "not retained None false".to_string(),
            "first [12, 11, 10]".to_string(),
            "cursors after reading []".to_string(),
            "restart [14, 13]".to_string(),
            "retain again []".to_string(),
            "longer [9, 8, 7]".to_string(),
            "expired [13, 14]".to_string(),
            "released []".to_string(),
            "retained anew [14, 13]".to_string(),
        ]
    );
}

/// record の cache の行の大きさ(`put_remote_content_in_tx` と同じ数え方)。
fn record_charge(replica: &str, key: &str, len: usize) -> u64 {
    let cache_key = SqliteStore::remote_record_cache_key(replica, key, "author");
    (len + "record".len() + cache_key.len() + replica.len() + 64) as u64
}

async fn protected(store: &SqliteStore, kind: &str, key: &str) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT is_protected FROM remote_content_cache WHERE kind = ?1 AND cache_key = ?2",
    )
    .bind(kind)
    .bind(key)
    .fetch_one(store.pool())
    .await
    .expect("cache row")
        == 1
}

/// #1909 AC-3: 保持する日の record と blob に保護参照を付けて大きさを足し、期間を過ぎた日・容量を超えた分の古い日・保持を
/// やめた topic の日から、1 回 `limit` 件まで外して引く。外した内容は通常の cache に戻る。
#[tokio::test]
async fn retained_days_protect_cache_content_and_release_the_oldest_first() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let replica = |bucket: u64| format!("bucket::v1::topic::61::{bucket}");
    let record_key = |bucket: u64, key: &str| {
        SqliteStore::remote_record_cache_key(&replica(bucket), key, "author")
    };
    store.retain_topic("topic-a", 1).await.expect("retain");
    store
        .open_topic_retention_days("topic-a", 10, 12)
        .await
        .expect("open");
    // 登録より前に置かれた record は、登録の後の段で `limit` 件ずつ保護する。
    for key in ["k1", "k2", "k3"] {
        store
            .put_remote_record(&replica(10), key, "author", &[1; 100])
            .await
            .expect("put early record");
    }
    store
        .put_remote_record(&replica(9), "other", "author", &[1; 100])
        .await
        .expect("put record of another day");
    assert!(
        store
            .protect_topic_retention_day("topic-a", 10, &replica(10))
            .await
            .expect("protect day 10")
    );
    let mut steps = Vec::new();
    for _ in 0..3 {
        steps.push(
            store
                .protect_topic_retention_scopes_step(2)
                .await
                .expect("scope step"),
        );
    }
    assert_eq!(steps, [2, 1, 0]);
    assert!(!protected(&store, "record", &record_key(9, "other")).await);
    // 登録の後に置かれた record は、書込みと同じ transaction で保護する。書き換えは差分を足す。
    assert!(
        store
            .protect_topic_retention_day("topic-a", 11, &replica(11))
            .await
            .expect("protect day 11")
    );
    store
        .put_remote_record(&replica(11), "k4", "author", &[2; 200])
        .await
        .expect("put record");
    store
        .put_remote_record(&replica(11), "k4", "author", &[2; 250])
        .await
        .expect("rewrite record");
    // blob は内容より先に保護参照を付け、届いた内容の大きさを足す。
    let hash = "c".repeat(64);
    store
        .claim_topic_retention_blob("topic-a", 12, &hash)
        .await
        .expect("claim blob");
    assert!(
        store
            .put_remote_content("blob", &hash, "blob", &[3; 300])
            .await
            .expect("put blob")
    );
    let day10 = 3 * record_charge(&replica(10), "k1", 100);
    let day11 = record_charge(&replica(11), "k4", 250);
    let day12 = (300 + "blob".len() + hash.len() + "blob".len() + 64) as u64;
    assert_eq!(used(&store).await, day10 + day11 + day12);
    assert_eq!(
        store.topic_retention_used("topic-a").await.expect("used"),
        day10 + day11 + day12
    );
    for (kind, key) in [
        ("record", record_key(10, "k1")),
        ("record", record_key(11, "k4")),
        ("blob", hash.clone()),
    ] {
        assert!(protected(&store, kind, &key).await, "{kind} {key:?}");
    }

    // 容量を超えたら、最も古い日から `limit` 件ずつ外し、合計が容量の内に戻ったら止める。
    store
        .set_topic_retention_limits(30, day11 + day12)
        .await
        .expect("shrink capacity");
    let mut steps = Vec::new();
    for _ in 0..3 {
        steps.push(
            store
                .reclaim_topic_retention_step(10, 2)
                .await
                .expect("reclaim over capacity"),
        );
    }
    assert_eq!(steps, [2, 1, 0]);
    assert_eq!(used(&store).await, day11 + day12);
    assert!(!protected(&store, "record", &record_key(10, "k1")).await);
    assert!(
        store
            .get_remote_content("record", &record_key(10, "k1"))
            .await
            .expect("read released record")
            .is_some(),
        "外した record は通常の cache に残る"
    );
    // 容量を超えて外した日と、それより前の日は、保護せず、作り直さず、読まない。
    assert!(
        !store
            .protect_topic_retention_day("topic-a", 10, &replica(10))
            .await
            .expect("released day")
    );
    store
        .open_topic_retention_days("topic-a", 8, 12)
        .await
        .expect("reopen");
    for bucket in [11, 12] {
        store
            .finish_topic_retention_day("topic-a", bucket)
            .await
            .expect("finish");
    }
    assert_eq!(
        store
            .next_topic_retention_backfill("topic-a", 8, 12)
            .await
            .expect("backfill"),
        None
    );

    // 期間を過ぎた日を外す。
    assert_eq!(
        store
            .reclaim_topic_retention_step(12, 128)
            .await
            .expect("reclaim expired"),
        1
    );
    assert_eq!(used(&store).await, day12);
    assert!(!protected(&store, "record", &record_key(11, "k4")).await);
    // 保持をやめた topic の日を外す。
    store.release_topic("topic-a").await.expect("release");
    assert_eq!(
        store
            .reclaim_topic_retention_step(12, 128)
            .await
            .expect("reclaim released topic"),
        1
    );
    assert_eq!(used(&store).await, 0);
    assert!(!protected(&store, "blob", &hash).await);
    assert_eq!(
        store
            .reclaim_topic_retention_step(12, 128)
            .await
            .expect("nothing left"),
        0
    );
}

async fn used(store: &SqliteStore) -> u64 {
    store
        .topic_retention_limits()
        .await
        .expect("limits")
        .used_bytes
}

/// #1909 AC-3(2026-10-22 ユーザー判断): 容量を超えて外した日は、前日より前の日としては作り直さず読まない。容量の設定を
/// 変えたときと、topic の保持をやめたときは、その日もまた読む日に戻す。
#[tokio::test]
async fn capacity_released_days_are_read_again_once_space_is_freed() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let hash = "d".repeat(64);
    store.retain_topic("topic-a", 1).await.expect("retain");
    // 10 日目の blob で容量を超え、10 日目を外す。
    let release_day_10 = || async {
        store
            .open_topic_retention_days("topic-a", 10, 12)
            .await
            .expect("open");
        store
            .claim_topic_retention_blob("topic-a", 10, &hash)
            .await
            .expect("claim");
        assert!(
            store
                .put_remote_content("blob", &hash, "blob", &[1; 100])
                .await
                .expect("put blob")
        );
        store
            .set_topic_retention_limits(30, used(&store).await - 1)
            .await
            .expect("shrink capacity");
        for _ in 0..8 {
            store
                .reclaim_topic_retention_step(10, 128)
                .await
                .expect("reclaim");
        }
        assert_eq!(used(&store).await, 0);
    };
    let day_10_read = || async {
        store
            .open_topic_retention_days("topic-a", 10, 12)
            .await
            .expect("reopen");
        store
            .protect_topic_retention_day("topic-a", 10, "scope-a-10")
            .await
            .expect("protect")
    };
    release_day_10().await;
    assert!(
        !day_10_read().await,
        "a day released for capacity is not read"
    );
    // 容量の設定を変えると、また読む。
    store
        .set_topic_retention_limits(30, 1 << 30)
        .await
        .expect("raise capacity");
    assert!(
        day_10_read().await,
        "the day is read again after the change"
    );
    // ほかの topic の保持をやめても、また読む。
    release_day_10().await;
    assert!(!day_10_read().await);
    store.retain_topic("topic-b", 1).await.expect("retain b");
    store.release_topic("topic-b").await.expect("release b");
    assert!(day_10_read().await, "the day is read again after a release");
}
