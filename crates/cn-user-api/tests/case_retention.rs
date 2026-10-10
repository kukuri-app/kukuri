//! #1704 AC-2: cn-user-api の起動は期限削除を待たず、背景の処理が起動直後から消す（ADR 0034 §4）。
//!
//! Postgres を要するため `KUKURI_CN_RUN_INTEGRATION_TESTS=1` で gate する。

use std::time::Duration;

use anyhow::Result;
use kukuri_cn_core::{
    TestDatabase, connect_postgres, get_community_node_report, initialize_database,
};
use kukuri_cn_user_api::spawn_retention_cleanup;
use sqlx::PgPool;

mod support;
use support::{TestServer, integration_test_admin_database_url};

async fn reports(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM cn_admin.reports")
        .fetch_one(pool)
        .await?)
}

#[tokio::test]
async fn startup_leaves_expired_rows_to_the_background_cleanup() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping case retention startup test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let prefix = "cn_user_api_retention_startup";
    let database = TestDatabase::create(admin_url.as_str(), prefix).await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    sqlx::query(
        "INSERT INTO cn_admin.reports (id, subject_kind, subject_id, capability, reason, created_at)
         VALUES ('expired-report', 'post', 'post-1', 'moderation', 'spam',
                 NOW() - INTERVAL '1000 days')",
    )
    .execute(&pool)
    .await?;
    let mut started = None;
    let server = TestServer::spawn_on(
        database,
        prefix,
        kukuri_cn_operator::SAMPLE_CONFIG,
        |state| {
            started = Some(state.clone());
            state
        },
    )
    .await?;
    let result = async {
        // 起動の経路は期限切れを消さない。通常読取りは期限切れを返さない。
        assert_eq!(reports(&pool).await?, 1);
        assert!(
            get_community_node_report(&pool, "expired-report")
                .await?
                .is_none()
        );
        // 背景の処理は、1 時間を待たずに最初の回で消す。
        let cleanup = spawn_retention_cleanup(started.expect("started state"));
        let deleted = tokio::time::timeout(Duration::from_secs(30), async {
            while reports(&pool).await? > 0 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            anyhow::Ok(())
        })
        .await;
        cleanup.abort();
        deleted?
    }
    .await;
    pool.close().await;
    server.shutdown().await?;
    result
}
