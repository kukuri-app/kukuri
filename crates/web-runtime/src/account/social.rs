//! 作者の関係・mute・docs author・private channel の参加者（`SocialProjectionStore`。native の `sqlite/social.rs`）。

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{FollowEdge, FollowEdgeStatus, Profile};
use kukuri_store::{
    AuthorRelationshipProjectionRow, MutedAuthorRow, PrivateChannelParticipantRow,
    SocialProjectionStore,
};
use wasm_bindgen::JsValue;

use super::now_ms;
use crate::IndexedDbCache;
use crate::content_cache::{DOCS_AUTHORS, FOLLOWS, MUTES, PARTICIPANTS};
use crate::idb::Mode;
use crate::rows::{self, Txn, between, key, only, prefix, text, top};

#[derive(serde::Serialize, serde::Deserialize)]
struct DocsAuthor {
    author_pubkey: String,
    docs_author: String,
    updated_at: i64,
}

/// 参加中の行だけが載る索引の値（`active` は channel と公開鍵、`active_epoch` は世代も）。
fn participant_extra(row: &PrivateChannelParticipantRow) -> [(&'static str, JsValue); 2] {
    if row.left_at.is_some() {
        return [
            ("active", JsValue::UNDEFINED),
            ("active_epoch", JsValue::UNDEFINED),
        ];
    }
    let (channel, epoch, pubkey) = (
        text(&row.channel_id),
        text(&row.epoch_id),
        text(&row.participant_pubkey),
    );
    [
        ("active", key(&[channel.clone(), pubkey.clone()])),
        ("active_epoch", key(&[channel, epoch, pubkey])),
    ]
}

fn participant_key(row: &PrivateChannelParticipantRow) -> JsValue {
    key(&[
        text(&row.channel_id),
        text(&row.epoch_id),
        text(&row.participant_pubkey),
    ])
}

#[async_trait]
impl SocialProjectionStore for IndexedDbCache {
    /// native の `profile_cache` はどの読み出しも引かない（書くだけの表）。Web は行を作らない。
    async fn upsert_profile_cache(&self, _profile: Profile) -> Result<()> {
        Ok(())
    }

    /// 対象の author の edge から求める（主 key の 2 回の点読みと、自分の follow の数までの点読み）。
    async fn get_author_relationship(
        &self,
        local_author_pubkey: &str,
        author_pubkey: &str,
    ) -> Result<Option<AuthorRelationshipProjectionRow>> {
        let (local, author) = (local_author_pubkey.to_owned(), author_pubkey.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[FOLLOWS], Mode::Read)?;
            let active = async |subject: &str, target: &str| -> Result<bool> {
                Ok(
                    rows::get::<FollowEdge>(&tx, FOLLOWS, &key(&[text(subject), text(target)]))
                        .await?
                        .is_some_and(|edge| edge.status == FollowEdgeStatus::Active),
                )
            };
            let following = active(&local, &author).await?;
            let followed_by = active(&author, &local).await?;
            let mut via = Vec::new();
            if !following {
                let mine: Vec<FollowEdge> = rows::scan(
                    &tx,
                    FOLLOWS,
                    None,
                    &prefix(&[text(&local)])?,
                    false,
                    usize::MAX,
                )
                .await?;
                for edge in mine {
                    if edge.status == FollowEdgeStatus::Active
                        && active(edge.target_pubkey.as_str(), &author).await?
                    {
                        via.push(edge.target_pubkey.as_str().to_owned());
                    }
                }
            }
            Ok(AuthorRelationshipProjectionRow::derive(
                &local,
                &author,
                following,
                followed_by,
                via,
            ))
        })
        .await
    }

    async fn put_muted_author(&self, row: MutedAuthorRow) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[MUTES], Mode::Write)?;
            rows::put(&tx, MUTES, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    async fn get_muted_author(&self, author_pubkey: &str) -> Result<Option<MutedAuthorRow>> {
        let author = author_pubkey.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[MUTES], Mode::Read)?;
            rows::get(&tx, MUTES, &text(&author)).await
        })
        .await
    }

    /// 全件の一覧（native と同じく上限の無い API）。新しい順・公開鍵の昇順。
    async fn list_muted_authors(&self) -> Result<Vec<MutedAuthorRow>> {
        self.run(|db| async move {
            let tx = Txn::begin(&db.idb, &[MUTES], Mode::Read)?;
            let all = web_sys::IdbKeyRange::lower_bound(&text("")).map_err(crate::idb::js_error)?;
            let mut rows: Vec<MutedAuthorRow> =
                rows::scan(&tx, MUTES, None, &all, false, usize::MAX).await?;
            rows.sort_by(|left, right| {
                right
                    .muted_at
                    .cmp(&left.muted_at)
                    .then_with(|| left.author_pubkey.cmp(&right.author_pubkey))
            });
            Ok(rows)
        })
        .await
    }

    async fn remove_muted_author(&self, author_pubkey: &str) -> Result<()> {
        let author = author_pubkey.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[MUTES], Mode::Write)?;
            rows::delete(&tx, MUTES, &text(&author))?;
            tx.commit().await
        })
        .await
    }

    async fn get_author_docs_author(&self, author_pubkey: &str) -> Result<Option<String>> {
        let author = author_pubkey.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DOCS_AUTHORS], Mode::Read)?;
            Ok(rows::get::<DocsAuthor>(&tx, DOCS_AUTHORS, &text(&author))
                .await?
                .map(|row| row.docs_author))
        })
        .await
    }

    async fn put_author_docs_author(&self, author_pubkey: &str, docs_author: &str) -> Result<()> {
        let row = DocsAuthor {
            author_pubkey: author_pubkey.to_owned(),
            docs_author: docs_author.to_owned(),
            updated_at: now_ms()?,
        };
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[DOCS_AUTHORS], Mode::Write)?;
            rows::put(&tx, DOCS_AUTHORS, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    /// 同じ (channel, epoch, 公開鍵) の行より新しいときだけ置き換える。退出なら、同じ channel のそれより古い参加中の行も
    /// 退出にする（その公開鍵の世代の数だけ読む）。
    async fn put_private_channel_participant(
        &self,
        row: PrivateChannelParticipantRow,
    ) -> Result<bool> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PARTICIPANTS], Mode::Write)?;
            let existing = rows::get::<PrivateChannelParticipantRow>(
                &tx,
                PARTICIPANTS,
                &participant_key(&row),
            )
            .await?;
            if existing.is_some_and(|existing| existing.updated_at >= row.updated_at) {
                return Ok(false);
            }
            rows::put(&tx, PARTICIPANTS, &row, &participant_extra(&row))?;
            if let Some(left_at) = row.left_at {
                let active = only(&key(&[
                    text(&row.channel_id),
                    text(&row.participant_pubkey),
                ]))?;
                let earlier: Vec<PrivateChannelParticipantRow> = rows::scan(
                    &tx,
                    PARTICIPANTS,
                    Some("active"),
                    &active,
                    false,
                    usize::MAX,
                )
                .await?;
                for mut other in earlier {
                    if other.updated_at < left_at {
                        other.left_at = Some(left_at);
                        other.updated_at = left_at;
                        rows::put(&tx, PARTICIPANTS, &other, &participant_extra(&other))?;
                    }
                }
            }
            tx.commit().await?;
            Ok(true)
        })
        .await
    }

    async fn has_private_channel_participant(
        &self,
        channel_id: &str,
        participant_pubkey: &str,
    ) -> Result<bool> {
        let (channel, pubkey) = (channel_id.to_owned(), participant_pubkey.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PARTICIPANTS], Mode::Read)?;
            let range = only(&key(&[text(&channel), text(&pubkey)]))?;
            Ok(rows::count(&tx, PARTICIPANTS, Some("pubkey"), &range).await? > 0)
        })
        .await
    }

    async fn list_private_channel_participants(
        &self,
        channel_id: &str,
        epoch_id: Option<&str>,
        after: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let (channel, epoch, after) = (
            channel_id.to_owned(),
            epoch_id.map(str::to_owned),
            after.to_owned(),
        );
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PARTICIPANTS], Mode::Read)?;
            let mut head = vec![text(&channel)];
            let index = match &epoch {
                Some(epoch) => {
                    head.push(text(epoch));
                    "active_epoch"
                }
                None => "active",
            };
            let mut lower = head.clone();
            lower.push(text(&after));
            let range = between(&lower, &top(&head), true, false)?;
            // 世代を問わないときは、同じ公開鍵の行（参加中の世代の数）が続くので重複を除く。
            let mut pubkeys: Vec<String> = Vec::new();
            if limit > 0 {
                rows::walk(&tx, PARTICIPANTS, Some(index), &range, false, |value| {
                    let row: PrivateChannelParticipantRow = rows::decode(value)?;
                    if pubkeys.last() != Some(&row.participant_pubkey) {
                        pubkeys.push(row.participant_pubkey);
                    }
                    Ok(pubkeys.len() < limit)
                })
                .await?;
            }
            Ok(pubkeys)
        })
        .await
    }

    async fn count_private_channel_participants(
        &self,
        channel_id: &str,
        epoch_id: &str,
    ) -> Result<usize> {
        let (channel, epoch) = (channel_id.to_owned(), epoch_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PARTICIPANTS], Mode::Read)?;
            let range = prefix(&[text(&channel), text(&epoch)])?;
            rows::count(&tx, PARTICIPANTS, Some("active_epoch"), &range).await
        })
        .await
    }
}
