//! 公開 channel（公開スレッド。#1840、ADR 0068）の一覧の行。一覧の表示は手元の行だけを読む。他人が作った行は
//! remote cache の容量に数え、7 日使わなければ回収する。自分が作った行は数えず、回収しない。native は
//! `SqliteStore`、Web は IndexedDB の store が実装する。

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Page;

/// 一覧の 1 行。時刻はどちらもミリ秒。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicChannelRow {
    pub channel_id: String,
    pub topic_id: String,
    pub title: String,
    pub creator_pubkey: String,
    pub created_at: i64,
    pub first_post_id: Option<String>,
    /// 最後の投稿の時刻。投稿が無ければ作成時刻。
    pub last_post_at: i64,
}

impl PublicChannelRow {
    pub fn sort_at(&self, order: PublicChannelOrder) -> i64 {
        match order {
            PublicChannelOrder::LastPost => self.last_post_at,
            PublicChannelOrder::Created => self.created_at,
        }
    }

    pub fn cursor(&self, order: PublicChannelOrder) -> PublicChannelCursor {
        PublicChannelCursor {
            at: self.sort_at(order),
            channel_id: self.channel_id.clone(),
        }
    }
}

/// 一覧の並び順（2026-10-17 のユーザー判断 2）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PublicChannelOrder {
    /// 最後の投稿の新しい順。
    #[default]
    LastPost,
    /// 作成の新しい順。
    Created,
}

/// 一覧の続きの位置。`at` は並び順の時刻（ミリ秒）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PublicChannelCursor {
    pub at: i64,
    pub channel_id: String,
}

#[async_trait]
pub trait PublicChannelStore: Send + Sync {
    /// 行を置く。`remote`（他人が作った channel）の行は remote cache の容量に数え、収まらなければ失敗にする。
    /// 既にある行は、最後の投稿の時刻の新しい方を残す。
    async fn put_public_channel(&self, row: PublicChannelRow, remote: bool) -> Result<()>;
    async fn get_public_channel_row(&self, channel_id: &str) -> Result<Option<PublicChannelRow>>;
    /// 最後の投稿の時刻を、新しいときだけ進める。行が無ければ何もしない。
    async fn note_public_channel_post(&self, channel_id: &str, posted_at: i64) -> Result<()>;
    /// topic の行を `order` の新しい順に、`cursor` より後ろから `limit` 件まで返す。索引の範囲だけを読む。
    /// 回収の期限を過ぎた他人の行は返さず、返した行は使った時刻を更新する。
    async fn list_public_channels(
        &self,
        topic_id: &str,
        order: PublicChannelOrder,
        cursor: Option<PublicChannelCursor>,
        limit: usize,
    ) -> Result<Page<PublicChannelRow, PublicChannelCursor>>;
}

/// 新しい順の 1 ページの続きの位置（`limit` 件に届いたときだけ、最後の行の位置）。
pub fn next_public_channel_cursor(
    items: &[PublicChannelRow],
    order: PublicChannelOrder,
    limit: usize,
) -> Option<PublicChannelCursor> {
    (items.len() == limit)
        .then(|| items.last().map(|row| row.cursor(order)))
        .flatten()
}
