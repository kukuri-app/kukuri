//! #1705: 旧平文の確認と `cn_admin.reports.reporter_contact` の削除の migration
//! （`202610090006_legacy_plaintext_guard.sql`）の Postgres integration テスト。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。直前の版まで適用した DB に
//! 旧平文を置くと適用が止まって DB が変わらず、旧平文が無ければ列が消えて、連絡先は機微区分からだけ読む。

use anyhow::Result;
use kukuri_cn_core::{
    LegalDataCipher, NewCommunityNodeReport, TestDatabase, connect_postgres,
    get_community_node_report, get_community_node_report_with_contact, initialize_database,
    insert_community_node_report, migrate_postgres, migrate_postgres_up_to,
};
use sqlx::PgPool;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const GUARD_MIGRATION_VERSION: i64 = 202610090006;

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

/// 失敗した migration は sqlx の advisory lock を解かずに戻るので、試行ごとに接続を作って閉じる。
async fn migrate_with_fresh_pool(database_url: &str) -> Result<()> {
    let pool = connect_postgres(database_url).await?;
    let result = migrate_postgres(&pool).await;
    pool.close().await;
    result
}

async fn reporter_contact_column_exists(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'cn_admin' AND table_name = 'reports'
              AND column_name = 'reporter_contact'
         )",
    )
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn legacy_plaintext_stops_migration_and_sealed_database_drops_column() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping legacy plaintext guard migration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_legacy_plaintext_guard").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        migrate_postgres_up_to(&pool, GUARD_MIGRATION_VERSION - 1).await?;

        // 旧平文の通報が残っていれば止まり、行も列も残る。
        sqlx::query(
            "INSERT INTO cn_admin.reports
                (id, subject_kind, subject_id, capability, reason, reporter_contact)
             VALUES ('legacy-report', 'post', 'post-1', 'moderation', 'spam', 'legacy@example.com')",
        )
        .execute(&pool)
        .await?;
        assert!(migrate_with_fresh_pool(&database.database_url).await.is_err());
        let contact: Option<String> = sqlx::query_scalar(
            "SELECT reporter_contact FROM cn_admin.reports WHERE id = 'legacy-report'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(contact.as_deref(), Some("legacy@example.com"));

        // 止まった DB で、#776 以後・#1705 より前の版の sealing と同じ書込み（期限の列を含む）ができる。
        // 通報を sealing 済みの形にしても、旧平文の申出が残っていれば止まる。
        sqlx::query(
            "INSERT INTO cn_legal.sensitive_items
                (id, owner_kind, owner_id, data_category, nonce, ciphertext, expires_at)
             VALUES ('sealed-report', 'report', 'legacy-report', 'report_contact',
                     decode(repeat('00', 24), 'hex'), decode(repeat('ab', 32), 'hex'),
                     NOW() + INTERVAL '90 days')",
        )
        .execute(&pool)
        .await?;
        sqlx::query("UPDATE cn_admin.reports SET reporter_contact = NULL")
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO cn_legal.rights_requests
                (id, tracking_secret_hash, scope_revision, scope_status, status, subject_kind,
                 subject_id, requested_capabilities, request_data)
             VALUES ('legacy-request', repeat('a', 64), 'scope-v1', 'unverified_scope',
                     'needs_information', 'post', 'post-1', ARRAY['moderation'],
                     '{\"email\": \"legacy@example.com\"}'::jsonb)",
        )
        .execute(&pool)
        .await?;
        assert!(migrate_with_fresh_pool(&database.database_url).await.is_err());
        assert!(reporter_contact_column_exists(&pool).await?);
        let email: String = sqlx::query_scalar(
            "SELECT request_data->>'email' FROM cn_legal.rights_requests
             WHERE id = 'legacy-request'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(email, "legacy@example.com");

        // 旧平文が無ければ適用されて列が消え、通報者連絡先は機微区分からだけ読む。
        sqlx::query("UPDATE cn_legal.rights_requests SET request_data = request_data - 'email'")
            .execute(&pool)
            .await?;
        initialize_database(&pool).await?;
        assert!(!reporter_contact_column_exists(&pool).await?);
        let cipher =
            LegalDataCipher::from_key_material("unit-test-legal-data-key-0123456789abcdef")?;
        let now = chrono::Utc::now();
        let stored = insert_community_node_report(
            &pool,
            &NewCommunityNodeReport {
                subject_kind: "post".to_string(),
                subject_id: "post-2".to_string(),
                capability: "moderation".to_string(),
                reason: "spam".to_string(),
                reporter_contact: Some("reporter@example.com".to_string()),
                ..Default::default()
            },
            Some(&cipher),
        )
        .await?;
        let plain = get_community_node_report(&pool, &stored.id)
            .await?
            .expect("stored report");
        assert!(plain.reporter_contact.is_none());
        let with_contact = get_community_node_report_with_contact(&pool, &cipher, &stored.id, now)
            .await?
            .expect("stored report");
        assert_eq!(
            with_contact.reporter_contact.as_deref(),
            Some("reporter@example.com")
        );
        anyhow::Ok(())
    }
    .await;
    pool.close().await;
    database.cleanup().await?;
    result
}
