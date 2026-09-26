use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::PgPool;

use kukuri_cn_core::{RelationAnalyzeRun, initialize_database, record_relation_analyze_run};
use kukuri_cn_indexer::{ArcadeDbConfig, ArcadeDbRelationGraph, analyze_relations};

use crate::RelationAction;

pub(super) async fn run(pool: &PgPool, action: RelationAction) -> Result<()> {
    match action {
        RelationAction::Analyze { limit } => {
            // 実行記録の書き込み先（cn_admin.relation_analyze_runs）を先に確保する。
            initialize_database(pool).await?;
            let started_at = Utc::now();
            let outcome = analyze(pool, limit).await;
            let finished_at = Utc::now();

            // 成否にかかわらず記録する（readiness の relation_analysis_recent が最終成否を
            // 機械判定するため）。error にはエラー種別の要約のみを入れ、応答本文を含めない。
            let run = match &outcome {
                Ok(report) => RelationAnalyzeRun {
                    started_at,
                    finished_at,
                    success: true,
                    edges_upserted: i64::try_from(report.edges_upserted).unwrap_or(i64::MAX),
                    clusters_assigned: i64::try_from(report.clusters_assigned).unwrap_or(i64::MAX),
                    error: None,
                },
                Err(error) => RelationAnalyzeRun {
                    started_at,
                    finished_at,
                    success: false,
                    edges_upserted: 0,
                    clusters_assigned: 0,
                    error: Some(format!("{error:#}")),
                },
            };
            record_relation_analyze_run(pool, &run).await?;

            let report = outcome?;
            println!(
                "relation analysis done: {} edge(s) upserted, {} removed, {} cluster(s) assigned, {} cleared",
                report.edges_upserted,
                report.edges_removed,
                report.clusters_assigned,
                report.clusters_cleared
            );
        }
    }
    Ok(())
}

async fn analyze(pool: &PgPool, limit: usize) -> Result<kukuri_cn_indexer::RelationAnalysisReport> {
    let graph = ArcadeDbRelationGraph::new(ArcadeDbConfig::from_env())
        .context("failed to build ArcadeDB relation graph client")?;
    graph
        .ensure_schema()
        .await
        .context("failed to ensure relation graph schema")?;
    analyze_relations(pool, &graph, limit).await
}
