use super::*;
use crate::public_channels::next_public_channel_cursor;
use crate::{PublicChannelCursor, PublicChannelOrder, PublicChannelRow, PublicChannelStore};

#[async_trait]
impl PublicChannelStore for MemoryStore {
    async fn put_public_channel(&self, mut row: PublicChannelRow, _remote: bool) -> Result<()> {
        let mut rows = self.public_channels.write().await;
        if let Some(previous) = rows.get(row.channel_id.as_str()) {
            row = row.merged_onto(previous);
        }
        rows.insert(row.channel_id.clone(), row);
        Ok(())
    }

    async fn get_public_channel_row(&self, channel_id: &str) -> Result<Option<PublicChannelRow>> {
        Ok(self.public_channels.read().await.get(channel_id).cloned())
    }

    async fn note_public_channel_post(&self, channel_id: &str, posted_at: i64) -> Result<()> {
        if let Some(row) = self.public_channels.write().await.get_mut(channel_id) {
            row.last_post_at = row.last_post_at.max(posted_at);
        }
        Ok(())
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
        let mut items = self
            .public_channels
            .read()
            .await
            .values()
            .filter(|row| {
                row.topic_id == topic_id
                    && cursor.as_ref().is_none_or(|cursor| {
                        (row.sort_at(order), row.channel_id.as_str())
                            < (cursor.at, cursor.channel_id.as_str())
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            (right.sort_at(order), &right.channel_id).cmp(&(left.sort_at(order), &left.channel_id))
        });
        items.truncate(limit);
        Ok(Page {
            next_cursor: next_public_channel_cursor(&items, order, limit),
            items,
        })
    }
}
