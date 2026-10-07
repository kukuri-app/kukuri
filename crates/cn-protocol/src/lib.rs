//! デスクトップ ↔ コミュニティノード間の通信プロトコル定義(WP-H3)。
//!
//! wire 型(serde 表現 = 凍結境界)・URL / pubkey の正規化・認証封筒の構築を、
//! サーバ実装(sqlx / axum / jwt / redis を持つ cn-core)から分離して共有する。
//! cn-core は本 crate を re-export するため、サーバ側の import は従来どおり。
//! desktop-runtime は本 crate だけに依存し、サーバ専用の重い依存を引き込まない。
//!
//! **ここにある型・関数の serde 表現と挙動は cn endpoint contract の一部**
//! (REFACTORING.md の凍結境界)。変更は互換性の検討とセットで行うこと。

pub mod advisory_lookup;
pub mod auth;
pub mod blob_providers;
pub mod dome_hosting;
pub mod error_codes;
pub mod index;
pub mod manifest;
pub mod models;
pub mod normalize;
pub mod paths;
pub mod rendezvous;
pub mod report;
pub mod requests;
pub mod rights_request;
pub mod tester_feedback;
pub mod trust_relation;

pub use advisory_lookup::*;
pub use auth::*;
pub use blob_providers::*;
pub use dome_hosting::*;
pub use error_codes::*;
pub use index::*;
pub use manifest::*;
pub use models::*;
pub use normalize::*;
pub use paths::*;
pub use rendezvous::*;
pub use report::*;
pub use requests::*;
pub use rights_request::*;
pub use tester_feedback::*;
pub use trust_relation::*;
