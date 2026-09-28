//! 保持している検証済みの record を、key の範囲で一覧する(#1395)。
//!
//! 他の参加者の key 一覧の要求へ、自分の docs namespace の entry と合わせて答えるために使う。
//! 索引 `(scope_key, record_key, record_author)` の範囲を `limit + 1` 行だけ読み、台帳の総件数に比例しない。
//! 一覧は利用ではないので `last_used_at` を更新しない(中継のために保持を延ばさない)。

use super::*;

/// 一覧の 1 行。値は読まず、record の hash と長さだけを返す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRecordKey {
    pub key: String,
    pub author: String,
    pub content_hash: String,
    pub content_len: u64,
}

/// `prefix` で始まる key の上界(これ未満が範囲)。末尾の文字を次の文字に置き換える。
fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let mut chars = prefix.chars();
    let last = chars.next_back()?;
    let next = char::from_u32(last as u32 + 1)?;
    Some(format!("{}{next}", chars.as_str()))
}

impl SqliteStore {
    /// `replica` の `prefix` で始まる record を、(key, author)の順(`descending` なら逆順)に `limit` 件まで返す。
    /// 戻り値の真偽は、範囲に `limit` を超える行があったか。失効した行は返さないが、この判定には数える。
    pub async fn remote_record_keys(
        &self,
        replica: &str,
        prefix: &str,
        descending: bool,
        author: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<RemoteRecordKey>, bool)> {
        let order = if descending { "DESC" } else { "ASC" };
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT record_key, record_author, is_protected, last_used_at, \
             json_extract(CAST(payload AS TEXT), '$.content_hash') AS content_hash, \
             json_extract(CAST(payload AS TEXT), '$.content_len') AS content_len \
             FROM remote_content_cache WHERE kind = 'record' AND scope_key = ",
        );
        query.push_bind(replica);
        query.push(" AND record_key >= ");
        query.push_bind(prefix);
        if let Some(upper) = prefix_upper_bound(prefix) {
            query.push(" AND record_key < ");
            query.push_bind(upper);
        }
        if let Some(author) = author {
            query.push(" AND record_author = ");
            query.push_bind(author);
        }
        query.push(format!(
            " ORDER BY record_key {order}, record_author {order} LIMIT "
        ));
        query.push_bind(i64::try_from(limit.saturating_add(1))?);
        let rows = query.build().fetch_all(&self.pool).await?;
        let reached_limit = rows.len() > limit;
        let expired_before = now_ms()? - REMOTE_CACHE_UNUSED_MS;
        let mut keys = Vec::with_capacity(rows.len().min(limit));
        for row in rows.into_iter().take(limit) {
            let key: String = row.get("record_key");
            let (Some(content_hash), Some(content_len)) = (
                row.get::<Option<String>, _>("content_hash"),
                row.get::<Option<i64>, _>("content_len"),
            ) else {
                continue;
            };
            if !key.starts_with(prefix)
                || (row.get::<i64, _>("is_protected") == 0
                    && row.get::<i64, _>("last_used_at") <= expired_before)
            {
                continue;
            }
            keys.push(RemoteRecordKey {
                key,
                author: row.get("record_author"),
                content_hash,
                content_len: u64::try_from(content_len)?,
            });
        }
        Ok((keys, reached_limit))
    }
}
