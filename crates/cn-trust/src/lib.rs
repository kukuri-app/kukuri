//! kukuri community node の trust / relation foundation の **pure domain 層**（#415）。
//!
//! この crate は DB / network / credential に依存しない。trust の合成式（ADR 0026 §6.2）・
//! 成分算出（絶対 / 相対）・根拠つき read view・relation の graph-store 抽象（§6.1）と
//! in-memory 実装を提供し、deterministic な単体テストで ADR 0026 の contract を固定する。
//!
//! 設計の真実源: `docs/adr/0026-community-node-trust-relation-foundation.md`
//! - §2.3 絶対指標と相対指標の分離（report-bombing 耐性）
//! - §2.7 risk signal category による絶対 / 相対振り分け（#406 の供給契約が上流）
//! - §6.1 graph-store 抽象境界（ArcadeDB 最小 / neo4j scale、Cypher 互換）
//! - §6.2 合成式・最終クランプ `[-1, 1]`・相対成分の半減期減衰・appeal 反映
//! - §6.3 cross-node 開示（confirmed 絶対成分のみ）・viewer 相対 read の認証
//! - §8 trust 絶対値 T と閲覧者別 relation 値 R の合算（ブロック / ミュート観測、#1061）
//! - §10 対象ごとの集計と basis のページ（#1702）
//!
//! スコープ境界（本 crate に含まないもの）:
//! - risk signal の永続化と、対象ごとの集計・basis のページの読取り（`cn-core`）
//! - relation graph の ArcadeDB 実装・解析 worker（`cn-indexer`）
//! - read エンドポイント / viewer 認証 / cross-node pull の HTTP 境界（`cn-user-api`）
//!
//! trust / relation は node-local **advisory** であり、network-wide command でも canonical でも
//! ない（ADR 0027 §2.1）。user identity / profile / social graph の canonical を所有・改変しない。

pub mod disclosure;
pub mod inputs;
pub mod memory;
pub mod params;
pub mod read;
pub mod relation;
pub mod relation_adjustment;
pub mod score;
pub mod totals;

/// `RelationStore` 実装への共有 contract スイート。`testing` feature でのみ有効
/// （in-memory / ArcadeDB / 将来の neo4j に同一契約を課し、drift を防ぐ）。
#[cfg(feature = "testing")]
pub mod relation_testing;

pub use disclosure::{CrossNodeTrustDisclosure, PullAudience, cross_node_trust_disclosure};
pub use inputs::{ObservedSignal, TrustComponentKind, TrustRiskInput, trust_component_for};
pub use memory::MemoryRelationStore;
pub use params::TrustParams;
pub use read::{TrustBasisEntry, TrustReadView, build_trust_read, trust_basis};
pub use relation::{
    ClusterRef, EdgeFeatures, FEATURE_CO_PARTICIPATION_EVENTS, FEATURE_FOLLOW_PROJECTION,
    FEATURE_SHARED_TOPICS, Proximity, ProximityBasisEntry, RelationStore, proximity_from_features,
};
pub use relation_adjustment::{
    RelationAdjustment, RelationObservation, RelationObservationKind, apply_viewer_relation,
    compose_relation_adjustment, compose_viewer_trust, relation_version,
};
pub use score::{ComposedTrust, compose_trust, decay_factor, signal_contribution};
pub use totals::{TRUST_UNITS_PER_CONTRIBUTION, TrustTotals};
