//! object store の行の読み書き（ADR 0059 §2）。行は `{ r: <serde の JSON の値>, <索引の値>... }` の形で置き、索引は
//! `r.<欄>` か、行の外に置いた値を引く。読み出しは索引の範囲を cursor で `limit` 件まで読み、全件を読まない。

use anyhow::{Result, anyhow};
use js_sys::{Array, JSON, Object, Reflect};
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsValue;
use web_sys::{IdbCursorDirection, IdbIndex, IdbKeyRange, IdbObjectStore, IdbTransaction};

use crate::idb::{self, StorageFailure, js_error};

/// 行の外に置く索引の値の名前と値。値が `undefined` の名前は置かない（その行は索引に載らない）。
pub(crate) type Extra<'a> = &'a [(&'a str, JsValue)];

pub(crate) fn store(tx: &IdbTransaction, name: &str) -> Result<IdbObjectStore> {
    tx.object_store(name).map_err(js_error)
}

/// 文字列と数の値を並べた key（`[a, b, …]`）。
pub(crate) fn key(parts: &[JsValue]) -> JsValue {
    parts.iter().collect::<Array>().into()
}

pub(crate) fn text(value: &str) -> JsValue {
    JsValue::from_str(value)
}

pub(crate) fn num(value: i64) -> JsValue {
    JsValue::from_f64(value as f64)
}

/// 先頭が `prefix` の key（配列）の範囲。配列は文字列・数より大きいので、`[..prefix, []]` が上界になる。
pub(crate) fn prefix(parts: &[JsValue]) -> Result<IdbKeyRange> {
    let mut upper = parts.to_vec();
    upper.push(Array::new().into());
    IdbKeyRange::bound(&key(parts), &key(&upper)).map_err(js_error)
}

/// `lower` から `upper` までの範囲（両端の開閉を指定する）。
pub(crate) fn between(
    lower: &[JsValue],
    upper: &[JsValue],
    lower_open: bool,
    upper_open: bool,
) -> Result<IdbKeyRange> {
    IdbKeyRange::bound_with_lower_open_and_upper_open(
        &key(lower),
        &key(upper),
        lower_open,
        upper_open,
    )
    .map_err(js_error)
}

/// 範囲の上端を `[..parts, []]`（`parts` で始まるすべての key より大きい値）にする。
pub(crate) fn top(parts: &[JsValue]) -> Vec<JsValue> {
    let mut upper = parts.to_vec();
    upper.push(Array::new().into());
    upper
}

pub(crate) fn encode<T: Serialize>(row: &T) -> Result<JsValue> {
    JSON::parse(&serde_json::to_string(row)?).map_err(js_error)
}

pub(crate) fn decode<T: DeserializeOwned>(value: &JsValue) -> Result<T> {
    let row = Reflect::get(value, &"r".into()).map_err(js_error)?;
    let json = JSON::stringify(&row)
        .map_err(js_error)?
        .as_string()
        .ok_or_else(|| anyhow!(StorageFailure::Corrupt))?;
    serde_json::from_str(&json).map_err(|error| anyhow!(error).context(StorageFailure::Corrupt))
}

/// 行を置く（同じ key の行は置き換える）。確定は transaction の `complete` で待つ。
pub(crate) fn put<T: Serialize>(
    tx: &IdbTransaction,
    name: &str,
    row: &T,
    extra: Extra<'_>,
) -> Result<()> {
    let value = Object::new();
    Reflect::set(&value, &"r".into(), &encode(row)?).map_err(js_error)?;
    for (name, index) in extra {
        if !index.is_undefined() {
            Reflect::set(&value, &(*name).into(), index).map_err(js_error)?;
        }
    }
    store(tx, name)?.put(&value).map_err(js_error)?;
    Ok(())
}

pub(crate) async fn get<T: DeserializeOwned>(
    tx: &IdbTransaction,
    name: &str,
    key: &JsValue,
) -> Result<Option<T>> {
    let value = idb::done(&store(tx, name)?.get(key).map_err(js_error)?).await?;
    (!value.is_undefined()).then(|| decode(&value)).transpose()
}

pub(crate) fn delete(tx: &IdbTransaction, name: &str, key: &JsValue) -> Result<()> {
    store(tx, name)?.delete(key).map_err(js_error)?;
    Ok(())
}

/// 読む元（object store そのもの、またはその索引）。
pub(crate) fn source(tx: &IdbTransaction, name: &str, index: Option<&str>) -> Result<Source> {
    let store = store(tx, name)?;
    Ok(match index {
        Some(index) => Source::Index(store.index(index).map_err(js_error)?),
        None => Source::Store(store),
    })
}

pub(crate) enum Source {
    Store(IdbObjectStore),
    Index(IdbIndex),
}

impl Source {
    pub(crate) fn cursor(&self, range: &JsValue, reverse: bool) -> Result<web_sys::IdbRequest> {
        let direction = if reverse {
            IdbCursorDirection::Prev
        } else {
            IdbCursorDirection::Next
        };
        match self {
            Self::Store(store) => store.open_cursor_with_range_and_direction(range, direction),
            Self::Index(index) => index.open_cursor_with_range_and_direction(range, direction),
        }
        .map_err(js_error)
    }

    pub(crate) async fn count(&self, range: &JsValue) -> Result<usize> {
        let request = match self {
            Self::Store(store) => store.count_with_key(range),
            Self::Index(index) => index.count_with_key(range),
        }
        .map_err(js_error)?;
        Ok(idb::done(&request).await?.as_f64().unwrap_or(0.0) as usize)
    }
}

/// `range` の行を、`reverse` なら降順に `limit` 件まで読む。`keep` が偽の行は数えずに進む（範囲が狭いときだけ使う）。
pub(crate) async fn scan<T: DeserializeOwned>(
    tx: &IdbTransaction,
    name: &str,
    index: Option<&str>,
    range: &IdbKeyRange,
    reverse: bool,
    limit: usize,
) -> Result<Vec<T>> {
    let mut rows = Vec::new();
    if limit == 0 {
        return Ok(rows);
    }
    walk(tx, name, index, range, reverse, |value| {
        rows.push(decode(value)?);
        Ok(rows.len() < limit)
    })
    .await?;
    Ok(rows)
}

/// `range` の行を順に `visit` へ渡し、偽が返ったら止める。`visit` は行の値（`r` を含む object）を受け取る。
pub(crate) async fn walk(
    tx: &IdbTransaction,
    name: &str,
    index: Option<&str>,
    range: &IdbKeyRange,
    reverse: bool,
    mut visit: impl FnMut(&JsValue) -> Result<bool>,
) -> Result<()> {
    let request = source(tx, name, index)?.cursor(range, reverse)?;
    while let Some(cursor) = idb::next(&request).await? {
        if !visit(&cursor.value().map_err(js_error)?)? {
            break;
        }
        cursor.continue_().map_err(js_error)?;
    }
    Ok(())
}

/// 行の外に置いた値（索引の値）。無ければ `undefined`。
pub(crate) fn extra(value: &JsValue, name: &str) -> Result<JsValue> {
    Reflect::get(value, &name.into()).map_err(js_error)
}

pub(crate) async fn count(
    tx: &IdbTransaction,
    name: &str,
    index: Option<&str>,
    range: &IdbKeyRange,
) -> Result<usize> {
    source(tx, name, index)?.count(range).await
}

/// 確定するまで待つ transaction。確定を待たずに捨てたもの（途中の失敗で返ったとき）は中断し、何も残さない。
pub(crate) struct Txn(pub(crate) IdbTransaction);

impl Txn {
    pub(crate) fn begin(
        db: &web_sys::IdbDatabase,
        stores: &[&str],
        mode: idb::Mode,
    ) -> Result<Self> {
        idb::transaction(db, stores, mode).map(Self)
    }

    pub(crate) async fn commit(self) -> Result<()> {
        #[cfg(test)]
        if test_hooks::take_write_failure() {
            self.0.abort().map_err(js_error)?;
        }
        idb::committed(&self.0).await
    }
}

impl std::ops::Deref for Txn {
    type Target = IdbTransaction;

    fn deref(&self) -> &IdbTransaction {
        &self.0
    }
}

impl Drop for Txn {
    fn drop(&mut self) {
        let _ = self.0.abort();
    }
}

/// `JsValue` を `IdbKeyRange` として使う（`only` の範囲）。
pub(crate) fn only(value: &JsValue) -> Result<IdbKeyRange> {
    IdbKeyRange::only(value).map_err(js_error)
}

/// 試験だけの失敗の注入（ブラウザは 1 thread なので thread local で足りる）。
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::cell::Cell;

    thread_local! {
        static WRITE_FAILURES: Cell<usize> = const { Cell::new(0) };
    }

    /// 次の `count` 回の確定の前に transaction を中断させる（quota 超過・中断と同じく何も残らない）。
    pub(crate) fn fail_writes(count: usize) {
        WRITE_FAILURES.set(count);
    }

    pub(super) fn take_write_failure() -> bool {
        let remaining = WRITE_FAILURES.get();
        WRITE_FAILURES.set(remaining.saturating_sub(1));
        remaining > 0
    }
}
