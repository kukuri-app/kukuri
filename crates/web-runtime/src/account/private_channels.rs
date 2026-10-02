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
                    rows::put(&tx, PRIVATE_EPOCHS, &epoch, &[])?;
                }
            }
            tx.commit().await
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
