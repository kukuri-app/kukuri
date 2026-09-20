pub(crate) use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
pub(crate) use std::sync::Arc;

pub(crate) use anyhow::{Context, Result};
pub(crate) use base64::Engine;
pub(crate) use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
pub(crate) use chrono::Utc;
pub(crate) use futures_util::StreamExt;
pub(crate) use kukuri_blob_service::{
    BlobService, BlobStatus, METAVERSE_ROLLBACK_REVISION_LIMIT, MemoryBlobService,
    MetaverseBlobCacheIndex, MetaverseBlobPin, MetaverseBlobPinReason, StoredBlob,
};
pub(crate) use kukuri_core::{
    AssetRole, AuthorProfileDocV1, AuthorProfilePostDocV1, AuthorProfileRepostDocV1, BlockEdge,
    BlockEdgeDocV1, BlockEdgeStatus, CanonicalPostHeader, ChannelAudienceKind, ChannelId,
    ChannelRef, ChannelSharingState, CreatePrivateChannelInput, CustomReactionAssetDocV1,
    CustomReactionAssetSnapshotV1, DOME_INSTANCE_MANIFEST_MIME, DOME_PRESET_MANIFEST_MIME,
    DirectMessageAttachmentKind, DirectMessageAttachmentManifestV1,
    DirectMessageEncryptedAttachmentV1, DirectMessageEncryptedBlobRefV1, DirectMessageFrameV1,
    DirectMessagePayloadV1, DomeConnectionEndpointV1, DomeConnectionProposalV1,
    DomeConnectionRecordV1, DomeConnectionStatusV1, DomeConnectionTerminalReasonV1,
    DomeInstanceManifestV1, DomeInstanceStateDocV1, DomeMoveRecordV1, DomeMoveStateDocV1,
    DomePresetManifestV1, DomePresetRefV1, DomePresetStateDocV1, DomeProposalSelectionV1,
    EnvelopeId, FollowEdge, FollowEdgeDocV1, FollowEdgeStatus, FriendOnlyGrantPreview,
    FriendPlusSharePreview, GAME_MANIFEST_MIME, GameParticipant, GameRoomKind,
    GameRoomManifestBlobV1, GameRoomStateDocV1, GameRoomStatus, GameScoreEntry, GossipHint,
    HintObjectRef, KukuriEnvelope, KukuriKeys, KukuriMediaManifestV1,
    KukuriProfileEnvelopeContentV1, KukuriProfilePostEnvelopeContentV1,
    KukuriProfileRepostEnvelopeContentV1, LIVE_MANIFEST_MIME, LiveSessionManifestBlobV1,
    LiveSessionStateDocV1, LiveSessionStatus, METAVERSE_WORLD_VERSION, ManifestBlobRef,
    MediaManifestItem, MetaverseAssetRef, MetaverseDomeV1, MetaverseRoomEventEnvelopeContentV1,
    MetaverseRoomEventV1, MetaverseRoomSpawnV1, MetaverseRoomStateV1, ObjectStatus,
    ObjectVisibility, PayloadRef, PostWithdrawalReason, PostWithdrawalV1,
    PrivateChannelEpochHandoffGrantDocV1, PrivateChannelEpochHandoffGrantPayloadV1,
    PrivateChannelInvitePreview, PrivateChannelInviteTokenParams, PrivateChannelJoinMode,
    PrivateChannelMetadataDocV1, PrivateChannelParticipantDocV1, PrivateChannelPolicyDocV1,
    Profile, ProfilePost, ProfileRepost, Pubkey, ReactionDocV1, ReactionKeyKind, ReactionKeyV1,
    ReplicaId, RepostSourceSnapshotV1, TimelineScope, TopicId, WithdrawalReasonVisibility,
    author_profile_topic_id, build_block_edge_envelope, build_custom_reaction_asset_envelope,
    build_direct_message_ack, build_dome_connection_agreement_envelope,
    build_dome_connection_proposal_envelope, build_dome_connection_selection_envelope,
    build_dome_instance_envelope, build_dome_move_envelope, build_dome_preset_envelope,
    build_follow_edge_envelope, build_friend_only_grant_token, build_friend_plus_share_token,
    build_game_session_envelope, build_live_session_envelope, build_media_manifest_envelope,
    build_metaverse_room_event_envelope, build_post_envelope_with_payload_in_channel,
    build_post_withdrawal_envelope, build_private_channel_epoch_handoff_grant_envelope,
    build_private_channel_invite_token, build_private_channel_participant_envelope,
    build_private_channel_policy_envelope, build_profile_envelope, build_profile_post_envelope,
    build_profile_repost_envelope, build_reaction_envelope, build_repost_envelope,
    decrypt_direct_message_attachment, decrypt_direct_message_frame,
    decrypt_private_channel_epoch_handoff_grant, derive_direct_message_topic,
    deterministic_reaction_id, direct_message_id_for_participants,
    encrypt_direct_message_attachment, encrypt_direct_message_frame,
    encrypt_private_channel_epoch_handoff_grant, generate_keys, metaverse_room_event_is_live,
    parse_block_edge, parse_custom_reaction_asset, parse_follow_edge,
    parse_friend_only_grant_token, parse_friend_plus_share_token,
    parse_private_channel_epoch_handoff_grant, parse_private_channel_invite_token,
    parse_private_channel_participant, parse_private_channel_policy, parse_profile,
    parse_profile_post, parse_profile_repost, parse_reaction, timeline_sort_key,
    validate_dome_connection_agreement, validate_dome_connection_proposal,
    validate_dome_connection_record, validate_dome_connection_selection,
    validate_dome_customization, validate_dome_preset_manifest,
    validate_metaverse_room_event_content, validate_metaverse_room_event_for_instance,
    validate_metaverse_room_state, verify_post_withdrawal,
};
pub(crate) use kukuri_docs_sync::{
    DocEvent, DocFetchPolicy, DocOp, DocQuery, DocRecord, DocsSync, MemoryDocsSync,
    author_replica_id, private_channel_epoch_replica_id, private_channel_hint_topic,
    private_channel_replica_id, stable_key, topic_replica_id,
};
pub(crate) use kukuri_metaverse_host::DomeSessionRuntime;
pub(crate) use kukuri_store::{
    AuthorRelationshipProjectionRow, BlobCacheStatus, BlobCacheStore, BookmarkedCustomReactionRow,
    BookmarkedPostRow, DirectMessageConversationRow, DirectMessageMessageRow,
    DirectMessageOutboxRow, DirectMessageTombstoneRow, DomeConnectionProjectionRow,
    DomeHostingProjectionRow, GameRoomProjectionRow, LiveSessionProjectionRow, MutedAuthorRow,
    NotificationKind, NotificationRow, ObjectProjectionRow, ObjectProjectionStore, Page,
    PostWithdrawalRow, ProjectionStore, ReactionProjectionRow, Store, TimelineCursor,
};
pub(crate) use kukuri_transport::{
    ConnectionPath, DiscoveryMode, DiscoverySnapshot, HintTransport, PeerSnapshot, SeedPeer,
    TopicPeerSnapshot, Transport,
};
pub(crate) use serde::{Serialize, de::DeserializeOwned};
pub(crate) use tokio::sync::Mutex;
pub(crate) use tokio::task::JoinHandle;
pub(crate) use tracing::{info, warn};

pub(crate) const REPLICA_SYNC_RESTART_RETRY_SECONDS: i64 = 5;
pub(crate) const DIRECT_MESSAGE_SUBSCRIPTION_RESTART_RETRY_SECONDS: i64 = 5;
pub(crate) const PUBLIC_TOPIC_RECOVERY_GRACE_MS: i64 = 3_000;
/// 自分の既存の repost を探すときに見る行数の上限(#1239)。引用つきの repost は同じ元に複数ありうる。
pub(crate) const EXISTING_REPOST_LOOKUP_LIMIT: usize = 64;
pub(crate) const PUBLIC_TOPIC_RECOVERY_BACKOFF_MS: [i64; 3] = [3_000, 10_000, 30_000];
pub(crate) const PUBLIC_CHANNEL_ID: &str = "public";
pub(crate) const DIRECT_MESSAGE_FRAME_MIME: &str =
    "application/vnd.kukuri.direct-message-frame+json";
pub(crate) const DIRECT_MESSAGE_ATTACHMENT_MIME: &str =
    "application/vnd.kukuri.direct-message-attachment+json";
pub(crate) const DIRECT_MESSAGE_RETRY_INTERVAL_MS: u64 = 2_000;
pub(crate) const NOTIFICATION_PREVIEW_LIMIT: usize = 80;

pub(crate) use crate::views::{
    AcceptDomeConnectionProposalInput, AttachmentView, AuthorSocialView, BlobMediaPayload,
    BlobViewStatus, BookmarkedCustomReactionView, BookmarkedPostView, ChannelAccessTokenExport,
    ChannelAccessTokenKind, ChannelAccessTokenPreview, CommunityIndexPostActionCapabilitiesView,
    CommunityIndexPostResolveInput, CommunityIndexPostResolveResponse,
    CommunityIndexResolvedPostView, CreateCustomReactionAssetInput,
    CreateDomeConnectionProposalInput, CreateGameRoomInput, CreateLiveSessionInput,
    CreateMetaverseRoomInput, CustomReactionAssetView, DeliveryState,
    DirectMessageConversationView, DirectMessageMessageView, DirectMessageStatusView,
    DirectMessageTimelineView, DirectMessageTopicStatusView, DiscoveryStatus,
    DomeConnectionProposalView, DomeConnectionTopologyView, DomeConnectionView, DomeMoveView,
    GameRoomView, GameScoreView, ImportMetaverseRoomAssetInput, JoinedPrivateChannelView,
    LiveSessionView, MetaverseAssetRefView, MetaverseRoomEventView, MoveDomeInput,
    NotificationStatusView, NotificationView, PendingAttachment, PostView, PostWithdrawalView,
    PrivateChannelCapability, PrivateChannelEpochCapability, ProfileAssetView, ProfileInput,
    PublishMetaverseRoomEventInput, ReactionKeyView, ReactionStateView, ReactionSummaryView,
    RecentReactionView, ReplyPreviewAuthorView, ReplyPreviewView, RepostSourceView,
    RevokeDomeConnectionInput, SocialConnectionKind, SyncStatus, TimelineView, TopicSyncStatus,
    UpdateGameRoomInput, UpdateMetaverseRoomInput, WithdrawDomeConnectionProposalInput,
};

mod attachment_support;
mod direct_messages_delivery_support;
mod direct_messages_subscription_support;
mod dome_connection_support;
pub(crate) use dome_connection_support::*;
mod errors;
mod game_projection_support;
mod gossip_subscription_support;
mod hydration_limits;
mod hydration_support;
use game_projection_support::GameRoomProjectionLocks;
pub(crate) use hydration_limits::{HintRecoveryGate, recovery_probe_peer_state};
#[cfg(test)]
pub(crate) use hydration_support::{hydrate_game_room_from_key, hydrate_game_rooms_from_replica};
mod live_game_support;
pub(crate) use live_game_support::{DomeReadUnavailable, fetch_verified_dome_envelope};
mod metaverse_room_event_support;
mod notifications_support;
mod object_hydration;
mod object_persistence_support;
mod post_integrity;
mod post_withdrawal_hydration;
mod private_channels_support;
mod profile_docs_support;
mod projection_support;
mod reaction_hydration;
pub(crate) use reaction_hydration::{
    hydrate_reaction_cache_for_target_bounded, hydrate_reaction_cache_from_key,
};
mod reaction_integrity;
mod replica_window;
mod session_integrity;
mod social_helpers;
mod social_runtime_support;
mod spatial_access_support;
mod subscription_registry;
mod timeline_subscription_support;
mod timeline_view_support;

pub(crate) use errors::{
    PrivateChannelImportError, PrivateChannelImportKind, PrivateChannelSnapshotWaitContext,
};

pub(crate) use attachment_support::{
    attachment_views_from_refs, blob_status, blob_view_status, blob_view_status_for_payload,
    channel_hint_topic_for, channel_id_for_view, channel_id_from_storage, channel_storage_id,
    combine_delivery_states, delivery_state_for_topic, direct_message_attachment_views,
    direct_message_preview, direct_message_topic_peer_count, effective_sync_status_detail,
    effective_topic_status_detail, joined_private_channel_key,
    joined_private_channel_subscription_key, joined_private_channel_subscription_prefix,
    live_presence_task_key, materialize_direct_message_manifest, merge_optional_timestamp,
    normalize_topic_diagnostics, normalize_topic_name, normalize_topics,
    register_private_channel_replica_secrets, sanitize_game_participants, short_id_suffix,
    subscription_replicas_for_topic, validate_game_room_scores, validate_game_room_transition,
};
pub(crate) use gossip_subscription_support::gossip_disabled_channel_key;
pub(crate) use hydration_support::{
    hint_refers_to_replica_content, hint_targets_topic, hydrate_subscription_event,
    hydrate_subscription_hint, hydrate_subscription_state, hydrate_topic_state,
    profile_timeline_page,
};
pub(crate) use metaverse_room_event_support::{
    metaverse_room_event_buffer_key, parse_metaverse_room_event_envelope,
    push_metaverse_room_event_buffer,
};
pub(crate) use notifications_support::{
    author_social_view_from_parts, author_social_view_sort_key, direct_message_notification_id,
    document_notification_id, normalize_author_pubkey, notification_candidate_from_follow_event,
    notification_candidate_from_object_event, notification_doc_event_fingerprint,
    notification_doc_event_fingerprint_parts, notification_preview_text,
};
pub(crate) use object_hydration::{
    BodyFetch, ObjectHydration, hydrate_object_in_topic, hydrate_object_in_topic_with,
};
pub(crate) use object_persistence_support::{
    best_effort_blob_cache_status, best_effort_blob_view_status,
    bookmarked_custom_reaction_view_from_row, custom_reaction_asset_view_from_doc,
    fetch_manifest_blob, fetch_private_channel_epoch_handoff_grant_from_replica,
    fetch_private_channel_participants_from_replica, fetch_private_channel_policy_from_replica,
    fetch_projection_blob_text, game_projection_row, live_projection_row, persist_game_room_state,
    persist_live_session_state, persist_media_manifest, persist_post_object,
    persist_post_withdrawal, persist_private_channel_epoch_handoff_grant,
    persist_private_channel_metadata, persist_private_channel_participant,
    persist_private_channel_policy, persist_session_envelope, post_withdrawal_row,
    private_channel_rotation_is_pending, projection_row_from_post, reaction_cache_key,
    reaction_projection_row, reaction_state_view_from_rows, recent_reaction_view_from_projection,
    search_key_or_asset_id, session_projection_retry_attempts, session_projection_retry_delay,
    store_manifest_blob, wait_for_private_channel_epoch_snapshot,
};
pub(crate) use post_integrity::{
    MAX_ENVELOPE_RECORDS_PER_OBJECT, MAX_WITHDRAWAL_RECORDS_PER_OBJECT, PostLoad, ReplicaPostScope,
    VerifiedPost, WithdrawalTargetCheck, load_post, load_verified_post, object_id_from_post_key,
    post_envelope_key, select_verified_post, verify_withdrawal_against_records, warn_rejected_post,
};
pub(crate) use post_withdrawal_hydration::{
    PostWithdrawalHydration, hydrate_post_withdrawal_for_object,
    hydrate_post_withdrawal_from_record, object_id_from_post_withdrawal_key,
};
pub(crate) use profile_docs_support::{
    fetch_author_envelope_by_id, hydrate_author_state,
    load_custom_reaction_assets_from_author_replica, load_profile_posts_from_author_replica,
    load_profile_reposts_from_author_replica, merge_seed_peers, persist_block_edge_doc,
    persist_custom_reaction_asset_doc, persist_follow_edge_doc, persist_profile_doc,
    persist_profile_post_doc, persist_profile_repost_doc, persist_reaction_doc,
    snapshot_follow_notification_baseline, snapshot_object_notification_baseline,
};
pub(crate) use projection_support::{
    active_private_channel_participants, archive_private_channel_epoch,
    bookmarked_post_row_is_hidden, current_private_channel_replica_id, filter_channel_rows,
    filtered_thread_page, filtered_timeline_page, initial_private_channel_epoch_id,
    joined_private_channel_state_from_capability, merged_private_channel_state_from_epoch_join,
    next_private_channel_epoch_id, private_channel_epoch_capabilities,
    private_channel_is_epoch_aware, private_channel_replica_for_epoch,
    profile_timeline_item_is_hidden,
};
pub(crate) use reaction_integrity::{
    ReactionKey, VerifiedReaction, load_verified_reaction, select_verified_reaction,
    warn_rejected_reaction,
};
pub(crate) use session_integrity::{
    VerifiedGameRoom, VerifiedLiveSession, load_verified_game_room, load_verified_live_session,
    owner_bound_id_suffix, verify_game_room_record, verify_live_session_record,
};
pub(crate) use social_helpers::{
    current_mutual_direct_message_peers, rebuild_author_relationships,
    reconcile_direct_message_subscriptions, schedule_direct_message_reconcile,
    stop_direct_message_subscription,
};
pub(crate) use subscription_registry::SubscriptionRegistry;
pub(crate) use timeline_view_support::{
    MAX_POST_CONTENT_CHARS, MAX_PROFILE_ABOUT_CHARS, MAX_PROFILE_DISPLAY_NAME_CHARS,
    MAX_PROFILE_NAME_CHARS, MAX_REPOST_COMMENTARY_CHARS, content_from_payload_ref,
    ensure_optional_text_within_limit, ensure_text_within_limit, normalize_optional_text,
    normalize_repost_commentary, profile_asset_view_from_ref,
};

// テストからのみ参照される再輸出(依存の可視化。WP-H5 PR1)。
#[cfg(test)]
pub(crate) use object_persistence_support::custom_reaction_asset_view_from_snapshot;

pub(crate) async fn maybe_restart_replica_sync_with_cooldown(
    docs_sync: &dyn DocsSync,
    deadlines: &Arc<Mutex<HashMap<String, i64>>>,
    topic_id: &str,
    replica: &ReplicaId,
) {
    let key = replica.as_str().to_string();
    let now = Utc::now().timestamp();
    {
        let mut guard = deadlines.lock().await;
        let next_due_at = guard.get(key.as_str()).copied().unwrap_or_default();
        if next_due_at > now {
            return;
        }
        guard.insert(key, now.saturating_add(REPLICA_SYNC_RESTART_RETRY_SECONDS));
    }
    if let Err(error) = docs_sync.restart_replica_sync(replica).await {
        warn!(
            topic = %topic_id,
            replica = %replica.as_str(),
            error = %error,
            "failed to restart replica sync"
        );
    }
}

pub(crate) async fn query_replica_with_fetch_policy(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    query: DocQuery,
    policy: DocFetchPolicy,
) -> Result<Vec<DocRecord>> {
    docs_sync
        .query_replica_with_policy(replica, query, policy)
        .await
}

pub(crate) async fn record_public_topic_docs_activity_if_current(
    delivery: &Arc<Mutex<HashMap<String, PublicTopicDeliveryStatus>>>,
    topic_id: &str,
    generation: u64,
    at_ms: i64,
) {
    let mut guard = delivery.lock().await;
    if let Some(entry) = guard.get_mut(topic_id)
        && entry.generation == generation
    {
        entry.last_docs_activity_at = Some(at_ms);
    }
}

pub(crate) async fn restart_replica_sync_with_backoff(
    docs_sync: &dyn DocsSync,
    topic_id: &str,
    replica: &ReplicaId,
    backoff: &mut SubscriptionRecoveryBackoff,
) {
    let now_ms = Utc::now().timestamp_millis();
    if !backoff.ready(now_ms) {
        return;
    }
    if let Err(error) = docs_sync.restart_replica_sync(replica).await {
        warn!(
            topic = %topic_id,
            replica = %replica.as_str(),
            error = %error,
            "failed to restart replica sync"
        );
    }
    backoff.schedule(now_ms);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProfileTimelineItem {
    Post(ProfilePost),
    Repost(ProfileRepost),
}

impl ProfileTimelineItem {
    pub(crate) fn created_at(&self) -> i64 {
        match self {
            Self::Post(post) => post.created_at,
            Self::Repost(repost) => repost.created_at,
        }
    }

    pub(crate) fn object_id(&self) -> &EnvelopeId {
        match self {
            Self::Post(post) => &post.object_id,
            Self::Repost(repost) => &repost.object_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedRepostSource {
    pub(crate) repost_of: RepostSourceSnapshotV1,
}

/// private channel capability registry の write-through 永続化 callback。
/// 実体(identity storage への保存)は desktop-runtime が注入する。未接続なら no-op。
pub type PrivateChannelCapabilityPersist =
    Arc<dyn Fn(&[crate::PrivateChannelCapability]) -> Result<()> + Send + Sync>;

#[derive(Clone)]
pub struct ServiceHandles {
    pub(crate) store: Arc<dyn Store>,
    pub(crate) projection_store: Arc<dyn ProjectionStore>,
    pub(crate) transport: Arc<dyn Transport>,
    pub(crate) hint_transport: Arc<dyn HintTransport>,
    pub(crate) docs_sync: Arc<dyn DocsSync>,
    pub(crate) blob_service: Arc<dyn BlobService>,
    pub(crate) keys: Arc<KukuriKeys>,
    pub(crate) game_room_projections: Arc<GameRoomProjectionLocks>,
    pub(crate) dome_mutations: Arc<Mutex<()>>,
    /// #1225: replica 全件走査の指紋と、欠損した本文 blob の試行台帳。
    pub(crate) replica_scan_cache: Arc<hydration_limits::ReplicaScanCache>,
    pub(crate) missing_body_ledger: Arc<hydration_limits::MissingBodyLedger>,
    /// #1239: 表示した投稿の取り下げの、背景での確認の台帳。
    pub(crate) withdrawal_checks: Arc<hydration_limits::WithdrawalCheckLedger>,
    /// #1239: ページの範囲と時系列の索引の照合の台帳。
    pub(crate) range_checks: Arc<replica_window::RangeCheckLedger>,
}

impl ServiceHandles {
    pub fn new(
        store: Arc<dyn Store>,
        projection_store: Arc<dyn ProjectionStore>,
        transport: Arc<dyn Transport>,
        hint_transport: Arc<dyn HintTransport>,
        docs_sync: Arc<dyn DocsSync>,
        blob_service: Arc<dyn BlobService>,
        keys: KukuriKeys,
    ) -> Self {
        Self {
            store,
            projection_store,
            transport,
            hint_transport,
            docs_sync,
            blob_service,
            keys: Arc::new(keys),
            game_room_projections: Arc::default(),
            dome_mutations: Arc::default(),
            replica_scan_cache: Arc::default(),
            missing_body_ledger: Arc::default(),
            withdrawal_checks: Arc::default(),
            range_checks: Arc::default(),
        }
    }
}

pub struct AppService {
    pub(crate) services: ServiceHandles,
    /// 購読タスクの台帳(task ×5 / 世代 / 復旧 deadline。WP-H5 PR5)。
    pub(crate) subscription_registry: SubscriptionRegistry,
    pub(crate) joined_private_channels: Arc<Mutex<HashMap<String, JoinedPrivateChannelState>>>,
    pub(crate) metaverse_room_events: Arc<Mutex<HashMap<String, VecDeque<MetaverseRoomEventView>>>>,
    pub(crate) dome_host_heartbeats:
        Arc<Mutex<HashMap<String, kukuri_core::SignedDomeHostHeartbeatV1>>>,
    pub(crate) dome_host_sessions: Arc<Mutex<HashMap<String, DomeSessionRuntime>>>,
    pub(crate) metaverse_blob_cache: Arc<Mutex<MetaverseBlobCacheIndex>>,
    pub(crate) metaverse_resource_budget: kukuri_core::MetaverseResourceBudgetConfig,
    pub(crate) last_sync_ts: Arc<Mutex<Option<i64>>>,
    pub(crate) public_topic_delivery: Arc<Mutex<HashMap<String, PublicTopicDeliveryStatus>>>,
    pub(crate) empty_recovery_candidates: Arc<Mutex<HashSet<String>>>,
    pub(crate) gossip_disabled_topics: Arc<Mutex<HashSet<String>>>,
    pub(crate) gossip_disabled_channels: Arc<Mutex<HashSet<String>>>,
    pub(crate) private_channel_capability_persist:
        std::sync::OnceLock<PrivateChannelCapabilityPersist>,
    pub(crate) private_channel_capability_persist_guard: Arc<Mutex<()>>,
    pub(crate) notification_inserted_notify: Arc<tokio::sync::Notify>,
    /// #858: 成人向け表現の表示設定(既定 OFF)。canonical source は desktop-runtime の
    /// ローカル JSON で、ここは blob 取得ゲートが参照する in-memory ミラー。
    pub(crate) adult_content_display_enabled: Arc<std::sync::atomic::AtomicBool>,
    /// #1055: Community Node の content advisory(ADR 0028 §8.6)が付いた添付 blob hash。
    /// node-local な advisory であり投稿の canonical でも署名対象でもないため、self-label 由来の
    /// `adult_media_hashes` と違って永続化せず、プロセス内の一時集合に留める(ADR 0028 §8.10)。
    /// 表示経路は必ず index 照会を経由するので、再起動後も表示より先に再登録される。
    pub(crate) advisory_media_hashes: Arc<Mutex<HashSet<String>>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PublicTopicDeliveryStatus {
    pub(crate) generation: u64,
    pub(crate) last_docs_activity_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SubscriptionRecoveryBackoff {
    pub(crate) next_retry_at_ms: i64,
    pub(crate) step: usize,
}

impl SubscriptionRecoveryBackoff {
    pub(crate) fn reset(&mut self) {
        self.next_retry_at_ms = 0;
        self.step = 0;
    }

    /// 変化が無い間の次の全件走査の時刻。再 sync の backoff に合わせて伸ばす(#1225)。
    pub(crate) fn next_probe_at(&self, now_ms: i64) -> i64 {
        now_ms
            .saturating_add(PUBLIC_TOPIC_RECOVERY_GRACE_MS)
            .max(self.next_retry_at_ms)
    }

    pub(crate) fn ready(&self, now_ms: i64) -> bool {
        self.next_retry_at_ms <= now_ms
    }

    pub(crate) fn schedule(&mut self, now_ms: i64) {
        let delay_ms = PUBLIC_TOPIC_RECOVERY_BACKOFF_MS[self
            .step
            .min(PUBLIC_TOPIC_RECOVERY_BACKOFF_MS.len().saturating_sub(1))];
        self.next_retry_at_ms = now_ms.saturating_add(delay_ms);
        if self.step + 1 < PUBLIC_TOPIC_RECOVERY_BACKOFF_MS.len() {
            self.step += 1;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JoinedPrivateChannelState {
    pub(crate) topic_id: String,
    pub(crate) channel_id: ChannelId,
    pub(crate) label: String,
    pub(crate) creator_pubkey: String,
    pub(crate) owner_pubkey: String,
    pub(crate) joined_via_pubkey: Option<String>,
    pub(crate) audience_kind: ChannelAudienceKind,
    pub(crate) current_epoch_id: String,
    pub(crate) current_epoch_secret_hex: String,
    pub(crate) archived_epochs: Vec<PrivateChannelEpochCapability>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrivateChannelDiagnostics {
    pub(crate) sharing_state: ChannelSharingState,
    pub(crate) participant_count: usize,
    pub(crate) stale_participant_count: usize,
    pub(crate) rotation_required: bool,
    pub(crate) entry_dome_instance_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrivateChannelOwnerAction {
    Write,
    Share,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NotificationCandidate {
    pub(crate) kind: NotificationKind,
    pub(crate) actor_pubkey: String,
    pub(crate) source_envelope_id: Option<EnvelopeId>,
    pub(crate) source_replica_id: Option<ReplicaId>,
    pub(crate) topic_id: Option<String>,
    pub(crate) channel_id: Option<String>,
    pub(crate) object_id: Option<EnvelopeId>,
    pub(crate) dm_id: Option<String>,
    pub(crate) message_id: Option<String>,
    pub(crate) preview_text: Option<String>,
    /// Object envelope 由来の署名済み labels。object を持たない通知は `None`。
    pub(crate) content_labels: Option<Vec<String>>,
    pub(crate) created_at: i64,
    pub(crate) received_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NotificationDocEventBaseline {
    fingerprints: BTreeSet<String>,
}

impl NotificationDocEventBaseline {
    pub(crate) fn from_records(records: &[DocRecord]) -> Self {
        Self {
            fingerprints: records
                .iter()
                .map(|record| {
                    notification_doc_event_fingerprint_parts(&record.key, &record.content_hash)
                })
                .collect(),
        }
    }

    pub(crate) fn contains(&self, event: &DocEvent) -> bool {
        self.fingerprints
            .contains(notification_doc_event_fingerprint(event).as_str())
    }
}

impl AppService {
    pub(crate) fn hint_transport(&self) -> &dyn HintTransport {
        self.services.hint_transport.as_ref()
    }

    pub(crate) fn docs_sync(&self) -> &dyn DocsSync {
        self.services.docs_sync.as_ref()
    }

    pub(crate) fn keys(&self) -> &KukuriKeys {
        self.services.keys.as_ref()
    }

    pub fn new<S, T>(store: Arc<S>, transport: Arc<T>) -> Self
    where
        S: Store + ProjectionStore + 'static,
        T: Transport + HintTransport + 'static,
    {
        let docs_sync = Arc::new(MemoryDocsSync::default());
        let blob_service = Arc::new(MemoryBlobService::default());
        Self::from_handles(ServiceHandles::new(
            store.clone() as Arc<dyn Store>,
            store as Arc<dyn ProjectionStore>,
            transport.clone(),
            transport as Arc<dyn HintTransport>,
            docs_sync,
            blob_service,
            generate_keys(),
        ))
    }

    pub fn from_handles(services: ServiceHandles) -> Self {
        Self::from_handles_with_metaverse_budget(
            services,
            kukuri_core::MetaverseResourceBudgetConfig::default(),
        )
        .expect("default metaverse resource budget is valid")
    }

    pub fn from_handles_with_metaverse_budget(
        services: ServiceHandles,
        budget: kukuri_core::MetaverseResourceBudgetConfig,
    ) -> Result<Self> {
        budget.validate()?;
        let cache = MetaverseBlobCacheIndex::new(budget.client.cache_capacity_bytes)?;
        Ok(Self {
            services,
            subscription_registry: SubscriptionRegistry::default(),
            joined_private_channels: Arc::new(Mutex::new(HashMap::new())),
            metaverse_room_events: Arc::new(Mutex::new(HashMap::new())),
            dome_host_heartbeats: Arc::new(Mutex::new(HashMap::new())),
            dome_host_sessions: Arc::new(Mutex::new(HashMap::new())),
            metaverse_blob_cache: Arc::new(Mutex::new(cache)),
            metaverse_resource_budget: budget,
            last_sync_ts: Arc::new(Mutex::new(None)),
            public_topic_delivery: Arc::new(Mutex::new(HashMap::new())),
            empty_recovery_candidates: Arc::new(Mutex::new(HashSet::new())),
            gossip_disabled_topics: Arc::new(Mutex::new(HashSet::new())),
            gossip_disabled_channels: Arc::new(Mutex::new(HashSet::new())),
            private_channel_capability_persist: std::sync::OnceLock::new(),
            private_channel_capability_persist_guard: Arc::new(Mutex::new(())),
            notification_inserted_notify: Arc::new(tokio::sync::Notify::new()),
            adult_content_display_enabled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            advisory_media_hashes: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    /// capability registry の write-through 永続化 callback を接続する。
    /// registry を変異させるメソッド(register/remove 経由の全経路)は、以後
    /// persist 試行が完了するまで return しない。復元
    /// (`restore_private_channel_capability`)完了後に 1 回だけ呼ぶこと —
    /// 復元前に接続すると復元途中の部分リストが永続化される。
    pub fn set_private_channel_capability_persist(&self, persist: PrivateChannelCapabilityPersist) {
        let _ = self.private_channel_capability_persist.set(persist);
    }

    pub fn notification_inserted_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.notification_inserted_notify)
    }

    pub fn metaverse_resource_budget(&self) -> &kukuri_core::MetaverseResourceBudgetConfig {
        &self.metaverse_resource_budget
    }

    pub(crate) async fn resolve_repost_source(
        &self,
        source_topic_id: &str,
        source_object_id: &str,
    ) -> Result<ResolvedRepostSource> {
        let source_object_id = EnvelopeId::from(source_object_id);
        // #1239: repost 元が projection に無ければ、その key だけを反映する。topic の replica は走査しない。
        self.ensure_object_projection(
            source_topic_id,
            &TimelineScope::Public,
            &source_object_id,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?;
        let projection = ObjectProjectionStore::get_object_projection(
            self.services.projection_store.as_ref(),
            &source_object_id,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("repost source object not found"))?;
        if projection.topic_id != source_topic_id {
            anyhow::bail!("repost source topic does not match");
        }
        if projection.channel_id != PUBLIC_CHANNEL_ID {
            anyhow::bail!("only public posts and comments can be reposted");
        }
        if !matches!(projection.object_kind.as_str(), "post" | "comment") {
            anyhow::bail!("only public posts and comments can be reposted");
        }

        // snapshot は検証済みの行から作る(#1248)。docs の `state` は読まない。行は署名つき envelope から
        // 作られているので、著者・添付・返信先は署名された値になる。
        let content = match &projection.payload_ref {
            PayloadRef::InlineText { text } => text.clone(),
            PayloadRef::BlobText { hash, .. } => {
                fetch_projection_blob_text(self.services.blob_service.as_ref(), hash)
                    .await
                    .ok_or_else(|| anyhow::anyhow!("repost source content is unavailable"))?
            }
        };
        Ok(ResolvedRepostSource {
            repost_of: RepostSourceSnapshotV1 {
                source_object_id: projection.object_id,
                source_topic_id: TopicId::new(projection.topic_id),
                source_author_pubkey: Pubkey::from(projection.author_pubkey),
                source_object_kind: projection.object_kind,
                content,
                attachments: projection.attachments,
                reply_to_object_id: projection.reply_to_object_id,
                root_id: projection.root_object_id,
                content_labels: projection.content_labels,
            },
        })
    }

    pub(crate) async fn find_existing_simple_repost(
        &self,
        target_topic_id: &str,
        source_object_id: &str,
        commentary: Option<&str>,
    ) -> Result<Option<String>> {
        if commentary.is_some() {
            return Ok(None);
        }
        // #1239: topic の全 `objects/` を読まず、projection の索引(著者 + repost 元)で引く。
        // 取り下げ済みの repost は既存の repost として扱わない。
        let local_author_pubkey = self.current_author_pubkey();
        let candidates = self
            .services
            .projection_store
            .find_author_reposts_of(
                target_topic_id,
                local_author_pubkey.as_str(),
                &EnvelopeId::from(source_object_id),
                EXISTING_REPOST_LOOKUP_LIMIT,
            )
            .await?;
        for row in candidates {
            if row.channel_id != PUBLIC_CHANNEL_ID {
                continue;
            }
            let commentary = match &row.payload_ref {
                PayloadRef::InlineText { text } => normalize_repost_commentary(Some(text.clone())),
                PayloadRef::BlobText { .. } => continue,
            };
            if commentary.is_some() {
                continue;
            }
            if self
                .services
                .projection_store
                .get_post_withdrawal(&row.object_id)
                .await?
                .is_some()
            {
                continue;
            }
            return Ok(Some(row.object_id.as_str().to_string()));
        }
        Ok(None)
    }

    pub(crate) async fn docs_assisted_peer_ids(&self) -> Result<Vec<String>> {
        self.services.docs_sync.assist_peer_ids().await
    }

    pub(crate) async fn blob_assisted_peer_ids(&self) -> Result<Vec<String>> {
        self.services.blob_service.assist_peer_ids().await
    }

    pub(crate) async fn next_subscription_generation(&self, key: &str) -> u64 {
        let mut generations = self
            .subscription_registry
            .subscription_generations
            .lock()
            .await;
        let generation = generations
            .get(key)
            .copied()
            .unwrap_or_default()
            .saturating_add(1);
        generations.insert(key.to_string(), generation);
        generation
    }

    pub(crate) async fn reset_public_topic_delivery_generation(
        &self,
        topic_id: &str,
        generation: u64,
    ) {
        self.public_topic_delivery.lock().await.insert(
            topic_id.to_string(),
            PublicTopicDeliveryStatus {
                generation,
                last_docs_activity_at: None,
            },
        );
    }

    pub(crate) async fn clear_public_topic_delivery(&self, topic_id: &str) {
        self.public_topic_delivery.lock().await.remove(topic_id);
    }

    pub(crate) async fn public_topic_delivery_status(
        &self,
        topic_id: &str,
    ) -> Option<PublicTopicDeliveryStatus> {
        self.public_topic_delivery
            .lock()
            .await
            .get(topic_id)
            .copied()
    }

    pub(crate) async fn restart_active_subscriptions(&self) -> Result<()> {
        let topics = self
            .subscription_registry
            .subscriptions
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for topic in topics {
            self.restart_topic_subscription(topic.as_str()).await?;
        }

        let private_channels = self
            .joined_private_channels
            .lock()
            .await
            .values()
            .map(|state| {
                (
                    state.topic_id.clone(),
                    state.channel_id.as_str().to_string(),
                )
            })
            .collect::<Vec<_>>();
        for (topic_id, channel_id) in private_channels {
            self.restart_private_channel_subscription(topic_id.as_str(), channel_id.as_str())
                .await?;
        }

        let authors = self
            .subscription_registry
            .author_subscriptions
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for author in authors {
            self.restart_author_subscription(author.as_str()).await?;
        }
        self.restart_direct_message_subscriptions().await?;
        Ok(())
    }

    pub async fn shutdown(&self) {
        let topics_to_unsubscribe = self
            .subscription_registry
            .subscriptions
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let private_channels_to_unsubscribe = self
            .subscription_registry
            .private_channel_subscriptions
            .lock()
            .await
            .keys()
            .filter_map(|key| key.split("::").nth(1).map(str::to_owned))
            .collect::<BTreeSet<_>>();
        let handles = {
            let mut subscriptions = self.subscription_registry.subscriptions.lock().await;
            subscriptions
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in handles {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
        let private_handles = {
            let mut subscriptions = self
                .subscription_registry
                .private_channel_subscriptions
                .lock()
                .await;
            subscriptions
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in private_handles {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
        for channel_id in private_channels_to_unsubscribe {
            let _ = self
                .services
                .hint_transport
                .unsubscribe_hints(&private_channel_hint_topic(channel_id.as_str()))
                .await;
        }
        for topic_id in topics_to_unsubscribe {
            let _ = self
                .services
                .hint_transport
                .unsubscribe_hints(&TopicId::new(topic_id))
                .await;
        }
        let dm_peers_to_unsubscribe = self
            .subscription_registry
            .direct_message_subscriptions
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let dm_handles = {
            let mut subscriptions = self
                .subscription_registry
                .direct_message_subscriptions
                .lock()
                .await;
            subscriptions
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in dm_handles {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
        for peer_pubkey in dm_peers_to_unsubscribe {
            if let Ok(topic) = derive_direct_message_topic(
                self.services.keys.as_ref(),
                &Pubkey::from(peer_pubkey.as_str()),
            ) {
                let _ = self.services.hint_transport.unsubscribe_hints(&topic).await;
            }
        }
        let author_handles = {
            let mut subscriptions = self.subscription_registry.author_subscriptions.lock().await;
            subscriptions
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in author_handles {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
        let presence_handles = {
            let mut tasks = self.subscription_registry.live_presence_tasks.lock().await;
            tasks.drain().map(|(_, handle)| handle).collect::<Vec<_>>()
        };
        for handle in presence_handles {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
    }

    pub(crate) fn current_author_pubkey(&self) -> String {
        self.services.keys.public_key_hex()
    }

    pub(crate) async fn reaction_state_for_target(
        &self,
        source_replica_id: &ReplicaId,
        target_object_id: &EnvelopeId,
    ) -> Result<ReactionStateView> {
        let rows = self
            .services
            .projection_store
            .list_reaction_cache_for_target(source_replica_id, target_object_id)
            .await?;
        Ok(reaction_state_view_from_rows(
            source_replica_id,
            target_object_id,
            rows,
            self.current_author_pubkey().as_str(),
        ))
    }
}

#[cfg(test)]
#[path = "../tests/mod.rs"]
mod tests;
