//! 実行時 readiness の収集と判定（#616 T2）。
//!
//! cn-indexer の状態エンドポイント・Postgres の整合検査・ArcadeDB 投影・関係解析の
//! 実行記録を集め、`RUNTIME_CHECK_IDS` のうち走査網羅系の判定項目を合格/不合格へ
//! 確定させる。判定は純関数（`evaluate`）に分離し、収集の失敗はすべて不合格へ倒す
//! （安全側）。出力へ秘匿情報を含めない。

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use kukuri_cn_core::{
    IndexIntegrityFindings, RelationAnalyzeRun, ensure_database_ready, inspect_index_integrity,
    latest_relation_analyze_run,
};
use kukuri_cn_indexer::{ArcadeDbConfig, ArcadeDbProjection, IndexerStateSnapshot};
use kukuri_cn_operator::{ReadinessCheck, ReadinessStatus};

/// 収集した実行時情報。取得に失敗した要素は Err(要約) を保持し、判定側で不合格へ倒す。
pub(crate) struct RuntimeFindings {
    pub snapshot: Result<IndexerStateSnapshot, String>,
    pub migrations_ready: Result<(), String>,
    pub integrity: Result<IndexIntegrityFindings, String>,
    pub projection_count: Result<u64, String>,
    pub relation_run: Result<Option<RelationAnalyzeRun>, String>,
    pub now: DateTime<Utc>,
    pub ingest_max_age_secs: i64,
    pub relation_max_age_secs: i64,
}

/// 実行時情報を収集する。個々の失敗はこの関数では握りつぶさず Err 文字列として保持する。
pub(crate) async fn collect(
    pool: &PgPool,
    indexer_status_url: &str,
    ingest_max_age_secs: i64,
    relation_max_age_secs: i64,
) -> RuntimeFindings {
    let snapshot = fetch_snapshot(indexer_status_url).await;
    let migrations_ready = ensure_database_ready(pool)
        .await
        .map_err(|error| format!("{error:#}"));
    let integrity = inspect_index_integrity(pool)
        .await
        .map_err(|error| format!("{error:#}"));
    let projection_count = match ArcadeDbProjection::new(ArcadeDbConfig::from_env()) {
        Ok(projection) => projection
            .count_all()
            .await
            .map_err(|error| format!("{error:#}")),
        Err(error) => Err(format!("{error:#}")),
    };
    let relation_run = latest_relation_analyze_run(pool)
        .await
        .map_err(|error| format!("{error:#}"));
    RuntimeFindings {
        snapshot,
        migrations_ready,
        integrity,
        projection_count,
        relation_run,
        now: Utc::now(),
        ingest_max_age_secs,
        relation_max_age_secs,
    }
}

async fn fetch_snapshot(indexer_status_url: &str) -> Result<IndexerStateSnapshot, String> {
    let url = format!("{}/v1/status", indexer_status_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| format!("状態エンドポイントへ到達できません: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "状態エンドポイントが HTTP {} を返しました",
            response.status().as_u16()
        ));
    }
    response
        .json::<IndexerStateSnapshot>()
        .await
        .map_err(|_| "状態エンドポイントの応答を解釈できません".to_string())
}

/// 収集結果から走査網羅系の判定項目を組み立てる純関数。
pub(crate) fn evaluate(findings: &RuntimeFindings) -> Vec<ReadinessCheck> {
    let mut checks = Vec::new();

    // indexer の常駐と取り込みの状態。
    match &findings.snapshot {
        Ok(snapshot) => {
            checks.push(check(
                "indexer_worker_running",
                snapshot.worker_running && snapshot.ingest_enabled,
                format!(
                    "worker_running={} ingest_enabled={}",
                    snapshot.worker_running, snapshot.ingest_enabled
                ),
            ));
            checks.push(check(
                "indexer_scopes_opened",
                snapshot.opened_scopes >= 1,
                format!("opened_scopes={}", snapshot.opened_scopes),
            ));
            let now = findings.now.timestamp();
            let fresh = snapshot
                .last_sync_at
                .is_some_and(|at| now - at <= findings.ingest_max_age_secs)
                && snapshot
                    .last_ingest_at
                    .is_some_and(|at| now - at <= findings.ingest_max_age_secs);
            checks.push(check(
                "indexer_ingest_fresh",
                fresh,
                format!(
                    "last_sync_at={:?} last_ingest_at={:?} (許容 {} 秒)",
                    snapshot.last_sync_at, snapshot.last_ingest_at, findings.ingest_max_age_secs
                ),
            ));
            // 走査網羅の計数の取得可否。索引の安全側不変条件は、判定の無い行を FK が、失敗が許可へ落ちた
            // 判定を CHECK が保存の時点で拒否し、許可以外・重大へ変わった行を読み口が落とすので数えない（#1714）。
            checks.push(check(
                "scan_coverage_metrics_available",
                true,
                format!(
                    "scanned={} indexed={} skipped_non_allow={} scan_errors={} \
                     provider_unavailable={} media_fetch(success/unavailable/timeout/oversize)={}/{}/{}/{}",
                    snapshot.scanned,
                    snapshot.indexed,
                    snapshot.skipped_non_allow,
                    snapshot.scan_errors,
                    snapshot.provider_unavailable,
                    snapshot.media_fetch_success,
                    snapshot.media_fetch_unavailable,
                    snapshot.media_fetch_timeout,
                    snapshot.media_fetch_oversize,
                ),
            ));
        }
        Err(error) => {
            for id in [
                "indexer_worker_running",
                "indexer_scopes_opened",
                "indexer_ingest_fresh",
                "scan_coverage_metrics_available",
            ] {
                checks.push(check(id, false, error.clone()));
            }
        }
    }

    checks.push(match &findings.migrations_ready {
        Ok(()) => check(
            "postgres_migrations_ready",
            true,
            "必要な schema / table がすべて存在します".to_string(),
        ),
        Err(error) => check("postgres_migrations_ready", false, error.clone()),
    });

    checks.push(match &findings.projection_count {
        Ok(count) => check(
            "arcadedb_projection_ready",
            true,
            format!("投影へ到達済み (項目数 {count})"),
        ),
        Err(error) => check("arcadedb_projection_ready", false, error.clone()),
    });

    // 真実源と投影の突合: 投影が真実源より多い（ghost の疑い）を不合格にする。
    checks.push(match (&findings.projection_count, &findings.integrity) {
        (Ok(projection), Ok(integrity)) => {
            let truth = u64::try_from(integrity.index_entries_total).unwrap_or(0);
            check(
                "index_truth_projection_consistent",
                *projection <= truth,
                format!("truth={truth} projection={projection}"),
            )
        }
        (Err(error), _) | (_, Err(error)) => {
            check("index_truth_projection_consistent", false, error.clone())
        }
    });

    checks.push(match &findings.relation_run {
        Ok(Some(run)) => {
            let fresh = findings.now - run.finished_at
                <= chrono::Duration::seconds(findings.relation_max_age_secs);
            check(
                "relation_analysis_recent",
                run.success && fresh,
                format!(
                    "success={} finished_at={} (許容 {} 秒){}",
                    run.success,
                    run.finished_at.to_rfc3339(),
                    findings.relation_max_age_secs,
                    run.error
                        .as_deref()
                        .map(|error| format!(" error={error}"))
                        .unwrap_or_default(),
                ),
            )
        }
        Ok(None) => check(
            "relation_analysis_recent",
            false,
            "関係解析の実行記録がありません".to_string(),
        ),
        Err(error) => check("relation_analysis_recent", false, error.clone()),
    });

    checks.push(match &findings.integrity {
        Ok(integrity) => check(
            "supported_scopes_public_only",
            integrity.private_scopes_supported == 0,
            format!(
                "プライベートチャンネルの索引対象={}",
                integrity.private_scopes_supported
            ),
        ),
        Err(error) => check("supported_scopes_public_only", false, error.clone()),
    });

    checks
}

fn check(id: &'static str, pass: bool, detail: String) -> ReadinessCheck {
    ReadinessCheck {
        id,
        status: if pass {
            ReadinessStatus::Pass
        } else {
            ReadinessStatus::Fail
        },
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn healthy_findings() -> RuntimeFindings {
        let now = Utc.timestamp_opt(1_700_010_000, 0).unwrap();
        RuntimeFindings {
            snapshot: Ok(IndexerStateSnapshot {
                worker_running: true,
                ingest_enabled: true,
                opened_scopes: 4,
                last_sync_at: Some(now.timestamp() - 60),
                last_ingest_at: Some(now.timestamp() - 60),
                ..IndexerStateSnapshot::default()
            }),
            migrations_ready: Ok(()),
            integrity: Ok(IndexIntegrityFindings {
                index_entries_total: 10,
                ..IndexIntegrityFindings::default()
            }),
            projection_count: Ok(10),
            relation_run: Ok(Some(RelationAnalyzeRun {
                started_at: now - chrono::Duration::seconds(120),
                finished_at: now - chrono::Duration::seconds(60),
                success: true,
                edges_upserted: 3,
                clusters_assigned: 2,
                error: None,
            })),
            now,
            ingest_max_age_secs: 900,
            relation_max_age_secs: 7200,
        }
    }

    fn status_of<'a>(checks: &'a [ReadinessCheck], id: &str) -> &'a ReadinessCheck {
        checks
            .iter()
            .find(|check| check.id == id)
            .unwrap_or_else(|| panic!("check {id} not found"))
    }

    #[test]
    fn healthy_findings_pass_all_runtime_checks() {
        let checks = evaluate(&healthy_findings());
        assert_eq!(checks.len(), 9);
        for check in &checks {
            assert_eq!(
                check.status,
                ReadinessStatus::Pass,
                "{}: {}",
                check.id,
                check.detail
            );
        }
    }

    #[test]
    fn stale_ingest_and_old_relation_run_fail() {
        let mut findings = healthy_findings();
        if let Ok(snapshot) = &mut findings.snapshot {
            snapshot.last_sync_at = Some(findings.now.timestamp() - 10_000);
        }
        if let Ok(Some(run)) = &mut findings.relation_run {
            run.finished_at = findings.now - chrono::Duration::seconds(100_000);
        }
        let checks = evaluate(&findings);
        assert_eq!(
            status_of(&checks, "indexer_ingest_fresh").status,
            ReadinessStatus::Fail
        );
        assert_eq!(
            status_of(&checks, "relation_analysis_recent").status,
            ReadinessStatus::Fail
        );
    }

    #[test]
    fn full_pass_timestamp_without_successful_ingest_fails_freshness() {
        let mut findings = healthy_findings();
        if let Ok(snapshot) = &mut findings.snapshot {
            snapshot.last_ingest_at = None;
        }
        let checks = evaluate(&findings);
        assert_eq!(
            status_of(&checks, "indexer_ingest_fresh").status,
            ReadinessStatus::Fail
        );
    }

    #[test]
    fn projection_ghost_and_private_scope_fail() {
        let mut findings = healthy_findings();
        findings.projection_count = Ok(11);
        if let Ok(integrity) = &mut findings.integrity {
            integrity.private_scopes_supported = 1;
        }
        let checks = evaluate(&findings);
        assert_eq!(
            status_of(&checks, "index_truth_projection_consistent").status,
            ReadinessStatus::Fail
        );
        assert_eq!(
            status_of(&checks, "supported_scopes_public_only").status,
            ReadinessStatus::Fail
        );
    }

    #[test]
    fn unreachable_status_endpoint_fails_indexer_checks() {
        let mut findings = healthy_findings();
        findings.snapshot = Err("状態エンドポイントへ到達できません".to_string());
        let checks = evaluate(&findings);
        for id in [
            "indexer_worker_running",
            "indexer_scopes_opened",
            "indexer_ingest_fresh",
            "scan_coverage_metrics_available",
        ] {
            assert_eq!(status_of(&checks, id).status, ReadinessStatus::Fail, "{id}");
        }
    }
}
