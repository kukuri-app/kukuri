//! live session・game room・Dome の projection と live presence（`LiveGameProjectionStore`。native の
//! `sqlite/live_game.rs`）。

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::LiveSessionStatus;
use kukuri_store::{
    DomeConnectionProjectionRow, DomeHostingProjectionRow, GameRoomProjectionRow,
    LiveGameProjectionStore, LiveSessionProjectionRow,
};
use web_sys::IdbTransaction;

use crate::IndexedDbCache;
use crate::content_cache::{DOME_CONNECTIONS, DOME_HOSTING, GAME_ROOMS, LIVE_SESSIONS, PRESENCE};
use crate::idb::Mode;
use crate::rows::{self, Txn, key, num, prefix, text};

#[derive(serde::Serialize, serde::Deserialize)]
struct Presence {
    topic_id: String,
    channel_id: String,
    session_id: String,
    author_pubkey: String,
    expires_at: i64,
    updated_at: i64,
}

/// 視聴者の数は読むときに presence の行から数える（終了した session は 0）。
async fn with_viewers(
    tx: &IdbTransaction,
    mut row: LiveSessionProjectionRow,
) -> Result<LiveSessionProjectionRow> {
    row.viewer_count = if row.status == LiveSessionStatus::Ended {
        0
    } else {
        let range = prefix(&[
            text(&row.topic_id),
            text(&row.channel_id),
            text(&row.session_id),
        ])?;
        rows::count(tx, PRESENCE, None, &range).await?
    };
    Ok(row)
}

#[async_trait]
impl LiveGameProjectionStore for IndexedDbCache {
    /// revision が新しいときだけ置き換える。
    async fn upsert_live_session_cache(&self, row: LiveSessionProjectionRow) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[LIVE_SESSIONS], Mode::Write)?;
            let existing =
                rows::get::<LiveSessionProjectionRow>(&tx, LIVE_SESSIONS, &text(&row.session_id))
                    .await?;
            if existing.is_none_or(|existing| row.revision > existing.revision) {
                rows::put(&tx, LIVE_SESSIONS, &row, &[])?;
            }
            tx.commit().await
        })
        .await
    }

    async fn list_channel_live_sessions(
        &self,
        topic_id: &str,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<LiveSessionProjectionRow>> {
        let (topic, channel) = (topic_id.to_owned(), channel_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[LIVE_SESSIONS, PRESENCE], Mode::Read)?;
            let range = prefix(&[text(&topic), text(&channel)])?;
            let rows: Vec<LiveSessionProjectionRow> =
                rows::scan(&tx, LIVE_SESSIONS, Some("channel"), &range, true, limit).await?;
            let mut counted = Vec::with_capacity(rows.len());
            for row in rows {
                counted.push(with_viewers(&tx, row).await?);
            }
            Ok(counted)
        })
        .await
    }

    async fn get_live_session(
        &self,
        topic_id: &str,
        session_id: &str,
    ) -> Result<Option<LiveSessionProjectionRow>> {
        let (topic, session) = (topic_id.to_owned(), session_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[LIVE_SESSIONS, PRESENCE], Mode::Read)?;
            match rows::get::<LiveSessionProjectionRow>(&tx, LIVE_SESSIONS, &text(&session)).await?
            {
                Some(row) if row.topic_id == topic => Ok(Some(with_viewers(&tx, row).await?)),
                _ => Ok(None),
            }
        })
        .await
    }

    /// score の revision が新しいとき（どちらかに revision が無いときも）だけ置き換える。
    async fn upsert_game_room_cache(&self, row: GameRoomProjectionRow) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[GAME_ROOMS], Mode::Write)?;
            let existing =
                rows::get::<GameRoomProjectionRow>(&tx, GAME_ROOMS, &text(&row.room_id)).await?;
            let newer = existing.is_none_or(|existing| {
                match (row.score_revision, existing.score_revision) {
                    (Some(next), Some(current)) => next > current,
                    _ => true,
                }
            });
            if newer {
                rows::put(&tx, GAME_ROOMS, &row, &[])?;
            }
            tx.commit().await
        })
        .await
    }

    async fn list_channel_game_rooms(
        &self,
        topic_id: &str,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<GameRoomProjectionRow>> {
        let (topic, channel) = (topic_id.to_owned(), channel_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[GAME_ROOMS], Mode::Read)?;
            let range = prefix(&[text(&topic), text(&channel)])?;
            rows::scan(&tx, GAME_ROOMS, Some("channel"), &range, true, limit).await
        })
        .await
    }

    async fn get_game_room(
        &self,
        topic_id: &str,
        room_id: &str,
    ) -> Result<Option<GameRoomProjectionRow>> {
        let (topic, room) = (topic_id.to_owned(), room_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[GAME_ROOMS], Mode::Read)?;
            Ok(
                rows::get::<GameRoomProjectionRow>(&tx, GAME_ROOMS, &text(&room))
                    .await?
                    .filter(|row| row.topic_id == topic),
            )
        })
        .await
    }

    async fn upsert_dome_connection_projection(
        &self,
        row: DomeConnectionProjectionRow,
    ) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DOME_CONNECTIONS], Mode::Write)?;
            rows::put(&tx, DOME_CONNECTIONS, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    async fn get_dome_connection_projection(
        &self,
        context_id: &str,
    ) -> Result<Option<DomeConnectionProjectionRow>> {
        let context = context_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DOME_CONNECTIONS], Mode::Read)?;
            rows::get(&tx, DOME_CONNECTIONS, &text(&context)).await
        })
        .await
    }

    async fn upsert_dome_hosting_projection(&self, row: DomeHostingProjectionRow) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DOME_HOSTING], Mode::Write)?;
            rows::put(&tx, DOME_HOSTING, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    async fn get_dome_hosting_projection(
        &self,
        instance_id: &str,
    ) -> Result<Option<DomeHostingProjectionRow>> {
        let instance = instance_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DOME_HOSTING], Mode::Read)?;
            rows::get(&tx, DOME_HOSTING, &text(&instance)).await
        })
        .await
    }

    async fn upsert_live_presence(
        &self,
        topic_id: &str,
        channel_id: &str,
        session_id: &str,
        author_pubkey: &str,
        expires_at: i64,
        updated_at: i64,
    ) -> Result<()> {
        let row = Presence {
            topic_id: topic_id.to_owned(),
            channel_id: channel_id.to_owned(),
            session_id: session_id.to_owned(),
            author_pubkey: author_pubkey.to_owned(),
            expires_at,
            updated_at,
        };
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRESENCE], Mode::Write)?;
            rows::put(&tx, PRESENCE, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    /// topic の行を範囲で消す（行を読まない）。
    async fn clear_topic_live_presence(&self, topic_id: &str) -> Result<()> {
        let topic = topic_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRESENCE], Mode::Write)?;
            rows::delete(&tx, PRESENCE, &prefix(&[text(&topic)])?.into())?;
            tx.commit().await
        })
        .await
    }

    /// 期限の過ぎた行を、期限の索引の範囲で消す。
    async fn clear_expired_live_presence(&self, now_ms: i64) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PRESENCE], Mode::Write)?;
            let mut keys = Vec::new();
            rows::walk(
                &tx,
                PRESENCE,
                Some("expiry"),
                &expired_range(now_ms)?,
                false,
                |value| {
                    let row: Presence = rows::decode(value)?;
                    keys.push(key(&[
                        text(&row.topic_id),
                        text(&row.channel_id),
                        text(&row.session_id),
                        text(&row.author_pubkey),
                    ]));
                    Ok(true)
                },
            )
            .await?;
            for id in keys {
                rows::delete(&tx, PRESENCE, &id)?;
            }
            tx.commit().await
        })
        .await
    }
}

/// `expires_at <= now` の範囲（期限の索引は数の key）。
fn expired_range(now_ms: i64) -> Result<web_sys::IdbKeyRange> {
    web_sys::IdbKeyRange::upper_bound(&num(now_ms)).map_err(crate::idb::js_error)
}
