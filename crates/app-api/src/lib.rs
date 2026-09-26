//! デスクトップ向けアプリケーション API(`AppService`)。
//!
//! 配置規約(WP-B10):
//! - トップレベルのドメインファイル(`timeline.rs` / `private_channels.rs` 等)=
//!   IPC(desktop-runtime / src-tauri)から呼ばれる**公開ドメイン API**。
//!   新メソッドはまず該当ドメインファイルへ置く。
//! - `service/` 配下 = pub(crate) の内部ヘルパ(`*_support.rs`)と合成
//!   (`mod.rs` の `ServiceHandles` / `SubscriptionRegistry`)。複数ドメインから
//!   使う内部処理だけをここへ下ろす。
//! - `AppService` へフィールドを足す前に、依存なら `ServiceHandles`、購読タスク
//!   なら `SubscriptionRegistry` への収容を先に検討する(単一型への集中は既知の
//!   負債。2026-07-13 完了レビュー D12)。
//!
//! ※ `private_channels.rs` ↔ `service/private_channels_support.rs` は公開 / 内部の
//! 正当な分割であり、同名を理由に統合しない(REFACTORING.md 地雷リスト)。

mod community_index;
mod direct_messages;
pub use direct_messages::PendingReceiveDestinationPage;
mod dome_connections;
mod dome_delete;
pub use dome_delete::{DeleteDomeInput, DeleteDomeView, PendingDomeDeletionView};
mod dome_hosting;
mod dome_management;
mod dome_move;
mod game;
mod live;
mod session_display;
pub use session_display::{SessionCandidateView, SessionDisplayRequest};
pub use sync::{ScopeDisplayRequest, ScopeDisplayTarget};
mod media;
mod notifications;
mod private_channel_indexing;
mod private_channel_rendezvous;
mod private_channels;
mod reactions;
mod service;
mod social;
mod sync;
mod timeline;
mod views;

pub use kukuri_store::{NOTIFICATION_DISPATCH_PAGE_SIZE, NotificationKind};
pub use private_channels::{
    is_retryable_friend_only_grant_import_error, is_retryable_friend_plus_share_import_error,
};
pub use service::{AppService, MAX_ACTIVE_SCOPES, ScopeLimitReached, ServiceHandles};
pub use views::*;
