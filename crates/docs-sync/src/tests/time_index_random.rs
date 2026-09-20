//! #1239: 時系列の索引の読み出しを、乱数で作った索引と基準実装で突き合わせる(独立監査 PR #1265 の確認 test を恒久化)。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Result;
use kukuri_core::ReplicaId;
use kukuri_iroh_node::IrohDocsNode;

use super::time_index::{CountingKeys, INDEX_PREFIX, ids, index_key, object_id, put_key};
use crate::{
    DocKeyOrder, DocsSync, IrohDocsSync, MemoryDocsSync, TimeIndexCursor, TimeIndexPage,
    query_time_index_asc, query_time_index_desc, stable_key, topic_replica_id,
};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn reference(
    rows: &[(i64, String)],
    order: DocKeyOrder,
    cursor: Option<&TimeIndexCursor>,
    limit: usize,
) -> Vec<(i64, String)> {
    let mut sorted = rows.to_vec();
    sorted.sort();
    sorted.dedup();
    if order == DocKeyOrder::Descending {
        sorted.reverse();
    }
    sorted
        .into_iter()
        .filter(|(created_at, id)| {
            let Some(cursor) = cursor else { return true };
            let entry = (*created_at, id.as_str());
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

/// 続きの起点を継ぎながら、`limit` 件に届くか尽きるまで読む。
async fn query_following_resume(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    order: DocKeyOrder,
    cursor: Option<&TimeIndexCursor>,
    limit: usize,
) -> Result<Vec<(i64, String)>> {
    let mut out = Vec::new();
    let mut position = cursor.cloned();
    for _ in 0..64 {
        let page = query(docs, replica, order, position.as_ref(), limit - out.len()).await?;
        out.extend(ids(&page));
        match page.resume {
            Some(resume) if out.len() < limit => position = Some(resume),
            _ => return Ok(out),
        }
    }
    anyhow::bail!("続きの起点が 64 回を超えて返った(読み進めが止まっている)")
}

/// 乱数の fixture。時刻は、桁の境界・密な範囲・疎な範囲・上限付近を混ぜる。形の違う key は、どの秒にも
/// 余裕(32 件)未満しか置かない(余裕の範囲なら、有効な entry を 1 件も落とさないことが契約)。
fn random_rows(rng: &mut Lcg, count: usize) -> Vec<(i64, String)> {
    let bases: [i64; 9] = [
        0,
        9,
        99_999,
        1_000_000_000,
        1_757_999_990,
        1_758_000_000,
        4_102_444_800,
        i64::MAX - 6_000,
        999_999_999_999_999_990,
    ];
    let mut rows = Vec::new();
    for _ in 0..count {
        let base = bases[rng.below(bases.len() as u64) as usize];
        let spread = [1u64, 3, 30, 5_000][rng.below(4) as usize];
        let offset = rng.below(spread) as i64;
        let created_at = base.saturating_add(offset);
        rows.push((created_at, object_id(rng.below(1 << 20) as usize)));
    }
    rows.sort();
    rows.dedup();
    rows
}

async fn seed_random(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    rng: &mut Lcg,
    rows: &[(i64, String)],
    junk_per_spot: usize,
) -> Result<()> {
    docs.open_replica(replica).await?;
    for (created_at, id) in rows {
        put_key(docs, replica, index_key(*created_at, id)).await?;
    }
    // 形の違う key: 有効な entry と同じ秒の前後、桁の途中、数字の前後に並ぶもの。
    for (spot, (created_at, _)) in rows.iter().enumerate() {
        if rng.below(3) != 0 {
            continue;
        }
        for index in 0..junk_per_spot {
            let lead = ['~', '!'][(spot + index) % 2];
            put_key(
                docs,
                replica,
                stable_key(
                    "indexes/timeline",
                    &format!("{created_at:020}-{lead}junk-{spot}-{index}"),
                ),
            )
            .await?;
        }
    }
    for index in 0..20usize {
        for lead in [
            "~",
            "!",
            "0!",
            "09",
            "0000000000!",
            "+0000000000000001000-",
            "1",
        ] {
            put_key(
                docs,
                replica,
                stable_key("indexes/timeline", &format!("{lead}edge-junk-{index:02}")),
            )
            .await?;
        }
    }
    Ok(())
}

async fn assert_random_matches_reference(
    docs: &dyn DocsSync,
    replica: &ReplicaId,
    seed: u64,
    count: usize,
    cursors_per_order: usize,
) -> Result<()> {
    let mut rng = Lcg(seed);
    let rows = random_rows(&mut rng, count);
    seed_random(docs, replica, &mut rng, &rows, 5).await?;
    for order in [DocKeyOrder::Descending, DocKeyOrder::Ascending] {
        let mut cursors: Vec<Option<TimeIndexCursor>> = vec![None];
        for created_at in [
            -1_i64,
            i64::MIN,
            0,
            1,
            i64::MAX,
            i64::MAX - 1,
            i64::MAX - 41,
        ] {
            for id in [String::new(), "0".repeat(64), "f".repeat(64)] {
                cursors.push(Some(TimeIndexCursor {
                    created_at,
                    object_id: id,
                }));
            }
        }
        for _ in 0..cursors_per_order {
            let (created_at, id) = rows[rng.below(rows.len() as u64) as usize].clone();
            let jitter = rng.below(5) as i64 - 2;
            cursors.push(Some(TimeIndexCursor {
                created_at: created_at.saturating_add(jitter),
                object_id: if rng.below(2) == 0 {
                    id
                } else {
                    object_id(rng.below(1 << 20) as usize)
                },
            }));
        }
        for cursor in &cursors {
            for limit in [1usize, 7, 200] {
                let got =
                    query_following_resume(docs, replica, order, cursor.as_ref(), limit).await?;
                assert_eq!(
                    got,
                    reference(&rows, order, cursor.as_ref(), limit),
                    "seed={seed} order={order:?} cursor={cursor:?} limit={limit}"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn random_walk_matches_reference_on_memory_docs() -> Result<()> {
    for seed in 1..=6u64 {
        let docs = MemoryDocsSync::default();
        let replica = topic_replica_id(format!("kukuri:topic:time-index-random-{seed}").as_str());
        assert_random_matches_reference(&docs, &replica, seed, 400, 60).await?;
    }
    Ok(())
}

#[tokio::test]
async fn random_walk_matches_reference_on_iroh_docs() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let replica = topic_replica_id("kukuri:topic:time-index-random-iroh");
    let result = assert_random_matches_reference(&docs, &replica, 42, 150, 12).await;
    docs.shutdown().await;
    node.shutdown().await?;
    result
}

// 1 回の読み出しの query 数の最大値を、形の違う key の置き方を変えながら測る。
#[tokio::test]
async fn query_count_of_one_call_is_bounded() -> Result<()> {
    let mut worst = 0usize;
    for order in [DocKeyOrder::Descending, DocKeyOrder::Ascending] {
        for clusters in [1usize, 2, 3, 30, 300] {
            let docs = Arc::new(CountingKeys::default());
            let replica = topic_replica_id(
                format!("kukuri:topic:time-index-bound-{order:?}-{clusters}").as_str(),
            );
            docs.open_replica(&replica).await?;
            let top = 1_758_000_000_i64;
            let step = if order == DocKeyOrder::Descending {
                -1
            } else {
                1
            };
            for cluster in 0..clusters as i64 {
                let second = top + step * (cluster * 7 + 5);
                for index in 0..45usize {
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
            let cursor = TimeIndexCursor {
                created_at: top,
                object_id: "8".repeat(64),
            };
            let mut position = Some(cursor);
            for _ in 0..64 {
                docs.queries.store(0, Ordering::SeqCst);
                let page = query(docs.as_ref(), &replica, order, position.as_ref(), 10).await?;
                let queries = docs.queries.load(Ordering::SeqCst);
                assert_eq!(page.queries, queries);
                worst = worst.max(queries);
                assert!(docs.largest_result.load(Ordering::SeqCst) <= 512);
                match page.resume {
                    Some(resume) => position = Some(resume),
                    None => break,
                }
            }
        }
    }
    assert!(worst <= 258, "{worst}");
    Ok(())
}

// 古い順の読み出しで、符号つき 64 bit に収まらない時刻の範囲(有効な entry は在りえない)に形の違う key を
// 置かれたとき、続きの起点が同じ位置を繰り返さないか。
#[tokio::test]
async fn ascending_resume_beyond_i64_max_makes_progress() -> Result<()> {
    let docs = Arc::new(CountingKeys::default());
    let replica = topic_replica_id("kukuri:topic:time-index-asc-clamp");
    docs.open_replica(&replica).await?;
    let rows = vec![
        (1_758_000_000_i64, object_id(1)),
        (1_758_000_010, object_id(2)),
    ];
    for (created_at, id) in &rows {
        put_key(docs.as_ref(), &replica, index_key(*created_at, id)).await?;
    }
    // 20 桁だが i64 に収まらない時刻の 3 つの秒に、余裕を超える数の形の違う key を置く。
    for time in [
        "09500000000000000001",
        "09700000000000000002",
        "09900000000000000003",
    ] {
        for index in 0..60usize {
            put_key(
                docs.as_ref(),
                &replica,
                stable_key("indexes/timeline", &format!("{time}-~junk-{index:03}")),
            )
            .await?;
        }
    }
    let start = TimeIndexCursor {
        created_at: rows[1].0,
        object_id: rows[1].1.clone(),
    };
    let mut position = start;
    let mut seen = Vec::new();
    for _ in 0..12usize {
        let page = query(
            docs.as_ref(),
            &replica,
            DocKeyOrder::Ascending,
            Some(&position),
            20,
        )
        .await?;
        assert!(page.entries.is_empty());
        let Some(resume) = page.resume else {
            return Ok(());
        };
        assert!(
            !seen.contains(&resume),
            "続きの起点が同じ位置を繰り返した: {resume:?}(query {} 回)",
            page.queries
        );
        seen.push(resume.clone());
        position = resume;
    }
    anyhow::bail!("12 回読み継いでも尽きない")
}
