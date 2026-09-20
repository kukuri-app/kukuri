use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::ReplicaId;
use kukuri_iroh_node::IrohDocsNode;

use crate::{
    DocEventStream, DocFetchPolicy, DocKeyOrder, DocKeyPage, DocKeyQuery, DocOp, DocQuery,
    DocRecord, DocsSync, IrohDocsSync, MemoryDocsSync, TimeIndexCursor, query_time_index_desc,
    query_time_index_window, stable_key, topic_replica_id,
};

const INDEX_PREFIX: &str = "indexes/timeline/";

fn index_key(created_at: i64, object_id: &str) -> String {
    stable_key(
        "indexes/timeline",
        &format!("{created_at:020}-{object_id}/{object_id}"),
    )
}

fn object_id(seed: usize) -> String {
    format!("{seed:064x}")
}

/// (created_at, object id)。秒の重なり、桁の繰り下がり、離れた時刻を含む。
fn fixture() -> Vec<(i64, String)> {
    let mut rows = Vec::new();
    let mut seed = 0usize;
    for created_at in [
        1_758_000_000_i64,
        1_758_000_000,
        1_758_000_000,
        1_757_999_999,
        1_757_999_990,
        1_757_999_000,
        1_757_990_000,
        1_757_000_001,
        1_700_000_000,
        1_699_999_999,
        999_999_999,
        1_000,
        9,
        0,
    ] {
        seed += 1;
        rows.push((created_at, object_id(seed)));
    }
    // 連続した時刻のかたまり(limit ちょうど・ページの継ぎ目を踏む)。
    for offset in 0..40_i64 {
        seed += 1;
        rows.push((1_757_500_000 + offset * 7, object_id(seed)));
    }
    rows
}

async fn seed(docs: &dyn DocsSync, replica: &ReplicaId, rows: &[(i64, String)]) -> Result<()> {
    docs.open_replica(replica).await?;
    for (created_at, object_id) in rows {
        docs.apply_doc_op(
            replica,
            DocOp::SetJson {
                key: index_key(*created_at, object_id),
                value: serde_json::json!({ "object_id": object_id, "created_at": created_at }),
            },
        )
        .await?;
    }
    // 別の索引と別の prefix の entry は結果に混ざらない。
    docs.apply_doc_op(
        replica,
        DocOp::SetJson {
            key: stable_key(
                "indexes/thread",
                &format!(
                    "root/{:020}-{id}/{id}",
                    1_758_000_000,
                    id = object_id(9_999)
                ),
            ),
            value: serde_json::json!({}),
        },
    )
    .await?;
    docs.apply_doc_op(
        replica,
        DocOp::SetJson {
            key: stable_key("objects", &format!("{}/state", object_id(1))),
            value: serde_json::json!({}),
        },
    )
    .await?;
    Ok(())
}

fn expected(
    rows: &[(i64, String)],
    before: Option<&TimeIndexCursor>,
    limit: usize,
) -> Vec<(i64, String)> {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|left, right| right.cmp(left));
    sorted
        .into_iter()
        .filter(|(created_at, object_id)| match before {
            None => true,
            Some(cursor) => {
                (*created_at, object_id.as_str()) < (cursor.created_at, cursor.object_id.as_str())
            }
        })
        .take(limit)
        .collect()
}

async fn assert_matches_reference(docs: &dyn DocsSync, replica: &ReplicaId) -> Result<()> {
    let rows = fixture();
    seed(docs, replica, &rows).await?;

    for limit in [1usize, 3, 10, 100] {
        let newest = query_time_index_desc(docs, replica, INDEX_PREFIX, None, limit).await?;
        assert_eq!(
            newest
                .iter()
                .map(|entry| (entry.created_at, entry.object_id.clone()))
                .collect::<Vec<_>>(),
            expected(&rows, None, limit),
            "newest window, limit={limit}"
        );
    }

    // 全 entry と、entry の無い時刻を cursor にして、基準実装と突き合わせる。
    let mut cursors = rows
        .iter()
        .map(|(created_at, object_id)| TimeIndexCursor {
            created_at: *created_at,
            object_id: object_id.clone(),
        })
        .collect::<Vec<_>>();
    for created_at in [0_i64, 1, 10, 1_000_000_000, 1_757_999_995, 2_000_000_000] {
        cursors.push(TimeIndexCursor {
            created_at,
            object_id: "f".repeat(64),
        });
        cursors.push(TimeIndexCursor {
            created_at,
            object_id: "0".repeat(64),
        });
    }
    for cursor in &cursors {
        for limit in [1usize, 5, 50] {
            let older =
                query_time_index_desc(docs, replica, INDEX_PREFIX, Some(cursor), limit).await?;
            assert_eq!(
                older
                    .iter()
                    .map(|entry| (entry.created_at, entry.object_id.clone()))
                    .collect::<Vec<_>>(),
                expected(&rows, Some(cursor), limit),
                "cursor={cursor:?}, limit={limit}"
            );
        }
    }

    // ページを継いで最後まで読むと、全 entry を重複なく新しい順に読める。
    let mut walked = Vec::new();
    let mut cursor: Option<TimeIndexCursor> = None;
    loop {
        let page = query_time_index_desc(docs, replica, INDEX_PREFIX, cursor.as_ref(), 7).await?;
        let Some(last) = page.last() else {
            break;
        };
        cursor = Some(TimeIndexCursor {
            created_at: last.created_at,
            object_id: last.object_id.clone(),
        });
        walked.extend(
            page.into_iter()
                .map(|entry| (entry.created_at, entry.object_id)),
        );
    }
    assert_eq!(walked, expected(&rows, None, usize::MAX));
    Ok(())
}

#[tokio::test]
async fn time_index_matches_reference_on_memory_docs() -> Result<()> {
    let docs = MemoryDocsSync::default();
    assert_matches_reference(&docs, &topic_replica_id("kukuri:topic:time-index-memory")).await
}

#[tokio::test]
async fn time_index_matches_reference_on_iroh_docs() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let result =
        assert_matches_reference(&docs, &topic_replica_id("kukuri:topic:time-index-iroh")).await;
    docs.shutdown().await;
    node.shutdown().await?;
    result
}

#[tokio::test]
async fn key_query_respects_prefix_order_and_limit_on_both_implementations() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let iroh = IrohDocsSync::new(node.clone());
    let memory = MemoryDocsSync::default();
    let implementations: [(&str, &dyn DocsSync); 2] = [("memory", &memory), ("iroh", &iroh)];
    for (name, docs) in implementations {
        let replica = topic_replica_id(format!("kukuri:topic:key-query-{name}").as_str());
        docs.open_replica(&replica).await?;
        for key in ["a/1", "a/2", "a/3", "ab/1", "b/1"] {
            docs.apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: key.into(),
                    value: serde_json::json!({ "key": key }),
                },
            )
            .await?;
        }
        let keys = |page: DocKeyPage| {
            page.entries
                .into_iter()
                .map(|entry| entry.key)
                .collect::<Vec<_>>()
        };
        let query = |order, limit| DocKeyQuery {
            prefix: "a/".into(),
            order,
            limit,
        };
        // #1257: 打ち切りの情報は、どちらの実装も同じ意味で返す(`limit` 件を読んだら「打ち切られた」)。
        let all = docs
            .query_replica_keys(&replica, query(DocKeyOrder::Ascending, 10))
            .await?;
        assert!(
            !all.reached_limit,
            "{name}: three entries under a limit of ten"
        );
        assert_eq!(keys(all), vec!["a/1", "a/2", "a/3"], "{name}: ascending");
        let newest = docs
            .query_replica_keys(&replica, query(DocKeyOrder::Descending, 2))
            .await?;
        assert!(newest.reached_limit, "{name}: the limit cut the listing");
        assert_eq!(
            keys(newest),
            vec!["a/3", "a/2"],
            "{name}: descending with limit"
        );
        let exact = docs
            .query_replica_keys(&replica, query(DocKeyOrder::Ascending, 3))
            .await?;
        assert!(
            exact.reached_limit,
            "{name}: reading exactly the limit cannot tell that the prefix is exhausted"
        );
        let none = docs
            .query_replica_keys(&replica, query(DocKeyOrder::Ascending, 0))
            .await?;
        assert!(none.entries.is_empty(), "{name}: zero limit");
        assert!(!none.reached_limit, "{name}: zero limit reads nothing");
        let entry = docs
            .query_replica_keys(&replica, query(DocKeyOrder::Ascending, 1))
            .await?
            .entries
            .remove(0);
        assert!(entry.content_len > 0, "{name}: content length");
        assert!(!entry.content_hash.is_empty(), "{name}: content hash");

        // 全件を読む既存の query は、key の昇順で同じ集合を返す。
        let rows = docs
            .query_replica(&replica, DocQuery::Prefix("a".into()))
            .await?;
        assert_eq!(
            rows.into_iter().map(|row| row.key).collect::<Vec<_>>(),
            vec!["a/1", "a/2", "a/3", "ab/1"],
            "{name}: prefix query order"
        );
        assert_eq!(
            docs.query_replica(&replica, DocQuery::Exact("a/2".into()))
                .await?
                .len(),
            1,
            "{name}: exact query"
        );
    }
    iroh.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

/// query の回数と、1 回の query が返した entry 数を数える。
#[derive(Default)]
struct CountingKeys {
    inner: MemoryDocsSync,
    queries: AtomicUsize,
    largest_result: AtomicUsize,
}

#[async_trait]
impl DocsSync for CountingKeys {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        _replica_id: &ReplicaId,
        _query: DocQuery,
        _policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        anyhow::bail!("the time index must not read whole records")
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        let page = self.inner.query_replica_keys(replica_id, query).await?;
        self.largest_result
            .fetch_max(page.entries.len(), Ordering::SeqCst);
        Ok(page)
    }

    async fn subscribe_replica(&self, replica_id: &ReplicaId) -> Result<DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

// 1 回の遡りの query 数は時刻の桁で決まる定数で、索引の大きさに依存しない。返す entry 数は limit 以下。
#[tokio::test]
async fn walking_older_entries_uses_a_bounded_number_of_bounded_queries() -> Result<()> {
    let mut counts = Vec::new();
    for size in [100usize, 10_000] {
        let docs = Arc::new(CountingKeys::default());
        let replica = topic_replica_id(format!("kukuri:topic:time-index-bound-{size}").as_str());
        docs.open_replica(&replica).await?;
        for index in 0..size {
            docs.apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: index_key(1_700_000_000 + index as i64, &object_id(index)),
                    value: serde_json::json!({}),
                },
            )
            .await?;
        }
        let cursor = TimeIndexCursor {
            created_at: 1_700_000_000 + size as i64,
            object_id: "f".repeat(64),
        };
        docs.queries.store(0, Ordering::SeqCst);
        let page =
            query_time_index_desc(docs.as_ref(), &replica, INDEX_PREFIX, Some(&cursor), 20).await?;
        assert_eq!(page.len(), 20);
        let queries = docs.queries.load(Ordering::SeqCst);
        // 時刻 10 桁の数字の和は最大 90。同じ秒の読み出しが 1 回。
        assert!(queries <= 91, "size={size}: {queries} queries");
        counts.push(queries);

        // 索引の末尾より古い側(何も無い)を読んでも、query 数は同じ上限に収まる。
        docs.queries.store(0, Ordering::SeqCst);
        let end = TimeIndexCursor {
            created_at: 1_700_000_000,
            object_id: "0".repeat(64),
        };
        let nothing =
            query_time_index_desc(docs.as_ref(), &replica, INDEX_PREFIX, Some(&end), 20).await?;
        assert!(nothing.is_empty());
        assert!(docs.queries.load(Ordering::SeqCst) <= 91);
        // 1 回の query が返す entry 数にも上限がある(同じ秒の読み出しの 512 件が最大)。
        let largest = docs.largest_result.load(Ordering::SeqCst);
        assert!(
            largest <= 512,
            "size={size}: a query returned {largest} keys"
        );
    }
    assert!(
        counts[1] <= counts[0] + 10,
        "the number of queries must not grow with the index size: {counts:?}"
    );
    Ok(())
}

async fn put_key(docs: &dyn DocsSync, replica: &ReplicaId, key: String) -> Result<()> {
    docs.apply_doc_op(
        replica,
        DocOp::SetJson {
            key,
            value: serde_json::json!({}),
        },
    )
    .await
}

fn ids(entries: &[crate::TimeIndexEntry]) -> Vec<(i64, String)> {
    entries
        .iter()
        .map(|entry| (entry.created_at, entry.object_id.clone()))
        .collect()
}

// 窓の読み出しは、現在時刻 + 許容幅より未来の entry を読み飛ばす。未来の entry が少ないときは読み出し 1 回、
// 枠を埋めるほど多いときは、起点つきの遡りへ落ちて同じ結果を返す。
#[tokio::test]
async fn window_skips_future_entries_with_a_bounded_number_of_queries() -> Result<()> {
    let now = 1_758_000_000_i64;
    for future_entries in [3usize, 400] {
        let docs = Arc::new(CountingKeys::default());
        let replica =
            topic_replica_id(format!("kukuri:topic:time-index-window-{future_entries}").as_str());
        docs.open_replica(&replica).await?;
        let mut rows = Vec::new();
        for index in 0..60usize {
            let row = (now - 10 * index as i64, object_id(index));
            put_key(docs.as_ref(), &replica, index_key(row.0, &row.1)).await?;
            rows.push(row);
        }
        for index in 0..future_entries {
            put_key(
                docs.as_ref(),
                &replica,
                index_key(now + 86_400 + index as i64, &object_id(10_000 + index)),
            )
            .await?;
        }
        docs.queries.store(0, Ordering::SeqCst);
        let window =
            query_time_index_window(docs.as_ref(), &replica, INDEX_PREFIX, now + 600, 20).await?;
        assert_eq!(
            ids(&window),
            expected(&rows, None, 20),
            "future_entries={future_entries}"
        );
        let queries = docs.queries.load(Ordering::SeqCst);
        if future_entries == 3 {
            assert_eq!(
                queries, 1,
                "a small number of future entries costs one query"
            );
        } else {
            assert!(queries <= 92, "{queries} queries");
        }
        let largest = docs.largest_result.load(Ordering::SeqCst);
        assert!(largest <= 512, "a query returned {largest} keys");
    }
    Ok(())
}

// 形の違う key は、読み出しごとの余裕の範囲なら有効な entry を押し出さない。余裕を超えたときは、古い側の
// prefix へ進まずに prefix を細かく分けて読み直し、有効な entry を飛ばしたページを返さない。
#[tokio::test]
async fn malformed_index_keys_do_not_make_the_walk_skip_valid_entries() -> Result<()> {
    let docs = MemoryDocsSync::default();
    let replica = topic_replica_id("kukuri:topic:time-index-malformed");
    docs.open_replica(&replica).await?;
    // 同じ 10 秒の prefix(175800000x)に、形の違う key と有効な entry を混ぜる。形の違う key は
    // 降順で有効な entry より先に並ぶ(`~` は 16 進の文字より大きい)。
    let mut rows = Vec::new();
    for index in 0..6usize {
        let row = (1_758_000_001 + index as i64, object_id(index));
        put_key(&docs, &replica, index_key(row.0, &row.1)).await?;
        rows.push(row);
    }
    let older = (1_757_000_000_i64, object_id(99));
    put_key(&docs, &replica, index_key(older.0, &older.1)).await?;
    rows.push(older.clone());
    for index in 0..8usize {
        put_key(
            &docs,
            &replica,
            stable_key(
                "indexes/timeline",
                &format!("{:020}-~malformed-{index}", 1_758_000_008),
            ),
        )
        .await?;
    }
    let cursor = TimeIndexCursor {
        created_at: 1_758_000_050,
        object_id: "f".repeat(64),
    };
    let page = query_time_index_desc(&docs, &replica, INDEX_PREFIX, Some(&cursor), 4).await?;
    assert_eq!(
        ids(&page),
        expected(&rows, Some(&cursor), 4),
        "within the allowance"
    );

    // 余裕(32 件)を超える数の形の違う key を足す。
    for index in 0..64usize {
        put_key(
            &docs,
            &replica,
            stable_key(
                "indexes/timeline",
                &format!("{:020}-~more-malformed-{index:03}", 1_758_000_009),
            ),
        )
        .await?;
    }
    let page = query_time_index_desc(&docs, &replica, INDEX_PREFIX, Some(&cursor), 4).await?;
    assert_eq!(
        ids(&page),
        expected(&rows, Some(&cursor), 4),
        "beyond the allowance: the walk splits the prefix instead of moving on to an older one"
    );
    // ページを継いでも、有効な entry をすべて新しい順に読める。
    let mut walked = Vec::new();
    let mut next = Some(cursor);
    while let Some(current) = next.take() {
        let page = query_time_index_desc(&docs, &replica, INDEX_PREFIX, Some(&current), 3).await?;
        if let Some(last) = page.last() {
            next = Some(TimeIndexCursor {
                created_at: last.created_at,
                object_id: last.object_id.clone(),
            });
        }
        walked.extend(ids(&page));
    }
    assert_eq!(walked, expected(&rows, None, usize::MAX));
    Ok(())
}

// `limit` が 0 の読み出しは、どちらの実装も replica を開いてから空を返す。
#[tokio::test]
async fn zero_limit_key_query_opens_the_replica_on_both_implementations() -> Result<()> {
    let query = || DocKeyQuery {
        prefix: INDEX_PREFIX.into(),
        order: DocKeyOrder::Descending,
        limit: 0,
    };
    let memory = MemoryDocsSync::default();
    let replica = topic_replica_id("kukuri:topic:time-index-zero-limit");
    assert!(
        memory
            .query_replica_keys(&replica, query())
            .await?
            .entries
            .is_empty()
    );

    let node = IrohDocsNode::memory().await?;
    let iroh = IrohDocsSync::new(node.clone());
    assert!(
        iroh.query_replica_keys(&replica, query())
            .await?
            .entries
            .is_empty()
    );
    // 権限の無い private replica は、`limit` が 0 でも失敗する(開けないものを開けたことにしない)。
    let private = crate::private_channel_epoch_replica_id("channel", "epoch");
    assert!(iroh.query_replica_keys(&private, query()).await.is_err());
    iroh.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

// TR-13: iroh-docs は、既定の並び(`AuthorKey`)の query を namespace 全体の table scan として実行する。
// どの種類の query も key の索引(`KeyAuthor`)で読む形に組まれていることを固定する。
#[test]
fn every_docs_query_is_built_on_the_key_index() {
    for query in [
        DocQuery::Exact("objects/a/state".into()),
        DocQuery::Prefix("objects/".into()),
        DocQuery::All,
    ] {
        let built = format!("{:?}", crate::iroh_sync::indexed_query(query.clone()));
        assert!(
            built.contains("KeyAuthor"),
            "{query:?} must use the key index, got {built}"
        );
        assert!(!built.contains("AuthorKey"), "{query:?}: {built}");
    }
}

// 窓の読み出しが遡りへ落ちたとき、`not_after` ちょうどの秒の entry を含み、その 1 秒先は含まない
// (独立監査の確認 test を恒久化)。
#[tokio::test]
async fn window_fallback_keeps_the_not_after_second() -> Result<()> {
    let docs = Arc::new(CountingKeys::default());
    let replica = topic_replica_id("kukuri:topic:time-index-window-boundary");
    docs.open_replica(&replica).await?;
    let not_after = 1_758_000_600_i64;
    let mut rows = Vec::new();
    // `not_after` ちょうどの秒に 2 件、それより古い側に 3 件。
    for (index, created_at) in [not_after, not_after, not_after - 1, not_after - 50, 1_000]
        .into_iter()
        .enumerate()
    {
        let row = (created_at, object_id(index + 1));
        put_key(docs.as_ref(), &replica, index_key(row.0, &row.1)).await?;
        rows.push(row);
    }
    // 1 秒だけ未来の entry と、遠い未来の entry を、余裕を超えて置く。
    put_key(
        docs.as_ref(),
        &replica,
        index_key(not_after + 1, &object_id(500)),
    )
    .await?;
    for index in 0..120usize {
        put_key(
            docs.as_ref(),
            &replica,
            index_key(not_after + 86_400 + index as i64, &object_id(1_000 + index)),
        )
        .await?;
    }
    for limit in [1usize, 2, 3, 5, 20] {
        docs.queries.store(0, Ordering::SeqCst);
        let window =
            query_time_index_window(docs.as_ref(), &replica, INDEX_PREFIX, not_after, limit)
                .await?;
        assert_eq!(ids(&window), expected(&rows, None, limit), "limit={limit}");
        assert!(docs.queries.load(Ordering::SeqCst) > 1, "the fallback ran");
    }
    Ok(())
}

// 形の違う key を多くの秒へ置かれても、遡りは query 数の上限で止まり、読めた分(新しい側から連続)を返す
// (独立監査の確認 test を恒久化)。
#[tokio::test]
async fn walk_stops_at_the_query_cap_and_returns_a_contiguous_head() -> Result<()> {
    let docs = Arc::new(CountingKeys::default());
    let replica = topic_replica_id("kukuri:topic:time-index-query-cap");
    docs.open_replica(&replica).await?;
    let top = 1_758_000_000_i64;
    let mut rows = vec![(top - 1, object_id(1)), (top - 2, object_id(2))];
    for (created_at, id) in &rows {
        put_key(docs.as_ref(), &replica, index_key(*created_at, id)).await?;
    }
    // その古い側の 40 個の「10 秒の範囲」それぞれの 1 秒に、形の違う key を余裕を超えて置く。
    for window in 1..=40_i64 {
        let second = top - window * 10 - 3;
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
    let old = (top - 1_000, object_id(3));
    put_key(docs.as_ref(), &replica, index_key(old.0, &old.1)).await?;
    rows.push(old);

    let cursor = TimeIndexCursor {
        created_at: top,
        object_id: "f".repeat(64),
    };
    docs.queries.store(0, Ordering::SeqCst);
    let page =
        query_time_index_desc(docs.as_ref(), &replica, INDEX_PREFIX, Some(&cursor), 5).await?;
    let queries = docs.queries.load(Ordering::SeqCst);
    // 同じ秒の読み出し 1 回と、遡りの上限 256 回。
    assert!(queries <= 257, "the walk is capped: {queries} queries");
    // 返した分は、基準実装の先頭と一致する(途中を飛ばさない)。
    let reference = expected(&rows, Some(&cursor), 5);
    assert!(!page.is_empty());
    assert_eq!(ids(&page), reference[..page.len()].to_vec());
    Ok(())
}

/// `indexes/timeline/` の下に、UTF-8 でない key の entry を `count` 件置く。public topic の replica は、
/// topic id を知る誰もが書ける(namespace の secret は replica id から決まる)。
async fn put_non_utf8_index_keys(
    node: &IrohDocsNode,
    replica: &ReplicaId,
    time_prefix: &str,
    count: usize,
) -> Result<()> {
    let secret = crate::replicas::public_replica_secret(replica).expect("public replica secret");
    let doc = node
        .docs()
        .import_namespace(iroh_docs::Capability::Write(secret))
        .await?;
    let author = node.docs().author_default().await?;
    for index in 0..count {
        let mut key = format!("{INDEX_PREFIX}{time_prefix}").into_bytes();
        key.extend_from_slice(&[0xff, 0xfe]);
        key.extend_from_slice(format!("{index:04}").as_bytes());
        doc.set_bytes(author, key, b"x".to_vec()).await?;
    }
    Ok(())
}

// #1257 AC-1 / TR-2・TR-3: UTF-8 でない key が、余裕を超えて索引の新しい側を埋めていても、窓の読み出しは
// 有効な entry を返す。UTF-8 でない key は読み出しの中で飛ばされて件数に入らないので、「打ち切られたか」を
// 返った件数で判定すると、索引が尽きたと誤判定して 0 件を返していた。
#[tokio::test]
async fn non_utf8_keys_filling_the_newest_side_do_not_hide_valid_entries_from_the_window()
-> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let replica = topic_replica_id("kukuri:topic:time-index-non-utf8-window");
    docs.open_replica(&replica).await?;
    let mut rows = Vec::new();
    for index in 0..5usize {
        let row = (1_000 + index as i64, object_id(index + 1));
        put_key(&docs, &replica, index_key(row.0, &row.1)).await?;
        rows.push(row);
    }
    // 未来の時刻の有効な entry も混ぜる(窓は読み飛ばす)。
    put_key(&docs, &replica, index_key(9_000, &object_id(900))).await?;
    // `0xff` は数字より後ろに並ぶので、降順の読み出しでは必ず先頭に来る。
    put_non_utf8_index_keys(&node, &replica, "", 80).await?;

    let window = query_time_index_window(&docs, &replica, INDEX_PREFIX, 2_000, 5).await;
    docs.shutdown().await;
    node.shutdown().await?;
    assert_eq!(
        ids(&window?),
        expected(&rows, None, 5),
        "valid entries must stay visible"
    );
    Ok(())
}

// #1257 AC-2・AC-3 / TR-4・TR-5: cursor つきの遡りは、UTF-8 でない key で埋まった prefix を細かく分けて
// 読み直し、有効な entry を飛ばさない。query 数は上限(同じ秒の読み出し 1 回 + 256 回)以下。
#[tokio::test]
async fn non_utf8_keys_do_not_make_the_walk_skip_valid_entries() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let inner = IrohDocsSync::new(node.clone());
    let replica = topic_replica_id("kukuri:topic:time-index-non-utf8-walk");
    inner.open_replica(&replica).await?;
    let mut rows = Vec::new();
    for index in 0..6usize {
        let row = (1_758_000_001 + index as i64, object_id(index + 1));
        put_key(&inner, &replica, index_key(row.0, &row.1)).await?;
        rows.push(row);
    }
    let older = (1_757_000_000_i64, object_id(99));
    put_key(&inner, &replica, index_key(older.0, &older.1)).await?;
    rows.push(older);
    // 有効な entry と同じ 10 秒の prefix(175800000x)の、より新しい秒に、余裕(32 件)を超える数を置く。
    put_non_utf8_index_keys(&node, &replica, &format!("{:020}-", 1_758_000_009_i64), 64).await?;

    let cursor = TimeIndexCursor {
        created_at: 1_758_000_050,
        object_id: "f".repeat(64),
    };
    let page = query_time_index_desc(&inner, &replica, INDEX_PREFIX, Some(&cursor), 4).await;
    let mut walked = Vec::new();
    let mut next = Some(cursor.clone());
    while let Some(current) = next.take() {
        let page = query_time_index_desc(&inner, &replica, INDEX_PREFIX, Some(&current), 3).await?;
        if let Some(last) = page.last() {
            next = Some(TimeIndexCursor {
                created_at: last.created_at,
                object_id: last.object_id.clone(),
            });
        }
        walked.extend(ids(&page));
    }
    inner.shutdown().await;
    node.shutdown().await?;
    assert_eq!(ids(&page?), expected(&rows, Some(&cursor), 4));
    assert_eq!(walked, expected(&rows, None, usize::MAX));
    Ok(())
}
