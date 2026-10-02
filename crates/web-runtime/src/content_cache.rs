//! 本人の書込みの保護と remote の内容の cache を IndexedDB に置く（ADR 0058 §2〜§4）。
//!
//! native の `SqliteStore` と同じ意味・同じ定数で、account ごとの database `kukuri-cache-v1-<公開鍵の hex>` に置く。
//! - `contents`: key は `[kind, key]`。長さ・`scope`・最後に使った時刻・容量の計数・保護の有無。非保護の行だけが
//!   `reclaim`（回収の順）と `adult`（成人向けの回収）の索引に載る。docs の record（`kind` が `record`、key は native と
//!   同じ `<replica>\0<key>\0<author>`）の行だけが `listing`（`[replica, key, author]`）と `by_author`
//!   （`[replica, author, key]`）の索引に載る（ADR 0058 §7）。
//! - `chunks`: key は `[kind, key, 連番]`。最大 1 MiB の bytes。
//! - `refs`: key は `[保護参照, kind, key]`。`item`（`[kind, key]`）の索引で参照の有無を数える。
//! - `meta`: 非保護分の合計 bytes（`usage`）。
//!
//! 1 つの内容は 1 つの readwrite transaction で書き、確定した時点を完成とする。中断した transaction は何も残さない。
//! quota 超過・中断で書けなければ 1 処理だけ回収して 1 回だけ書き直し、それでも書けなければ失敗を返す。chunk の欠けた
//! （破損した）内容は完成と扱わず、消して取り直させる。非保護分の上限は quota の半分と 3 GiB の小さい方（ADR 0058 §4）。
//! 起動時に内容を読まず、key を指定して読む。回収は索引を古い順に 1 処理 `REMOTE_CACHE_RECLAIM_STEP` 件まで歩く。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow, ensure};
use async_trait::async_trait;
use js_sys::{Array, Object, Reflect, Uint8Array};
use kukuri_store::{
    ContentCacheStore, REMOTE_CACHE_CAPACITY_BYTES, REMOTE_CACHE_RECLAIM_STEP,
    REMOTE_CACHE_TOUCH_INTERVAL_MS, REMOTE_CACHE_UNUSED_MS, RemoteCacheReservation,
    RemoteRecordKey,
};
use tokio::sync::{broadcast, mpsc, oneshot};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    IdbCursorDirection, IdbDatabase, IdbKeyRange, IdbObjectStore, IdbObjectStoreParameters,
    IdbTransaction, IdbTransactionMode, StorageEstimate,
};

use crate::idb::{self, js_error};

mod tx;
use tx::{RecordMeta, Tx, delete_step, make_room, prefix_upper_bound, put_content, record_key};

const VERSION: u32 = 1;
/// 1 つの chunk の上限（`/kukuri/remote-blob/1` の 1 回の単位と同じ）。
const CHUNK_BYTES: usize = 1024 * 1024;
/// 処理待ちの操作の上限。超えた呼出元は空くまで待つ。
const QUEUE: usize = 64;
const STORES: [&str; 4] = ["contents", "chunks", "refs", "meta"];
const USAGE: &str = "usage";
/// 表示設定 ON の間に置いた成人向けの blob の `scope`（native の `REMOTE_ADULT_BLOB_SCOPE`）。
const ADULT: &str = "adult";
/// 本人が書いた docs record の保護参照（native の `OWN_DOCS_REF`）。
const OWN_DOCS: &str = "own_docs";
/// 1 回の key 指定の record の読み出しの上限（native と同じ）。
const EXACT_RECORDS: usize = 8;

type Job = Box<dyn FnOnce(IdbDatabase) -> Pin<Box<dyn Future<Output = ()>>> + Send>;

/// IndexedDB の保存 trait の実装。database は 1 つの task が持ち、操作を channel で受けて 1 つずつ行う
/// （native の `remote_cache_gate` と同じ直列化。ADR 0056 §4）。drop で database を閉じる。
pub struct IndexedDbCache {
    jobs: mpsc::Sender<Job>,
    reserved: Arc<AtomicU64>,
    evictions: broadcast::Sender<String>,
    /// 非保護分の上限。起動時の quota の半分と 3 GiB の小さい方（ADR 0058 §4）。
    capacity: i64,
}

impl IndexedDbCache {
    /// `account`（公開鍵の hex）の database を開く。内容は読まない。
    pub async fn open(account: &str) -> Result<Self> {
        Self::start(account, None).await
    }

    /// `capacity` を渡さなければ、quota から決める。
    pub(crate) async fn start(account: &str, capacity: Option<i64>) -> Result<Self> {
        let name = format!("kukuri-cache-v1-{account}");
        let (jobs, mut queue) = mpsc::channel::<Job>(QUEUE);
        let (opened, receiver) = oneshot::channel();
        n0_future::task::spawn(async move {
            let opening = async {
                let capacity = match capacity {
                    Some(capacity) => capacity,
                    None => quota_capacity().await?,
                };
                anyhow::Ok((idb::open(&name, VERSION, create_stores).await?, capacity))
            };
            let db = match opening.await {
                Ok((db, capacity)) => {
                    let _ = opened.send(Ok(capacity));
                    db
                }
                Err(error) => {
                    let _ = opened.send(Err(error));
                    return;
                }
            };
            while let Some(job) = queue.recv().await {
                job(db.clone()).await;
            }
            db.close();
        });
        let capacity = receiver
            .await
            .map_err(|_| anyhow!("indexeddb open dropped"))??;
        Ok(Self {
            jobs,
            reserved: Arc::default(),
            evictions: broadcast::channel(64).0,
            capacity,
        })
    }

    /// 操作を database の task で行う。呼出元が待つのをやめても、始めた transaction は最後まで進む。
    async fn run<T, F, Fut>(&self, job: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(IdbDatabase) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let job: Job = Box::new(move |db| {
            Box::pin(async move {
                let _ = sender.send(job(db).await);
            })
        });
        let closed = || anyhow!("the browser cache is closed");
        self.jobs.send(job).await.map_err(|_| closed())?;
        receiver.await.map_err(|_| closed())?
    }

    /// 内容を 1 つ置く。偽は、容量に収まらなかったこと。
    async fn put(
        &self,
        (kind, key, scope): (&str, &str, &str),
        payload: &[u8],
        record: Option<RecordMeta>,
    ) -> Result<bool> {
        let (kind, key, scope) = (kind.to_owned(), key.to_owned(), scope.to_owned());
        let payload = payload.to_vec();
        let (reserved, capacity) = (self.reserved.clone(), self.capacity);
        self.run(move |db| async move {
            let reserved = i64::try_from(reserved.load(Ordering::Acquire))?;
            let budget = capacity.saturating_sub(reserved);
            let item = (kind.as_str(), key.as_str(), scope.as_str());
            match put_content(&db, item, &payload, record.clone(), budget, now_ms()?).await {
                // quota 超過・中断で書けなかったときは、1 処理だけ回収して 1 回だけ書き直す。それでも書けなければ
                // 保存の失敗として返す（ADR 0058 §4）。中断した transaction は何も残していない。
                Err(_) => {
                    make_room(&db, i64::try_from(payload.len())?).await?;
                    put_content(&db, item, &payload, record, budget, now_ms()?).await
                }
                stored => stored,
            }
        })
        .await
    }

    /// 内容の chunk ごとの bytes（試験で chunk の上限を確かめる）。
    #[cfg(test)]
    pub(crate) async fn chunk_sizes(&self, kind: &str, key: &str) -> Result<Vec<usize>> {
        let (kind, key) = (kind.to_owned(), key.to_owned());
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            let request = tx
                .chunks
                .get_all_with_key(&chunk_range(&kind, &key, 0, u64::from(u32::MAX))?)
                .map_err(js_error)?;
            let pieces: Array = idb::done(&request).await?.unchecked_into();
            Ok(pieces
                .iter()
                .map(|piece| Uint8Array::new(&piece).length() as usize)
                .collect())
        })
        .await
    }

    /// 最後に使った時刻を `used` として内容を置く（試験で失効した行を作る）。
    #[cfg(test)]
    pub(crate) async fn put_used_at(&self, key: &str, payload: &[u8], used: i64) -> Result<bool> {
        let (key, payload, capacity) = (key.to_owned(), payload.to_vec(), self.capacity);
        self.run(move |db| async move {
            put_content(&db, ("blob", &key, "blob"), &payload, None, capacity, used).await
        })
        .await
    }

    /// 内容の 2 つ目の chunk を消す（試験で破損を作る）。
    #[cfg(test)]
    pub(crate) async fn drop_second_chunk(&self, key: &str) -> Result<()> {
        let key = key.to_owned();
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            tx.chunks
                .delete(&chunk_key("blob", &key, 1))
                .map_err(js_error)?;
            tx.commit().await
        })
        .await
    }
}

async fn quota_capacity() -> Result<i64> {
    let estimate = web_sys::window()
        .ok_or_else(|| anyhow!("the storage estimate needs a window"))?
        .navigator()
        .storage()
        .estimate()
        .map_err(js_error)?;
    let estimate: StorageEstimate = wasm_bindgen_futures::JsFuture::from(estimate)
        .await
        .map_err(js_error)?
        .unchecked_into();
    Ok(estimate
        .get_quota()
        .map_or(REMOTE_CACHE_CAPACITY_BYTES, |quota| {
            REMOTE_CACHE_CAPACITY_BYTES.min((quota / 2.0) as i64)
        }))
}

fn create_stores(db: &IdbDatabase) -> std::result::Result<(), JsValue> {
    let in_line = |path: &[&str]| {
        let parameters = IdbObjectStoreParameters::new();
        parameters.set_key_path(&strings(path));
        parameters
    };
    let contents =
        db.create_object_store_with_optional_parameters("contents", &in_line(&["kind", "key"]))?;
    contents.create_index_with_str("reclaim", "reclaim")?;
    contents.create_index_with_str("adult", "adult")?;
    contents.create_index_with_str_sequence("listing", &strings(&["replica", "rkey", "author"]))?;
    contents
        .create_index_with_str_sequence("by_author", &strings(&["replica", "author", "rkey"]))?;
    db.create_object_store("chunks")?;
    db.create_object_store_with_optional_parameters(
        "refs",
        &in_line(&["reference", "kind", "key"]),
    )?
    .create_index_with_str_sequence("item", &strings(&["kind", "key"]))?;
    db.create_object_store("meta")?;
    Ok(())
}

fn strings(parts: &[&str]) -> JsValue {
    parts
        .iter()
        .map(|part| JsValue::from_str(part))
        .collect::<Array>()
        .into()
}

fn chunk_key(kind: &str, key: &str, seq: u64) -> JsValue {
    Array::of3(&kind.into(), &key.into(), &(seq as f64).into()).into()
}

fn chunk_range(kind: &str, key: &str, first: u64, last: u64) -> Result<JsValue> {
    Ok(
        IdbKeyRange::bound(&chunk_key(kind, key, first), &chunk_key(kind, key, last))
            .map_err(js_error)?
            .into(),
    )
}

fn now_ms() -> Result<i64> {
    Ok(i64::try_from(
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)?
            .as_millis(),
    )?)
}

#[async_trait]
impl ContentCacheStore for IndexedDbCache {
    fn remote_cache_capacity(&self) -> u64 {
        self.capacity as u64
    }

    fn subscribe_adult_label_evictions(&self) -> broadcast::Receiver<String> {
        self.evictions.subscribe()
    }

    fn empty_remote_cache_reservation(&self) -> RemoteCacheReservation {
        RemoteCacheReservation {
            counter: self.reserved.clone(),
            bytes: 0,
        }
    }

    async fn reserve_remote_cache_bytes(
        &self,
        reservation: &mut RemoteCacheReservation,
        bytes: u64,
    ) -> Result<bool> {
        ensure!(
            Arc::ptr_eq(&reservation.counter, &self.reserved),
            "reservation belongs to another cache"
        );
        let (reserved, capacity) = (self.reserved.clone(), self.remote_cache_capacity());
        // 予約は database の task で数え、待つのをやめた呼出元の分は返された予約の drop で戻る。
        let granted = self
            .run(move |db| async move {
                let target = reserved.load(Ordering::Acquire).saturating_add(bytes);
                if target > capacity {
                    return Ok(None);
                }
                let limit = i64::try_from(capacity - target)?;
                let tx = Tx::begin(&db)?;
                let mut used = tx.used().await?;
                tx.evict("reclaim", None, &mut used, |_, used| used > limit)
                    .await?;
                tx.set_used(used)?;
                tx.commit().await?;
                if used > limit {
                    return Ok(None);
                }
                reserved.fetch_add(bytes, Ordering::AcqRel);
                #[cfg(test)]
                test_hooks::reserved().notify_one();
                Ok(Some(RemoteCacheReservation {
                    counter: reserved,
                    bytes,
                }))
            })
            .await?;
        let Some(mut granted) = granted else {
            return Ok(false);
        };
        reservation.bytes += std::mem::take(&mut granted.bytes);
        Ok(true)
    }

    async fn put_remote_content(
        &self,
        kind: &str,
        key: &str,
        scope: &str,
        payload: &[u8],
    ) -> Result<bool> {
        self.put((kind, key, scope), payload, None).await
    }

    async fn get_remote_content(&self, kind: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let (kind, key) = (kind.to_owned(), key.to_owned());
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            let bytes = match tx.row(&kind, &key).await? {
                Some(row) => tx.payload(row, now_ms()?).await?,
                None => None,
            };
            tx.commit().await?;
            Ok(bytes)
        })
        .await
    }

    async fn has_remote_content(&self, kind: &str, key: &str) -> Result<bool> {
        let (kind, key) = (kind.to_owned(), key.to_owned());
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            let usable = tx.usable_row(&kind, &key, now_ms()?, false).await?;
            tx.commit().await?;
            Ok(usable.is_some())
        })
        .await
    }

    async fn remote_content_len(&self, kind: &str, key: &str) -> Result<Option<u64>> {
        let (kind, key) = (kind.to_owned(), key.to_owned());
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            let len = tx
                .usable_row(&kind, &key, now_ms()?, true)
                .await?
                .map(|row| row.len);
            tx.commit().await?;
            Ok(len)
        })
        .await
    }

    async fn remote_content_chunk(
        &self,
        kind: &str,
        key: &str,
        offset: u64,
        limit: usize,
    ) -> Result<Option<Vec<u8>>> {
        ensure!(limit <= CHUNK_BYTES, "remote cache chunk limit exceeded");
        let (kind, key) = (kind.to_owned(), key.to_owned());
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            let Some(row) = tx.row(&kind, &key).await? else {
                return Ok(None);
            };
            let end = row.len.min(offset.saturating_add(limit as u64));
            if offset >= end {
                return Ok(Some(Vec::new()));
            }
            let chunk = CHUNK_BYTES as u64;
            let (first, last) = (offset / chunk, (end - 1) / chunk);
            let bytes = tx.read_chunks(&kind, &key, first, last).await?;
            let start = usize::try_from(offset - first * chunk)?;
            let stop = usize::try_from(end - first * chunk)?.min(bytes.len());
            Ok(Some(bytes.get(start..stop).unwrap_or_default().to_vec()))
        })
        .await
    }

    async fn put_remote_record(
        &self,
        replica: &str,
        key: &str,
        author: &str,
        payload: &[u8],
    ) -> Result<bool> {
        let record = RecordMeta {
            replica: replica.to_owned(),
            ..serde_json::from_slice(payload)?
        };
        ensure!(
            record.rkey == key && record.author == author,
            "record payload does not match its key"
        );
        let cache_key = record_key(replica, key, author);
        self.put(("record", &cache_key, replica), payload, Some(record))
            .await
    }

    async fn get_remote_records(
        &self,
        replica: &str,
        key: &str,
        author: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Vec<u8>>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        ensure!(limit <= EXACT_RECORDS, "remote record cache limit exceeded");
        let (replica, key) = (replica.to_owned(), key.to_owned());
        let author = author.map(str::to_owned);
        self.run(move |db| async move {
            let now = now_ms()?;
            let tx = Tx::begin(&db)?;
            let rows = match &author {
                Some(author) => {
                    let cache_key = record_key(&replica, &key, author);
                    tx.row("record", &cache_key).await?.into_iter().collect()
                }
                None => {
                    let lower = Array::of2(&replica.as_str().into(), &key.as_str().into());
                    let upper = Array::of3(
                        &replica.as_str().into(),
                        &key.as_str().into(),
                        &Array::new(),
                    );
                    let range = IdbKeyRange::bound(&lower, &upper).map_err(js_error)?;
                    tx.rows("listing", &range, IdbCursorDirection::Next, limit)
                        .await?
                }
            };
            let mut records = Vec::with_capacity(rows.len());
            for row in rows {
                records.extend(tx.payload(row, now).await?);
            }
            tx.commit().await?;
            Ok(records)
        })
        .await
    }

    /// `replica` の `prefix` で始まる record を、(key, author)の順（`descending` なら逆順）に `limit` 件まで返す。
    /// 索引の範囲を `limit + 1` 行だけ読み、最後に使った時刻を変えない（native と同じ）。
    async fn remote_record_keys(
        &self,
        replica: &str,
        prefix: &str,
        descending: bool,
        author: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<RemoteRecordKey>, bool)> {
        let (replica, prefix) = (replica.to_owned(), prefix.to_owned());
        let author = author.map(str::to_owned);
        self.run(move |db| async move {
            let (index, scope) = match &author {
                Some(author) => (
                    "by_author",
                    vec![replica.as_str().into(), author.as_str().into()],
                ),
                None => ("listing", vec![JsValue::from_str(&replica)]),
            };
            let bound = |last: JsValue| {
                let parts = scope.iter().cloned().collect::<Array>();
                parts.push(&last);
                parts
            };
            let range = IdbKeyRange::bound_with_lower_open_and_upper_open(
                &bound(prefix.as_str().into()),
                &bound(prefix_upper_bound(&prefix)),
                false,
                true,
            )
            .map_err(js_error)?;
            let direction = if descending {
                IdbCursorDirection::Prev
            } else {
                IdbCursorDirection::Next
            };
            let tx = Tx::begin(&db)?;
            let rows = tx.rows(index, &range, direction, limit + 1).await?;
            let reached_limit = rows.len() > limit;
            let now = now_ms()?;
            let keys = rows
                .into_iter()
                .take(limit)
                .filter(|row| row.alive(now))
                .filter_map(|row| row.record)
                .map(|record| RemoteRecordKey {
                    key: record.rkey,
                    author: record.author,
                    content_hash: record.content_hash,
                    content_len: record.content_len,
                })
                .collect();
            Ok((keys, reached_limit))
        })
        .await
    }

    async fn add_protected_ref(&self, reference: &str, kind: &str, key: &str) -> Result<()> {
        let (reference, kind, key) = (reference.to_owned(), kind.to_owned(), key.to_owned());
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            let entry = Object::new();
            for (name, value) in [("reference", &reference), ("kind", &kind), ("key", &key)] {
                Reflect::set(&entry, &name.into(), &value.as_str().into()).map_err(js_error)?;
            }
            tx.refs.put(&entry).map_err(js_error)?;
            // 内容がまだ無くても参照を先に置き、後から置く内容を容量の計数と回収の外にする。
            if let Some(mut row) = tx.row(&kind, &key).await?
                && !row.protected
            {
                tx.set_used(tx.used().await? - row.charge)?;
                row.protected = true;
                tx.put_row(&row)?;
            }
            tx.commit().await
        })
        .await
    }

    async fn put_owned_blob(&self, reference: &str, hash: &str, bytes: &[u8]) -> Result<()> {
        self.add_protected_ref(reference, "blob", hash).await?;
        ensure!(
            self.put_remote_content("blob", hash, "blob", bytes).await?,
            "owned blob was not stored"
        );
        Ok(())
    }

    async fn put_owned_record(
        &self,
        replica: &str,
        key: &str,
        author: &str,
        payload: &[u8],
    ) -> Result<()> {
        let cache_key = record_key(replica, key, author);
        self.add_protected_ref(OWN_DOCS, "record", &cache_key)
            .await?;
        ensure!(
            self.put_remote_record(replica, key, author, payload)
                .await?,
            "owned record was not stored"
        );
        Ok(())
    }

    async fn reclaim_remote_cache_step(&self) -> Result<usize> {
        self.run(|db| async move {
            delete_step(&db, "reclaim", now_ms()? - REMOTE_CACHE_UNUSED_MS).await
        })
        .await
    }

    async fn mark_remote_blob_adult(&self, hash: &str) -> Result<()> {
        let hash = hash.to_owned();
        self.run(move |db| async move {
            let tx = Tx::begin(&db)?;
            if let Some(mut row) = tx.row("blob", &hash).await? {
                row.scope = ADULT.into();
                tx.put_row(&row)?;
            }
            tx.commit().await
        })
        .await
    }

    async fn forget_adult_remote_blobs_step(&self) -> Result<usize> {
        self.run(|db| async move { delete_step(&db, "adult", i64::MAX).await })
            .await
    }
}

/// 試験だけの観測と失敗の注入（ブラウザは 1 thread なので thread local で足りる）。
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::cell::Cell;
    use std::rc::Rc;

    use tokio::sync::Notify;

    thread_local! {
        /// 予約を認めたことの知らせ。
        static RESERVED: Rc<Notify> = Rc::new(Notify::new());
        /// deserialize した `contents` の行の数。
        pub(crate) static ROWS_READ: Cell<usize> = const { Cell::new(0) };
        /// 中断させる内容の書込みの数。
        static WRITE_FAILURES: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn reserved() -> Rc<Notify> {
        RESERVED.with(Rc::clone)
    }

    pub(crate) fn fail_writes(count: usize) {
        WRITE_FAILURES.set(count);
    }

    pub(super) fn take_write_failure() -> bool {
        let remaining = WRITE_FAILURES.get();
        WRITE_FAILURES.set(remaining.saturating_sub(1));
        remaining > 0
    }
}
