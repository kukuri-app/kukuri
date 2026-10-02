// ブラウザでも動く共用 crate（ADR 0056 §3）。tokio の時刻・task と std の時刻を直接使わない（native では
// n0_future・web_time がそれらの再公開なので、wasm32 の clippy で確かめる）。
#![cfg_attr(
    all(target_family = "wasm", not(test)),
    warn(clippy::disallowed_methods)
)]
mod account_store;
mod account_sync;
mod cache;
mod memory;
mod models;
mod pagination;
/// backend の差分ハーネス(試験と、他の crate の試験から `test-support` で使う)。
#[cfg(any(test, feature = "test-support"))]
pub mod parity;
mod peer_candidates;
mod private_channel_keys;
// SQLite の実装は native だけ（ADR 0056 §5）。
#[cfg(not(target_family = "wasm"))]
mod row_mapping;
#[cfg(not(target_family = "wasm"))]
mod sqlite;
mod traits;

#[cfg(test)]
mod tests;

pub use account_store::{AccountStore, PrivateIndexGrant, PrivateIndexGrantStore};
pub use account_sync::{AccountSyncRow, AccountSyncStore};
pub use cache::{
    ContentCacheStore, OWNED_INLINE_BLOB_BYTES, REMOTE_CACHE_CAPACITY_BYTES,
    REMOTE_CACHE_RECLAIM_STEP, REMOTE_CACHE_TOUCH_INTERVAL_MS, REMOTE_CACHE_UNUSED_MS,
    RemoteCacheReservation, RemoteRecordKey, merge_record_keys,
};
pub use memory::MemoryStore;
pub use models::{
    AuthorRelationshipProjectionRow, BlobCacheStatus, BookmarkCursor, BookmarkedCustomReactionRow,
    BookmarkedPostRow, ContentObservationRow, DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT,
    DirectMessageConversationRow, DirectMessageMessageRow, DirectMessageOutboxCursor,
    DirectMessageOutboxPage, DirectMessageOutboxRow, DirectMessageTombstoneRow,
    DomeConnectionProjectionRow, DomeHostingProjectionRow, GameRoomProjectionRow,
    LiveSessionProjectionRow, MutedAuthorRow, NotificationCursor, NotificationKind,
    NotificationRow, ObjectProjectionRow, Page, PostWithdrawalRow, PrivateChannelParticipantRow,
    ReactionProjectionRow, TimelineCursor, VERIFIED_OBJECT_PROJECTION_VERSION,
    VERIFIED_REACTION_PROJECTION_VERSION, VERIFIED_SESSION_PROJECTION_VERSION, WithdrawalWriteRow,
    adult_media_hashes_for_row, bookmark_cache_refs,
};
pub use peer_candidates::{
    LEARNED_BUDGET_BYTES, LEARNED_PRUNE_STEP, LEARNED_RETENTION_MS, LEARNED_SOURCE, MAX_ADDR_BYTES,
    PeerCandidateStore, peer_candidate_bytes,
};
pub use private_channel_keys::{
    PrivateChannelEpochRange, PrivateChannelEpochRow, PrivateChannelFilter, PrivateChannelKeyStore,
    PrivateChannelRow,
};
#[cfg(not(target_family = "wasm"))]
pub use sqlite::{
    EMPTY_NAMESPACES_KIND, LEGACY_STORE_KINDS, LEGACY_STORE_PAGE, LegacyProjectionPage,
    PROTECTED_MIGRATION_KINDS, PROTECTED_MIGRATION_PAGE, ProtectedCandidate,
    ProtectedMigrationPage, ProtectedSource, SqliteStore, StoreStartupError,
};
pub use traits::{
    BOOKMARKED_CUSTOM_REACTION_LIMIT, CONTENT_OBSERVATION_RETENTION_MS, ContentObservationStore,
    DirectMessageStore, LiveGameProjectionStore, MAX_CONTENT_OBSERVATIONS,
    NOTIFICATION_DISPATCH_PAGE_SIZE, NOTIFICATION_PAGE_SIZE, NotificationStore,
    ObjectProjectionStore, PostWithdrawalStore, ProjectionStore, ReactionBookmarkStore,
    SocialProjectionStore, Store,
};
