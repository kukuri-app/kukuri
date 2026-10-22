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
