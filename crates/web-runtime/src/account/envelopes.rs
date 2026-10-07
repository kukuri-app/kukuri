//! 署名済み envelope と、それから作る profile・follow・block の行（`Store`。native の `sqlite/envelopes.rs`・`social.rs`）。

use std::cmp::Ordering;
use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{
    BlockEdge, EnvelopeId, FollowEdge, KukuriEnvelope, Profile, ThreadRef, parse_block_edge,
    parse_follow_edge, parse_profile,
};
use kukuri_store::{Page, Store, TimelineCursor};
use serde::Serialize;
use serde::de::DeserializeOwned;
use web_sys::IdbTransaction;

use super::social::{PARTICIPANT_STORES, restale_participants};
use super::{newest_first, next_cursor};
use crate::IndexedDbCache;
use crate::content_cache::{BLOCKS, ENVELOPES, FOLLOWS, PROFILES};
use crate::idb::{self, Mode, js_error};
use crate::rows::{self, Txn, between, key, num, prefix, text, top};

fn envelope_cursor(envelope: &KukuriEnvelope) -> TimelineCursor {
    TimelineCursor {
        created_at: envelope.created_at,
        object_id: envelope.id.clone(),
    }
}

/// 同じ (subject, target) の行より古くなければ置き換える（native と同じ）。
async fn put_edge<T: Serialize + DeserializeOwned>(
    tx: &IdbTransaction,
    name: &str,
    (subject, target, updated_at): (&str, &str, i64),
    edge: &T,
    updated: impl Fn(&T) -> i64,
) -> Result<()> {
    let id = key(&[text(subject), text(target)]);
    if let Some(existing) = rows::get::<T>(tx, name, &id).await?
        && updated(&existing) > updated_at
    {
        return Ok(());
    }
    rows::put(tx, name, edge, &[])
}

/// follow の edge を置き、両端の公開鍵の参加者の資格喪失の印を読み直す（#1219 AC-2）。
async fn put_follow_edge(tx: &IdbTransaction, edge: &FollowEdge) -> Result<()> {
    let (subject, target) = (edge.subject_pubkey.as_str(), edge.target_pubkey.as_str());
    put_edge(
        tx,
        FOLLOWS,
        (subject, target, edge.updated_at),
        edge,
        |edge: &FollowEdge| edge.updated_at,
    )
    .await?;
    restale_participants(tx, subject).await?;
    restale_participants(tx, target).await
}

async fn put_profile(tx: &IdbTransaction, profile: &Profile) -> Result<()> {
    if let Some(existing) =
        rows::get::<Profile>(tx, PROFILES, &text(profile.pubkey.as_str())).await?
        && existing.updated_at > profile.updated_at
    {
        return Ok(());
    }
    rows::put(tx, PROFILES, profile, &[])
}

/// subject（主 key の先頭）か target（索引）の edge を、新しい順・相手の昇順に並べる。
async fn edges<T: DeserializeOwned + Send + 'static>(
    store: &IndexedDbCache,
    name: &'static str,
    pubkey: &str,
    by_target: bool,
    order: impl Fn(&T) -> (i64, String) + Send + 'static,
) -> Result<Vec<T>> {
    let pubkey = pubkey.to_owned();
    store
        .run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[name], Mode::Read)?;
            let (index, range) = if by_target {
                (Some("target"), rows::only(&text(&pubkey))?)
            } else {
                (None, prefix(&[text(&pubkey)])?)
            };
            let mut rows: Vec<T> = rows::scan(&tx, name, index, &range, false, usize::MAX).await?;
            rows.sort_by(|left, right| {
                let (left, right) = (order(left), order(right));
                right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1))
            });
            Ok(rows)
        })
        .await
}

/// subject（主 key の先頭）の edge を、相手の昇順に `after` より後から `limit` 件。
async fn edges_after<T: DeserializeOwned + Send + 'static>(
    store: &IndexedDbCache,
    name: &'static str,
    subject: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<T>> {
    let (subject, after) = (subject.to_owned(), after.map(str::to_owned));
    store
        .run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[name], Mode::Read)?;
            let range = match &after {
                Some(after) => between(
                    &[text(&subject), text(after)],
                    &top(&[text(&subject)]),
                    true,
                    false,
                )?,
                None => prefix(&[text(&subject)])?,
            };
            rows::scan(&tx, name, None, &range, false, limit).await
        })
        .await
}

#[async_trait]
impl Store for IndexedDbCache {
    async fn put_envelope(&self, envelope: KukuriEnvelope) -> Result<()> {
        let profile = parse_profile(&envelope)?;
        let follow = parse_follow_edge(&envelope)?;
        let block = parse_block_edge(&envelope)?;
        self.run(move |db| async move {
            let mut stores = vec![ENVELOPES, PROFILES, BLOCKS];
            stores.extend(PARTICIPANT_STORES);
            let tx = Txn::begin(&db.idb, &stores, Mode::Write)?;
            let topic = envelope.topic_id();
            let thread = envelope.thread_ref().unwrap_or(ThreadRef {
                root: envelope.id.clone(),
                reply_to: None,
            });
            let extra = match &topic {
                Some(topic) => [
                    ("topic", text(topic.as_str())),
                    ("root", text(thread.root.as_str())),
                ],
                None => [
                    ("topic", wasm_bindgen::JsValue::UNDEFINED),
                    ("root", wasm_bindgen::JsValue::UNDEFINED),
                ],
            };
            rows::put(&tx, ENVELOPES, &envelope, &extra)?;
            if let Some(profile) = profile {
                put_profile(&tx, &profile).await?;
            }
            if let Some(edge) = follow {
                put_follow_edge(&tx, &edge).await?;
            }
            if let Some(edge) = block {
                let id = (
                    edge.subject_pubkey.as_str(),
                    edge.target_pubkey.as_str(),
                    edge.updated_at,
                );
                put_edge(&tx, BLOCKS, id, &edge, |edge: &BlockEdge| edge.updated_at).await?;
            }
            tx.commit().await
        })
        .await
    }

    async fn get_envelope(&self, envelope_id: &EnvelopeId) -> Result<Option<KukuriEnvelope>> {
        let id = envelope_id.as_str().to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[ENVELOPES], Mode::Read)?;
            rows::get(&tx, ENVELOPES, &text(&id)).await
        })
        .await
    }

    async fn list_topic_timeline(
        &self,
        topic_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<KukuriEnvelope>> {
        let topic = topic_id.to_owned();
        let items = self
            .run(move |db| async move {
                let tx = Txn::begin(&db.idb, &[ENVELOPES], Mode::Read)?;
                let range = newest_first(&[text(&topic)], cursor.as_ref())?;
                rows::scan(&tx, ENVELOPES, Some("topic"), &range, true, limit).await
            })
            .await?;
        let next_cursor = next_cursor(&items, limit, envelope_cursor);
        Ok(Page { items, next_cursor })
    }

    /// root（cursor より後なら）を先頭に、返信を古い順に（native の `store_list_thread_impl`）。
    async fn list_thread(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<KukuriEnvelope>> {
        let (topic, root) = (
            topic_id.to_owned(),
            thread_root_object_id.as_str().to_owned(),
        );
        let items = self
            .run(move |db| async move {
                let mut items = Vec::new();
                if limit == 0 {
                    return Ok(items);
                }
                let tx = Txn::begin(&db.idb, &[ENVELOPES], Mode::Read)?;
                let after = |envelope: &KukuriEnvelope| {
                    cursor.as_ref().is_none_or(|cursor| {
                        (envelope.created_at, envelope.id.as_str())
                            > (cursor.created_at, cursor.object_id.as_str())
                    })
                };
                if let Some(head) =
                    rows::get::<KukuriEnvelope>(&tx, ENVELOPES, &text(&root)).await?
                    && head.topic_id().is_some_and(|id| id.as_str() == topic)
                    && head
                        .thread_ref()
                        .is_none_or(|thread| thread.root.as_str() == root)
                    && after(&head)
                {
                    items.push(head);
                }
                let head = [text(&topic), text(&root)];
                let range = match &cursor {
                    Some(cursor) => {
                        let mut lower = head.to_vec();
                        lower.extend([num(cursor.created_at), text(cursor.object_id.as_str())]);
                        between(&lower, &top(&head), true, false)?
                    }
                    None => prefix(&head)?,
                };
                let remaining = limit - items.len();
                let mut replies = Vec::new();
                if remaining > 0 {
                    rows::walk(&tx, ENVELOPES, Some("thread"), &range, false, |value| {
                        let envelope: KukuriEnvelope = rows::decode(value)?;
                        if envelope.id.as_str() != root {
                            replies.push(envelope);
                        }
                        Ok(replies.len() < remaining)
                    })
                    .await?;
                }
                items.extend(replies);
                Ok(items)
            })
            .await?;
        let next_cursor = next_cursor(&items, limit, envelope_cursor);
        Ok(Page { items, next_cursor })
    }

    async fn upsert_profile(&self, profile: Profile) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PROFILES], Mode::Write)?;
            put_profile(&tx, &profile).await?;
            tx.commit().await
        })
        .await
    }

    async fn get_profile(&self, pubkey: &str) -> Result<Option<Profile>> {
        let pubkey = pubkey.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PROFILES], Mode::Read)?;
            rows::get(&tx, PROFILES, &text(&pubkey)).await
        })
        .await
    }

    async fn get_profiles(&self, pubkeys: &[String]) -> Result<HashMap<String, Profile>> {
        let pubkeys = pubkeys.to_vec();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PROFILES], Mode::Read)?;
            let mut profiles = HashMap::new();
            for pubkey in pubkeys {
                if let Some(profile) = rows::get::<Profile>(&tx, PROFILES, &text(&pubkey)).await? {
                    profiles.insert(pubkey, profile);
                }
            }
            Ok(profiles)
        })
        .await
    }

    async fn upsert_follow_edge(&self, edge: FollowEdge) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &PARTICIPANT_STORES, Mode::Write)?;
            put_follow_edge(&tx, &edge).await?;
            tx.commit().await
        })
        .await
    }

    async fn list_follow_edges_by_subject(&self, subject_pubkey: &str) -> Result<Vec<FollowEdge>> {
        edges(self, FOLLOWS, subject_pubkey, false, |edge: &FollowEdge| {
            (edge.updated_at, edge.target_pubkey.as_str().to_owned())
        })
        .await
    }

    async fn list_follow_edges_by_target(&self, target_pubkey: &str) -> Result<Vec<FollowEdge>> {
        edges(self, FOLLOWS, target_pubkey, true, |edge: &FollowEdge| {
            (edge.updated_at, edge.subject_pubkey.as_str().to_owned())
        })
        .await
    }

    async fn get_follow_edge(
        &self,
        subject_pubkey: &str,
        target_pubkey: &str,
    ) -> Result<Option<FollowEdge>> {
        let (subject, target) = (subject_pubkey.to_owned(), target_pubkey.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[FOLLOWS], Mode::Read)?;
            rows::get(&tx, FOLLOWS, &key(&[text(&subject), text(&target)])).await
        })
        .await
    }

    async fn upsert_block_edge(&self, edge: BlockEdge) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[BLOCKS], Mode::Write)?;
            let id = (
                edge.subject_pubkey.as_str(),
                edge.target_pubkey.as_str(),
                edge.updated_at,
            );
            put_edge(&tx, BLOCKS, id, &edge, |edge: &BlockEdge| edge.updated_at).await?;
            tx.commit().await
        })
        .await
    }

    async fn list_block_edges_by_subject(&self, subject_pubkey: &str) -> Result<Vec<BlockEdge>> {
        edges(self, BLOCKS, subject_pubkey, false, |edge: &BlockEdge| {
            (edge.updated_at, edge.target_pubkey.as_str().to_owned())
        })
        .await
    }

    async fn list_block_edges_by_target(&self, target_pubkey: &str) -> Result<Vec<BlockEdge>> {
        edges(self, BLOCKS, target_pubkey, true, |edge: &BlockEdge| {
            (edge.updated_at, edge.subject_pubkey.as_str().to_owned())
        })
        .await
    }

    async fn get_block_edge(
        &self,
        subject_pubkey: &str,
        target_pubkey: &str,
    ) -> Result<Option<BlockEdge>> {
        let (subject, target) = (subject_pubkey.to_owned(), target_pubkey.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[BLOCKS], Mode::Read)?;
            rows::get(&tx, BLOCKS, &key(&[text(&subject), text(&target)])).await
        })
        .await
    }

    async fn list_follow_edges_by_subject_after(
        &self,
        subject_pubkey: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<FollowEdge>> {
        edges_after(self, FOLLOWS, subject_pubkey, after, limit).await
    }

    async fn list_block_edges_by_subject_after(
        &self,
        subject_pubkey: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<BlockEdge>> {
        edges_after(self, BLOCKS, subject_pubkey, after, limit).await
    }

    /// target の索引の中は主 key（subject・target）の順。続きの位置へは主 key で飛び、手前の行を読まない。
    async fn list_follow_edges_by_target_after(
        &self,
        target_pubkey: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<FollowEdge>> {
        let (target, after) = (target_pubkey.to_owned(), after.map(str::to_owned));
        self.run(move |db| async move {
            let mut edges = Vec::new();
            if limit == 0 {
                return Ok(edges);
            }
            let tx = Txn::begin(&db.idb, &[FOLLOWS], Mode::Read)?;
            let range = rows::only(&text(&target))?;
            let request = rows::source(&tx, FOLLOWS, Some("target"))?.cursor(&range, false)?;
            while let Some(cursor) = idb::next(&request).await? {
                let edge: FollowEdge = rows::decode(&cursor.value().map_err(js_error)?)?;
                match after
                    .as_deref()
                    .map(|after| (edge.subject_pubkey.as_str().cmp(after), after))
                {
                    Some((Ordering::Less, after)) => {
                        let to = key(&[text(after), text(&target)]);
                        cursor
                            .continue_primary_key(&text(&target), &to)
                            .map_err(js_error)?;
                        continue;
                    }
                    Some((Ordering::Equal, _)) => {}
                    _ => {
                        edges.push(edge);
                        if edges.len() == limit {
                            break;
                        }
                    }
                }
                cursor.continue_().map_err(js_error)?;
            }
            Ok(edges)
        })
        .await
    }
}
