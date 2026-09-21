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
    AuthorRelationshipProjectionRow, BlobCacheStatus, BookmarkedCustomReactionRow,
    BookmarkedPostRow, ContentObservationRow, DirectMessageConversationRow,
    DirectMessageMessageRow, DirectMessageOutboxRow, DirectMessageTombstoneRow,
    DomeConnectionProjectionRow, DomeHostingProjectionRow, GameRoomProjectionRow,
    LiveSessionProjectionRow, MutedAuthorRow, NotificationRow, ObjectProjectionRow, Page,
    PostWithdrawalRow, ReactionProjectionRow, TimelineCursor,
};
use crate::pagination::{
    apply_asc_cursor, apply_asc_projection_cursor, apply_desc_cursor,
    apply_desc_direct_message_cursor, apply_desc_projection_cursor,
};
use crate::traits::{
    BlobCacheStore, ContentObservationStore, DirectMessageStore, LiveGameProjectionStore,
    NotificationStore, ObjectProjectionStore, PostWithdrawalStore, ReactionBookmarkStore,
    SocialProjectionStore, Store,
};

/// sqlite の live_presence_cache 主キー ON CONFLICT(topic_id, channel_id, session_id,
/// author_pubkey) と同義のキー(WP-S6 T7 で topic_id 欠落による上書き divergence を修正)。
type LivePresenceKey = (String, String, String, String);
/// (expires_at, updated_at)
type LivePresenceValue = (i64, i64);
type MemoryReactionProjectionRows = HashMap<(String, String, String), ReactionProjectionRow>;
type MemoryDirectMessageRows = HashMap<(String, String), DirectMessageMessageRow>;
type MemoryDirectMessageOutboxRows = HashMap<(String, String), DirectMessageOutboxRow>;
type MemoryDirectMessageTombstones = HashMap<(String, String), DirectMessageTombstoneRow>;
type MemoryNotificationRows = HashMap<String, NotificationRow>;
type MemoryContentObservationRows =
    HashMap<(String, String, String, String), ContentObservationRow>;

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
    game_room_rows: Arc<RwLock<HashMap<String, GameRoomProjectionRow>>>,
    dome_connection_rows: Arc<RwLock<HashMap<String, DomeConnectionProjectionRow>>>,
    dome_hosting_rows: Arc<RwLock<HashMap<String, DomeHostingProjectionRow>>>,
    author_relationship_rows:
        Arc<RwLock<HashMap<(String, String), AuthorRelationshipProjectionRow>>>,
    muted_authors: Arc<RwLock<HashMap<String, MutedAuthorRow>>>,
    author_docs_authors: Arc<RwLock<HashMap<String, String>>>,
    live_presence: Arc<RwLock<HashMap<LivePresenceKey, LivePresenceValue>>>,
    blob_statuses: Arc<RwLock<HashMap<String, BlobCacheStatus>>>,
    reaction_projection_rows: Arc<RwLock<MemoryReactionProjectionRows>>,
    bookmarked_custom_reactions: Arc<RwLock<HashMap<String, BookmarkedCustomReactionRow>>>,
    bookmarked_posts: Arc<RwLock<HashMap<String, BookmarkedPostRow>>>,
    direct_message_conversations: Arc<RwLock<HashMap<String, DirectMessageConversationRow>>>,
    direct_message_rows: Arc<RwLock<MemoryDirectMessageRows>>,
    direct_message_outbox_rows: Arc<RwLock<MemoryDirectMessageOutboxRows>>,
    direct_message_tombstones: Arc<RwLock<MemoryDirectMessageTombstones>>,
    notification_rows: Arc<RwLock<MemoryNotificationRows>>,
    content_observation_rows: Arc<RwLock<MemoryContentObservationRows>>,
    post_withdrawal_rows: Arc<RwLock<HashMap<EnvelopeId, PostWithdrawalRow>>>,
}

mod bookmarks;
mod direct_messages;
mod envelopes;
mod live_game;
mod notifications;
mod observations;
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
}
