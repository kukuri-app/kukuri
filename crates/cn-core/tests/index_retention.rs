//! #1736: 満杯の回収が、他の行数によらず選ぶ行と消す行だけを読む。

use std::collections::BTreeMap;

use anyhow::Result;
use kukuri_cn_core::{
    RECLAIM_EXPIRED_SQL, REMOVE_INDEX_SCOPE_PAGE_SQL, TestDatabase, connect_postgres,
    initialize_database,
};
use serde_json::Value;
use sqlx::PgPool;

#[path = "support/query_plan.rs"]
mod query_plan;

const TABLES: [&str; 5] = [
    "cn_index.relation_actions",
    "cn_index.known_post_withdrawals",
    "cn_index.index_entries",
    "cn_safety.scan_verdicts",
    "cn_safety.content_scan_cache",
];
const FLOOR: i64 = 1_790_000_000;

/// VACUUM 直後の可視性の印を外す参照を除くため、2 回目を比較する。削除は毎回取り消す。
async fn measure(pool: &PgPool, sql: &str, table: &str, scope: bool) -> Result<(f64, f64, i64)> {
    let mut measured = (0.0, 0.0, 0);
    for _ in 0..2 {
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL session_replication_role = replica")
            .execute(&mut *tx)
            .await?;
        let blocks_sql = "SELECT pg_stat_get_xact_blocks_fetched($1::regclass)";
        let before: i64 = sqlx::query_scalar(blocks_sql)
            .bind(table)
            .fetch_one(&mut *tx)
            .await?;
        let explain = sqlx::AssertSqlSafe(format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}"));
        let plan: Value = if scope {
            sqlx::query_scalar(explain)
                .bind("public_topic")
                .bind("kukuri:topic:a5597e8979d5ae6e4c58a4bc14cefd05") // md5('retired')
                .bind(128_i64)
                .fetch_one(&mut *tx)
                .await?
        } else {
            sqlx::query_scalar(explain)
                .bind(FLOOR)
                .bind(128_i64)
                .fetch_one(&mut *tx)
                .await?
        };
        let after: i64 = sqlx::query_scalar(blocks_sql)
            .bind(table)
            .fetch_one(&mut *tx)
            .await?;
        tx.rollback().await?;
        let plan = &plan[0]["Plan"];
        let mut reads = BTreeMap::new();
        for child in plan["Plans"].as_array().into_iter().flatten() {
            query_plan::add_reads(child, &mut reads);
        }
        let name = table.split('.').next_back().expect("qualified table");
        measured = (
            reads.get(name).map_or(0.0, |read| read.0),
            if name == "scan_verdicts" {
                reads.get("index_entries").map_or(0.0, |read| read.0)
            } else {
                0.0
            },
            after - before,
        );
        let buffers = plan["Shared Hit Blocks"].as_f64().unwrap_or(0.0)
            + plan["Shared Read Blocks"].as_f64().unwrap_or(0.0);
        let deleted: f64 = plan["Plans"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|child| child["Parent Relationship"] != "InitPlan")
            .map(|child| child["Actual Rows"].as_f64().unwrap_or(0.0))
            .sum();
        assert_eq!(deleted, 128.0, "{table}: full batch");
        eprintln!(
            "{table} scope={scope}: rows={} referenced_rows={} table_blocks={} buffers={buffers} deleted={deleted}",
            measured.0, measured.1, measured.2
        );
    }
    Ok(measured)
}

#[tokio::test]
async fn full_reclaim_reads_do_not_grow_with_other_rows() -> Result<()> {
    let Some(admin_url) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        eprintln!("skipping index retention test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(&admin_url, "cn_index_reclaim_reads").await?;
    let pool = connect_postgres(&database.database_url).await?;
    let result = async {
        initialize_database(&pool).await?;
        sqlx::raw_sql(include_str!("fixtures/index_reclaim.sql"))
            .execute(&pool)
            .await?;
        sqlx::query("SELECT public.probe_seed('expired', 1, 1500)")
            .execute(&pool)
            .await?;
        let mut measurements = Vec::new();
        let mut previous = 0;
        for others in [2_000, 20_000] {
            sqlx::query("SELECT public.probe_seed('live', $1, $2)")
                .bind(previous + 1)
                .bind(others)
                .execute(&pool)
                .await?;
            previous = others;
            let mut reads = Vec::new();
            for (sql, table) in RECLAIM_EXPIRED_SQL.into_iter().zip(TABLES) {
                sqlx::raw_sql(sqlx::AssertSqlSafe(format!("VACUUM ANALYZE {table}")))
                    .execute(&pool)
                    .await?;
                reads.push(measure(&pool, sql, table, false).await?);
            }
            measurements.push(reads);
        }
        let mut scopes = Vec::new();
        for others in [2_000, 20_000] {
            sqlx::query("SELECT public.probe_seed_scope(1500, $1)")
                .bind(others)
                .execute(&pool)
                .await?;
            sqlx::query("VACUUM ANALYZE cn_index.index_entries")
                .execute(&pool)
                .await?;
            scopes.push(measure(&pool, REMOVE_INDEX_SCOPE_PAGE_SQL, TABLES[2], true).await?);
        }
        measurements[0].push(scopes[0]);
        measurements[1].push(scopes[1]);
        assert_eq!(
            measurements[0], measurements[1],
            "rows and table blocks must not grow"
        );
        for reads in &measurements[0] {
            assert_eq!(
                (reads.0, reads.1),
                (256.0, 0.0),
                "only selected and deleted rows"
            );
        }
        anyhow::Ok(())
    }
    .await;
    pool.close().await;
    database.cleanup().await?;
    result
}
