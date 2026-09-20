//! 時系列の索引(`indexes/timeline/…`・`indexes/thread/<root>/…`)を、上限つきで読む(#1239)。
//!
//! 索引の key は `<index prefix><created_at を 20 桁で 0 埋め>-<object id>/<object id>`。20 桁の 0 埋めなので、
//! 10 進の桁の prefix がそのまま時間の範囲になる。iroh-docs の query は prefix・key 順・`limit` しか
//! 持たず、「この key より古い側」という範囲指定が無い。そこで、cursor の時刻の桁を下の桁から順に
//! 1 つずつ減らした prefix を新しい側からたどり、cursor より古い entry を新しい順に集める。
//! 1 回の呼び出しの query 数は「時刻の各桁の数字の和 + 1」以下の定数で、replica の総 entry 数に依存しない。

use anyhow::Result;
use kukuri_core::ReplicaId;

use crate::types::{DocKeyEntry, DocKeyOrder, DocKeyQuery, DocsSync};

/// 索引の時刻部分の桁数(`kukuri_core::timeline_sort_key` の 0 埋めと同じ)。
const TIME_DIGITS: usize = 20;
/// 同じ秒の中で cursor より古い entry を探すときの読み出しの上限。
/// 1 秒に同じ索引へこの件数を超える投稿があると、超えた分は遡りで取りこぼしうる(best effort)。
const SAME_SECOND_SCAN_LIMIT: usize = 512;

/// 時系列の索引の 1 件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeIndexEntry {
    pub created_at: i64,
    pub object_id: String,
    pub key: String,
}

/// 遡りの起点。この位置より古い entry だけを返す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeIndexCursor {
    pub created_at: i64,
    pub object_id: String,
}

/// `index_prefix`(末尾が `/`)の索引から、新しい順に最大 `limit` 件を返す。
/// `before` があれば、その位置より古い entry だけを返す。
pub async fn query_time_index_desc(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    index_prefix: &str,
    before: Option<&TimeIndexCursor>,
    limit: usize,
) -> Result<Vec<TimeIndexEntry>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(before) = before else {
        let keys = docs_sync
            .query_replica_keys(
                replica_id,
                DocKeyQuery {
                    prefix: index_prefix.to_string(),
                    order: DocKeyOrder::Descending,
                    limit,
                },
            )
            .await?;
        return Ok(parse_entries(index_prefix, keys));
    };
    if before.created_at < 0 {
        return Ok(Vec::new());
    }
    let digits = format!("{:0width$}", before.created_at, width = TIME_DIGITS);
    if digits.len() != TIME_DIGITS {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();

    // 同じ秒の中で、cursor より小さい object id を持つ entry。
    let same_second = docs_sync
        .query_replica_keys(
            replica_id,
            DocKeyQuery {
                prefix: format!("{index_prefix}{digits}-"),
                order: DocKeyOrder::Descending,
                limit: SAME_SECOND_SCAN_LIMIT,
            },
        )
        .await?;
    entries.extend(
        parse_entries(index_prefix, same_second)
            .into_iter()
            .filter(|entry| entry.object_id.as_str() < before.object_id.as_str())
            .take(limit),
    );

    // 下の桁から順に、その桁を 1 つずつ減らした prefix を新しい側からたどる。
    let digit_bytes = digits.as_bytes();
    for position in (0..TIME_DIGITS).rev() {
        if entries.len() >= limit {
            break;
        }
        let digit = digit_bytes[position] - b'0';
        for smaller in (0..digit).rev() {
            if entries.len() >= limit {
                break;
            }
            let prefix = format!("{index_prefix}{}{smaller}", &digits[..position]);
            let keys = docs_sync
                .query_replica_keys(
                    replica_id,
                    DocKeyQuery {
                        prefix,
                        order: DocKeyOrder::Descending,
                        limit: limit - entries.len(),
                    },
                )
                .await?;
            entries.extend(parse_entries(index_prefix, keys));
        }
    }
    entries.truncate(limit);
    Ok(entries)
}

fn parse_entries(index_prefix: &str, keys: Vec<DocKeyEntry>) -> Vec<TimeIndexEntry> {
    keys.into_iter()
        .filter_map(|entry| parse_entry(index_prefix, entry.key))
        .collect()
}

/// `<index prefix><20 桁>-<object id>/<object id>` を分解する。形が違う key は捨てる。
fn parse_entry(index_prefix: &str, key: String) -> Option<TimeIndexEntry> {
    let rest = key.strip_prefix(index_prefix)?;
    let (sort_key, object_id) = rest.rsplit_once('/')?;
    let (time, sort_object_id) = sort_key.split_once('-')?;
    if time.len() != TIME_DIGITS || sort_object_id != object_id || object_id.is_empty() {
        return None;
    }
    let created_at = time.parse::<i64>().ok()?;
    Some(TimeIndexEntry {
        created_at,
        object_id: object_id.to_string(),
        key,
    })
}
