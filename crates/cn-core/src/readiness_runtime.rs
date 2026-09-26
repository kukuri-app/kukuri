//! 実行時 readiness のための Postgres 側検査（#616）。
//!
//! 索引の安全側不変条件（判定の無い索引項目が無い・許可以外や重大判定が表出しない・
//! プロバイダ失敗が許可へ落ちていない）と、索引対象が公開トピックのみであることを
//! 実測で数える。判定そのものは行わず件数を返し、合否の解釈は呼び出し側
//! （`cn-cli readiness`）が行う。
//!
//! あわせて、関係解析（`cn-cli relation analyze`）の実行記録の書き込みと最新取得を持つ。

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// 索引の整合検査の件数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexIntegrityFindings {
    /// 索引項目の総数（cn_index.index_entries）。
    pub index_entries_total: i64,
    /// 対応する判定行が存在しない索引項目の件数（0 であるべき）。
    pub entries_without_verdict: i64,
    /// 最新判定が許可以外または重大に変わっているのに索引へ残っている件数（0 であるべき）。
    pub non_allow_or_critical_surfaced: i64,
    /// プロバイダ失敗系（scan_failed / provider_unavailable / unscanned）の理由で
    /// 許可になっている判定の件数（0 であるべき = 失敗が許可へ落ちていない）。
    pub provider_failure_allowed: i64,
    /// 索引対象のうちプライベートチャンネルの件数（初期解禁では 0 であるべき）。
    pub private_scopes_supported: i64,
}

/// 索引の安全側不変条件を実測で数える。
pub async fn inspect_index_integrity(pool: &PgPool) -> Result<IndexIntegrityFindings> {
    let (index_entries_total,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM cn_index.index_entries")
            .fetch_one(pool)
            .await
            .context("failed to count index entries")?;
    let (entries_without_verdict,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM cn_index.index_entries e \
         LEFT JOIN cn_safety.scan_verdicts v ON v.id = e.verdict_id \
         WHERE v.id IS NULL",
    )
    .fetch_one(pool)
    .await
    .context("failed to count index entries without a verdict")?;
    let (non_allow_or_critical_surfaced,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM cn_index.index_entries e \
         JOIN cn_safety.scan_verdicts v ON v.id = e.verdict_id \
         WHERE v.action <> 'allow' OR v.critical",
    )
    .fetch_one(pool)
    .await
    .context("failed to count surfaced non-allow entries")?;
    let (provider_failure_allowed,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM cn_safety.scan_verdicts \
         WHERE action = 'allow' \
           AND reason_code IN ('scan_failed', 'provider_unavailable', 'unscanned')",
    )
    .fetch_one(pool)
    .await
    .context("failed to count provider failures that fell through to allow")?;
    let (private_scopes_supported,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM cn_index.supported_topics WHERE kind = 'private_channel'",
    )
    .fetch_one(pool)
    .await
    .context("failed to count supported private channel scopes")?;
    Ok(IndexIntegrityFindings {
        index_entries_total,
        entries_without_verdict,
        non_allow_or_critical_surfaced,
        provider_failure_allowed,
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
