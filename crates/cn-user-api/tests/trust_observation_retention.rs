//! #1704 AC-3: 観測の期限削除の読む量と、128 件を超える残件の回収。
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use kukuri_cn_core::{
    EXPIRED_TRUST_OBSERVATIONS, RETENTION_CLEANUP_BATCH, TestDatabase, cleanup_trust_observations,
    connect_postgres, initialize_database,
};
use serde_json::Value;
use sqlx::PgPool;

mod support;
use support::integration_test_admin_database_url;

async fn seed(pool: &PgPool, active: bool, at: DateTime<Utc>, tag: &str, count: i32) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_trust.observations
         (observer_pubkey, target_pubkey, kind, active, observed_at, observed_at_ms,
          envelope_id, received_at)
         SELECT 'observer', md5($4 || n::TEXT), 'block', $1, $2, 0, 'fixture', $2
         FROM generate_series(1, $3::INTEGER) n",
    )
    .bind(active)
    .bind(at)
    .bind(count)
    .bind(tag)
    .execute(pool)
    .await?;
    Ok(())
}

// AC-1(e) と同じく、候補選択で読む行・ページと、Tid Scan で消す行を数える。
// Tid Scan のページには InitPlan の分も含まれるため、候補選択側だけでページを比較する。
fn add_reads(plan: &Value, totals: &mut (f64, f64)) {
    if plan["Relation Name"] == "observations" && plan["Node Type"] != "ModifyTable" {
        let rows: f64 = [
            "Actual Rows",
            "Rows Removed by Filter",
            "Rows Removed by Index Recheck",
        ]
        .iter()
        .filter_map(|key| plan[*key].as_f64())
        .sum();
        totals.0 += rows * plan["Actual Loops"].as_f64().unwrap_or(1.0);
        if plan["Node Type"] != "Tid Scan" {
            totals.1 += ["Shared Hit Blocks", "Shared Read Blocks"]
                .iter()
                .filter_map(|key| plan[*key].as_f64())
                .sum::<f64>();
        }
    }
    for child in plan["Plans"].as_array().into_iter().flatten() {
        add_reads(child, totals);
    }
}

async fn reads(pool: &PgPool, now: DateTime<Utc>) -> Result<(f64, f64)> {
    sqlx::query("ANALYZE cn_trust.observations")
        .execute(pool)
        .await?;
    let mut tx = pool.begin().await?;
    let explain: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {EXPIRED_TRUST_OBSERVATIONS}"
    )))
    .bind(now - Duration::days(30))
    .bind(now - Duration::days(180))
    .bind(RETENTION_CLEANUP_BATCH)
    .fetch_one(&mut *tx)
    .await?;
    tx.rollback().await?;
    let mut totals = (0.0, 0.0);
    add_reads(&explain[0]["Plan"], &mut totals);
    Ok(totals)
}

#[tokio::test]
async fn observation_cleanup_reads_stay_fixed_and_drain_multiple_batches() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping observation retention test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(&admin_url, "cn_observation_retention_reads").await?;
    let pool = connect_postgres(&database.database_url).await?;
    let result = async {
        initialize_database(&pool).await?;
        let now = Utc::now();
        // 両方の部分索引を、もう一方の状態に期限切れがない場合も検証する。
        for active in [false, true] {
            sqlx::query("TRUNCATE cn_trust.observations")
                .execute(&pool)
                .await?;
            let expired_at = now - Duration::days(if active { 180 } else { 30 });
            seed(&pool, active, expired_at, "expired", 3).await?;
            seed(&pool, active, now, "small", 2_000).await?;
            let small = reads(&pool, now).await?;
            seed(&pool, active, now, "large", 18_000).await?;
            let large = reads(&pool, now).await?;
            eprintln!("active={active}: 2,000行={small:?}, 20,000行={large:?}");
            assert!(small.0 > 0.0 && small.1 > 0.0);
            assert_eq!(small, large, "active={active}");
        }
        sqlx::query("TRUNCATE cn_trust.observations")
            .execute(&pool)
            .await?;
        for active in [false, true] {
            seed(
                &pool,
                active,
                now - Duration::days(181),
                &format!("expired-{active}"),
                130,
            )
            .await?;
            seed(&pool, active, now, &format!("live-{active}"), 1).await?;
        }
        assert_eq!(cleanup_trust_observations(&pool, now).await?, 260);
        assert_eq!(cleanup_trust_observations(&pool, now).await?, 0);
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cn_trust.observations")
            .fetch_one(&pool)
            .await?;
        assert_eq!(remaining, 2);
        anyhow::Ok(())
    }
    .await;
    pool.close().await;
    database.cleanup().await?;
    result
}
