//! 公開 channel の一覧の行（#1840）。他人が作った行は remote cache の `public_channel` に数え、回収のときは
//! `delete_cache_item` が行も消す。

use super::remote_cache::{CachePayload, REMOTE_CACHE_CAPACITY_BYTES, now_ms};
use super::*;
use crate::public_channels::next_public_channel_cursor;
use crate::{PublicChannelCursor, PublicChannelOrder, PublicChannelRow, PublicChannelStore};

macro_rules! columns {
    () => {
        "channel_id, topic_id, title, creator_pubkey, created_at, first_post_id, last_post_at, \
         revision, deleted"
    };
}
const COLUMNS: &str = columns!();

/// topic の行を `order` の新しい順に読む SQL。cursor の有無で SQL を分け、(topic, 時刻, ID) の索引の範囲だけを読む
/// (ADR 0052 §5)。`prefix` は試験が `EXPLAIN QUERY PLAN ` を付ける。
pub(crate) fn public_channel_page_query<'a>(
    prefix: &str,
    topic_id: &'a str,
    order: PublicChannelOrder,
    cursor: Option<&'a PublicChannelCursor>,
    limit: usize,
) -> QueryBuilder<Sqlite> {
    let column = match order {
        PublicChannelOrder::LastPost => "last_post_at",
        PublicChannelOrder::Created => "created_at",
    };
    let mut query = QueryBuilder::<Sqlite>::new(format!(
        "{prefix}SELECT {COLUMNS} FROM public_channels WHERE topic_id = "
    ));
    query.push_bind(topic_id);
    if let Some(cursor) = cursor {
        query.push(format!(" AND ({column}, channel_id) < ("));
        query.push_bind(cursor.at);
        query.push(", ");
        query.push_bind(cursor.channel_id.as_str());
        query.push(")");
    }
    query.push(format!(" ORDER BY {column} DESC, channel_id DESC LIMIT "));
    query.push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
    query
}

fn public_channel_row(row: &sqlx::sqlite::SqliteRow) -> Result<PublicChannelRow> {
    Ok(PublicChannelRow {
        channel_id: row.try_get("channel_id")?,
        topic_id: row.try_get("topic_id")?,
        title: row.try_get("title")?,
        creator_pubkey: row.try_get("creator_pubkey")?,
        created_at: row.try_get("created_at")?,
        first_post_id: row.try_get("first_post_id")?,
        last_post_at: row.try_get("last_post_at")?,
        revision: row.try_get("revision")?,
        deleted: row.try_get("deleted")?,
    })
}

#[async_trait]
impl PublicChannelStore for SqliteStore {
    async fn put_public_channel(&self, row: PublicChannelRow, remote: bool) -> Result<()> {
        let _gate = self.remote_cache_gate.lock().await;
        let budget = REMOTE_CACHE_CAPACITY_BYTES.saturating_sub(i64::try_from(
            self.remote_cache_reserved
                .load(std::sync::atomic::Ordering::Acquire),
        )?);
        let mut tx = self.pool.begin().await?;
        // 他の remote cache の書込と同じく、最初の文で書込の lock を取る。
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
            .execute(&mut *tx)
            .await?;
        let mut label_evictions = Vec::new();
        let mut removed_files = Vec::new();
        if remote {
            let charge = i64::try_from(serde_json::to_vec(&row)?.len())? * 2 + 512;
            anyhow::ensure!(
                self.put_remote_content_in_tx(
                    &mut tx,
                    "public_channel",
                    row.channel_id.as_str(),
                    row.topic_id.as_str(),
                    CachePayload::Bytes(&[]),
                    None,
                    None,
                    budget,
                    now_ms()?,
                    Some(charge),
                    &mut label_evictions,
                    &mut removed_files,
                )
                .await?,
                "remote public channel cache capacity exceeded"
            );
        }
        // タイトルと削除は版の大きい方を残す(`PublicChannelRow::merged_onto` と同じ)。
        sqlx::query(concat!(
            "INSERT INTO public_channels (",
            columns!(),
            ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT(channel_id) DO UPDATE SET \
             last_post_at = MAX(public_channels.last_post_at, excluded.last_post_at), \
             title = IIF(excluded.revision > public_channels.revision, excluded.title, \
             public_channels.title), \
             deleted = IIF(excluded.revision > public_channels.revision, excluded.deleted, \
             public_channels.deleted), \
             revision = MAX(public_channels.revision, excluded.revision)"
        ))
        .bind(&row.channel_id)
        .bind(&row.topic_id)
        .bind(&row.title)
        .bind(&row.creator_pubkey)
        .bind(row.created_at)
        .bind(&row.first_post_id)
        .bind(row.last_post_at)
        .bind(row.revision)
        .bind(row.deleted)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.publish_adult_label_evictions(label_evictions);
        self.remove_remote_blob_files(removed_files).await
    }

    async fn get_public_channel_row(&self, channel_id: &str) -> Result<Option<PublicChannelRow>> {
        let Some(row) = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM public_channels WHERE channel_id = ?1"
        ))
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await?
        .as_ref()
        .map(public_channel_row)
        .transpose()?
        else {
            return Ok(None);
        };
        // 回収の期限を過ぎた他人の行は、回収の前でも無いものとして扱う(照合が確かめて置き直す。投稿の行と同じ)。
        let expired = self
            .expired_remote_keys("public_channel", &[channel_id])
            .await?;
        Ok((!expired.contains(channel_id)).then_some(row))
    }

    async fn note_public_channel_post(&self, channel_id: &str, posted_at: i64) -> Result<()> {
        sqlx::query(
            "UPDATE public_channels SET last_post_at = ?2 WHERE channel_id = ?1 AND last_post_at < ?2",
        )
        .bind(channel_id)
        .bind(posted_at)
        .execute(&self.pool)
        .await?;
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
        let mut items = public_channel_page_query("", topic_id, order, cursor.as_ref(), limit)
            .build()
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(public_channel_row)
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = next_public_channel_cursor(&items, order, limit);
        let keys = items
            .iter()
            .map(|row| row.channel_id.as_str())
            .collect::<Vec<_>>();
        let expired = self.expired_remote_keys("public_channel", &keys).await?;
        items.retain(|row| !expired.contains(row.channel_id.as_str()));
        Ok(Page { items, next_cursor })
    }
}
