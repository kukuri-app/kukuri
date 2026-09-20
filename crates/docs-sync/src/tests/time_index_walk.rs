//! #1239: 時系列の索引の読み出しの、向き(古い順・新しい順)、query 数の上限からの続き、索引の端での停止。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Result;
use kukuri_core::ReplicaId;
use kukuri_iroh_node::IrohDocsNode;

use super::time_index::{
    CountingKeys, INDEX_PREFIX, fixture, ids, index_key, object_id, put_key, seed,
};
use crate::{
    DocKeyOrder, DocsSync, IrohDocsSync, MemoryDocsSync, TimeIndexCursor, TimeIndexPage,
    query_time_index_asc, query_time_index_desc, stable_key, topic_replica_id,
};

/// 基準実装。`order` の向きに並べ、`cursor` より先の entry を `limit` 件返す。
fn expected(
    rows: &[(i64, String)],
    order: DocKeyOrder,
    cursor: Option<&TimeIndexCursor>,
    limit: usize,
) -> Vec<(i64, String)> {
    let mut sorted = rows.to_vec();
    sorted.sort();
    if order == DocKeyOrder::Descending {
        sorted.reverse();
    }
    sorted
        .into_iter()
        .filter(|(created_at, object_id)| {
            let Some(cursor) = cursor else {
                return true;
            };
            let entry = (*created_at, object_id.as_str());
            let position = (cursor.created_at, cursor.object_id.as_str());
            match order {
                DocKeyOrder::Descending => entry < position,
                DocKeyOrder::Ascending => entry > position,
            }
        })
        .take(limit)
        .collect()
}

async fn query(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    order: DocKeyOrder,
    cursor: Option<&TimeIndexCursor>,
    limit: usize,
) -> Result<TimeIndexPage> {
    match order {
        DocKeyOrder::Descending => {
            query_time_index_desc(docs, replica, INDEX_PREFIX, cursor, limit).await
        }
        DocKeyOrder::Ascending => {
            query_time_index_asc(docs, replica, INDEX_PREFIX, cursor, limit).await
        }
    }
}

fn cursor_at(entry: &(i64, String)) -> TimeIndexCursor {
    TimeIndexCursor {
        created_at: entry.0,
        object_id: entry.1.clone(),
    }
}

async fn assert_ascending_matches_reference(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
) -> Result<()> {
    let rows = fixture();
    seed(docs, replica, &rows).await?;
    let order = DocKeyOrder::Ascending;

    for limit in [1usize, 3, 10, 100] {
        let oldest = query(docs, replica, order, None, limit).await?;
        assert_eq!(
            ids(&oldest),
            expected(&rows, order, None, limit),
            "oldest side, limit={limit}"
        );
        assert_eq!(oldest.resume, None);
    }

    // 全 entry と、entry の無い時刻を起点にして、基準実装と突き合わせる。
    let mut cursors = rows.iter().map(cursor_at).collect::<Vec<_>>();
    for created_at in [
        -5_i64,
        0,
        1,
        10,
        1_000_000_000,
        1_757_999_995,
        2_000_000_000,
    ] {
        for object_id in ["f".repeat(64), "0".repeat(64), String::new()] {
            cursors.push(TimeIndexCursor {
                created_at,
                object_id,
            });
        }
    }
    for cursor in &cursors {
        for limit in [1usize, 5, 50] {
            let newer = query(docs, replica, order, Some(cursor), limit).await?;
            assert_eq!(
                ids(&newer),
                expected(&rows, order, Some(cursor), limit),
                "cursor={cursor:?}, limit={limit}"
            );
            assert_eq!(newer.resume, None);
        }
    }

    // ページを継いで最後まで読むと、全 entry を重複なく古い順に読める。
    let mut walked = Vec::new();
    let mut cursor: Option<TimeIndexCursor> = None;
    loop {
        let page = query(docs, replica, order, cursor.as_ref(), 7).await?;
        let Some(last) = page.entries.last() else {
            break;
        };
        cursor = Some(TimeIndexCursor {
            created_at: last.created_at,
            object_id: last.object_id.clone(),
        });
        walked.extend(ids(&page));
    }
    assert_eq!(walked, expected(&rows, order, None, usize::MAX));
    Ok(())
}

#[tokio::test]
async fn ascending_walk_matches_reference_on_memory_docs() -> Result<()> {
    let docs = MemoryDocsSync::default();
    assert_ascending_matches_reference(
        &docs,
        &topic_replica_id("kukuri:topic:time-index-asc-memory"),
    )
    .await
}

#[tokio::test]
async fn ascending_walk_matches_reference_on_iroh_docs() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let result = assert_ascending_matches_reference(
        &docs,
        &topic_replica_id("kukuri:topic:time-index-asc-iroh"),
    )
    .await;
    docs.shutdown().await;
    node.shutdown().await?;
    result
}

// 形の違う key で埋まった秒が続くと、1 回の読み出しは query 数の上限で止まる。そのとき、続きの起点を返す。
// 起点から読み継ぐと、形の違う key の先にある有効な entry へ届き、有効な entry を飛ばさない。
// 「件数が足りない」だけを見て「索引は尽きた」と判定すると、先の entry へ永久に届かない(独立監査の指摘)。
#[tokio::test]
async fn walk_resumes_from_the_query_cap_without_skipping_valid_entries() -> Result<()> {
    for order in [DocKeyOrder::Descending, DocKeyOrder::Ascending] {
        let docs = Arc::new(CountingKeys::default());
        let replica =
            topic_replica_id(format!("kukuri:topic:time-index-resume-{order:?}").as_str());
        docs.open_replica(&replica).await?;
        let top = 1_758_000_000_i64;
        // 読む向きの符号。新しい順は古い側(-)へ、古い順は新しい側(+)へ進む。
        let step = match order {
            DocKeyOrder::Descending => -1_i64,
            DocKeyOrder::Ascending => 1_i64,
        };
        let mut rows = vec![(top + step, object_id(1)), (top + 2 * step, object_id(2))];
        // 先の 40 個の「10 秒の範囲」それぞれの 1 秒に、形の違う key を余裕を超えて置く。
        for window in 1..=40_i64 {
            let second = top + step * (window * 10 + 3);
            for index in 0..40usize {
                put_key(
                    docs.as_ref(),
                    &replica,
                    stable_key(
                        "indexes/timeline",
                        &format!("{second:020}-~junk-{index:03}"),
                    ),
                )
                .await?;
            }
        }
        rows.push((top + step * 1_000, object_id(3)));
        rows.push((top + step * 100_000, object_id(4)));
        for (created_at, id) in &rows {
            put_key(docs.as_ref(), &replica, index_key(*created_at, id)).await?;
        }

        let start = TimeIndexCursor {
            created_at: top,
            object_id: match order {
                DocKeyOrder::Descending => "f".repeat(64),
                DocKeyOrder::Ascending => "0".repeat(64),
            },
        };
        let mut walked = Vec::new();
        let mut cursor = start.clone();
        let mut calls = 0usize;
        let mut resumed = 0usize;
        loop {
            calls += 1;
            assert!(calls <= 16, "{order:?}: the walk must make progress");
            docs.queries.store(0, Ordering::SeqCst);
            let page = query(docs.as_ref(), &replica, order, Some(&cursor), 10).await?;
            let queries = docs.queries.load(Ordering::SeqCst);
            // 同じ秒の読み出し 1 回と、上限の 256 回。
            assert!(queries <= 257, "{order:?}: {queries} queries in one call");
            assert_eq!(
                page.queries, queries,
                "{order:?}: the page reports its cost"
            );
            walked.extend(ids(&page));
            match page.resume {
                Some(resume) => {
                    resumed += 1;
                    cursor = resume;
                }
                None => break,
            }
        }
        assert!(
            resumed >= 1,
            "{order:?}: the first call must stop at the cap"
        );
        assert_eq!(
            walked,
            expected(&rows, order, Some(&start), usize::MAX),
            "{order:?}: every valid entry is read, in order, exactly once"
        );
    }
    Ok(())
}

// entry が尽きた先を読むときは、索引の端を 1 回調べて、端より先の prefix を読まない。調べないと、古い順の
// 読み出しは、時刻の上の桁(0 が並ぶ)の prefix を 100 回以上空振りで読む。索引が空のときも同じ。
#[tokio::test]
async fn exhausted_walk_stops_at_the_index_edge() -> Result<()> {
    let docs = Arc::new(CountingKeys::default());
    let replica = topic_replica_id("kukuri:topic:time-index-edge");
    docs.open_replica(&replica).await?;
    let mut rows = Vec::new();
    for index in 0..100usize {
        let row = (1_700_000_000 + index as i64, object_id(index));
        put_key(docs.as_ref(), &replica, index_key(row.0, &row.1)).await?;
        rows.push(row);
    }
    for (order, end) in [
        (DocKeyOrder::Descending, &rows[0]),
        (DocKeyOrder::Ascending, &rows[99]),
    ] {
        docs.queries.store(0, Ordering::SeqCst);
        let page = query(docs.as_ref(), &replica, order, Some(&cursor_at(end)), 20).await?;
        assert!(page.entries.is_empty(), "{order:?}");
        assert_eq!(page.resume, None, "{order:?}");
        let queries = docs.queries.load(Ordering::SeqCst);
        // 同じ秒、最初の空の prefix、索引の端。
        assert!(queries <= 3, "{order:?}: {queries} queries past the edge");
        assert_eq!(
            page.queries, queries,
            "{order:?}: the page reports its cost"
        );
    }

    // 端の手前から読むと、端までの entry を返して止まる。
    for (order, from, want) in [
        (
            DocKeyOrder::Descending,
            &rows[3],
            vec![&rows[2], &rows[1], &rows[0]],
        ),
        (
            DocKeyOrder::Ascending,
            &rows[96],
            vec![&rows[97], &rows[98], &rows[99]],
        ),
    ] {
        docs.queries.store(0, Ordering::SeqCst);
        let page = query(docs.as_ref(), &replica, order, Some(&cursor_at(from)), 20).await?;
        assert_eq!(
            ids(&page),
            want.into_iter().cloned().collect::<Vec<_>>(),
            "{order:?}"
        );
        assert_eq!(page.resume, None);
        let queries = docs.queries.load(Ordering::SeqCst);
        assert!(queries <= 12, "{order:?}: {queries} queries near the edge");
    }

    // 索引が空の replica では、起点つきの読み出しは 3 回の query で終わる。
    let empty = topic_replica_id("kukuri:topic:time-index-edge-empty");
    docs.open_replica(&empty).await?;
    for order in [DocKeyOrder::Descending, DocKeyOrder::Ascending] {
        docs.queries.store(0, Ordering::SeqCst);
        let page = query(
            docs.as_ref(),
            &empty,
            order,
            Some(&cursor_at(&rows[50])),
            20,
        )
        .await?;
        assert!(page.entries.is_empty(), "{order:?}");
        assert_eq!(page.resume, None, "{order:?}");
        let queries = docs.queries.load(Ordering::SeqCst);
        assert!(
            queries <= 3,
            "{order:?}: {queries} queries on an empty index"
        );
        assert_eq!(
            page.queries, queries,
            "{order:?}: the page reports its cost"
        );
    }
    Ok(())
}

// 索引の端が形の違う key に埋まっていて分からないときは、端を使わずに読み続ける。有効な entry は落とさない。
#[tokio::test]
async fn edge_hidden_by_malformed_keys_does_not_drop_valid_entries() -> Result<()> {
    let docs = Arc::new(CountingKeys::default());
    let replica = topic_replica_id("kukuri:topic:time-index-edge-hidden");
    docs.open_replica(&replica).await?;
    let rows = vec![
        (1_700_000_000_i64, object_id(1)),
        (1_700_000_005, object_id(2)),
        (1_700_001_000, object_id(3)),
    ];
    for (created_at, id) in &rows {
        put_key(docs.as_ref(), &replica, index_key(*created_at, id)).await?;
    }
    // `~` は数字より後ろに、`!` は数字より前に並ぶ。索引の両端を、余裕を超える数の形の違う key で埋める。
    for index in 0..40usize {
        for lead in ['~', '!'] {
            put_key(
                docs.as_ref(),
                &replica,
                stable_key("indexes/timeline", &format!("{lead}junk-{index:03}")),
            )
            .await?;
        }
    }
    for (order, from) in [
        (DocKeyOrder::Ascending, &rows[0]),
        (DocKeyOrder::Descending, &rows[2]),
    ] {
        docs.queries.store(0, Ordering::SeqCst);
        let start = cursor_at(from);
        let page = query(docs.as_ref(), &replica, order, Some(&start), 10).await?;
        assert_eq!(
            ids(&page),
            expected(&rows, order, Some(&start), 10),
            "{order:?}"
        );
        assert_eq!(page.resume, None, "{order:?}");
        let queries = docs.queries.load(Ordering::SeqCst);
        assert!(queries <= 257, "{order:?}: {queries} queries");
    }
    // 起点の無い読み出しも、端の形の違う key を避けて有効な entry を返す。
    for order in [DocKeyOrder::Ascending, DocKeyOrder::Descending] {
        let page = query(docs.as_ref(), &replica, order, None, 10).await?;
        assert_eq!(ids(&page), expected(&rows, order, None, 10), "{order:?}");
    }
    Ok(())
}

// 索引の時刻は 20 桁の数字だけ。符号つきの表記は整数として読めても、索引の entry としては扱わない
// (同じ時刻を別の key で表せると、桁の prefix でたどる読み出しと並びが合わなくなる)。
#[tokio::test]
async fn signed_time_keys_are_not_index_entries() -> Result<()> {
    let docs = MemoryDocsSync::default();
    let replica = topic_replica_id("kukuri:topic:time-index-signed");
    docs.open_replica(&replica).await?;
    let valid = (1_000_i64, object_id(1));
    put_key(&docs, &replica, index_key(valid.0, &valid.1)).await?;
    for time in ["+0000000000000002000", "-0000000000000002000"] {
        let id = object_id(2);
        put_key(
            &docs,
            &replica,
            stable_key("indexes/timeline", &format!("{time}-{id}/{id}")),
        )
        .await?;
    }
    // sort key の object id と、末尾の object id が合わない key も entry ではない。
    put_key(
        &docs,
        &replica,
        stable_key(
            "indexes/timeline",
            &format!("{:020}-{}/{}", 3_000, object_id(3), object_id(4)),
        ),
    )
    .await?;
    for order in [DocKeyOrder::Ascending, DocKeyOrder::Descending] {
        let page = query(&docs, &replica, order, None, 10).await?;
        assert_eq!(ids(&page), vec![valid.clone()], "{order:?}");
    }
    Ok(())
}
