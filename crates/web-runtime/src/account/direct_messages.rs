//! DM の会話・メッセージ・送信待ち・取り消し（`DirectMessageStore`。native の `sqlite/direct_messages.rs`）。
//! どれも端末だけのデータで、回収しない。送信待ちを消すときの保護参照の付け外しは同じ transaction で行う。

use anyhow::{Result, ensure};
use async_trait::async_trait;
use kukuri_core::EnvelopeId;
use kukuri_store::{
    DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT, DirectMessageConversationRow, DirectMessageMessageRow,
    DirectMessageOutboxCursor, DirectMessageOutboxPage, DirectMessageOutboxRow, DirectMessageStore,
    DirectMessageTombstoneRow, Page, TimelineCursor,
};
use wasm_bindgen::JsValue;
use web_sys::IdbTransaction;

use super::{newest_first, next_cursor};
use crate::IndexedDbCache;
use crate::content_cache::{DM_CONVERSATIONS, DM_MESSAGES, DM_OUTBOX, DM_TOMBSTONES, Tx};
use crate::idb::Mode;
use crate::rows::{self, Txn, between, key, num, only, prefix, text, top};

fn message_key(dm_id: &str, message_id: &str) -> JsValue {
    key(&[text(dm_id), text(message_id)])
}

/// 送信待ちの行の索引の値。未送信（`fresh`）と再送の期限（`due`）は、どちらか一方にだけ載る。
fn outbox_extra(row: &DirectMessageOutboxRow) -> [(&'static str, JsValue); 2] {
    let tail = [
        num(row.created_at),
        text(&row.message_id),
        text(&row.dm_id),
        text(&row.peer_pubkey),
    ];
    match row.last_attempt_at {
        None => [("fresh", key(&tail)), ("due", JsValue::UNDEFINED)],
        Some(attempted) => {
            let mut due = vec![num(attempted)];
            due.extend(tail);
            [("fresh", JsValue::UNDEFINED), ("due", key(&due))]
        }
    }
}

fn outbox_position(row: &DirectMessageOutboxRow) -> DirectMessageOutboxCursor {
    DirectMessageOutboxCursor {
        created_at: row.created_at,
        message_id: row.message_id.clone(),
        dm_id: row.dm_id.clone(),
    }
}

fn position(cursor: &DirectMessageOutboxCursor) -> [JsValue; 3] {
    [
        num(cursor.created_at),
        text(&cursor.message_id),
        text(&cursor.dm_id),
    ]
}

/// 送信待ちの (時刻, message, dm) の順の 1 周の 1 ページ（`head` は peer で絞るときの先頭の列）。
async fn outbox_page(
    tx: &IdbTransaction,
    index: &str,
    head: &[JsValue],
    after: Option<&DirectMessageOutboxCursor>,
    cycle_end: Option<&DirectMessageOutboxCursor>,
    limit: usize,
) -> Result<DirectMessageOutboxPage> {
    let end = match cycle_end {
        Some(end) => Some(end.clone()),
        None => rows::scan::<DirectMessageOutboxRow>(
            tx,
            DM_OUTBOX,
            Some(index),
            &prefix(head)?,
            true,
            1,
        )
        .await?
        .first()
        .map(outbox_position),
    };
    let Some(end) = end else {
        return Ok(DirectMessageOutboxPage {
            items: Vec::new(),
            next_cursor: None,
            cycle_end: None,
        });
    };
    let mut upper = head.to_vec();
    upper.extend(position(&end));
    let range = match after {
        Some(after) => {
            let mut lower = head.to_vec();
            lower.extend(position(after));
            between(&lower, &upper, true, false)?
        }
        None => between(head, &upper, false, false)?,
    };
    let mut items: Vec<DirectMessageOutboxRow> =
        rows::scan(tx, DM_OUTBOX, Some(index), &range, false, limit + 1).await?;
    let has_more = items.len() > limit;
    items.truncate(limit);
    Ok(DirectMessageOutboxPage {
        next_cursor: has_more
            .then(|| items.last().map(outbox_position))
            .flatten(),
        items,
        cycle_end: Some(end),
    })
}

impl IndexedDbCache {
    /// DM の行を消し、`references` の保護参照を外す（1 つの transaction）。
    async fn forget_direct_message(
        &self,
        stores: &'static [&'static str],
        id: (String, String),
        references: Vec<String>,
    ) -> Result<()> {
        let budget = self.budget();
        self.run(move |db| async move {
            let tx = Tx::begin_with(&db, stores)?;
            for store in stores {
                rows::delete(&tx, store, &message_key(&id.0, &id.1))?;
            }
            for reference in references {
                tx.replace_refs(&reference, &[], budget).await?;
            }
            tx.commit().await
        })
        .await
    }
}

#[async_trait]
impl DirectMessageStore for IndexedDbCache {
    async fn upsert_direct_message_conversation(
        &self,
        row: DirectMessageConversationRow,
    ) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_CONVERSATIONS], Mode::Write)?;
            rows::put(&tx, DM_CONVERSATIONS, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    async fn get_direct_message_conversation_by_peer(
        &self,
        peer_pubkey: &str,
    ) -> Result<Option<DirectMessageConversationRow>> {
        let peer = peer_pubkey.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_CONVERSATIONS], Mode::Read)?;
            Ok(rows::scan(
                &tx,
                DM_CONVERSATIONS,
                Some("peer"),
                &only(&text(&peer))?,
                false,
                1,
            )
            .await?
            .pop())
        })
        .await
    }

    async fn get_direct_message_conversation_by_dm_id(
        &self,
        dm_id: &str,
    ) -> Result<Option<DirectMessageConversationRow>> {
        let dm_id = dm_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_CONVERSATIONS], Mode::Read)?;
            rows::get(&tx, DM_CONVERSATIONS, &text(&dm_id)).await
        })
        .await
    }

    /// 全件の一覧（native と同じく上限の無い API）。更新の新しい順。
    async fn list_direct_message_conversations(&self) -> Result<Vec<DirectMessageConversationRow>> {
        self.run(|db| async move {
            let tx = Txn::begin(&db.idb, &[DM_CONVERSATIONS], Mode::Read)?;
            let all = between(&[num(i64::MIN)], &top(&[num(i64::MAX)]), false, false)?;
            rows::scan(&tx, DM_CONVERSATIONS, Some("order"), &all, true, usize::MAX).await
        })
        .await
    }

    /// 取り消したメッセージは置かない。
    async fn put_direct_message_message(&self, row: DirectMessageMessageRow) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_MESSAGES, DM_TOMBSTONES], Mode::Write)?;
            let id = message_key(&row.dm_id, &row.message_id);
            if rows::get::<DirectMessageTombstoneRow>(&tx, DM_TOMBSTONES, &id)
                .await?
                .is_some()
            {
                return Ok(());
            }
            rows::put(&tx, DM_MESSAGES, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    async fn get_direct_message_message(
        &self,
        dm_id: &str,
        message_id: &str,
    ) -> Result<Option<DirectMessageMessageRow>> {
        let (dm_id, message_id) = (dm_id.to_owned(), message_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_MESSAGES], Mode::Read)?;
            rows::get(&tx, DM_MESSAGES, &message_key(&dm_id, &message_id)).await
        })
        .await
    }

    async fn list_direct_message_messages(
        &self,
        dm_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<DirectMessageMessageRow>> {
        let dm_id = dm_id.to_owned();
        let items = self
            .run(move |db| async move {
                let tx = Txn::begin(&db.idb, &[DM_MESSAGES], Mode::Read)?;
                let range = newest_first(&[text(&dm_id)], cursor.as_ref())?;
                rows::scan(&tx, DM_MESSAGES, Some("timeline"), &range, true, limit).await
            })
            .await?;
        let next_cursor = next_cursor(&items, limit, |row: &DirectMessageMessageRow| {
            TimelineCursor {
                created_at: row.created_at,
                object_id: EnvelopeId::from(row.message_id.clone()),
            }
        });
        Ok(Page { items, next_cursor })
    }

    /// 最初の ACK の時刻だけを残す。
    async fn set_direct_message_acked_at(
        &self,
        dm_id: &str,
        message_id: &str,
        acked_at: i64,
    ) -> Result<()> {
        let (dm_id, message_id) = (dm_id.to_owned(), message_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_MESSAGES], Mode::Write)?;
            let id = message_key(&dm_id, &message_id);
            if let Some(mut row) =
                rows::get::<DirectMessageMessageRow>(&tx, DM_MESSAGES, &id).await?
                && row.acked_at.is_none()
            {
                row.acked_at = Some(acked_at);
                rows::put(&tx, DM_MESSAGES, &row, &[])?;
            }
            tx.commit().await
        })
        .await
    }

    async fn put_direct_message_outbox(&self, row: DirectMessageOutboxRow) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_OUTBOX], Mode::Write)?;
            rows::put(&tx, DM_OUTBOX, &row, &outbox_extra(&row))?;
            tx.commit().await
        })
        .await
    }

    async fn get_direct_message_outbox(
        &self,
        dm_id: &str,
        message_id: &str,
    ) -> Result<Option<DirectMessageOutboxRow>> {
        let (dm_id, message_id) = (dm_id.to_owned(), message_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_OUTBOX], Mode::Read)?;
            rows::get(&tx, DM_OUTBOX, &message_key(&dm_id, &message_id)).await
        })
        .await
    }

    /// 全件の一覧（native と同じく上限の無い API）。古い順。
    async fn list_direct_message_outbox(&self) -> Result<Vec<DirectMessageOutboxRow>> {
        self.run(|db| async move {
            let tx = Txn::begin(&db.idb, &[DM_OUTBOX], Mode::Read)?;
            let all = between(&[num(i64::MIN)], &top(&[num(i64::MAX)]), false, false)?;
            rows::scan(&tx, DM_OUTBOX, Some("order"), &all, false, usize::MAX).await
        })
        .await
    }

    async fn list_direct_message_outbox_candidate_page(
        &self,
        after: Option<&DirectMessageOutboxCursor>,
        cycle_end: Option<&DirectMessageOutboxCursor>,
        limit: usize,
    ) -> Result<DirectMessageOutboxPage> {
        ensure!(
            (1..=DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT).contains(&limit),
            "invalid direct message outbox candidate page limit"
        );
        ensure!(
            after.is_none() || cycle_end.is_some(),
            "missing candidate cycle end"
        );
        let (after, cycle_end) = (after.cloned(), cycle_end.cloned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_OUTBOX], Mode::Read)?;
            outbox_page(&tx, "order", &[], after.as_ref(), cycle_end.as_ref(), limit).await
        })
        .await
    }

    async fn list_direct_message_outbox_for_peer_page(
        &self,
        peer_pubkey: &str,
        after: Option<&DirectMessageOutboxCursor>,
        cycle_end: Option<&DirectMessageOutboxCursor>,
        limit: usize,
    ) -> Result<DirectMessageOutboxPage> {
        ensure!(
            (1..=DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT).contains(&limit),
            "invalid direct message outbox page limit"
        );
        ensure!(
            after.is_none() || cycle_end.is_some(),
            "missing outbox cycle end"
        );
        let (peer, after, cycle_end) = (peer_pubkey.to_owned(), after.cloned(), cycle_end.cloned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_OUTBOX], Mode::Read)?;
            outbox_page(
                &tx,
                "peer",
                &[text(&peer)],
                after.as_ref(),
                cycle_end.as_ref(),
                limit,
            )
            .await
        })
        .await
    }

    /// 1 回の再送の枠: 未送信を古い順に `new_limit` 件と、期限の来た再送を `retry_limit` 件（索引で選ぶ）。
    async fn list_due_direct_message_outbox(
        &self,
        retry_due_at_or_before: i64,
        new_limit: usize,
        retry_limit: usize,
    ) -> Result<Vec<DirectMessageOutboxRow>> {
        ensure!(
            new_limit <= 3 && retry_limit <= 1 && new_limit + retry_limit > 0,
            "invalid direct message outbox due limits"
        );
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_OUTBOX], Mode::Read)?;
            let all = between(&[num(i64::MIN)], &top(&[num(i64::MAX)]), false, false)?;
            let mut selected: Vec<DirectMessageOutboxRow> =
                rows::scan(&tx, DM_OUTBOX, Some("fresh"), &all, false, new_limit).await?;
            let due = between(
                &[num(i64::MIN)],
                &top(&[num(retry_due_at_or_before)]),
                false,
                false,
            )?;
            selected.extend(
                rows::scan::<DirectMessageOutboxRow>(
                    &tx,
                    DM_OUTBOX,
                    Some("due"),
                    &due,
                    false,
                    retry_limit,
                )
                .await?,
            );
            Ok(selected)
        })
        .await
    }

    async fn touch_direct_message_outbox_attempt(
        &self,
        dm_id: &str,
        message_id: &str,
        attempted_at: i64,
    ) -> Result<()> {
        let (dm_id, message_id) = (dm_id.to_owned(), message_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_OUTBOX], Mode::Write)?;
            let id = message_key(&dm_id, &message_id);
            if let Some(mut row) = rows::get::<DirectMessageOutboxRow>(&tx, DM_OUTBOX, &id).await? {
                row.last_attempt_at = Some(attempted_at);
                rows::put(&tx, DM_OUTBOX, &row, &outbox_extra(&row))?;
            }
            tx.commit().await
        })
        .await
    }

    /// ACK で送信待ちを消す transaction の中で、frame と暗号化添付の保護を外す。
    async fn remove_direct_message_outbox(&self, dm_id: &str, message_id: &str) -> Result<()> {
        self.forget_direct_message(
            &[DM_OUTBOX],
            (dm_id.to_owned(), message_id.to_owned()),
            vec![format!("dm_outbox:{dm_id}/{message_id}")],
        )
        .await
    }

    async fn put_direct_message_tombstone(&self, row: DirectMessageTombstoneRow) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_TOMBSTONES], Mode::Write)?;
            rows::put(&tx, DM_TOMBSTONES, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    async fn list_direct_message_tombstones(
        &self,
        dm_id: &str,
    ) -> Result<Vec<DirectMessageTombstoneRow>> {
        let dm_id = dm_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_TOMBSTONES], Mode::Read)?;
            rows::scan(
                &tx,
                DM_TOMBSTONES,
                Some("order"),
                &prefix(&[text(&dm_id)])?,
                true,
                usize::MAX,
            )
            .await
        })
        .await
    }

    async fn has_direct_message_tombstone(&self, dm_id: &str, message_id: &str) -> Result<bool> {
        let (dm_id, message_id) = (dm_id.to_owned(), message_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DM_TOMBSTONES], Mode::Read)?;
            Ok(rows::get::<DirectMessageTombstoneRow>(
                &tx,
                DM_TOMBSTONES,
                &message_key(&dm_id, &message_id),
            )
            .await?
            .is_some())
        })
        .await
    }

    /// 手元から消した message の添付と送信待ちの保護を外す。
    async fn delete_direct_message_message_local(
        &self,
        dm_id: &str,
        message_id: &str,
    ) -> Result<()> {
        self.forget_direct_message(
            &[DM_MESSAGES, DM_OUTBOX],
            (dm_id.to_owned(), message_id.to_owned()),
            vec![
                format!("dm_message:{dm_id}/{message_id}"),
                format!("dm_outbox:{dm_id}/{message_id}"),
            ],
        )
        .await
    }

    /// 会話の行・メッセージ・送信待ちを消す（範囲の削除は行を読まない）。
    async fn clear_direct_message_local(&self, dm_id: &str) -> Result<()> {
        let dm_id = dm_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(
                &db.idb,
                &[DM_MESSAGES, DM_OUTBOX, DM_CONVERSATIONS],
                Mode::Write,
            )?;
            let messages: JsValue = prefix(&[text(&dm_id)])?.into();
            rows::delete(&tx, DM_MESSAGES, &messages)?;
            rows::delete(&tx, DM_OUTBOX, &messages)?;
            rows::delete(&tx, DM_CONVERSATIONS, &text(&dm_id))?;
            tx.commit().await
        })
        .await
    }
}
