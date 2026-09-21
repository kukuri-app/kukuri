//! 時系列の索引(`indexes/timeline/…`・`indexes/thread/<root>/…`)を、上限つきで読む(#1239)。
//!
//! 索引の key は `<index prefix><created_at を 20 桁で 0 埋め>-<object id>/<object id>`。20 桁の 0 埋めなので、
//! 10 進の桁の prefix がそのまま時間の範囲になる。iroh-docs の query は prefix・key 順・`limit` しか
//! 持たず、「この key より古い側」「この key より新しい側」という範囲指定が無い。そこで、cursor の時刻の桁を
//! 下の桁から順に 1 つずつ減らした(新しい側へ読むときは増やした)prefix を cursor に近い側からたどり、
//! cursor より先の entry を順に集める。1 回の呼び出しの query 数は定数以下で、replica の総 entry 数に依存しない。
//!
//! 索引の key は、その replica に書ける誰もが置ける。形の違う key と、未来の時刻の key への耐性は
//! best effort とする(読み出しごとに固定の余裕を持ち、余裕を超えたら prefix を細かく分けて読み直す。
//! query 数には上限を置き、達したら続きの起点を返す)。
//! 有効な形の key を大量に置く書き込みは、この層では防げない。

use anyhow::Result;
use kukuri_core::ReplicaId;

use crate::types::{DocKeyEntry, DocKeyOrder, DocKeyQuery, DocsSync};

/// 索引の時刻部分の桁数(`kukuri_core::timeline_sort_key` の 0 埋めと同じ)。
const TIME_DIGITS: usize = 20;
/// 同じ秒の中で cursor より先の entry を探すときの読み出しの上限。
/// 1 秒に同じ索引へこの件数を超える投稿があると、超えた分は取りこぼしうる(best effort)。
const SAME_SECOND_SCAN_LIMIT: usize = 512;
/// 1 回の読み出しで、形の違う key が占めてよい枠。`limit` に足して読む。
const MALFORMED_KEY_ALLOWANCE: usize = 32;
/// 窓の読み出しで、未来の時刻の entry が占めてよい枠。超えたら cursor つきの読み出しへ落ちる。
const FUTURE_ENTRY_ALLOWANCE: usize = 32;
/// 1 回の読み出しで発行する query 数の上限。通常は時刻の桁から決まる数十回以下で、形の違う key を避けるために
/// prefix を細かく分けたときだけ増える。上限に達したら、そこまでに読めた分と、続きの起点を返す。
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

/// 読み出しの起点。古い側へ読むときはこの位置より古い entry だけを、新しい側へ読むときはこの位置より新しい
/// entry だけを返す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeIndexCursor {
    pub created_at: i64,
    pub object_id: String,
}

impl TimeIndexCursor {
    /// 古い側へ読むときの起点。`created_at` 以前の entry をすべて含む(`created_at` の秒の entry も含む)。
    pub fn not_after(created_at: i64) -> Self {
        Self {
            created_at: created_at.saturating_add(1),
            object_id: String::new(),
        }
    }

    /// 新しい側へ読むときの起点。`created_at` 以後の entry をすべて含む(object id は空にならないので、
    /// `created_at` の秒の entry も含む)。
    pub fn not_before(created_at: i64) -> Self {
        Self {
            created_at,
            object_id: String::new(),
        }
    }
}

/// 読み出し 1 回の結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimeIndexPage {
    /// 読んだ向きに並んだ entry。件数は `limit` 以下。
    pub entries: Vec<TimeIndexEntry>,
    /// query 数の上限に達して、`limit` 件に届く前に打ち切ったときの、続きの起点。同じ向きの読み出しへ渡すと、
    /// 打ち切った位置から続ける。
    ///
    /// `None` で `entries` が `limit` 件に満たなければ、その向きの entry は尽きている。`entries` の件数だけで
    /// 「尽きた」と判定してはならない(形の違う key が並んだ範囲では、1 件も読めないまま上限に達する)。
    pub resume: Option<TimeIndexCursor>,
    /// この読み出しが発行した key の一覧の query の数。呼び出し側が、読み出しの重さに応じて次の読み出しまでの
    /// 間隔を決めるために使う。
    pub queries: usize,
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
) -> Result<TimeIndexPage> {
    if limit == 0 {
        return Ok(TimeIndexPage::default());
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
        return Ok(TimeIndexPage {
            entries,
            resume: None,
            queries: 1,
        });
    }
    let mut page = walk_time_index(
        docs_sync,
        replica_id,
        index_prefix,
        DocKeyOrder::Descending,
        Some(&TimeIndexCursor::not_after(not_after)),
        limit,
    )
    .await?;
    page.queries += 1;
    Ok(page)
}

/// `index_prefix`(末尾が `/`)の索引から、新しい順に最大 `limit` 件を返す。
/// `before` があれば、その位置より古い entry だけを返す。
pub async fn query_time_index_desc(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    index_prefix: &str,
    before: Option<&TimeIndexCursor>,
    limit: usize,
) -> Result<TimeIndexPage> {
    walk_time_index(
        docs_sync,
        replica_id,
        index_prefix,
        DocKeyOrder::Descending,
        before,
        limit,
    )
    .await
}

/// `index_prefix`(末尾が `/`)の索引から、古い順に最大 `limit` 件を返す。
/// `after` があれば、その位置より新しい entry だけを返す。
pub async fn query_time_index_asc(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    index_prefix: &str,
    after: Option<&TimeIndexCursor>,
    limit: usize,
) -> Result<TimeIndexPage> {
    walk_time_index(
        docs_sync,
        replica_id,
        index_prefix,
        DocKeyOrder::Ascending,
        after,
        limit,
    )
    .await
}

/// 索引の、読む向きの端。端より先の prefix は読まない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IndexEdge {
    /// まだ調べていない、または形の違う key に埋まっていて分からない。
    Unknown,
    /// その向きで最後の有効な entry の時刻。
    At(u64),
    /// 索引に有効な entry が 1 件も無い。
    Empty,
}

/// key の一覧。`docs_author` があれば、その docs author の entry だけを読む(ADR 0053 §6)。
async fn list_index_keys(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    docs_author: Option<&str>,
    query: DocKeyQuery,
) -> Result<crate::types::DocKeyPage> {
    match docs_author {
        Some(docs_author) => {
            docs_sync
                .query_replica_keys_by_author(replica_id, docs_author, query)
                .await
        }
        None => docs_sync.query_replica_keys(replica_id, query).await,
    }
}

/// `query_time_index_desc` と同じ読み出しを、`docs_author` の entry だけで行う(ADR 0053 §6)。他の名義の key は、
/// 何件あっても読まず、ページの枠を埋めない。著者だけが書く索引(プロフィールの索引)に使う。
pub async fn query_time_index_desc_by_author(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    index_prefix: &str,
    docs_author: &str,
    before: Option<&TimeIndexCursor>,
    limit: usize,
) -> Result<TimeIndexPage> {
    walk_time_index_as(
        docs_sync,
        replica_id,
        index_prefix,
        Some(docs_author),
        DocKeyOrder::Descending,
        before,
        limit,
    )
    .await
}

async fn walk_time_index(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    index_prefix: &str,
    order: DocKeyOrder,
    cursor: Option<&TimeIndexCursor>,
    limit: usize,
) -> Result<TimeIndexPage> {
    walk_time_index_as(
        docs_sync,
        replica_id,
        index_prefix,
        None,
        order,
        cursor,
        limit,
    )
    .await
}

async fn walk_time_index_as(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    index_prefix: &str,
    docs_author: Option<&str>,
    order: DocKeyOrder,
    cursor: Option<&TimeIndexCursor>,
    limit: usize,
) -> Result<TimeIndexPage> {
    if limit == 0 {
        return Ok(TimeIndexPage::default());
    }
    let mut entries = Vec::new();
    // 同じ秒の読み出しと、索引の端の読み出しも数える。`MAX_WALK_QUERIES` は prefix の読み出しと端の読み出しに掛かる。
    let mut same_second_queries = 0usize;
    // 時刻の prefix の stack。後ろから取り出すので、読む順の逆に積む。空の prefix は索引の全体。
    let mut pending: Vec<String> = Vec::new();
    match cursor {
        // 索引の時刻は 0 以上。負の位置より古い entry は無く、負の位置より新しい entry は索引の全体。
        Some(cursor) if cursor.created_at < 0 => match order {
            DocKeyOrder::Descending => return Ok(TimeIndexPage::default()),
            DocKeyOrder::Ascending => pending.push(String::new()),
        },
        Some(cursor) => {
            let digits = format!("{:0width$}", cursor.created_at, width = TIME_DIGITS);
            if digits.len() != TIME_DIGITS {
                return Ok(TimeIndexPage::default());
            }
            // 同じ秒の中で、cursor より先の object id を持つ entry。
            same_second_queries += 1;
            let same_second = list_index_keys(
                docs_sync,
                replica_id,
                docs_author,
                DocKeyQuery {
                    prefix: format!("{index_prefix}{digits}-"),
                    order,
                    limit: SAME_SECOND_SCAN_LIMIT,
                },
            )
            .await?;
            entries.extend(
                parse_entries(index_prefix, same_second.entries)
                    .into_iter()
                    .filter(|entry| match order {
                        DocKeyOrder::Descending => {
                            entry.object_id.as_str() < cursor.object_id.as_str()
                        }
                        DocKeyOrder::Ascending => {
                            entry.object_id.as_str() > cursor.object_id.as_str()
                        }
                    })
                    .take(limit),
            );
            // 下の桁から順に、その桁を 1 つずつ動かした prefix を、cursor に近い側から順に読む。
            let digit_bytes = digits.as_bytes();
            for position in 0..TIME_DIGITS {
                let digit = digit_bytes[position] - b'0';
                match order {
                    DocKeyOrder::Descending => {
                        for smaller in 0..digit {
                            pending.push(format!("{}{smaller}", &digits[..position]));
                        }
                    }
                    DocKeyOrder::Ascending => {
                        // 先頭の桁が 0 でない 20 桁の時刻は、索引の時刻(符号つき 64 bit)に収まらない。
                        if position == 0 {
                            continue;
                        }
                        for larger in ((digit + 1)..=9).rev() {
                            pending.push(format!("{}{larger}", &digits[..position]));
                        }
                    }
                }
            }
        }
        None => pending.push(String::new()),
    }

    let mut queries = 0usize;
    let mut resume = None;
    let mut edge = IndexEdge::Unknown;
    let mut edge_checked = false;
    while let Some(time_prefix) = pending.pop() {
        if entries.len() >= limit {
            break;
        }
        let (prefix_min, prefix_max) = time_prefix_bounds(time_prefix.as_str());
        if prefix_min > i64::MAX as u64 {
            // この prefix の時刻は符号つき 64 bit に収まらず、有効な entry は在りえない。読まない。
            // 古い順では、残りの prefix もすべて同じなので、索引は尽きている(続きの起点を作ると、
            // `i64::MAX` に張り付いて読み進めが先へ進まなくなる)。
            match order {
                DocKeyOrder::Descending => continue,
                DocKeyOrder::Ascending => break,
            }
        }
        if let IndexEdge::At(edge) = edge {
            let past_the_edge = match order {
                DocKeyOrder::Descending => prefix_max < edge,
                DocKeyOrder::Ascending => prefix_min > edge,
            };
            if past_the_edge {
                // 残りの prefix は、すべて索引の端より先にある。
                break;
            }
        }
        if queries >= MAX_WALK_QUERIES {
            resume = Some(match order {
                DocKeyOrder::Descending => TimeIndexCursor::not_after(clamp_time(prefix_max)),
                DocKeyOrder::Ascending => TimeIndexCursor::not_before(clamp_time(prefix_min)),
            });
            break;
        }
        let needed = limit - entries.len();
        let requested = needed.saturating_add(MALFORMED_KEY_ALLOWANCE);
        let page = list_index_keys(
            docs_sync,
            replica_id,
            docs_author,
            DocKeyQuery {
                prefix: format!("{index_prefix}{time_prefix}"),
                order,
                limit: requested,
            },
        )
        .await?;
        queries += 1;
        let truncated = page.reached_limit;
        let valid = parse_entries(index_prefix, page.entries);
        if truncated && valid.len() < needed && time_prefix.len() < TIME_DIGITS {
            // 形の違う key が余裕を超えて枠を埋めた。この prefix には、読めていない有効な entry が残りうる。
            // 先の prefix へ進むと、その entry を飛ばしたページを返してしまうので、1 桁細かい prefix に
            // 分けて読み直す。1 秒まで分けても埋まっている場合は、その秒だけを諦める。
            if time_prefix.is_empty() {
                // 先頭の桁が 0 でない 20 桁の時刻は、索引の時刻(符号つき 64 bit)に収まらない。
                pending.push("0".to_string());
                continue;
            }
            match order {
                DocKeyOrder::Descending => {
                    for digit in 0..=9 {
                        pending.push(format!("{time_prefix}{digit}"));
                    }
                }
                DocKeyOrder::Ascending => {
                    for digit in (0..=9).rev() {
                        pending.push(format!("{time_prefix}{digit}"));
                    }
                }
            }
            continue;
        }
        if valid.is_empty() && !truncated && !edge_checked && queries < MAX_WALK_QUERIES {
            // entry の無い範囲へ出た。索引の端を 1 回だけ調べ、端より先の prefix を読まない。
            // 調べないと、entry が尽きた後も、残りの桁の prefix を空振りで読み続ける。
            edge_checked = true;
            queries += 1;
            edge = index_edge(docs_sync, replica_id, index_prefix, docs_author, order).await?;
            if edge == IndexEdge::Empty {
                break;
            }
        }
        entries.extend(valid);
    }
    entries.truncate(limit);
    Ok(TimeIndexPage {
        entries,
        resume,
        queries: queries + same_second_queries,
    })
}

/// 索引の、読む向きの端(その向きで最後の有効な entry の時刻)を、逆向きの読み出し 1 回で調べる。
///
/// 逆向きに読んだ最初の有効な entry より先には、有効な entry は無い。形の違う key が余裕を超えて端を
/// 埋めているときは分からないので、`Unknown` を返す(呼び出し側は端を使わずに読み続ける)。
/// 読み出しが打ち切られずに有効な entry が 1 件も無ければ、索引は空(`Empty`)。
async fn index_edge(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    index_prefix: &str,
    docs_author: Option<&str>,
    order: DocKeyOrder,
) -> Result<IndexEdge> {
    let opposite = match order {
        DocKeyOrder::Descending => DocKeyOrder::Ascending,
        DocKeyOrder::Ascending => DocKeyOrder::Descending,
    };
    let page = list_index_keys(
        docs_sync,
        replica_id,
        docs_author,
        DocKeyQuery {
            prefix: index_prefix.to_string(),
            order: opposite,
            limit: MALFORMED_KEY_ALLOWANCE.saturating_add(1),
        },
    )
    .await?;
    let truncated = page.reached_limit;
    let edge = parse_entries(index_prefix, page.entries)
        .first()
        .and_then(|entry| u64::try_from(entry.created_at).ok());
    Ok(match edge {
        Some(created_at) => IndexEdge::At(created_at),
        None if truncated => IndexEdge::Unknown,
        None => IndexEdge::Empty,
    })
}

/// 時刻の prefix が表す範囲の両端(20 桁まで 0 と 9 を詰めた値)。
fn time_prefix_bounds(time_prefix: &str) -> (u64, u64) {
    let pad = TIME_DIGITS.saturating_sub(time_prefix.len());
    let min = format!("{time_prefix}{}", "0".repeat(pad));
    let max = format!("{time_prefix}{}", "9".repeat(pad));
    (
        min.parse::<u64>().unwrap_or(0),
        max.parse::<u64>().unwrap_or(u64::MAX),
    )
}

fn clamp_time(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
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
    // 時刻は 20 桁の数字だけ。符号つきの表記(`+…`・`-…`)は、整数としては読めても索引の形ではない。
    if time.len() != TIME_DIGITS
        || !time.bytes().all(|byte| byte.is_ascii_digit())
        || sort_object_id != object_id
        || object_id.is_empty()
    {
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
