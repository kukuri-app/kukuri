//! 通知（`NotificationStore`。native の `sqlite/notifications.rs`）。
//!
//! 足した行には配信の連番を振り、未読の数と「ここまで既読」の連番を `meta` の 1 行に持つ（native の
//! `notification_dispatch_clock`・`notification_inbox_state`）。全件既読は 1 行の更新で、通知の行を読まない。

use anyhow::{Result, ensure};
use async_trait::async_trait;
use kukuri_store::{
    NOTIFICATION_DISPATCH_PAGE_SIZE, NOTIFICATION_PAGE_SIZE, NotificationCursor, NotificationRow,
    NotificationStore,
};
use wasm_bindgen::JsValue;
use web_sys::IdbTransaction;

use crate::IndexedDbCache;
use crate::content_cache::{META, NOTIFICATIONS};
use crate::idb::{self, Mode, js_error};
use crate::rows::{self, Txn, between, key, num, only, text, top};

const STATE: &str = "notifications";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Inbox {
    last_seq: i64,
    unread: usize,
    read_through_seq: i64,
    read_through_at: Option<i64>,
}

async fn inbox(tx: &IdbTransaction) -> Result<Inbox> {
    let request = rows::store(tx, META)?.get(&text(STATE)).map_err(js_error)?;
    let value = idb::done(&request).await?;
    if value.is_undefined() {
        return Ok(Inbox::default());
    }
    rows::decode(&value)
}

fn put_inbox(tx: &IdbTransaction, state: &Inbox) -> Result<()> {
    let value = js_sys::Object::new();
    js_sys::Reflect::set(&value, &"r".into(), &rows::encode(state)?).map_err(js_error)?;
    rows::store(tx, META)?
        .put_with_key(&value, &text(STATE))
        .map_err(js_error)?;
    Ok(())
}

/// 行の既読の時刻（全件既読の連番までの行は、その時刻）。
fn effective(mut row: NotificationRow, seq: i64, state: &Inbox) -> NotificationRow {
    if row.read_at.is_none() && seq <= state.read_through_seq {
        row.read_at = state.read_through_at;
    }
    row
}

fn seq_of(value: &JsValue) -> Result<i64> {
    Ok(rows::extra(value, "seq")?.as_f64().unwrap_or(0.0) as i64)
}

fn kind_name(row: &NotificationRow) -> Result<String> {
    Ok(serde_json::to_value(&row.kind)?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

/// 重複を除く key（docs の envelope ごと、DM の message ごと）。無ければ `undefined`。
fn dedupe(row: &NotificationRow) -> Result<[(&'static str, JsValue); 2]> {
    let kind = kind_name(row)?;
    let docs = match &row.source_envelope_id {
        Some(source) => key(&[
            text(&row.recipient_pubkey),
            text(&kind),
            text(source.as_str()),
        ]),
        None => JsValue::UNDEFINED,
    };
    let dm = match (&row.dm_id, &row.message_id) {
        (Some(dm), Some(message)) => key(&[
            text(&row.recipient_pubkey),
            text(&kind),
            text(dm),
            text(message),
        ]),
        _ => JsValue::UNDEFINED,
    };
    Ok([("docs_dedupe", docs), ("dm_dedupe", dm)])
}

#[async_trait]
impl NotificationStore for IndexedDbCache {
    async fn get_notification(&self, notification_id: &str) -> Result<Option<NotificationRow>> {
        let id = notification_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[NOTIFICATIONS, META], Mode::Read)?;
            let value = idb::done(
                &rows::store(&tx, NOTIFICATIONS)?
                    .get(&text(&id))
                    .map_err(js_error)?,
            )
            .await?;
            if value.is_undefined() {
                return Ok(None);
            }
            Ok(Some(effective(
                rows::decode(&value)?,
                seq_of(&value)?,
                &inbox(&tx).await?,
            )))
        })
        .await
    }

    async fn put_notification_if_absent(&self, row: NotificationRow) -> Result<bool> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[NOTIFICATIONS, META], Mode::Write)?;
            if rows::get::<NotificationRow>(&tx, NOTIFICATIONS, &text(&row.notification_id))
                .await?
                .is_some()
            {
                return Ok(false);
            }
            let [docs, dm] = dedupe(&row)?;
            for (index, value) in [&docs, &dm] {
                if !value.is_undefined()
                    && rows::count(&tx, NOTIFICATIONS, Some(index), &only(value)?).await? > 0
                {
                    return Ok(false);
                }
            }
            let mut state = inbox(&tx).await?;
            state.last_seq += 1;
            if row.read_at.is_none() {
                state.unread += 1;
            }
            rows::put(
                &tx,
                NOTIFICATIONS,
                &row,
                &[("seq", num(state.last_seq)), docs, dm],
            )?;
            put_inbox(&tx, &state)?;
            tx.commit().await?;
            Ok(true)
        })
        .await
    }

    async fn list_notifications_page(
        &self,
        cursor: Option<&NotificationCursor>,
        before: bool,
    ) -> Result<Vec<NotificationRow>> {
        ensure!(
            !before || cursor.is_some(),
            "newer notification page requires cursor"
        );
        let cursor = cursor.cloned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[NOTIFICATIONS, META], Mode::Read)?;
            let state = inbox(&tx).await?;
            let (low, high) = ([num(i64::MIN)], top(&[num(i64::MAX)]));
            let range = match &cursor {
                Some(cursor) => {
                    let at = [num(cursor.received_at), text(&cursor.notification_id)];
                    if before {
                        between(&at, &high, true, false)?
                    } else {
                        between(&low, &at, false, true)?
                    }
                }
                None => between(&low, &high, false, false)?,
            };
            let mut items = Vec::new();
            rows::walk(
                &tx,
                NOTIFICATIONS,
                Some("inbox"),
                &range,
                !before,
                |value| {
                    items.push(effective(rows::decode(value)?, seq_of(value)?, &state));
                    Ok(items.len() <= NOTIFICATION_PAGE_SIZE)
                },
            )
            .await?;
            Ok(items)
        })
        .await
    }

    async fn list_notification_dispatch_after(
        &self,
        after_sequence: i64,
    ) -> Result<Vec<(i64, NotificationRow)>> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[NOTIFICATIONS, META], Mode::Read)?;
            let state = inbox(&tx).await?;
            let range = web_sys::IdbKeyRange::lower_bound_with_open(&num(after_sequence), true)
                .map_err(js_error)?;
            let mut items = Vec::new();
            rows::walk(&tx, NOTIFICATIONS, Some("seq"), &range, false, |value| {
                let seq = seq_of(value)?;
                items.push((seq, effective(rows::decode(value)?, seq, &state)));
                Ok(items.len() < NOTIFICATION_DISPATCH_PAGE_SIZE)
            })
            .await?;
            Ok(items)
        })
        .await
    }

    async fn notification_dispatch_head(&self) -> Result<i64> {
        self.run(|db| async move {
            let tx = Txn::begin(&db.idb, &[META], Mode::Read)?;
            Ok(inbox(&tx).await?.last_seq)
        })
        .await
    }

    async fn mark_notification_read(&self, notification_id: &str, read_at: i64) -> Result<()> {
        let id = notification_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[NOTIFICATIONS, META], Mode::Write)?;
            let request = rows::store(&tx, NOTIFICATIONS)?
                .get(&text(&id))
                .map_err(js_error)?;
            let value = idb::done(&request).await?;
            if value.is_undefined() {
                return Ok(());
            }
            let mut state = inbox(&tx).await?;
            let mut row: NotificationRow = rows::decode(&value)?;
            let seq = seq_of(&value)?;
            if row.read_at.is_none() && seq > state.read_through_seq {
                row.read_at = Some(read_at);
                let [docs, dm] = dedupe(&row)?;
                rows::put(&tx, NOTIFICATIONS, &row, &[("seq", num(seq)), docs, dm])?;
                state.unread = state.unread.saturating_sub(1);
                put_inbox(&tx, &state)?;
            }
            tx.commit().await
        })
        .await
    }

    async fn mark_all_notifications_read(&self, read_at: i64) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[META], Mode::Write)?;
            let mut state = inbox(&tx).await?;
            if state.unread > 0 {
                state.unread = 0;
                state.read_through_seq = state.last_seq;
                state.read_through_at = Some(read_at);
                put_inbox(&tx, &state)?;
            }
            tx.commit().await
        })
        .await
    }

    async fn count_unread_notifications(&self) -> Result<usize> {
        self.run(|db| async move {
            let tx = Txn::begin(&db.idb, &[META], Mode::Read)?;
            Ok(inbox(&tx).await?.unread)
        })
        .await
    }
}
