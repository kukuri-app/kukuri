//! #1211 AC-3: 移行の任意の投稿の履歴。
//!
//! 移行元は、保護所有先の自分の record（保護参照 `own_docs`。すべてを選んだときは、旧形式の移行で守った `own:<id>`
//! も）を索引の順に 1 page ずつ読み、範囲の外（範囲より前の bucket・対象外の replica）は seek で飛ばして行を読まない。
//! 投稿の envelope から本文・添付の blob の hash を集める。
//! 移行先は、page ごとに受けたアカウントの DB の隣の置き場（page の file と blob の部分の file）へ保存し、journal
//! （範囲・続きの位置とその送り元の端末・page 数・反映した数）を進める。本人の端末どうしの同期（#1650）の受ける向きも、
//! 「すべて」の範囲で同じ置き場を使う。反映はそのアカウントの runtime が page ごとに自分の record・
//! blob として保存し（既にある自分の record は上書きしない）、反映した page から消す。範囲の終わりまで反映したら
//! journal も消す。

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Weak};

use anyhow::Result;
use kukuri_core::{
    AccountHistoryCursor, AccountHistoryRecord, AccountTransferFailure as Failure,
    AccountTransferHistory, KukuriEnvelope, MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES, PayloadRef,
    ReplicaId, blob_hash,
};
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};
use kukuri_iroh_node::{AccountHistoryPage, AccountHistoryStaging, DocReadRecord};
use kukuri_store::ContentCacheStore;
use serde::{Deserialize, Serialize};

use crate::ClientHost;
use crate::storage::{delete_file, read_file, write_file};

/// 1 page の record の数。
const PAGE_RECORDS: usize = 64;
/// 1 page の照会の数（範囲の外への seek を含む）。
pub(crate) const PAGE_QUERIES: usize = 8;
/// 本人が書いた record の保護参照（ADR 0058 §7）。
const OWN_DOCS: &str = "own_docs";
/// 旧形式の移行で本人の投稿を守った保護参照（`own:<envelope id>`、#1221 R5-I）。
const LEGACY_OWN: &str = "own:";
/// replica の中の、どの key（ASCII）よりも後ろの位置。
const PAST_KEYS: &str = "\u{FFFF}";

fn at(reference: &str, replica: &str, key: &str) -> AccountHistoryCursor {
    AccountHistoryCursor {
        reference: reference.to_string(),
        replica: replica.to_string(),
        key: key.to_string(),
        author: String::new(),
    }
}

/// 範囲での record の扱い。
enum Place {
    Take,
    /// 範囲の外。この位置へ飛ぶ。
    Seek(AccountHistoryCursor),
    /// 範囲の終わり。
    End,
}

/// `position` の record を、範囲（`since` の時間 bucket から。`None` はすべて）でどう扱うか。`account` は公開鍵の hex。
fn place(position: &AccountHistoryCursor, account: &str, since: Option<u64>) -> Place {
    let (reference, replica) = (position.reference.as_str(), position.replica.as_str());
    let past_replica = || Place::Seek(at(reference, replica, PAST_KEYS));
    if replica.starts_with("bucket::") {
        let Ok(bucket) = BucketReplica::parse(&ReplicaId::new(replica)) else {
            return past_replica();
        };
        if matches!(bucket.scope(), BucketScope::Author { author_pubkey } if author_pubkey != account)
        {
            return past_replica();
        }
        let Some(since) = since.filter(|since| bucket.bucket().index() < *since) else {
            return Place::Take;
        };
        // 同じ scope の範囲の最初の bucket へ（文字列の順で手前に戻るなら、replica の後ろへ）。
        return match TimeBucket::from_index(since)
            .and_then(|first| BucketReplica::new(bucket.scope().clone(), first))
        {
            Ok(first) if first.replica_id().as_str() > replica => {
                Place::Seek(at(reference, first.replica_id().as_str(), ""))
            }
            _ => past_replica(),
        };
    }
    if since.is_some() {
        // 期間は時間 bucket で選ぶ（旧形式は含めない）。
        return match replica < "bucket::" {
            true => Place::Seek(at(reference, "bucket::", "")),
            false => Place::End,
        };
    }
    if replica.starts_with("topic::")
        || replica.starts_with("channel::")
        || replica.strip_prefix("author::") == Some(account)
    {
        return Place::Take;
    }
    // 他人の author 領域は replica ごと、account・device の replica と未知の種類は種類ごと飛ばす。
    match replica.find("::") {
        Some(end) if !replica.starts_with("author::") => {
            let kind = &replica[..end + 1];
            Place::Seek(at(reference, &format!("{kind};"), ""))
        }
        _ => past_replica(),
    }
}

/// 保護参照の範囲の record を位置の次から読む照会の結果（`ContentCacheStore::protected_records_after`）。
pub(crate) type ProtectedRecords = Vec<(AccountHistoryCursor, Vec<u8>)>;

/// 移行元: 範囲の履歴の 1 page。`cursor` の次から保護所有先の自分の record を `read`（参照・位置・件数の照会）で読み、
/// 範囲の外は seek で飛ばす。照会は `PAGE_QUERIES` 回まで（届かなければ、その位置を次の位置にして返す）。
pub(crate) async fn page<F, Fut>(
    read: F,
    account: &str,
    since: Option<u64>,
    cursor: Option<AccountHistoryCursor>,
) -> Result<AccountHistoryPage>
where
    F: Fn(&'static str, AccountHistoryCursor, usize) -> Fut,
    Fut: std::future::Future<Output = Result<ProtectedRecords>>,
{
    let mut position = cursor.unwrap_or_else(|| at(OWN_DOCS, "", ""));
    let mut found = Vec::new();
    for _ in 0..PAGE_QUERIES {
        let reference = match position.reference.starts_with(LEGACY_OWN) {
            true => LEGACY_OWN,
            false => OWN_DOCS,
        };
        let limit = PAGE_RECORDS - found.len();
        let rows = read(reference, position.clone(), limit).await?;
        let rest = rows.len() < limit;
        let mut moved = false;
        for (row, payload) in rows {
            match place(&row, account, since) {
                Place::Take => {
                    position = row.clone();
                    found.push((row, payload));
                }
                Place::Seek(to) => {
                    position = to;
                    moved = true;
                    break;
                }
                Place::End => return Ok(finish(found, None)),
            }
        }
        if found.len() == PAGE_RECORDS {
            break;
        }
        if rest && !moved {
            // この参照は尽きた。すべてを選んだときは、旧形式の参照へ進む。
            if reference == OWN_DOCS && since.is_none() {
                position = at(LEGACY_OWN, "", "");
                continue;
            }
            return Ok(finish(found, None));
        }
    }
    Ok(finish(found, Some(position)))
}

fn is_post_envelope(key: &str) -> bool {
    key.strip_prefix("objects/")
        .is_some_and(|rest| rest.ends_with("/envelope"))
}

/// 投稿の envelope が指す本文・添付の blob。
fn post_blobs(value: &[u8]) -> Vec<String> {
    let post = serde_json::from_slice::<KukuriEnvelope>(value)
        .ok()
        .and_then(|envelope| envelope.to_post_object().ok().flatten());
    let Some(post) = post else {
        return Vec::new();
    };
    let text = match post.payload_ref {
        PayloadRef::BlobText { hash, .. } => Some(hash),
        PayloadRef::InlineText { .. } => None,
    };
    text.into_iter()
        .chain(post.attachments.into_iter().map(|asset| asset.hash))
        .map(|hash| hash.as_str().to_string())
        .collect()
}

/// 読んだ record を page にする。1 frame に収まらない record・読めない record は送れない数に数える。
fn finish(
    found: Vec<(AccountHistoryCursor, Vec<u8>)>,
    next: Option<AccountHistoryCursor>,
) -> AccountHistoryPage {
    let mut page = AccountHistoryPage {
        next,
        ..AccountHistoryPage::default()
    };
    for (position, payload) in found {
        let record = match serde_json::from_slice::<DocReadRecord>(&payload) {
            Ok(record) if record.value.len() <= MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES => record,
            _ => {
                page.unavailable += 1;
                continue;
            }
        };
        if is_post_envelope(&record.key) {
            page.posts += 1;
            for hash in post_blobs(&record.value) {
                if !page.blobs.contains(&hash) {
                    page.blobs.push(hash);
                }
            }
        }
        page.records.push(AccountHistoryRecord {
            replica: position.replica,
            key: record.key,
            docs_author: record.docs_author,
            value: record.value,
        });
    }
    page
}

/// 移行先の journal。
#[derive(Default, Serialize, Deserialize)]
struct Journal {
    history: Option<AccountTransferHistory>,
    /// 続きの位置の送り元の端末（endpoint id）。位置は送り元の索引の位置なので、別の端末からは最初から受ける（#1650）。
    /// この欄より前の journal は持たない（どの端末からでも続ける）。
    #[serde(default)]
    peer: Option<String>,
    since: Option<u64>,
    cursor: Option<AccountHistoryCursor>,
    /// 置き場へ保存した page の数と、反映した page の数。
    pages: u64,
    merged: u64,
    /// 範囲の終わりまで受けた。
    finished: bool,
}

/// 置き場の 1 page。blob は page の部分の file の `first` から `parts` 個。
#[derive(Serialize, Deserialize)]
struct StagedPage {
    records: Vec<AccountHistoryRecord>,
    blobs: Vec<StagedBlob>,
}

#[derive(Serialize, Deserialize)]
struct StagedBlob {
    hash: String,
    first: u64,
    parts: u64,
}

fn journal_path(db: &Path) -> PathBuf {
    db.with_extension("account-history.json")
}

fn page_path(db: &Path, page: u64) -> PathBuf {
    db.with_extension(format!("account-history-{page}.json"))
}

fn part_path(db: &Path, page: u64, part: u64) -> PathBuf {
    db.with_extension(format!("account-history-{page}-{part}.bin"))
}

/// journal の読み書きを 1 つずつにする（移行の置き場と、反映の task が同じ journal を更新する）。
static JOURNAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

async fn update_journal<T>(db: &Path, change: impl FnOnce(&mut Option<Journal>) -> T) -> Result<T> {
    let _guard = JOURNAL.lock().await;
    let path = journal_path(db);
    let mut journal = read_file(&path)
        .await?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?;
    let result = change(&mut journal);
    match &journal {
        Some(journal) => write_file(&path, &serde_json::to_vec(journal)?).await?,
        None => delete_file(&path).await?,
    }
    Ok(result)
}

/// 確定していない page（`page` 番）の file を消す（再起動などで残ったもの）。部分の file は 0 から順に書くので、最初に
/// 無い番号で止める。
async fn clear_page(db: &Path, page: u64) -> Result<()> {
    delete_file(&page_path(db, page)).await?;
    let mut part = 0;
    while read_file(&part_path(db, page, part)).await?.is_some() {
        delete_file(&part_path(db, page, part)).await?;
        part += 1;
    }
    Ok(())
}

/// 移行先: 履歴の置き場を開く。同じ送り元の端末（`peer`）の同じ範囲の途中で止まっていれば、その位置から続ける（別の
/// 端末・別の範囲・終わった範囲は最初から）。範囲の最初の時間 bucket、続きの位置、次に保存する page の番号を返す。
pub(crate) async fn begin(
    db: &Path,
    history: AccountTransferHistory,
    peer: &str,
    now_seconds: i64,
) -> Result<(Option<u64>, Option<AccountHistoryCursor>, u64)> {
    let (since, cursor, page) = update_journal(db, |journal| {
        let journal = journal.get_or_insert_default();
        let other_peer = journal.peer.as_deref().is_some_and(|known| known != peer);
        if journal.history != Some(history) || journal.finished || other_peer {
            journal.history = Some(history);
            journal.since = history.days().map(|days| {
                TimeBucket::from_unix_seconds((now_seconds - days * 86_400).max(0))
                    .map_or(0, TimeBucket::index)
            });
            journal.cursor = None;
            journal.finished = false;
        }
        journal.peer = Some(peer.to_string());
        (journal.since, journal.cursor.clone(), journal.pages)
    })
    .await?;
    clear_page(db, page).await?;
    Ok((since, cursor, page))
}

fn storage(error: anyhow::Error) -> Failure {
    tracing::warn!(%error, "the transferred history could not be stored");
    Failure::Storage
}

/// 移行先: 受けた record・blob を page ごとに置き場へ保存する。確定しないまま落とされた page の部分の file は消す。
pub(crate) struct Staging {
    db: PathBuf,
    account: String,
    since: Option<u64>,
    page: u64,
    records: Vec<AccountHistoryRecord>,
    blobs: Vec<StagedBlob>,
    /// 受けている blob と、その長さ・受けた bytes・hash の計算。
    blob: Option<(StagedBlob, u64, u64, blake3::Hasher)>,
    /// この page で書いた部分の file の数。
    parts: u64,
    /// 受けたアカウントを使っていれば、page の保存の後に反映させる。
    active: Option<Weak<ClientHost>>,
}

impl Staging {
    pub(crate) fn new(
        db: PathBuf,
        account: String,
        since: Option<u64>,
        page: u64,
        active: Option<Weak<ClientHost>>,
    ) -> Self {
        Self {
            db,
            account,
            since,
            page,
            records: Vec::new(),
            blobs: Vec::new(),
            blob: None,
            parts: 0,
            active,
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.parts == 0 {
            return;
        }
        let (db, page, parts) = (self.db.clone(), self.page, self.parts);
        n0_future::task::spawn(async move {
            for part in 0..parts {
                if let Err(error) = delete_file(&part_path(&db, page, part)).await {
                    tracing::warn!(%error, "the stopped history page stays until the next transfer");
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl AccountHistoryStaging for Staging {
    async fn records(&mut self, records: Vec<AccountHistoryRecord>) -> Result<(), Failure> {
        for record in &records {
            let fields = [&record.replica, &record.key, &record.docs_author];
            let valid = fields
                .iter()
                .all(|field| !field.is_empty() && !field.contains('\0'))
                && matches!(
                    place(
                        &at(OWN_DOCS, &record.replica, &record.key),
                        &self.account,
                        self.since
                    ),
                    Place::Take
                );
            if !valid {
                return Err(Failure::Invalid);
            }
        }
        self.records.extend(records);
        Ok(())
    }

    async fn blob(
        &mut self,
        hash: &str,
        len: u64,
        offset: u64,
        bytes: Vec<u8>,
    ) -> Result<(), Failure> {
        if offset == 0 && self.blob.is_none() {
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Failure::Invalid);
            }
            let blob = StagedBlob {
                hash: hash.to_string(),
                first: self.parts,
                parts: 0,
            };
            self.blob = Some((blob, len, 0, blake3::Hasher::new()));
        }
        let Some((blob, total, received, hasher)) = self.blob.as_mut() else {
            return Err(Failure::Invalid);
        };
        if blob.hash != hash || *total != len || *received != offset {
            return Err(Failure::Invalid);
        }
        write_file(&part_path(&self.db, self.page, self.parts), &bytes)
            .await
            .map_err(storage)?;
        self.parts += 1;
        blob.parts += 1;
        hasher.update(&bytes);
        *received += bytes.len() as u64;
        if *received == len {
            let (blob, _, _, hasher) = self.blob.take().expect("the blob is being received");
            if hasher.finalize().to_hex().as_str() != blob.hash {
                return Err(Failure::Invalid);
            }
            self.blobs.push(blob);
        }
        Ok(())
    }

    async fn commit(&mut self, next: Option<AccountHistoryCursor>) -> Result<(), Failure> {
        if self.blob.is_some() {
            return Err(Failure::Invalid);
        }
        let page = StagedPage {
            records: std::mem::take(&mut self.records),
            blobs: std::mem::take(&mut self.blobs),
        };
        let bytes = serde_json::to_vec(&page).map_err(|error| storage(error.into()))?;
        write_file(&page_path(&self.db, self.page), &bytes)
            .await
            .map_err(storage)?;
        let pages = self.page + 1;
        update_journal(&self.db, |journal| {
            let journal = journal.get_or_insert_default();
            journal.pages = pages;
            journal.finished = next.is_none();
            journal.cursor = next;
        })
        .await
        .map_err(storage)?;
        self.page = pages;
        self.parts = 0;
        if let Some(host) = self.active.as_ref().and_then(Weak::upgrade) {
            host.runtime().merge_account_transfer().await;
        }
        Ok(())
    }
}

/// 置き場の page を、そのアカウントの自分の record・blob として反映する（既にある自分の record は上書きしない）。
/// 反映した page から消し、範囲の終わりまで反映したら journal も消す。
pub(crate) async fn merge_staged_history(db: &Path, store: &dyn ContentCacheStore) -> Result<()> {
    loop {
        let next = update_journal(db, |journal| {
            let state = journal.as_ref()?;
            if state.merged < state.pages {
                return Some(state.merged);
            }
            if state.finished {
                *journal = None;
            }
            None
        })
        .await?;
        let Some(index) = next else {
            return Ok(());
        };
        if let Some(bytes) = read_file(&page_path(db, index)).await? {
            let page: StagedPage = serde_json::from_slice(&bytes)?;
            for record in page.records {
                merge_record(store, record).await?;
            }
            for blob in page.blobs {
                merge_blob(db, index, store, &blob).await?;
            }
            delete_file(&page_path(db, index)).await?;
        }
        update_journal(db, |journal| {
            if let Some(journal) = journal {
                journal.merged = journal.merged.max(index + 1);
            }
        })
        .await?;
    }
}

async fn merge_record(store: &dyn ContentCacheStore, record: AccountHistoryRecord) -> Result<()> {
    let author = record.docs_author;
    let held = store
        .get_remote_records(&record.replica, &record.key, Some(&author), 1, true)
        .await?;
    if !held.is_empty() {
        return Ok(());
    }
    let payload = serde_json::to_vec(&DocReadRecord {
        key: record.key.clone(),
        content_hash: blob_hash(&record.value).0,
        content_len: record.value.len() as u64,
        value: record.value,
        docs_author: author.clone(),
    })?;
    store
        .put_owned_record(&record.replica, &record.key, &author, &payload)
        .await
}

/// 部分の file を順に読んで保存し、消す。部分が欠けていれば、保存の後の削除の途中で止まった（反映の task は止めて
/// 作り直す）ので、保存せずに残りを消す。
async fn merge_blob(
    db: &Path,
    page: u64,
    store: &dyn ContentCacheStore,
    blob: &StagedBlob,
) -> Result<()> {
    let parts = blob.first..blob.first + blob.parts;
    let mut bytes = Vec::new();
    for part in parts.clone() {
        match read_file(&part_path(db, page, part)).await? {
            Some(chunk) => bytes.extend(chunk),
            None => break,
        }
    }
    if blake3::hash(&bytes).to_hex().as_str() == blob.hash {
        store
            .put_owned_blob(&format!("own_blob:{}", blob.hash), &blob.hash, &bytes)
            .await?;
    }
    for part in parts {
        delete_file(&part_path(db, page, part)).await?;
    }
    Ok(())
}
