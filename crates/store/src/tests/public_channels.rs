//! #1840: 公開 channel の一覧の行の保持と、ページの SQL。

use super::page_query_plans::{EXPLAIN, assert_range_read, plan};
use super::*;
use crate::sqlite::public_channels::public_channel_page_query;
use crate::{PublicChannelCursor, PublicChannelOrder, PublicChannelRow, PublicChannelStore};

fn row(channel_id: &str, created_at: i64) -> PublicChannelRow {
    PublicChannelRow {
        channel_id: channel_id.into(),
        topic_id: "topic".into(),
        title: channel_id.into(),
        creator_pubkey: "a".repeat(64),
        created_at,
        first_post_id: None,
        last_post_at: created_at,
        revision: 0,
        deleted: false,
    }
}

async fn listed(store: &SqliteStore) -> Vec<String> {
    store
        .list_public_channels("topic", PublicChannelOrder::Created, None, 20)
        .await
        .expect("list")
        .items
        .into_iter()
        .map(|row| row.channel_id)
        .collect()
}

// 他人が作った行は remote cache に数え、7 日使わなければ一覧に出さずに回収する。自分が作った行は数えず、残る。
// 一覧で返した行は、使った時刻を更新する。
#[tokio::test]
async fn remote_public_channel_rows_expire_and_are_reclaimed_but_own_rows_stay() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    store
        .put_public_channel(row("pc-own", 1), false)
        .await
        .expect("own row");
    store
        .put_public_channel(row("pc-used", 2), true)
        .await
        .expect("used row");
    store
        .put_public_channel(row("pc-unused", 3), true)
        .await
        .expect("unused row");
    let charged = sqlx::query_scalar::<_, String>(
        "SELECT cache_key FROM remote_content_cache WHERE kind = 'public_channel' ORDER BY cache_key",
    )
    .fetch_all(store.pool())
    .await
    .expect("charged rows");
    assert_eq!(
        charged,
        vec!["pc-unused".to_string(), "pc-used".to_string()]
    );

    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("now")
            .as_millis(),
    )
    .expect("now");
    for (key, last_used_at) in [
        ("pc-unused", 0),
        ("pc-used", now - REMOTE_CACHE_TOUCH_INTERVAL_MS - 1),
    ] {
        sqlx::query(
            "UPDATE remote_content_cache SET last_used_at = ?1 \
             WHERE kind = 'public_channel' AND cache_key = ?2",
        )
        .bind(last_used_at)
        .bind(key)
        .execute(store.pool())
        .await
        .expect("age the row");
    }
    assert_eq!(listed(&store).await, vec!["pc-used", "pc-own"]);
    let touched = sqlx::query_scalar::<_, i64>(
        "SELECT last_used_at FROM remote_content_cache \
         WHERE kind = 'public_channel' AND cache_key = 'pc-used'",
    )
    .fetch_one(store.pool())
    .await
    .expect("touched");
    assert!(touched >= now, "a listed row is used: {touched} < {now}");

    assert_eq!(store.reclaim_remote_cache_step().await.expect("reclaim"), 1);
    assert!(
        store
            .get_public_channel_row("pc-unused")
            .await
            .expect("reclaimed")
            .is_none()
    );
    assert!(
        store
            .get_public_channel_row("pc-own")
            .await
            .expect("own")
            .is_some()
    );
    assert_eq!(listed(&store).await, vec!["pc-used", "pc-own"]);
}

// 一覧のページは、どちらの並び順でも (topic, 時刻, ID) の索引の範囲を読み、条件に合う行を並べ替えない。
#[tokio::test]
async fn public_channel_pages_are_index_range_reads() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let cursor = PublicChannelCursor {
        at: 100,
        channel_id: "pc-x".into(),
    };
    for (order, index) in [
        (
            PublicChannelOrder::LastPost,
            "idx_public_channels_last_post",
        ),
        (PublicChannelOrder::Created, "idx_public_channels_created"),
    ] {
        for cursor in [None, Some(&cursor)] {
            let name = format!("{order:?}, cursor={}", cursor.is_some());
            let plan = plan(
                &store,
                public_channel_page_query(EXPLAIN, "topic", order, cursor, 20),
            )
            .await;
            assert_range_read(name.as_str(), plan.as_str());
            assert!(plan.contains(index), "{name}: {plan}");
        }
    }
}
