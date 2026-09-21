//! #1239(inventory の Q-1): タイムラインと thread のページの取得が、索引の範囲の読み出しになっていること。

use super::*;
use sqlx::Row;
use std::collections::BTreeSet;

fn row(topic: &str, channel_id: &str, object_id: &str, created_at: i64) -> ObjectProjectionRow {
    let hash = BlobHash::new(format!("{created_at:064x}"));
    ObjectProjectionRow {
        object_id: EnvelopeId::from(object_id),
        topic_id: topic.to_string(),
        channel_id: channel_id.to_string(),
        author_pubkey: "a".repeat(64),
        created_at,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::BlobText {
            hash: hash.clone(),
            mime: "text/plain".into(),
            bytes: 1,
        },
        content: Some(object_id.to_string()),
        attachments: Vec::new(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new(format!("topic::{topic}")),
        source_key: format!("objects/{object_id}/envelope"),
        source_envelope_id: EnvelopeId::from(object_id),
        source_blob_hash: Some(hash),
        source_docs_author: None,
        derived_at: created_at,
        projection_version: 3,
    }
}

fn reply(
    topic: &str,
    channel_id: &str,
    object_id: &str,
    created_at: i64,
    root: &EnvelopeId,
) -> ObjectProjectionRow {
    let mut reply = row(topic, channel_id, object_id, created_at);
    reply.root_object_id = Some(root.clone());
    reply.reply_to_object_id = Some(root.clone());
    reply.object_kind = "comment".into();
    reply
}

async fn plan(store: &SqliteStore, sql: &str) -> String {
    sqlx::query(format!("EXPLAIN QUERY PLAN {sql}").as_str())
        .fetch_all(store.pool())
        .await
        .expect("query plan")
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn assert_range_read(name: &str, plan: &str) {
    assert!(
        !plan.contains("TEMP B-TREE"),
        "{name}: the page must be read in index order, without sorting the matching rows: {plan}"
    );
    assert!(
        plan.contains("USING INDEX") || plan.contains("USING COVERING INDEX"),
        "{name}: {plan}"
    );
}

// ページの取得の SQL の形(`projections.rs` と同じ形)が、索引の範囲の読み出しになること。並べ替えのための
// 一時的な木を作る形は、条件に合う行の総数に比例する。
#[tokio::test]
async fn page_queries_are_index_range_reads() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    for (name, sql) in [
        (
            "timeline, all channels, with a cursor",
            "SELECT object_id FROM object_index_cache WHERE topic_id = 't' \
             AND (created_at, object_id) < (100, 'x') ORDER BY created_at DESC, object_id DESC LIMIT 20",
        ),
        (
            "timeline, one channel, with a cursor",
            "SELECT object_id FROM object_index_cache WHERE topic_id = 't' AND channel_id = 'public' \
             AND (created_at, object_id) < (100, 'x') ORDER BY created_at DESC, object_id DESC LIMIT 20",
        ),
        (
            "timeline, several channels, with a cursor",
            "SELECT object_id FROM object_index_cache INDEXED BY idx_object_index_cache_topic_created_all \
             WHERE topic_id = 't' AND channel_id IN ('public', 'private:a') \
             AND (created_at, object_id) < (100, 'x') ORDER BY created_at DESC, object_id DESC LIMIT 20",
        ),
        (
            "thread replies, with a cursor",
            "SELECT oic.object_id FROM object_thread_cache tc \
             INNER JOIN object_index_cache oic ON oic.object_id = tc.object_id \
             WHERE tc.topic_id = 't' AND tc.root_object_id = 'r' AND tc.object_id != 'r' \
             AND (tc.created_at, tc.object_id) > (100, 'x') ORDER BY tc.created_at ASC, tc.object_id ASC LIMIT 20",
        ),
        (
            "thread replies of one channel, with a cursor",
            "SELECT oic.object_id FROM object_thread_cache tc \
             INNER JOIN object_index_cache oic ON oic.object_id = tc.object_id \
             WHERE tc.topic_id = 't' AND tc.root_object_id = 'r' AND tc.channel_id = 'public' AND tc.object_id != 'r' \
             AND (tc.created_at, tc.object_id) > (100, 'x') ORDER BY tc.created_at ASC, tc.object_id ASC LIMIT 20",
        ),
    ] {
        assert_range_read(name, plan(&store, sql).await.as_str());
    }
}

// thread のページは、root が先頭、返信は古い順。root の時刻が返信より後でも(時計のずれ)、ページを継いで
// 全行を 1 回ずつ読める。channel で絞っても同じ。
#[tokio::test]
async fn thread_pages_list_the_root_first_and_every_reply_once() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic = "kukuri:topic:thread-pages";
    let root_id = EnvelopeId::from("thread-root");
    // root の時刻(50)は、最初の返信(10〜)より後。
    ObjectProjectionStore::put_object_projection(
        &store,
        row(topic, "public", root_id.as_str(), 50),
    )
    .await
    .expect("root");
    let mut expected = vec![root_id.as_str().to_string()];
    for index in 0..23_i64 {
        let id = format!("reply-{index:02}");
        ObjectProjectionStore::put_object_projection(
            &store,
            reply(topic, "public", id.as_str(), 10 + index * 5, &root_id),
        )
        .await
        .expect("reply");
        expected.push(id);
    }
    // 別の channel の返信は、channel で絞った取得には入らない。
    ObjectProjectionStore::put_object_projection(
        &store,
        reply(topic, "private:a", "reply-private", 12, &root_id),
    )
    .await
    .expect("private reply");

    for limit in [1usize, 4, 50] {
        let mut listed = Vec::new();
        let mut cursor = None;
        loop {
            let page = ObjectProjectionStore::list_thread_filtered(
                &store,
                topic,
                &root_id,
                Some("public"),
                cursor,
                limit,
            )
            .await
            .expect("thread page");
            assert!(page.items.len() <= limit);
            listed.extend(
                page.items
                    .iter()
                    .map(|row| row.object_id.as_str().to_string()),
            );
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(listed, expected, "limit={limit}");
    }

    let all = ObjectProjectionStore::list_thread(&store, topic, &root_id, None, 100)
        .await
        .expect("unfiltered thread");
    assert_eq!(all.items.len(), 25);
    assert_eq!(all.items[0].object_id, root_id);
}

// タイムラインのページを、channel の絞り方ごとに継いで読む。全行を新しい順に 1 回ずつ読める。
#[tokio::test]
async fn timeline_pages_list_every_row_once_for_each_channel_filter() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic = "kukuri:topic:timeline-pages";
    let channels = ["public", "private:a", "private:b"];
    let mut rows = Vec::new();
    for index in 0..60_i64 {
        let channel = channels[(index % 3) as usize];
        let id = format!("post-{index:02}");
        // 同じ時刻の行も混ぜる。
        let created_at = 1_000 + index / 2;
        ObjectProjectionStore::put_object_projection(
            &store,
            row(topic, channel, id.as_str(), created_at),
        )
        .await
        .expect("row");
        rows.push((created_at, id, channel));
    }
    for allowed in [
        vec!["public"],
        vec!["public", "private:a"],
        vec!["public", "private:a", "private:b"],
    ] {
        let allowed_set = allowed
            .iter()
            .map(|value| value.to_string())
            .collect::<BTreeSet<_>>();
        let mut expected = rows
            .iter()
            .filter(|(_, _, channel)| allowed.contains(channel))
            .map(|(created_at, id, _)| (*created_at, id.clone()))
            .collect::<Vec<_>>();
        expected.sort();
        expected.reverse();
        let mut listed = Vec::new();
        let mut cursor = None;
        loop {
            let page = ObjectProjectionStore::list_topic_timeline_filtered(
                &store,
                topic,
                &allowed_set,
                cursor,
                7,
            )
            .await
            .expect("page");
            listed.extend(
                page.items
                    .iter()
                    .map(|row| (row.created_at, row.object_id.as_str().to_string())),
            );
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(listed, expected, "allowed={allowed:?}");
    }
}
