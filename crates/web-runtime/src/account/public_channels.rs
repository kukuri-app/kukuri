//! 公開 channel の一覧の行（`PublicChannelStore`。native の `sqlite/public_channels.rs`）。他人が作った行は内容の
//! cache の `public_channel` に数え、回収のときは `Tx::drop_item` が行も消す。

use anyhow::{Result, ensure};
use async_trait::async_trait;
use kukuri_store::{
    Page, PublicChannelCursor, PublicChannelOrder, PublicChannelRow, PublicChannelStore,
    next_public_channel_cursor,
};

use super::now_ms;
use crate::IndexedDbCache;
use crate::content_cache::{PUBLIC_CHANNELS, Tx, place};
use crate::idb::Mode;
use crate::rows::{self, Txn, between, num, prefix, text};

#[async_trait]
impl PublicChannelStore for IndexedDbCache {
    async fn put_public_channel(&self, mut row: PublicChannelRow, remote: bool) -> Result<()> {
        let budget = self.budget();
        self.run(move |db| async move {
            let tx = Tx::begin_with(&db, &[PUBLIC_CHANNELS])?;
            if remote {
                let charge = i64::try_from(serde_json::to_vec(&row)?.len())? * 2 + 512;
                ensure!(
                    place(
                        &tx,
                        (
                            "public_channel",
                            row.channel_id.as_str(),
                            row.topic_id.as_str()
                        ),
                        &[],
                        None,
                        budget,
                        now_ms()?,
                        Some(charge),
                    )
                    .await?,
                    "remote public channel cache capacity exceeded"
                );
            }
            if let Some(previous) =
                rows::get::<PublicChannelRow>(&tx, PUBLIC_CHANNELS, &text(&row.channel_id)).await?
            {
                row = row.merged_onto(&previous);
            }
            rows::put(&tx, PUBLIC_CHANNELS, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    async fn get_public_channel_row(&self, channel_id: &str) -> Result<Option<PublicChannelRow>> {
        let key = channel_id.to_owned();
        let Some(row) = self
            .run(move |db| async move {
                let tx = Txn::begin(&db.idb, &[PUBLIC_CHANNELS], Mode::Read)?;
                rows::get::<PublicChannelRow>(&tx, PUBLIC_CHANNELS, &text(&key)).await
            })
            .await?
        else {
            return Ok(None);
        };
        // 回収の期限を過ぎた他人の行は、回収の前でも無いものとして扱う(native と同じ)。
        let expired = self
            .expired_remote_keys("public_channel", vec![channel_id.to_owned()])
            .await?;
        Ok((!expired.contains(channel_id)).then_some(row))
    }

    async fn note_public_channel_post(&self, channel_id: &str, posted_at: i64) -> Result<()> {
        let channel_id = channel_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PUBLIC_CHANNELS], Mode::Write)?;
            if let Some(mut row) =
                rows::get::<PublicChannelRow>(&tx, PUBLIC_CHANNELS, &text(&channel_id)).await?
                && row.last_post_at < posted_at
            {
                row.last_post_at = posted_at;
                rows::put(&tx, PUBLIC_CHANNELS, &row, &[])?;
            }
            tx.commit().await
        })
        .await
    }

    async fn list_public_channels(
        &self,
        topic_id: &str,
        order: PublicChannelOrder,
        cursor: Option<PublicChannelCursor>,
        limit: usize,
    ) -> Result<Page<PublicChannelRow, PublicChannelCursor>> {
        if limit == 0 {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: cursor,
            });
        }
        let topic = topic_id.to_owned();
        let mut items = self
            .run(move |db| async move {
                let tx = Txn::begin(&db.idb, &[PUBLIC_CHANNELS], Mode::Read)?;
                let head = [text(&topic)];
                let range = match &cursor {
                    Some(cursor) => {
                        let mut upper = head.to_vec();
                        upper.extend([num(cursor.at), text(&cursor.channel_id)]);
                        between(&head, &upper, false, true)?
                    }
                    None => prefix(&head)?,
                };
                let index = match order {
                    PublicChannelOrder::LastPost => "last_post",
                    PublicChannelOrder::Created => "created",
                };
                rows::scan::<PublicChannelRow>(
                    &tx,
                    PUBLIC_CHANNELS,
                    Some(index),
                    &range,
                    true,
                    limit,
                )
                .await
            })
            .await?;
        let next_cursor = next_public_channel_cursor(&items, order, limit);
        let keys = items
            .iter()
            .map(|row| row.channel_id.clone())
            .collect::<Vec<_>>();
        let expired = self.expired_remote_keys("public_channel", keys).await?;
        items.retain(|row| !expired.contains(row.channel_id.as_str()));
        Ok(Page { items, next_cursor })
    }
}
