//! #1706 AC-2: legal hold を解除で消す migration
//! （`202610090008_legal_hold_release_deletes_row.sql`）の Postgres integration テスト。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。直前の schema まで
//! 適用した DB に有効な hold と解除済みの hold を置き、残りの migration の後に有効な行だけが同じ値で
//! 残ることを確かめる。

use anyhow::Result;
use kukuri_cn_core::{TestDatabase, connect_postgres, migrate_postgres, migrate_postgres_up_to};

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const RELEASE_DELETION_VERSION: i64 = 202610090008;

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

#[tokio::test]
async fn migration_deletes_released_holds_and_keeps_active_ones() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping legal hold migration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_legal_hold_migration").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        migrate_postgres_up_to(&pool, RELEASE_DELETION_VERSION - 1).await?;
        // rr-1 は解除の後に hold を開始し直した形（同じ対象に解除済みと有効な行がある）。
        sqlx::query(
            "INSERT INTO cn_legal.legal_holds
                (id, target_kind, target_id, data_categories, basis, release_condition,
                 started_by, started_at, released_by, released_at)
             VALUES
                ('hold-released', 'rights_request', 'rr-1', ARRAY['rights_request'],
                 '命令 A', '手続の終了', 'legal@node.example', '2026-01-01T00:00:00Z',
                 'legal@node.example', '2026-02-01T00:00:00Z'),
                ('hold-active', 'rights_request', 'rr-1',
                 ARRAY['rights_request', 'rights_request_history'], '命令 B', '手続の終了',
                 'legal@node.example', '2026-03-01T00:00:00Z', NULL, NULL),
                ('hold-report', 'report', 'report-1', ARRAY['report'], '照会 C', '回答の完了',
                 'legal@node.example', '2026-03-02T00:00:00Z', NULL, NULL)",
        )
        .execute(&pool)
        .await?;
        let active: Vec<String> = sqlx::query_scalar(
            "SELECT (to_jsonb(hold) - 'released_by' - 'released_at')::text
             FROM cn_legal.legal_holds hold WHERE released_at IS NULL ORDER BY id",
        )
        .fetch_all(&pool)
        .await?;

        migrate_postgres(&pool).await?;

        let remaining: Vec<String> = sqlx::query_scalar(
            "SELECT (to_jsonb(hold) - 'released_by' - 'released_at')::text
             FROM cn_legal.legal_holds hold ORDER BY id",
        )
        .fetch_all(&pool)
        .await?;
        assert_eq!(remaining, active, "only active holds remain, unchanged");
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}
