use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::ReplicaId;
use kukuri_iroh_node::IrohDocsNode;

use crate::{
    DocEventStream, DocFetchPolicy, DocKeyEntry, DocKeyOrder, DocKeyQuery, DocOp, DocQuery,
    DocRecord, DocsSync, IrohDocsSync, MemoryDocsSync, TimeIndexCursor, query_time_index_desc,
    stable_key, topic_replica_id,
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
        let keys = |entries: Vec<DocKeyEntry>| {
            entries
                .into_iter()
                .map(|entry| entry.key)
                .collect::<Vec<_>>()
        };
        let query = |order, limit| DocKeyQuery {
            prefix: "a/".into(),
            order,
            limit,
        };
        assert_eq!(
            keys(
                docs.query_replica_keys(&replica, query(DocKeyOrder::Ascending, 10))
                    .await?
            ),
            vec!["a/1", "a/2", "a/3"],
            "{name}: ascending"
        );
        assert_eq!(
            keys(
                docs.query_replica_keys(&replica, query(DocKeyOrder::Descending, 2))
                    .await?
            ),
            vec!["a/3", "a/2"],
            "{name}: descending with limit"
        );
        assert!(
            docs.query_replica_keys(&replica, query(DocKeyOrder::Ascending, 0))
                .await?
                .is_empty(),
            "{name}: zero limit"
        );
        let entry = docs
            .query_replica_keys(&replica, query(DocKeyOrder::Ascending, 1))
            .await?
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
    ) -> Result<Vec<DocKeyEntry>> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        let entries = self.inner.query_replica_keys(replica_id, query).await?;
        self.largest_result
            .fetch_max(entries.len(), Ordering::SeqCst);
        Ok(entries)
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
    }
    assert!(
        counts[1] <= counts[0] + 10,
        "the number of queries must not grow with the index size: {counts:?}"
    );
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
