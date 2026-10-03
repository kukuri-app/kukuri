//! community node user-api(公開 HTTP surface)。
//!
//! WP-H4 でドメイン別モジュールへ分割した。公開面(re-export)は分割前と同一で、
//! 呼び出し側(main / テスト / harness)は変更不要。
//!
//! - `config` … 起動設定(env 読込)と rate limit 設定
//! - `state` … 実行時 state(DI)と構築
//! - `rate_limit` … per-client rate limit layer
//! - `routes` … route 定義(パスは kukuri-cn-protocol の共有定数)と起動
//! - `errors` … 共通のエラー写像
//! - `handlers/` … ドメイン別ハンドラ(auth / consents / reports / indexing /
//!   trust_relation / bootstrap)

mod admin;
mod admin_action_render;
mod admin_appeal_render;
mod admin_rights_requests;
mod admin_shell;
mod admin_tester_feedback;
mod config;
mod dome_hosting;
mod errors;
mod handlers;
mod rate_limit;
mod routes;
mod state;

pub use config::{RateLimitConfig, UserApiConfig};
pub use rate_limit::apply_rate_limit;
pub use routes::{app_router, manifest_routes, run_from_env, with_cors};
pub use state::{RelationVisibilityState, TrustReadState, UserApiState, build_state};
