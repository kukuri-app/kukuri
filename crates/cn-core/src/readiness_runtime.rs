//! 実行時 readiness のための Postgres 側検査（#616）。
//!
//! 索引の総数（投影との突合に使う）と、索引対象が公開トピックのみであることを数える。判定そのものは
//! 行わず件数を返し、合否の解釈は呼び出し側（`cn-cli readiness`）が行う。索引の安全側不変条件は数えない:
//! 判定の無い索引項目は FK が、失敗の理由で許可になった判定は `cn_safety.scan_verdicts` の CHECK が保存の
//! 時点で拒否し、許可以外・重大へ変わった判定の索引項目は読み口（`filter_surfaceable_objects`）が落とす（#1714）。
//!
//! あわせて、関係解析（`cn-cli relation analyze`）の実行記録の書き込みと最新取得を持つ。

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// 索引の整合検査の件数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexIntegrityFindings {
    /// 索引項目の総数（cn_index.index_entries の行数。行数の trigger が数える計数を読む）。
    pub index_entries_total: i64,
    /// 索引対象のうちプライベートチャンネルの件数（初期解禁では 0 であるべき）。
    pub private_scopes_supported: i64,
}

/// 索引の整合検査の文（試験が同じ文の読む行を確かめる）。索引の総行数によらず、計数の 1 行と
/// プライベートチャンネルの行だけを読む（#1714）。
pub const INDEX_INTEGRITY_SQL: &str = "SELECT (SELECT index_entries FROM cn_index.retention_state), \
     (SELECT count(*) FROM cn_index.supported_topics WHERE kind = 'private_channel')";

/// 索引の総数と、索引対象のプライベートチャンネルの件数を数える。
pub async fn inspect_index_integrity(pool: &PgPool) -> Result<IndexIntegrityFindings> {
    let (index_entries_total, private_scopes_supported): (i64, i64) =
        sqlx::query_as(INDEX_INTEGRITY_SQL)
            .fetch_one(pool)
            .await
            .context("failed to inspect index integrity")?;
    Ok(IndexIntegrityFindings {
        index_entries_total,
        private_scopes_supported,
    })
}

/// 関係解析の実行記録 1 件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationAnalyzeRun {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub success: bool,
    pub edges_upserted: i64,
    pub clusters_assigned: i64,
    /// 失敗時のエラー種別の要約（秘匿情報を含めない契約）。
    pub error: Option<String>,
}

/// 残す実行記録の件数（最新の成功は件数外でも残す）。
const RELATION_ANALYZE_RUNS_KEPT: i64 = 100;

/// 関係解析の実行結果を記録する。
pub async fn record_relation_analyze_run(pool: &PgPool, run: &RelationAnalyzeRun) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_admin.relation_analyze_runs \
             (started_at, finished_at, success, edges_upserted, clusters_assigned, error) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(run.started_at)
    .bind(run.finished_at)
    .bind(run.success)
    .bind(run.edges_upserted)
    .bind(run.clusters_assigned)
    .bind(run.error.as_deref())
    .execute(pool)
    .await
    .context("failed to record the relation analyze run")?;
    // 記録は直近の上限件数と最新の成功だけを残す（#1221 R5-E。台帳を無期限に増やさない）。
    sqlx::query(
        "DELETE FROM cn_admin.relation_analyze_runs
         WHERE id < (SELECT MIN(id) FROM (
                 SELECT id FROM cn_admin.relation_analyze_runs ORDER BY id DESC LIMIT $1
             ) AS recent)
           AND id IS DISTINCT FROM (
                 SELECT MAX(id) FROM cn_admin.relation_analyze_runs WHERE success
             )",
    )
    .bind(RELATION_ANALYZE_RUNS_KEPT)
    .execute(pool)
    .await
    .context("failed to prune relation analyze runs")?;
    Ok(())
}

/// 実行記録の行表現（読み戻し用の中間型）。
type RelationAnalyzeRunRow = (DateTime<Utc>, DateTime<Utc>, bool, i64, i64, Option<String>);

/// 最新の関係解析の実行記録を返す（無ければ None）。
pub async fn latest_relation_analyze_run(pool: &PgPool) -> Result<Option<RelationAnalyzeRun>> {
    let row: Option<RelationAnalyzeRunRow> = sqlx::query_as(
        "SELECT started_at, finished_at, success, edges_upserted, clusters_assigned, error \
         FROM cn_admin.relation_analyze_runs ORDER BY finished_at DESC, id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .context("failed to fetch the latest relation analyze run")?;
    Ok(row.map(
        |(started_at, finished_at, success, edges_upserted, clusters_assigned, error)| {
            RelationAnalyzeRun {
                started_at,
                finished_at,
                success,
                edges_upserted,
                clusters_assigned,
                error,
            }
        },
    ))
}
