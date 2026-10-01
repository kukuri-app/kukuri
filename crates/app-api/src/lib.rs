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
// ブラウザでも動く共用 crate（ADR 0056 §3）。tokio・std の時刻と task を直接使わない（native では
// n0_future・web_time がそれらの再公開なので、wasm32 の clippy で確かめる）。
#![cfg_attr(
    all(target_family = "wasm", not(test)),
    warn(clippy::disallowed_methods)
)]

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
pub use sync::{
    CONNECTIVITY_PEER_PAGE_LIMIT, ConnectivityPeersRequest, ScopeDisplayRequest, ScopeDisplayTarget,
};
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

/// `deadline` までに終わらなければ `Elapsed`（tokio の `timeout_at` と同じ。`n0_future::time` には無い）。
pub(crate) fn timeout_at<F: std::future::IntoFuture>(
    deadline: n0_future::time::Instant,
    future: F,
) -> n0_future::time::Timeout<F::IntoFuture> {
    n0_future::time::timeout(
        deadline.saturating_duration_since(n0_future::time::Instant::now()),
        future,
    )
}
