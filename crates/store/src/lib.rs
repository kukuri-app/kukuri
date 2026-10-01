// ブラウザでも動く共用 crate（ADR 0056 §3）。tokio の時刻・task と std の時刻を直接使わない（native では
// n0_future・web_time がそれらの再公開なので、wasm32 の clippy で確かめる）。
#![cfg_attr(
    all(target_family = "wasm", not(test)),
    warn(clippy::disallowed_methods)
)]
mod account_store;
mod cache;
mod memory;
mod models;
mod pagination;
// SQLite の実装は native だけ（ADR 0056 §5）。
#[cfg(not(target_family = "wasm"))]
mod row_mapping;
#[cfg(not(target_family = "wasm"))]
mod sqlite;
mod traits;

#[cfg(test)]
mod tests;

pub use account_store::{AccountStore, PrivateIndexGrant, PrivateIndexGrantStore};
pub use cache::{
    ContentCacheStore, OWNED_INLINE_BLOB_BYTES, REMOTE_CACHE_CAPACITY_BYTES,
    REMOTE_CACHE_RECLAIM_STEP, RemoteCacheReservation, RemoteRecordKey,
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
    adult_media_hashes_for_row,
};
#[cfg(not(target_family = "wasm"))]
pub use sqlite::{
    EMPTY_NAMESPACES_KIND, LEGACY_STORE_KINDS, LEGACY_STORE_PAGE, LegacyProjectionPage,
    PROTECTED_MIGRATION_KINDS, PROTECTED_MIGRATION_PAGE, ProtectedCandidate,
    ProtectedMigrationPage, ProtectedSource, SqliteStore, StoreStartupError,
};
pub use traits::{
    BOOKMARKED_CUSTOM_REACTION_LIMIT, ContentObservationStore, DirectMessageStore,
    LiveGameProjectionStore, NOTIFICATION_DISPATCH_PAGE_SIZE, NOTIFICATION_PAGE_SIZE,
    NotificationStore, ObjectProjectionStore, PostWithdrawalStore, ProjectionStore,
    ReactionBookmarkStore, SocialProjectionStore, Store,
};
