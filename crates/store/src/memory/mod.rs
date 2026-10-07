use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{
    BlobHash, BlockEdge, EnvelopeId, FollowEdge, KukuriEnvelope, LiveSessionStatus, Profile,
    ReplicaId, parse_block_edge, parse_follow_edge, parse_profile,
};
use tokio::sync::RwLock;

use crate::models::{
    AuthorRelationshipProjectionRow, BookmarkCursor, BookmarkedCustomReactionRow,
    BookmarkedPostRow, ContentObservationRow, DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT,
    DirectMessageConversationRow, DirectMessageMessageRow, DirectMessageOutboxCursor,
    DirectMessageOutboxPage, DirectMessageOutboxRow, DirectMessageTombstoneRow,
    DomeConnectionProjectionRow, DomeHostingProjectionRow, GameRoomProjectionRow,
    LiveSessionProjectionRow, MutedAuthorRow, NotificationCursor, NotificationRow,
    ObjectProjectionRow, Page, PostWithdrawalRow, PrivateChannelParticipantRow,
    ReactionProjectionRow, TimelineCursor, WithdrawalWriteRow,
};
use crate::pagination::{
    apply_asc_cursor, apply_asc_projection_cursor, apply_desc_cursor,
    apply_desc_direct_message_cursor, apply_desc_projection_cursor,
};
use crate::traits::{
    ContentObservationStore, DirectMessageStore, LiveGameProjectionStore, NOTIFICATION_PAGE_SIZE,
    NotificationStore, ObjectProjectionStore, PostWithdrawalStore, ReactionBookmarkStore,
    SocialProjectionStore, Store,
};

/// sqlite の live_presence_cache 主キー ON CONFLICT(topic_id, channel_id, session_id,
/// author_pubkey) と同義のキー(WP-S6 T7 で topic_id 欠落による上書き divergence を修正)。
type LivePresenceKey = (String, String, String, String);
/// (expires_at, updated_at)
type LivePresenceValue = (i64, i64);
type MemoryReactionProjectionRows = HashMap<(String, String, String), ReactionProjectionRow>;
#[derive(Default)]
struct MemoryBookmarkedPosts {
    rows: HashMap<String, BookmarkedPostRow>,
    by_bookmarked_at: BTreeSet<(i64, String)>,
}
type MemoryDirectMessageRows = HashMap<(String, String), DirectMessageMessageRow>;
type DirectMessageOutboxPeerKey = (String, i64, String, String);
type DirectMessageOutboxNewKey = (i64, String, String, String);
type DirectMessageOutboxRetryKey = (i64, i64, String, String, String);
#[derive(Default)]
struct MemoryDirectMessageOutboxRows {
    rows: HashMap<(String, String), DirectMessageOutboxRow>,
    by_created: BTreeSet<(i64, String, String)>,
    by_peer: BTreeSet<DirectMessageOutboxPeerKey>,
    never_attempted: BTreeSet<DirectMessageOutboxNewKey>,
    attempted: BTreeSet<DirectMessageOutboxRetryKey>,
    by_dm: HashMap<String, BTreeSet<String>>,
}
type MemoryDirectMessageTombstones = HashMap<(String, String), DirectMessageTombstoneRow>;
#[derive(Default)]
struct MemoryNotificationRows {
    rows: HashMap<String, NotificationRow>,
    by_received_at: BTreeSet<(i64, String)>,
    by_sequence: BTreeMap<i64, String>,
    sequence_by_id: HashMap<String, i64>,
    last_sequence: i64,
    read_through_sequence: i64,
    read_through_at: Option<i64>,
    unread_count: usize,
}
type MemoryContentObservationRows =
    HashMap<(String, String, String, String), ContentObservationRow>;
type ProjectionScope = (String, String);
type MemoryPrivateChannelParticipants =
    BTreeMap<(String, String, String), PrivateChannelParticipantRow>;
type DescendingProjectionKey = (Reverse<i64>, Reverse<String>);
type ProjectionIndex = HashMap<ProjectionScope, BTreeSet<DescendingProjectionKey>>;

#[derive(Clone, Default)]
pub struct MemoryStore {
    envelopes: Arc<RwLock<HashMap<EnvelopeId, KukuriEnvelope>>>,
    topic_objects: Arc<RwLock<HashMap<String, Vec<EnvelopeId>>>>,
    object_threads: Arc<RwLock<HashMap<String, BTreeMap<String, EnvelopeId>>>>,
    profiles: Arc<RwLock<HashMap<String, Profile>>>,
    follow_edges: Arc<RwLock<HashMap<(String, String), FollowEdge>>>,
    block_edges: Arc<RwLock<HashMap<(String, String), BlockEdge>>>,
    object_projection_rows: Arc<RwLock<HashMap<EnvelopeId, ObjectProjectionRow>>>,
    adult_media_hashes: Arc<RwLock<HashSet<String>>>,
    live_session_rows: Arc<RwLock<HashMap<String, LiveSessionProjectionRow>>>,
    live_session_index: Arc<RwLock<ProjectionIndex>>,
    game_room_rows: Arc<RwLock<HashMap<String, GameRoomProjectionRow>>>,
    game_room_index: Arc<RwLock<ProjectionIndex>>,
    dome_connection_rows: Arc<RwLock<HashMap<String, DomeConnectionProjectionRow>>>,
    dome_hosting_rows: Arc<RwLock<HashMap<String, DomeHostingProjectionRow>>>,
    muted_authors: Arc<RwLock<HashMap<String, MutedAuthorRow>>>,
    author_docs_authors: Arc<RwLock<HashMap<String, String>>>,
    live_presence: Arc<RwLock<HashMap<LivePresenceKey, LivePresenceValue>>>,
    reaction_projection_rows: Arc<RwLock<MemoryReactionProjectionRows>>,
    bookmarked_custom_reactions: Arc<RwLock<HashMap<String, BookmarkedCustomReactionRow>>>,
    bookmarked_posts: Arc<RwLock<MemoryBookmarkedPosts>>,
    direct_message_conversations: Arc<RwLock<HashMap<String, DirectMessageConversationRow>>>,
    direct_message_rows: Arc<RwLock<MemoryDirectMessageRows>>,
    direct_message_outbox_rows: Arc<RwLock<MemoryDirectMessageOutboxRows>>,
    direct_message_tombstones: Arc<RwLock<MemoryDirectMessageTombstones>>,
    notification_rows: Arc<RwLock<MemoryNotificationRows>>,
    content_observation_rows: Arc<RwLock<MemoryContentObservationRows>>,
    post_withdrawal_rows: Arc<RwLock<HashMap<EnvelopeId, PostWithdrawalRow>>>,
    withdrawal_write_rows: Arc<RwLock<Vec<WithdrawalWriteRow>>>,
    private_channel_participants: Arc<RwLock<MemoryPrivateChannelParticipants>>,
    account_sync: Arc<RwLock<MemoryAccountSync>>,
    private_channel_keys: Arc<RwLock<MemoryPrivateChannelKeys>>,
    /// private channel の参加と鍵の行を読み書きした数(試験が、操作ごとに対象の行だけを触ることを確かめる)。
    private_channel_key_rows_touched: Arc<std::sync::atomic::AtomicUsize>,
    /// 公開参照にしたリンクプレビュー画像（投稿の id → 画像の hash。試験が確かめる。#1632）。
    link_preview_images: Arc<RwLock<HashMap<String, String>>>,
}

impl MemoryStore {
    pub fn private_channel_key_rows_touched(&self) -> usize {
        self.private_channel_key_rows_touched
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn link_preview_image(&self, object_id: &str) -> Option<String> {
        self.link_preview_images
            .read()
            .await
            .get(object_id)
            .cloned()
    }
}

#[derive(Default)]
struct MemoryPrivateChannelKeys {
    channels: BTreeMap<String, crate::PrivateChannelRow>,
    epochs: BTreeMap<(String, String), crate::PrivateChannelEpochRow>,
    /// replica へ未書込みの世代の鍵の行。
    unwritten: BTreeSet<(String, String)>,
}

#[derive(Default)]
struct MemoryAccountSync {
    rows: BTreeMap<String, crate::AccountSyncRow>,
    /// replica へ未書込みの行の key。
    unwritten: BTreeSet<String>,
    cursors: BTreeMap<String, crate::AccountSyncCursor>,
}

mod account_sync;
mod bookmarks;
mod direct_messages;
mod envelopes;
mod live_game;
mod notifications;
mod observations;
mod private_channel_keys;
mod projections;
mod social;
mod withdrawals;

#[async_trait]
impl Store for MemoryStore {
    async fn put_envelope(&self, envelope: KukuriEnvelope) -> Result<()> {
        self.store_put_envelope_impl(envelope).await
    }

    async fn get_envelope(&self, envelope_id: &EnvelopeId) -> Result<Option<KukuriEnvelope>> {
        self.store_get_envelope_impl(envelope_id).await
    }

    async fn list_topic_timeline(
        &self,
        topic_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<KukuriEnvelope>> {
        self.store_list_topic_timeline_impl(topic_id, cursor, limit)
            .await
    }

    async fn list_thread(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<KukuriEnvelope>> {
        self.store_list_thread_impl(topic_id, thread_root_object_id, cursor, limit)
            .await
    }

    async fn upsert_profile(&self, profile: Profile) -> Result<()> {
        self.store_upsert_profile_impl(profile).await
    }

    async fn get_profile(&self, pubkey: &str) -> Result<Option<Profile>> {
        self.store_get_profile_impl(pubkey).await
    }

    async fn get_profiles(&self, pubkeys: &[String]) -> Result<HashMap<String, Profile>> {
        self.store_get_profiles_impl(pubkeys).await
    }

    async fn upsert_follow_edge(&self, edge: FollowEdge) -> Result<()> {
        self.store_upsert_follow_edge_impl(edge).await
    }

    async fn list_follow_edges_by_subject(&self, subject_pubkey: &str) -> Result<Vec<FollowEdge>> {
        self.store_list_follow_edges_by_subject_impl(subject_pubkey)
            .await
    }

    async fn list_follow_edges_by_target(&self, target_pubkey: &str) -> Result<Vec<FollowEdge>> {
        self.store_list_follow_edges_by_target_impl(target_pubkey)
            .await
    }

    async fn get_follow_edge(
        &self,
        subject_pubkey: &str,
        target_pubkey: &str,
    ) -> Result<Option<FollowEdge>> {
        Ok(self
            .follow_edges
            .read()
            .await
            .get(&(subject_pubkey.to_string(), target_pubkey.to_string()))
            .cloned())
    }

    async fn upsert_block_edge(&self, edge: BlockEdge) -> Result<()> {
        self.store_upsert_block_edge_impl(edge).await
    }

    async fn list_block_edges_by_subject(&self, subject_pubkey: &str) -> Result<Vec<BlockEdge>> {
        self.store_list_block_edges_by_subject_impl(subject_pubkey)
            .await
    }

    async fn list_block_edges_by_target(&self, target_pubkey: &str) -> Result<Vec<BlockEdge>> {
        self.store_list_block_edges_by_target_impl(target_pubkey)
            .await
    }

    async fn get_block_edge(
        &self,
        subject_pubkey: &str,
        target_pubkey: &str,
    ) -> Result<Option<BlockEdge>> {
        self.store_get_block_edge_impl(subject_pubkey, target_pubkey)
            .await
    }

    async fn list_follow_edges_by_subject_after(
        &self,
        subject_pubkey: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<FollowEdge>> {
        let edges = self.follow_edges.read().await;
        Ok(edges_after(
            edges.values(),
            subject_pubkey,
            after,
            limit,
            |edge| (edge.subject_pubkey.as_str(), edge.target_pubkey.as_str()),
        ))
    }

    async fn list_block_edges_by_subject_after(
        &self,
        subject_pubkey: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<BlockEdge>> {
        let edges = self.block_edges.read().await;
        Ok(edges_after(
            edges.values(),
            subject_pubkey,
            after,
            limit,
            |edge| (edge.subject_pubkey.as_str(), edge.target_pubkey.as_str()),
        ))
    }

    async fn list_follow_edges_by_target_after(
        &self,
        target_pubkey: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<FollowEdge>> {
        let edges = self.follow_edges.read().await;
        Ok(edges_after(
            edges.values(),
            target_pubkey,
            after,
            limit,
            |edge| (edge.target_pubkey.as_str(), edge.subject_pubkey.as_str()),
        ))
    }
}

/// `ends` の左が `subject` の edge を、右（相手）の順に `after` より後から `limit` 件(試験用の store なので、全件から
/// 選ぶ)。
fn edges_after<'a, T: Clone + 'a>(
    edges: impl Iterator<Item = &'a T>,
    subject: &str,
    after: Option<&str>,
    limit: usize,
    ends: impl Fn(&T) -> (&str, &str),
) -> Vec<T> {
    let mut selected = edges
        .filter(|edge| {
            let (edge_subject, target) = ends(edge);
            edge_subject == subject && after.is_none_or(|after| target > after)
        })
        .cloned()
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| ends(left).1.cmp(ends(right).1));
    selected.truncate(limit);
    selected
}
