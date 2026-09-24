use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{
    BlobHash, BlockEdge, EnvelopeId, FollowEdge, KukuriEnvelope, Profile, ReplicaId, ThreadRef,
    parse_block_edge, parse_follow_edge, parse_profile,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, QueryBuilder, Row, Sqlite};

use crate::models::adult_media_hashes_for_row;
use crate::models::{
    AuthorRelationshipProjectionRow, BlobCacheStatus, BookmarkedCustomReactionRow,
    BookmarkedPostRow, ContentObservationRow, DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT,
    DirectMessageConversationRow, DirectMessageMessageRow, DirectMessageOutboxCursor,
    DirectMessageOutboxPage, DirectMessageOutboxRow, DirectMessageTombstoneRow,
    DomeConnectionProjectionRow, DomeHostingProjectionRow, GameRoomProjectionRow,
    LiveSessionProjectionRow, MutedAuthorRow, NotificationCursor, NotificationRow,
    ObjectProjectionRow, Page, PostWithdrawalRow, ReactionProjectionRow, TimelineCursor,
};
use crate::pagination::{
    direct_message_page_from_rows, envelope_page_from_rows, object_projection_page_from_rows,
};
use crate::row_mapping::{
    block_edge_status_name, follow_edge_status_name, game_room_kind_name, game_status_name,
    live_status_name, notification_kind_name, object_status_name, reaction_key_kind_name,
    row_to_author_relationship_projection, row_to_block_edge, row_to_bookmarked_custom_reaction,
    row_to_bookmarked_post, row_to_direct_message_conversation, row_to_direct_message_message,
    row_to_direct_message_outbox, row_to_direct_message_tombstone, row_to_envelope,
    row_to_follow_edge, row_to_game_room_projection, row_to_live_session_projection,
    row_to_muted_author, row_to_notification, row_to_object_projection, row_to_reaction_projection,
};
use crate::traits::{
    BlobCacheStore, ContentObservationStore, DirectMessageStore, LiveGameProjectionStore,
    NOTIFICATION_PAGE_SIZE, NotificationStore, ObjectProjectionStore, PostWithdrawalStore,
    ReactionBookmarkStore, SocialProjectionStore, Store,
};

mod bookmarks;
mod connection;
mod direct_messages;
mod envelopes;
pub(crate) mod live_game;
mod notifications;
mod observations;
pub(crate) mod projections;
mod social;
mod withdrawals;

pub use connection::StoreStartupError;

#[derive(Clone)]
pub struct SqliteStore {
    pool: Pool<Sqlite>,
}

#[cfg(test)]
pub(crate) use connection::STORE_MIGRATOR;

#[async_trait]
impl Store for SqliteStore {
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

    async fn get_profiles(
        &self,
        pubkeys: &[String],
    ) -> Result<std::collections::HashMap<String, Profile>> {
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
