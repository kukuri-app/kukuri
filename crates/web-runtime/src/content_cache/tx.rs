//! 1 つの readwrite transaction の中の処理（`contents` の行・chunk・保護参照・計数の読み書きと回収）。

use super::*;

/// native の `remote_record_cache_key`。
pub(super) fn record_key(replica: &str, key: &str, author: &str) -> String {
    format!("{replica}\0{key}\0{author}")
}

/// `prefix` で始まる key の上界（これ未満が範囲）。無ければ同じ replica のすべての key より大きい値（配列は文字列より
/// 大きい）。
pub(super) fn prefix_upper_bound(prefix: &str) -> JsValue {
    let mut chars = prefix.chars();
    match chars
        .next_back()
        .and_then(|last| char::from_u32(last as u32 + 1))
    {
        Some(next) => format!("{}{next}", chars.as_str()).into(),
        None => Array::new().into(),
    }
}

/// docs record の行だけが持つ、索引と一覧の値（一覧は値を読まずに hash と長さを返す）。
#[derive(Clone, serde::Deserialize)]
pub(super) struct RecordMeta {
    #[serde(skip)]
    pub(super) replica: String,
    #[serde(rename = "key")]
    pub(super) rkey: String,
    #[serde(rename = "docs_author")]
    pub(super) author: String,
    pub(super) content_hash: String,
    pub(super) content_len: u64,
}

/// `contents` の 1 行。
pub(super) struct Row {
    pub(super) kind: String,
    pub(super) key: String,
    pub(super) len: u64,
    pub(super) scope: String,
    pub(super) used: i64,
    pub(super) charge: i64,
    pub(super) protected: bool,
    pub(super) record: Option<RecordMeta>,
}

impl Row {
    pub(super) fn to_js(&self) -> Result<JsValue> {
        let row = Object::new();
        let set = |name: &str, value: JsValue| {
            Reflect::set(&row, &name.into(), &value)
                .map(drop)
                .map_err(js_error)
        };
        set("kind", self.kind.as_str().into())?;
        set("key", self.key.as_str().into())?;
        set("len", (self.len as f64).into())?;
        set("scope", self.scope.as_str().into())?;
        set("used", (self.used as f64).into())?;
        set("charge", (self.charge as f64).into())?;
        set("protected", self.protected.into())?;
        if let Some(record) = &self.record {
            set("replica", record.replica.as_str().into())?;
            set("rkey", record.rkey.as_str().into())?;
            set("author", record.author.as_str().into())?;
            set("hash", record.content_hash.as_str().into())?;
            set("hlen", (record.content_len as f64).into())?;
        }
        // 索引は値のある行だけを載せるので、保護した行は回収の対象に現れない。
        if !self.protected {
            set("reclaim", (self.used as f64).into())?;
            if self.scope == ADULT {
                set("adult", (self.used as f64).into())?;
            }
        }
        Ok(row.into())
    }

    pub(super) fn from_js(value: &JsValue) -> Result<Self> {
        #[cfg(test)]
        test_hooks::ROWS_READ.set(test_hooks::ROWS_READ.get() + 1);
        let get = |name: &str| Reflect::get(value, &name.into()).map_err(js_error);
        let text = |name: &str| {
            get(name)?
                .as_string()
                .ok_or_else(|| anyhow!("cache row {name} is not a string"))
        };
        let number = |name: &str| {
            get(name)?
                .as_f64()
                .ok_or_else(|| anyhow!("cache row {name} is not a number"))
        };
        Ok(Self {
            kind: text("kind")?,
            key: text("key")?,
            len: number("len")? as u64,
            scope: text("scope")?,
            used: number("used")? as i64,
            charge: number("charge")? as i64,
            protected: get("protected")?.is_truthy(),
            record: match get("replica")?.as_string() {
                Some(replica) => Some(RecordMeta {
                    replica,
                    rkey: text("rkey")?,
                    author: text("author")?,
                    content_hash: text("hash")?,
                    content_len: number("hlen")? as u64,
                }),
                None => None,
            },
        })
    }

    pub(super) fn alive(&self, now: i64) -> bool {
        self.protected || self.used > now - REMOTE_CACHE_UNUSED_MS
    }
}

/// 4 つの object store にまたがる readwrite transaction。
pub(super) struct Tx {
    pub(super) transaction: IdbTransaction,
    pub(super) contents: IdbObjectStore,
    pub(super) chunks: IdbObjectStore,
    pub(super) refs: IdbObjectStore,
    pub(super) meta: IdbObjectStore,
}

/// 確定を待たずに捨てた transaction（途中の失敗で返ったとき）は中断し、途中までの書込みを残さない。
/// 確定した後の中断は何もしない。
impl Drop for Tx {
    fn drop(&mut self) {
        let _ = self.transaction.abort();
    }
}

impl Tx {
    pub(super) fn begin(db: &IdbDatabase) -> Result<Self> {
        let transaction = db
            .transaction_with_str_sequence_and_mode(
                &strings(&STORES),
                IdbTransactionMode::Readwrite,
            )
            .map_err(js_error)?;
        let store = |name: &str| transaction.object_store(name).map_err(js_error);
        Ok(Self {
            contents: store("contents")?,
            chunks: store("chunks")?,
            refs: store("refs")?,
            meta: store("meta")?,
            transaction,
        })
    }

    pub(super) async fn commit(self) -> Result<()> {
        idb::committed(&self.transaction).await
    }

    pub(super) async fn used(&self) -> Result<i64> {
        let request = self.meta.get(&USAGE.into()).map_err(js_error)?;
        Ok(idb::done(&request).await?.as_f64().unwrap_or(0.0) as i64)
    }

    pub(super) fn set_used(&self, used: i64) -> Result<()> {
        self.meta
            .put_with_key(&(used as f64).into(), &USAGE.into())
            .map_err(js_error)?;
        Ok(())
    }

    pub(super) async fn row(&self, kind: &str, key: &str) -> Result<Option<Row>> {
        let request = self
            .contents
            .get(&strings(&[kind, key]))
            .map_err(js_error)?;
        let value = idb::done(&request).await?;
        (!value.is_undefined())
            .then(|| Row::from_js(&value))
            .transpose()
    }

    /// 失効しておらず、chunk が揃った行。chunk が欠けた（破損した）内容は完成と扱わず、消して取り直させる
    /// （保護参照は残す）。`touch` なら、最後に使った時刻が古ければ書き直す。
    pub(super) async fn usable_row(
        &self,
        kind: &str,
        key: &str,
        now: i64,
        touch: bool,
    ) -> Result<Option<Row>> {
        let Some(mut row) = self.row(kind, key).await?.filter(|row| row.alive(now)) else {
            return Ok(None);
        };
        let request = self
            .chunks
            .count_with_key(&chunk_range(kind, key, 0, u64::from(u32::MAX))?)
            .map_err(js_error)?;
        let expected = row.len.div_ceil(CHUNK_BYTES as u64) as f64;
        if idb::done(&request).await?.as_f64() != Some(expected) {
            self.forget(&row).await?;
            return Ok(None);
        }
        if touch && row.used <= now - REMOTE_CACHE_TOUCH_INTERVAL_MS {
            row.used = now;
            self.put_row(&row)?;
        }
        Ok(Some(row))
    }

    /// 失効しておらず、長さの揃った内容の bytes。揃っていなければ（破損）消して取り直させる。
    pub(super) async fn payload(&self, mut row: Row, now: i64) -> Result<Option<Vec<u8>>> {
        if !row.alive(now) {
            return Ok(None);
        }
        let bytes = self
            .read_chunks(&row.kind, &row.key, 0, u64::from(u32::MAX))
            .await?;
        if bytes.len() as u64 != row.len {
            self.forget(&row).await?;
            return Ok(None);
        }
        if row.used <= now - REMOTE_CACHE_TOUCH_INTERVAL_MS {
            row.used = now;
            self.put_row(&row)?;
        }
        Ok(Some(bytes))
    }

    /// `index` の `range` の行を、`direction` の順に `limit` 件まで読む。
    pub(super) async fn rows(
        &self,
        index: &str,
        range: &IdbKeyRange,
        direction: IdbCursorDirection,
        limit: usize,
    ) -> Result<Vec<Row>> {
        let request = self
            .contents
            .index(index)
            .map_err(js_error)?
            .open_cursor_with_range_and_direction(range, direction)
            .map_err(js_error)?;
        let mut rows = Vec::new();
        while rows.len() < limit
            && let Some(cursor) = idb::next(&request).await?
        {
            rows.push(Row::from_js(&cursor.value().map_err(js_error)?)?);
            cursor.continue_().map_err(js_error)?;
        }
        Ok(rows)
    }

    /// 内容（行と chunk）を消す。保護参照は残す。
    pub(super) async fn forget(&self, row: &Row) -> Result<()> {
        self.delete(&row.kind, &row.key)?;
        if !row.protected {
            self.set_used(self.used().await? - row.charge)?;
        }
        Ok(())
    }

    pub(super) fn put_row(&self, row: &Row) -> Result<()> {
        self.contents.put(&row.to_js()?).map_err(js_error)?;
        Ok(())
    }

    pub(super) async fn protected(&self, kind: &str, key: &str) -> Result<bool> {
        let request = self
            .refs
            .index("item")
            .map_err(js_error)?
            .count_with_key(&strings(&[kind, key]))
            .map_err(js_error)?;
        Ok(idb::done(&request).await?.as_f64().unwrap_or(0.0) > 0.0)
    }

    pub(super) fn delete(&self, kind: &str, key: &str) -> Result<()> {
        self.contents
            .delete(&strings(&[kind, key]))
            .map_err(js_error)?;
        self.chunks
            .delete(&chunk_range(kind, key, 0, u64::from(u32::MAX))?)
            .map_err(js_error)?;
        Ok(())
    }

    pub(super) fn write_chunks(&self, kind: &str, key: &str, payload: &[u8]) -> Result<()> {
        for (seq, piece) in (0..).zip(payload.chunks(CHUNK_BYTES)) {
            self.chunks
                .put_with_key(&Uint8Array::from(piece).into(), &chunk_key(kind, key, seq))
                .map_err(js_error)?;
        }
        Ok(())
    }

    /// 連番 `first..=last` の chunk をつなげて読む。
    pub(super) async fn read_chunks(
        &self,
        kind: &str,
        key: &str,
        first: u64,
        last: u64,
    ) -> Result<Vec<u8>> {
        let request = self
            .chunks
            .get_all_with_key(&chunk_range(kind, key, first, last)?)
            .map_err(js_error)?;
        let pieces: Array = idb::done(&request).await?.unchecked_into();
        let mut bytes = Vec::new();
        for piece in pieces.iter() {
            bytes.extend(Uint8Array::new(&piece).to_vec());
        }
        Ok(bytes)
    }

    /// `index` を古い順に歩き、`evict` が真を返す行を消す（1 処理 `REMOTE_CACHE_RECLAIM_STEP` 件まで）。
    /// 偽を返した行で止める。`skip` の行は消さずに進む。
    pub(super) async fn evict(
        &self,
        index: &str,
        skip: Option<(&str, &str)>,
        used: &mut i64,
        mut evict: impl FnMut(&Row, i64) -> bool,
    ) -> Result<usize> {
        let request = self
            .contents
            .index(index)
            .map_err(js_error)?
            .open_cursor()
            .map_err(js_error)?;
        let mut count = 0;
        while count < REMOTE_CACHE_RECLAIM_STEP
            && let Some(cursor) = idb::next(&request).await?
        {
            let row = Row::from_js(&cursor.value().map_err(js_error)?)?;
            if skip != Some((row.kind.as_str(), row.key.as_str())) {
                if !evict(&row, *used) {
                    break;
                }
                self.delete(&row.kind, &row.key)?;
                *used -= row.charge;
                count += 1;
            }
            cursor.continue_().map_err(js_error)?;
        }
        Ok(count)
    }
}

/// 1 つの内容を置く（native の `put_remote_content_in_tx`）。偽は、1 処理の回収では容量に収まらなかったこと。
pub(super) async fn put_content(
    db: &IdbDatabase,
    (kind, key, scope): (&str, &str, &str),
    payload: &[u8],
    record: Option<RecordMeta>,
    budget: i64,
    now: i64,
) -> Result<bool> {
    let charge = i64::try_from(payload.len() + kind.len() + key.len() + scope.len())? + 64;
    let tx = Tx::begin(db)?;
    let protected = tx.protected(kind, key).await?;
    if !protected && charge > budget {
        return Ok(false);
    }
    let old_unprotected = tx
        .row(kind, key)
        .await?
        .filter(|row| !row.protected)
        .map_or(0, |row| row.charge);
    let new_charge = if protected { 0 } else { charge };
    let mut used = tx.used().await? - old_unprotected + new_charge;
    let expired = now - REMOTE_CACHE_UNUSED_MS;
    tx.evict("reclaim", Some((kind, key)), &mut used, |row, used| {
        row.used <= expired || used > budget
    })
    .await?;
    if used > budget {
        tx.set_used(used - new_charge + old_unprotected)?;
        tx.commit().await?;
        return Ok(false);
    }
    tx.delete(kind, key)?;
    tx.write_chunks(kind, key, payload)?;
    tx.put_row(&Row {
        kind: kind.into(),
        key: key.into(),
        len: payload.len() as u64,
        scope: scope.into(),
        used: now,
        charge,
        protected,
        record,
    })?;
    tx.set_used(used)?;
    #[cfg(test)]
    if test_hooks::take_write_failure() {
        tx.transaction.abort().map_err(js_error)?;
    }
    tx.commit().await?;
    Ok(true)
}

/// 書けなかった内容の `bytes` の分を、非保護の行を古い順に消して空ける（1 処理 `REMOTE_CACHE_RECLAIM_STEP` 件まで）。
pub(super) async fn make_room(db: &IdbDatabase, bytes: i64) -> Result<()> {
    let tx = Tx::begin(db)?;
    let before = tx.used().await?;
    let mut used = before;
    tx.evict("reclaim", None, &mut used, |_, used| before - used < bytes)
        .await?;
    tx.set_used(used)?;
    tx.commit().await
}

/// `index` の行を 1 処理消す（非利用の回収と、成人向けの回収）。
pub(super) async fn delete_step(
    db: &IdbDatabase,
    index: &str,
    expired_before: i64,
) -> Result<usize> {
    let tx = Tx::begin(db)?;
    let mut used = tx.used().await?;
    let count = tx
        .evict(index, None, &mut used, |row, _| row.used <= expired_before)
        .await?;
    tx.set_used(used)?;
    tx.commit().await?;
    Ok(count)
}
