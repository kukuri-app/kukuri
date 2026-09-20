//! kukuri community node indexing participant（#413 / ADR 0025 §6）。
//!
//! CN は現状 docs 非参加のため、cn-indexer が iroh-docs を駆動する docs replica sync participant を
//! 新設する。責務は「取得経路 = Model C」の ingest→投影まで:
//!
//! 1. supported topic / 許可 channel の共有 replica を sync する participant（`participant`）。
//! 2. 共有 replica に実在する post entry のみを scan→`allow` 判定して index 投影に書く（`ingest`）。
//! 3. index 投影 store の境界と ArcadeDB adapter（`projection` / `arcadedb`）。全文のみ、canonical
//!    ではない写像。
//! 4. relay validation 起動 gate（`config`）: 自前 relay も外部 relay も無ければ indexing を起動しない。
//!
//! scope 管理 state（supported set / user request / channel capability）は cn-core（Postgres）が所有し、
//! ユーザー向け indexing request 受付 API は cn-user-api が持つ。ユーザー向け search / discovery /
//! recommendation の query 境界と fail-closed query gate は `query`（#404）が持ち、cn-user-api が
//! それを read エンドポイントとして公開する。
//!
//! 設計の真実源:
//! - `docs/adr/0025-community-node-indexing-foundation.md`（§2.2 scope / §2.5 fail-closed / §2.7 検索 UX / §6 Model C）

pub mod arcadedb;
pub mod config;
pub mod ingest;
pub mod media_fetcher;
pub mod participant;
pub mod projection;
pub mod query;
pub mod relation_graph;
pub mod relation_worker;
pub mod runtime;
pub mod scheduler;
pub mod state;
pub mod status;
pub mod worker;

pub use arcadedb::ArcadeDbProjection;
pub use config::{ArcadeDbConfig, IndexerConfig, MediaFetchConfig, RelayConfig, RelayValidation};
pub use ingest::{IngestPipeline, IngestSummary};
pub use media_fetcher::BlobMediaFetcher;
pub use participant::{IndexerParticipant, ScopeReplica};
pub use projection::{IndexProjection, IndexedEntry, MemoryIndexProjection};
pub use query::{FailClosedIndexQuery, IndexQuery, MAX_QUERY_LIMIT, clamp_query_limit};
pub use relation_graph::ArcadeDbRelationGraph;
pub use relation_worker::{
    DEFAULT_ANALYSIS_LIMIT, RelationAnalysisReport, analyze_relations, topic_cluster,
};
pub use runtime::{run_from_env, validate_config_from_env};
pub use scheduler::{
    PostFetchJobKey, PostFetchJobState, PostFetchScheduler, PostFetchSchedulerSnapshot,
};
pub use state::{IndexerRuntimeState, IndexerStateSnapshot};
pub use status::{StatusServerHandle, spawn_status_server};
pub use worker::{IndexerWorker, WorkerConfig, WorkerHandle};
