//! private channel の参加と世代の鍵の行（`PrivateChannelKeyStore`。ADR 0061 §9、native の
//! `sqlite/private_channel_keys.rs`）。cache の database の保護行で、strict の transaction で書き、回収しない。
//! 各操作は対象の行だけを索引で読む。

use anyhow::Result;
use async_trait::async_trait;
use kukuri_store::{
    PrivateChannelEpochRange, PrivateChannelEpochRow, PrivateChannelFilter, PrivateChannelKeyStore,
    PrivateChannelRow,
};
use wasm_bindgen::JsValue;

use crate::IndexedDbCache;
use crate::content_cache::{PRIVATE_CHANNELS, PRIVATE_EPOCHS};
use crate::idb::{Mode, js_error};
use crate::rows::{self, Txn, between, key, num, only, prefix, text, top};

/// 参加中の行だけが載る一覧の索引の値（SQLite の `joined = 1` の部分索引）。
fn channel_extra(row: &PrivateChannelRow) -> [(&'static str, JsValue); 3] {
    if !row.joined {
        return [
            ("joined", JsValue::UNDEFINED),
            ("joined_topic", JsValue::UNDEFINED),
            ("joined_owner", JsValue::UNDEFINED),
        ];
    }
    let channel_key = text(&row.channel_key);
    [
        ("joined", channel_key.clone()),
        (
            "joined_topic",
            key(&[text(&row.topic_id), channel_key.clone()]),
        ),
        ("joined_owner", key(&[text(&row.owner_pubkey), channel_key])),
    ]
}

fn epoch_key(channel_id: &str, epoch_id: &str) -> JsValue {
    key(&[text(channel_id), text(epoch_id)])
}

/// 世代の鍵の行の索引の値。`rotation` は鍵更新が終わっていない世代（SQLite の `rotation_from IS NOT NULL` の
/// 部分索引）。`unwritten` は replica へ未書込み（`pending`）で、予約した鍵更新の確定前でない世代（SQLite の
/// `written = 0` の部分索引と、未書込みの一覧の条件。ADR 0061 §10）。`pending` は行を置き直しても保つ。
fn epoch_extra(row: &PrivateChannelEpochRow, pending: bool) -> [(&'static str, JsValue); 3] {
    let id = epoch_key(&row.channel_id, &row.epoch_id);
    let reserved = row.rotation_from.is_some() && row.rotation_after.is_none();
    [
        (
            "rotation",
            match row.rotation_from {
                Some(_) => id.clone(),
                None => JsValue::UNDEFINED,
            },
        ),
        ("pending", JsValue::from_bool(pending)),
        (
            "unwritten",
            if pending && !reserved {
                id
            } else {
                JsValue::UNDEFINED
            },
        ),
    ]
}

/// 世代の鍵の行と、replica へ未書込みか。
async fn get_epoch(
    tx: &web_sys::IdbTransaction,
    key: &JsValue,
) -> Result<Option<(PrivateChannelEpochRow, bool)>> {
    let value = crate::idb::done(
        &rows::store(tx, PRIVATE_EPOCHS)?
            .get(key)
            .map_err(js_error)?,
    )
    .await?;
    if value.is_undefined() {
        return Ok(None);
    }
    let pending = rows::extra(&value, "pending")?.as_bool().unwrap_or(false);
    Ok(Some((rows::decode(&value)?, pending)))
}

#[async_trait]
impl PrivateChannelKeyStore for IndexedDbCache {
    /// 参加の行を置き換え、まだ無い世代の鍵の行を足す（1 つの strict の transaction）。
    async fn put_private_channel(
        &self,
        row: &PrivateChannelRow,
        epochs: &[PrivateChannelEpochRow],
    ) -> Result<()> {
        let (row, epochs) = (row.clone(), epochs.to_vec());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_CHANNELS, PRIVATE_EPOCHS], Mode::Strict)?;
            rows::put(&tx, PRIVATE_CHANNELS, &row, &channel_extra(&row))?;
            for epoch in epochs {
                let id = epoch_key(&epoch.channel_id, &epoch.epoch_id);
                if rows::get::<PrivateChannelEpochRow>(&tx, PRIVATE_EPOCHS, &id)
                    .await?
                    .is_none()
                {
                    rows::put(&tx, PRIVATE_EPOCHS, &epoch, &epoch_extra(&epoch, true))?;
                }
            }
            tx.commit().await
        })
        .await
    }

    /// 参加の行の無い channel の世代の鍵の行だけを足す。既にあれば何もしない。
    async fn put_private_channel_epoch(&self, epoch: &PrivateChannelEpochRow) -> Result<bool> {
        let epoch = epoch.clone();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_EPOCHS], Mode::Strict)?;
            let id = epoch_key(&epoch.channel_id, &epoch.epoch_id);
            if rows::get::<PrivateChannelEpochRow>(&tx, PRIVATE_EPOCHS, &id)
                .await?
                .is_some()
            {
                return Ok(false);
            }
            rows::put(&tx, PRIVATE_EPOCHS, &epoch, &epoch_extra(&epoch, true))?;
            tx.commit().await?;
            Ok(true)
        })
        .await
    }

    async fn get_private_channel(&self, channel_key: &str) -> Result<Option<PrivateChannelRow>> {
        let channel_key = channel_key.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_CHANNELS], Mode::Read)?;
            rows::get(&tx, PRIVATE_CHANNELS, &text(&channel_key)).await
        })
        .await
    }

    async fn get_private_channel_by_id(
        &self,
        channel_id: &str,
    ) -> Result<Option<PrivateChannelRow>> {
        let channel_id = channel_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_CHANNELS], Mode::Read)?;
            let range = only(&text(&channel_id))?;
            Ok(
                rows::scan(&tx, PRIVATE_CHANNELS, Some("channel_id"), &range, false, 1)
                    .await?
                    .pop(),
            )
        })
        .await
    }

    async fn list_joined_private_channels(
        &self,
        filter: PrivateChannelFilter<'_>,
        after: &str,
        limit: usize,
    ) -> Result<Vec<PrivateChannelRow>> {
        let (index, head) = match filter {
            PrivateChannelFilter::All => ("joined", None),
            PrivateChannelFilter::Topic(topic) => ("topic", Some(topic.to_owned())),
            PrivateChannelFilter::Owner(owner) => ("owner", Some(owner.to_owned())),
        };
        let after = after.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_CHANNELS], Mode::Read)?;
            let range = match &head {
                Some(head) => between(
                    &[text(head), text(&after)],
                    &top(&[text(head)]),
                    true,
                    false,
                )?,
                None => web_sys::IdbKeyRange::lower_bound_with_open(&text(&after), true)
                    .map_err(js_error)?,
            };
            rows::scan(&tx, PRIVATE_CHANNELS, Some(index), &range, false, limit).await
        })
        .await
    }

    async fn get_private_channel_epoch(
        &self,
        channel_id: &str,
        epoch_id: &str,
    ) -> Result<Option<PrivateChannelEpochRow>> {
        let id = (channel_id.to_owned(), epoch_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_EPOCHS], Mode::Read)?;
            rows::get(&tx, PRIVATE_EPOCHS, &epoch_key(&id.0, &id.1)).await
        })
        .await
    }

    async fn find_private_channel_epoch(
        &self,
        receive_key_id: &str,
    ) -> Result<Option<PrivateChannelEpochRow>> {
        let receive = receive_key_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_EPOCHS], Mode::Read)?;
            let range = only(&text(&receive))?;
            Ok(
                rows::scan(&tx, PRIVATE_EPOCHS, Some("receive"), &range, false, 1)
                    .await?
                    .pop(),
            )
        })
        .await
    }

    async fn list_private_channel_epochs(
        &self,
        channel_id: &str,
        range: PrivateChannelEpochRange,
        limit: usize,
    ) -> Result<Vec<PrivateChannelEpochRow>> {
        let channel = channel_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_EPOCHS], Mode::Read)?;
            let head = [text(&channel)];
            let (bounds, newest_first) = match range {
                // 開始時刻が `at` 以前の世代を新しい順に。
                PrivateChannelEpochRange::AtOrBefore(at) => (
                    between(&head, &top(&[text(&channel), num(at)]), false, false)?,
                    true,
                ),
                // `at` より後に始まった世代を古い順に。
                PrivateChannelEpochRange::After(at) => (
                    between(&top(&[text(&channel), num(at)]), &top(&head), true, false)?,
                    false,
                ),
            };
            rows::scan(
                &tx,
                PRIVATE_EPOCHS,
                Some("started"),
                &bounds,
                newest_first,
                limit,
            )
            .await
        })
        .await
    }

    /// 未書込みの索引から外す（行を置き直す）。
    async fn mark_private_channel_epoch_written(
        &self,
        channel_id: &str,
        epoch_id: &str,
    ) -> Result<()> {
        let id = (channel_id.to_owned(), epoch_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_EPOCHS], Mode::Strict)?;
            if let Some((epoch, _)) = get_epoch(&tx, &epoch_key(&id.0, &id.1)).await? {
                rows::put(&tx, PRIVATE_EPOCHS, &epoch, &epoch_extra(&epoch, false))?;
            }
            tx.commit().await
        })
        .await
    }

    async fn set_private_channel_rotation(
        &self,
        channel_id: &str,
        epoch_id: &str,
        after: Option<&str>,
    ) -> Result<()> {
        let id = (channel_id.to_owned(), epoch_id.to_owned());
        let after = after.map(str::to_owned);
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_EPOCHS], Mode::Strict)?;
            let key = epoch_key(&id.0, &id.1);
            if let Some((mut row, pending)) = get_epoch(&tx, &key).await? {
                if after.is_none() {
                    row.rotation_from = None;
                }
                row.rotation_after = after;
                rows::put(&tx, PRIVATE_EPOCHS, &row, &epoch_extra(&row, pending))?;
            }
            tx.commit().await
        })
        .await
    }

    async fn list_unwritten_private_channel_epochs(
        &self,
        limit: usize,
    ) -> Result<Vec<PrivateChannelEpochRow>> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_EPOCHS], Mode::Read)?;
            let range = web_sys::IdbKeyRange::lower_bound(&key(&[])).map_err(js_error)?;
            rows::scan(&tx, PRIVATE_EPOCHS, Some("unwritten"), &range, false, limit).await
        })
        .await
    }

    async fn list_private_channel_rotations(
        &self,
        after: (&str, &str),
        limit: usize,
    ) -> Result<Vec<PrivateChannelEpochRow>> {
        let after = epoch_key(after.0, after.1);
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_EPOCHS], Mode::Read)?;
            let range =
                web_sys::IdbKeyRange::lower_bound_with_open(&after, true).map_err(js_error)?;
            rows::scan(&tx, PRIVATE_EPOCHS, Some("rotation"), &range, false, limit).await
        })
        .await
    }

    /// その channel の世代の鍵の行を `limit` 件まで消す（strict の transaction）。
    async fn delete_private_channel_epochs(
        &self,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let channel = channel_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRIVATE_EPOCHS], Mode::Strict)?;
            let rows: Vec<PrivateChannelEpochRow> = rows::scan(
                &tx,
                PRIVATE_EPOCHS,
                None,
                &prefix(&[text(&channel)])?,
                false,
                limit,
            )
            .await?;
            let mut deleted = Vec::with_capacity(rows.len());
            for row in rows {
                rows::delete(
                    &tx,
                    PRIVATE_EPOCHS,
                    &epoch_key(&row.channel_id, &row.epoch_id),
                )?;
                deleted.push(row.epoch_id);
            }
            tx.commit().await?;
            Ok(deleted)
        })
        .await
    }
}
