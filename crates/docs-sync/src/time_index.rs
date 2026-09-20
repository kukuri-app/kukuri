//! 時系列の索引(`indexes/timeline/…`・`indexes/thread/<root>/…`)を、上限つきで読む(#1239)。
//!
//! 索引の key は `<index prefix><created_at を 20 桁で 0 埋め>-<object id>/<object id>`。20 桁の 0 埋めなので、
//! 10 進の桁の prefix がそのまま時間の範囲になる。iroh-docs の query は prefix・key 順・`limit` しか
//! 持たず、「この key より古い側」という範囲指定が無い。そこで、cursor の時刻の桁を下の桁から順に
//! 1 つずつ減らした prefix を新しい側からたどり、cursor より古い entry を新しい順に集める。
//! 1 回の呼び出しの query 数は「時刻の各桁の数字の和 + 1」以下の定数で、replica の総 entry 数に依存しない。
//!
//! 索引の key は、その replica に書ける誰もが置ける。形の違う key と、未来の時刻の key への耐性は
//! best effort とする(読み出しごとに固定の余裕を持ち、余裕を超えたら prefix を細かく分けて読み直す。
//! query 数には上限を置く)。
//! 有効な形の key を大量に置く書き込みは、この層では防げない。

use anyhow::Result;
use kukuri_core::ReplicaId;

use crate::types::{DocKeyEntry, DocKeyOrder, DocKeyQuery, DocsSync};

/// 索引の時刻部分の桁数(`kukuri_core::timeline_sort_key` の 0 埋めと同じ)。
const TIME_DIGITS: usize = 20;
/// 同じ秒の中で cursor より古い entry を探すときの読み出しの上限。
/// 1 秒に同じ索引へこの件数を超える投稿があると、超えた分は遡りで取りこぼしうる(best effort)。
const SAME_SECOND_SCAN_LIMIT: usize = 512;
/// 1 回の読み出しで、形の違う key が占めてよい枠。`limit` に足して読む。
const MALFORMED_KEY_ALLOWANCE: usize = 32;
/// 窓の読み出しで、未来の時刻の entry が占めてよい枠。超えたら cursor つきの読み出しへ落ちる。
const FUTURE_ENTRY_ALLOWANCE: usize = 32;
/// 1 回の遡りで発行する query 数の上限。通常は「時刻の各桁の数字の和」以下で、形の違う key を避けるために
/// prefix を細かく分けたときだけ増える。上限に達したら、そこまでに読めた分を返す。
const MAX_WALK_QUERIES: usize = 256;

/// 時系列の索引の 1 件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeIndexEntry {
    pub created_at: i64,
    pub object_id: String,
    pub key: String,
    /// 索引の entry を書いた docs author の id(ADR 0053)。その object の envelope を読むときの手がかりになる
    /// (投稿と索引は、同じ docs author が書く)。署名の無い値で、読む record を選ぶことだけに使う。
    pub docs_author: Option<String>,
}

/// 遡りの起点。この位置より古い entry だけを返す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeIndexCursor {
    pub created_at: i64,
    pub object_id: String,
}

impl TimeIndexCursor {
    /// `created_at` 以前の entry をすべて含む起点(`created_at` の秒の entry も含む)。
    pub fn not_after(created_at: i64) -> Self {
        Self {
            created_at: created_at.saturating_add(1),
            object_id: String::new(),
        }
    }
}

/// 索引の新しい側から最大 `limit` 件を返す(窓の読み出し)。`not_after`(秒)より未来の時刻の entry は読み飛ばす。
///
/// `created_at` は投稿者の申告値なので、極端に未来の時刻の entry が新しい側を占有しうる。通常は降順の
/// 読み出し 1 回で済ませ、未来の entry が枠を埋めたときだけ、`not_after` を起点にした遡りの読み出しへ落ちる。
pub async fn query_time_index_window(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    index_prefix: &str,
    not_after: i64,
    limit: usize,
) -> Result<Vec<TimeIndexEntry>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let requested = limit
        .saturating_add(FUTURE_ENTRY_ALLOWANCE)
        .saturating_add(MALFORMED_KEY_ALLOWANCE);
    let page = docs_sync
        .query_replica_keys(
            replica_id,
            DocKeyQuery {
                prefix: index_prefix.to_string(),
                order: DocKeyOrder::Descending,
                limit: requested,
            },
        )
        .await?;
    // 返った件数では判定しない。UTF-8 でない key の entry は読み出しの中で飛ばされ、件数に入らない(#1257)。
    let truncated = page.reached_limit;
    let mut entries = parse_entries(index_prefix, page.entries)
        .into_iter()
        .filter(|entry| entry.created_at <= not_after)
        .collect::<Vec<_>>();
    if entries.len() >= limit || !truncated {
        entries.truncate(limit);
        return Ok(entries);
    }
    query_time_index_desc(
        docs_sync,
        replica_id,
        index_prefix,
        Some(&TimeIndexCursor::not_after(not_after)),
        limit,
    )
    .await
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
        return Ok(parse_entries(index_prefix, keys.entries));
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
        parse_entries(index_prefix, same_second.entries)
            .into_iter()
            .filter(|entry| entry.object_id.as_str() < before.object_id.as_str())
            .take(limit),
    );

    // 下の桁から順に、その桁を 1 つずつ減らした prefix を、新しい側から順に読む。
    // `pending` は後ろから取り出すので、古い側の prefix から積む。
    let digit_bytes = digits.as_bytes();
    let mut pending = Vec::new();
    for position in 0..TIME_DIGITS {
        let digit = digit_bytes[position] - b'0';
        for smaller in 0..digit {
            pending.push(format!("{}{smaller}", &digits[..position]));
        }
    }
    let mut queries = 0usize;
    while let Some(time_prefix) = pending.pop() {
        if entries.len() >= limit || queries >= MAX_WALK_QUERIES {
            break;
        }
        let needed = limit - entries.len();
        let requested = needed.saturating_add(MALFORMED_KEY_ALLOWANCE);
        let page = docs_sync
            .query_replica_keys(
                replica_id,
                DocKeyQuery {
                    prefix: format!("{index_prefix}{time_prefix}"),
                    order: DocKeyOrder::Descending,
                    limit: requested,
                },
            )
            .await?;
        queries += 1;
        let truncated = page.reached_limit;
        let valid = parse_entries(index_prefix, page.entries);
        if !truncated || valid.len() >= needed || time_prefix.len() >= TIME_DIGITS {
            entries.extend(valid);
            continue;
        }
        // 形の違う key が余裕を超えて枠を埋めた。この prefix には、読めていない有効な entry が残りうる。
        // 古い側の prefix へ進むと、その entry を飛ばしたページを返してしまうので、1 桁細かい prefix に
        // 分けて新しい側から読み直す。1 秒まで分けても埋まっている場合は、その秒だけを諦める。
        for digit in 0..=9 {
            pending.push(format!("{time_prefix}{digit}"));
        }
    }
    entries.truncate(limit);
    Ok(entries)
}

fn parse_entries(index_prefix: &str, keys: Vec<DocKeyEntry>) -> Vec<TimeIndexEntry> {
    keys.into_iter()
        .filter_map(|entry| parse_entry(index_prefix, entry.key, entry.docs_author))
        .collect()
}

/// `<index prefix><20 桁>-<object id>/<object id>` を分解する。形が違う key は捨てる。
fn parse_entry(
    index_prefix: &str,
    key: String,
    docs_author: Option<String>,
) -> Option<TimeIndexEntry> {
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
        docs_author,
    })
}
