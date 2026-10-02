use anyhow::Result;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::SqliteStore;
use crate::{
    PrivateChannelEpochRange, PrivateChannelEpochRow, PrivateChannelFilter, PrivateChannelKeyStore,
    PrivateChannelRow,
};

// sqlx は固定の SQL だけを受け付けるので、列の一覧は macro で埋め込む。
macro_rules! channel_columns {
    () => {
        "channel_key, topic_id, channel_id, label, creator_pubkey, owner_pubkey, joined_via_pubkey, \
         audience_kind, current_epoch_id, controller, joined, updated_at, op_id"
    };
}
macro_rules! epoch_columns {
    () => {
        "channel_id, epoch_id, started_at, receive_key_id, updated_at, sealed_secret"
    };
}

// 一覧の SQL。絞り込みごとに分け、それぞれ部分索引(`idx_private_channels_joined`・`_topic`・`_owner`)の範囲で読む。
pub(crate) const LIST_JOINED_ALL: &str = concat!(
    "SELECT ",
    channel_columns!(),
    " FROM private_channels WHERE joined = 1 AND channel_key > ? ORDER BY channel_key LIMIT ?"
);
pub(crate) const LIST_JOINED_BY_TOPIC: &str = concat!(
    "SELECT ",
    channel_columns!(),
    " FROM private_channels WHERE joined = 1 AND topic_id = ? AND channel_key > ?
     ORDER BY channel_key LIMIT ?"
);
pub(crate) const LIST_JOINED_BY_OWNER: &str = concat!(
    "SELECT ",
    channel_columns!(),
    " FROM private_channels WHERE joined = 1 AND owner_pubkey = ? AND channel_key > ?
     ORDER BY channel_key LIMIT ?"
);

fn channel_row(row: &SqliteRow) -> PrivateChannelRow {
    PrivateChannelRow {
        channel_key: row.get("channel_key"),
        topic_id: row.get("topic_id"),
        channel_id: row.get("channel_id"),
        label: row.get("label"),
        creator_pubkey: row.get("creator_pubkey"),
        owner_pubkey: row.get("owner_pubkey"),
        joined_via_pubkey: row.get("joined_via_pubkey"),
        audience_kind: row.get("audience_kind"),
        current_epoch_id: row.get("current_epoch_id"),
        controller: row.get("controller"),
        joined: row.get("joined"),
        updated_at: row.get("updated_at"),
        op_id: row.get("op_id"),
    }
}

fn epoch_row(row: &SqliteRow) -> PrivateChannelEpochRow {
    PrivateChannelEpochRow {
        channel_id: row.get("channel_id"),
        epoch_id: row.get("epoch_id"),
        started_at: row.get("started_at"),
        receive_key_id: row.get("receive_key_id"),
        updated_at: row.get("updated_at"),
        sealed_secret: row.get("sealed_secret"),
    }
}

#[async_trait::async_trait]
impl PrivateChannelKeyStore for SqliteStore {
    async fn put_private_channel(
        &self,
        row: &PrivateChannelRow,
        epochs: &[PrivateChannelEpochRow],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(concat!(
            "INSERT OR REPLACE INTO private_channels (",
            channel_columns!(),
            ") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(&row.channel_key)
        .bind(&row.topic_id)
        .bind(&row.channel_id)
        .bind(&row.label)
        .bind(&row.creator_pubkey)
        .bind(&row.owner_pubkey)
        .bind(&row.joined_via_pubkey)
        .bind(&row.audience_kind)
        .bind(&row.current_epoch_id)
        .bind(&row.controller)
        .bind(row.joined)
        .bind(row.updated_at)
        .bind(&row.op_id)
        .execute(&mut *tx)
        .await?;
        for epoch in epochs {
            sqlx::query(concat!(
                "INSERT OR IGNORE INTO private_channel_epochs (",
                epoch_columns!(),
                ") VALUES (?, ?, ?, ?, ?, ?)"
            ))
            .bind(&epoch.channel_id)
            .bind(&epoch.epoch_id)
            .bind(epoch.started_at)
            .bind(&epoch.receive_key_id)
            .bind(epoch.updated_at)
            .bind(&epoch.sealed_secret)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn put_private_channel_epoch(&self, epoch: &PrivateChannelEpochRow) -> Result<bool> {
        let result = sqlx::query(concat!(
            "INSERT OR IGNORE INTO private_channel_epochs (",
            epoch_columns!(),
            ") VALUES (?, ?, ?, ?, ?, ?)"
        ))
        .bind(&epoch.channel_id)
        .bind(&epoch.epoch_id)
        .bind(epoch.started_at)
        .bind(&epoch.receive_key_id)
        .bind(epoch.updated_at)
        .bind(&epoch.sealed_secret)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn get_private_channel(&self, channel_key: &str) -> Result<Option<PrivateChannelRow>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            channel_columns!(),
            " FROM private_channels WHERE channel_key = ?"
        ))
        .bind(channel_key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(channel_row))
    }

    async fn get_private_channel_by_id(
        &self,
        channel_id: &str,
    ) -> Result<Option<PrivateChannelRow>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            channel_columns!(),
            " FROM private_channels WHERE channel_id = ? LIMIT 1"
        ))
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(channel_row))
    }

    async fn list_joined_private_channels(
        &self,
        filter: PrivateChannelFilter<'_>,
        after: &str,
        limit: usize,
    ) -> Result<Vec<PrivateChannelRow>> {
        let query = match filter {
            PrivateChannelFilter::All => sqlx::query(LIST_JOINED_ALL),
            PrivateChannelFilter::Topic(topic_id) => {
                sqlx::query(LIST_JOINED_BY_TOPIC).bind(topic_id)
            }
            PrivateChannelFilter::Owner(owner_pubkey) => {
                sqlx::query(LIST_JOINED_BY_OWNER).bind(owner_pubkey)
            }
        };
        let rows = query
            .bind(after)
            .bind(i64::try_from(limit)?)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(channel_row).collect())
    }

    async fn get_private_channel_epoch(
        &self,
        channel_id: &str,
        epoch_id: &str,
    ) -> Result<Option<PrivateChannelEpochRow>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            epoch_columns!(),
            " FROM private_channel_epochs WHERE channel_id = ? AND epoch_id = ?"
        ))
        .bind(channel_id)
        .bind(epoch_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(epoch_row))
    }

    async fn find_private_channel_epoch(
        &self,
        receive_key_id: &str,
    ) -> Result<Option<PrivateChannelEpochRow>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            epoch_columns!(),
            " FROM private_channel_epochs WHERE receive_key_id = ? LIMIT 1"
        ))
        .bind(receive_key_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(epoch_row))
    }

    async fn list_private_channel_epochs(
        &self,
        channel_id: &str,
        range: PrivateChannelEpochRange,
        limit: usize,
    ) -> Result<Vec<PrivateChannelEpochRow>> {
        let (query, at) = match range {
            PrivateChannelEpochRange::AtOrBefore(at) => (
                concat!(
                    "SELECT ",
                    epoch_columns!(),
                    " FROM private_channel_epochs WHERE channel_id = ? AND started_at <= ?
                     ORDER BY started_at DESC, epoch_id DESC LIMIT ?"
                ),
                at,
            ),
            PrivateChannelEpochRange::After(at) => (
                concat!(
                    "SELECT ",
                    epoch_columns!(),
                    " FROM private_channel_epochs WHERE channel_id = ? AND started_at > ?
                     ORDER BY started_at ASC, epoch_id ASC LIMIT ?"
                ),
                at,
            ),
        };
        let rows = sqlx::query(query)
            .bind(channel_id)
            .bind(at)
            .bind(i64::try_from(limit)?)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(epoch_row).collect())
    }

    async fn delete_private_channel_epochs(
        &self,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "DELETE FROM private_channel_epochs WHERE channel_id = ?1 AND epoch_id IN
               (SELECT epoch_id FROM private_channel_epochs WHERE channel_id = ?1 LIMIT ?2)
             RETURNING epoch_id",
        )
        .bind(channel_id)
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?)
    }
}
