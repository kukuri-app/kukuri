//! #1211 AC-2: 移行の必須 bundle の送り元（手元の account の replica）と、移行先の保存（staging）・確定・反映。
//! AC-3: 任意の投稿の履歴の送り元と置き場（`history.rs`）をつなぐ。
//!
//! 置き場はアカウントごとに 1 つ（`<db>.account-transfer.json` の manifest と、chunk の
//! `<db>.account-transfer-<n>.json`）。chunk は受けた封のまま置く（秘密は封の中）。manifest の `complete` が保存の
//! 確定で、新規のアカウントはその後に登録簿へ足す。反映はそのアカウントの runtime が chunk ごとに W5 の item 単位の
//! merge で行い、反映した chunk から消す。受信を失敗・取消・停止で止めたら置き場を消し、再起動などで残った受信途中
//! （`complete` でない）の置き場は反映せず、次の受信・起動で消す。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use anyhow::Result;
use kukuri_app_api::AppService;
use kukuri_core::{
    AccountHistoryCursor, AccountSyncKeys, AccountTransferFailure as Failure,
    AccountTransferHistory, AccountTransferItem, KukuriKeys, Pubkey,
};
use kukuri_iroh_node::{
    AccountBundleSink, AccountBundleSource, AccountBundleStaging, AccountHistoryPage,
    AccountHistoryResume,
};
use kukuri_store::AccountStore;
use serde::{Deserialize, Serialize};

use super::{account_db_path, account_id_for_pubkey, history, list_accounts};
use crate::ClientHost;
use crate::storage::{delete_file, read_file, write_file};

#[derive(Default, Serialize, Deserialize)]
struct Manifest {
    complete: bool,
    chunks: u64,
    merged: u64,
}

fn manifest_path(db: &Path) -> PathBuf {
    db.with_extension("account-transfer.json")
}

fn chunk_path(db: &Path, index: u64) -> PathBuf {
    db.with_extension(format!("account-transfer-{index}.json"))
}

async fn read_manifest(db: &Path) -> Result<Option<Manifest>> {
    read_file(&manifest_path(db))
        .await?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()
        .map_err(Into::into)
}

async fn write_manifest(db: &Path, manifest: &Manifest) -> Result<()> {
    write_file(&manifest_path(db), &serde_json::to_vec(manifest)?).await
}

/// 置き場を消す（反映していない chunk と manifest）。
async fn clear(db: &Path) -> Result<()> {
    if let Some(manifest) = read_manifest(db).await? {
        for index in manifest.merged..manifest.chunks {
            delete_file(&chunk_path(db, index)).await?;
        }
    }
    delete_file(&manifest_path(db)).await
}

/// 保存した必須 bundle を chunk ごとに反映する。消えている chunk は反映済みとして進む。受信途中の置き場は消す。
pub(crate) async fn merge_staged(db: &Path, app: &AppService) -> Result<()> {
    let Some(mut manifest) = read_manifest(db).await? else {
        return Ok(());
    };
    if !manifest.complete {
        return clear(db).await;
    }
    while manifest.merged < manifest.chunks {
        let path = chunk_path(db, manifest.merged);
        if let Some(bytes) = read_file(&path).await? {
            app.merge_account_transfer_items(serde_json::from_slice(&bytes)?)
                .await?;
            delete_file(&path).await?;
        }
        manifest.merged += 1;
        write_manifest(db, &manifest).await?;
    }
    delete_file(&manifest_path(db)).await
}

/// 移行元: アカウントの鍵と、手元の account の replica の page、保護所有先の履歴（AC-3）。
pub(crate) struct RuntimeBundleSource {
    pub(crate) app: AppService,
    pub(crate) keys: Arc<KukuriKeys>,
    pub(crate) store: Arc<dyn AccountStore>,
}

#[async_trait::async_trait]
impl AccountBundleSource for RuntimeBundleSource {
    fn secret_hex(&self) -> String {
        self.keys.export_secret_hex()
    }

    async fn page(
        &self,
        cursor: Option<String>,
    ) -> Result<(Vec<AccountTransferItem>, Option<String>)> {
        self.app.account_transfer_page(cursor).await
    }

    /// 履歴の 1 page。最初の page の前に、送信待ちの取り下げを書く（取り下げた投稿を取り下げの record とともに送る）。
    async fn history_page(
        &self,
        since: Option<u64>,
        cursor: Option<AccountHistoryCursor>,
    ) -> Result<AccountHistoryPage> {
        if cursor.is_none() {
            self.app.resume_withdrawal_writes().await?;
        }
        let read = |reference, after: AccountHistoryCursor, limit| async move {
            self.store
                .protected_records_after(reference, &after, limit)
                .await
        };
        history::page(read, &self.keys.public_key_hex(), since, cursor).await
    }

    async fn blob_part(
        &self,
        hash: &str,
        offset: u64,
        limit: usize,
    ) -> Result<Option<(u64, Vec<u8>)>> {
        let Some(len) = self.store.remote_content_len("blob", hash).await? else {
            return Ok(None);
        };
        if offset >= len {
            return Ok(Some((len, Vec::new())));
        }
        let part = self
            .store
            .remote_content_chunk("blob", hash, offset, limit)
            .await?;
        Ok(part.map(|bytes| (len, bytes)))
    }
}

fn storage(error: anyhow::Error) -> Failure {
    tracing::warn!(%error, "the transferred account bundle could not be stored");
    Failure::Storage
}

/// 移行先: 受けたアカウントの置き場へ保存し、確定で登録簿へ足す（host のアカウントの操作と排他にする）。
pub(crate) struct TransferSink {
    pub(crate) host: Weak<ClientHost>,
}

struct TransferStaging {
    host: Weak<ClientHost>,
    keys: KukuriKeys,
    sync_keys: AccountSyncKeys,
    account: Pubkey,
    db: PathBuf,
    chunks: u64,
    /// 保存を確定した（manifest を完了にした）か、破棄した。どちらでもなく落とされた（取消・停止で移行の task ごと
    /// 止めた）ら、置き場を消す。
    settled: bool,
}

impl Drop for TransferStaging {
    fn drop(&mut self) {
        if !self.settled {
            let db = self.db.clone();
            n0_future::task::spawn(async move {
                if let Err(error) = clear(&db).await {
                    tracing::warn!(%error, "the cancelled account bundle stays until the next transfer");
                }
            });
        }
    }
}

#[async_trait::async_trait]
impl AccountBundleSink for TransferSink {
    async fn begin(&self, secret_hex: &str) -> Result<Box<dyn AccountBundleStaging>, Failure> {
        let keys = KukuriKeys::parse(secret_hex).map_err(|_| Failure::Invalid)?;
        let host = self.host.upgrade().ok_or(Failure::Cancelled)?;
        let account = keys.public_key();
        let id = account_id_for_pubkey(account.as_str()).map_err(|_| Failure::Invalid)?;
        // 同じ ID で別の公開鍵のアカウントの置き場には書かない。
        let accounts = list_accounts(host.app_data_dir()).await.map_err(storage)?;
        if accounts
            .accounts
            .iter()
            .any(|record| record.id == id && record.pubkey != account.as_str())
        {
            return Err(Failure::Invalid);
        }
        let runtime = host.runtime();
        // 使っているアカウントへの移行は、前の置き場の反映を止めてから置き場を作り直す（反映はやり直せる）。
        if runtime.local_author_pubkey() == account.as_str() {
            runtime.stop_account_transfer_merge().await;
        }
        let db = account_db_path(host.app_data_dir(), &id);
        clear(&db).await.map_err(storage)?;
        write_manifest(&db, &Manifest::default())
            .await
            .map_err(storage)?;
        Ok(Box::new(TransferStaging {
            host: self.host.clone(),
            sync_keys: keys.derive_account_sync(),
            keys,
            account,
            db,
            chunks: 0,
            settled: false,
        }))
    }

    async fn history(
        &self,
        account_id: &str,
        history: AccountTransferHistory,
        peer: &str,
    ) -> Result<AccountHistoryResume, Failure> {
        let host = self.host.upgrade().ok_or(Failure::Cancelled)?;
        let accounts = list_accounts(host.app_data_dir()).await.map_err(storage)?;
        let account = accounts
            .accounts
            .into_iter()
            .find(|record| record.id == account_id)
            .ok_or(Failure::Invalid)?
            .pubkey;
        let db = account_db_path(host.app_data_dir(), account_id);
        let now = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64);
        let (since, cursor, page) = history::begin(&db, history, peer, now)
            .await
            .map_err(storage)?;
        // 使っているアカウントへの履歴は、page の保存の後に反映させる。
        let active = (host.runtime().local_author_pubkey() == account).then(|| self.host.clone());
        Ok(AccountHistoryResume {
            since,
            cursor,
            staging: Box::new(history::Staging::new(db, account, since, page, active)),
        })
    }
}

#[async_trait::async_trait]
impl AccountBundleStaging for TransferStaging {
    async fn stage(&mut self, items: Vec<AccountTransferItem>) -> Result<(), Failure> {
        for item in &items {
            item.open(&self.sync_keys, &self.account)
                .map_err(|_| Failure::Invalid)?;
        }
        // manifest を先に進める（chunk を書く前に止まっても、消すときに漏れない）。
        let index = self.chunks;
        self.chunks += 1;
        let manifest = Manifest {
            chunks: self.chunks,
            ..Manifest::default()
        };
        write_manifest(&self.db, &manifest).await.map_err(storage)?;
        let bytes = serde_json::to_vec(&items).map_err(|error| storage(error.into()))?;
        write_file(&chunk_path(&self.db, index), &bytes)
            .await
            .map_err(storage)
    }

    async fn commit(&mut self) -> Result<String, Failure> {
        let manifest = Manifest {
            complete: true,
            chunks: self.chunks,
            merged: 0,
        };
        write_manifest(&self.db, &manifest).await.map_err(storage)?;
        // ここから先で止まっても、確定した置き場は残す（登録できていれば、そのアカウントの起動で反映する）。
        self.settled = true;
        let host = self.host.upgrade().ok_or(Failure::Cancelled)?;
        let record = host
            .register_transferred_account(&self.keys)
            .await
            .map_err(storage)?;
        let runtime = host.runtime();
        if runtime.local_author_pubkey() == self.account.as_str() {
            runtime.merge_account_transfer().await;
        }
        Ok(record.id)
    }

    async fn abort(&mut self) {
        self.settled = true;
        if let Err(error) = clear(&self.db).await {
            tracing::warn!(%error, "the transferred account bundle stays until the next transfer");
        }
    }
}
