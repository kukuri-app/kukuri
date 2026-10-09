//! #1699 AC-2: 観測の revision と、観測提供の取消時刻の表を廃止する migration の Postgres
//! integration テスト。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。直前の版まで適用した DB に
//! 取消の行と、取消の前・後の同意の行を入れてから残りを適用し、取消が無効にしていた同意の行だけが
//! 消え、2 表と sequence が無くなることを確かめる。

use anyhow::Result;
use kukuri_cn_core::{TestDatabase, connect_postgres, migrate_postgres, migrate_postgres_up_to};
use kukuri_cn_protocol::TRUST_OBSERVATION_SHARING_POLICY_SLUG;
use sqlx::PgPool;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
/// 廃止の migration の直前の版（`202610090004_rights_request_events_independent_retention.sql`）。
const PREVIOUS_MIGRATION_VERSION: i64 = 202610090004;
const SNAPSHOT: &str = "snapshot-sharing-1";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

async fn seed_consent(
    pool: &PgPool,
    subscriber: &str,
    policy_slug: &str,
    accepted_at: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_user.subscriber_accounts (subscriber_pubkey) VALUES ($1)
         ON CONFLICT DO NOTHING",
    )
    .bind(subscriber)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO cn_user.policy_consents
            (subscriber_pubkey, policy_slug, policy_version, policy_snapshot_revision, accepted_at)
         VALUES ($1, $2, 1, $3, $4::timestamptz)",
    )
    .bind(subscriber)
    .bind(policy_slug)
    .bind(SNAPSHOT)
    .bind(accepted_at)
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_revocation(pool: &PgPool, observer: &str, revoked_at: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_trust.observation_sharing_revocations (observer_pubkey, revoked_at)
         VALUES ($1, $2::timestamptz)",
    )
    .bind(observer)
    .bind(revoked_at)
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
async fn migration_drops_revocations_and_the_consents_they_voided() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core trust observation migration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database =
        TestDatabase::create(admin_url.as_str(), "cn_core_trust_observation_tables").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        migrate_postgres_up_to(&pool, PREVIOUS_MIGRATION_VERSION).await?;
        for (policy_slug, required) in [
            (TRUST_OBSERVATION_SHARING_POLICY_SLUG, false),
            ("terms_of_service", true),
        ] {
            sqlx::query(
                "INSERT INTO cn_admin.policies
                    (policy_slug, policy_version, title, body_markdown, required, effective_date,
                     language, is_current, policy_snapshot_revision, material_change,
                     requires_reconsent)
                 VALUES ($1, 1, '文書', '本文', $2, '2026-09-18', 'ja', TRUE, $3, FALSE, FALSE)",
            )
            .bind(policy_slug)
            .bind(required)
            .bind(SNAPSHOT)
            .execute(&pool)
            .await?;
        }
        // a は同意の後に取り消した（利用規約の同意もある）。b は取り消した後に同意し直した。
        // c は取り消していない。
        let revoked = "a".repeat(64);
        let reconsented = "b".repeat(64);
        let never_revoked = "c".repeat(64);
        let sharing = TRUST_OBSERVATION_SHARING_POLICY_SLUG;
        seed_consent(&pool, &revoked, sharing, "2026-09-20T00:00:00Z").await?;
        seed_consent(&pool, &revoked, "terms_of_service", "2026-09-20T00:00:00Z").await?;
        seed_revocation(&pool, &revoked, "2026-09-21T00:00:00Z").await?;
        seed_consent(&pool, &reconsented, sharing, "2026-09-22T00:00:00Z").await?;
        seed_revocation(&pool, &reconsented, "2026-09-21T00:00:00Z").await?;
        seed_consent(&pool, &never_revoked, sharing, "2026-09-20T00:00:00Z").await?;
        sqlx::query(
            "INSERT INTO cn_trust.observation_target_revisions (target_pubkey, revision)
             VALUES ($1, nextval('cn_trust.observation_revision_seq'))",
        )
        .bind("d".repeat(64))
        .execute(&pool)
        .await?;

        migrate_postgres(&pool).await?;

        let consents: Vec<String> = sqlx::query_scalar(
            "SELECT subscriber_pubkey FROM cn_user.policy_consents
             WHERE policy_slug = $1
             ORDER BY subscriber_pubkey",
        )
        .bind(TRUST_OBSERVATION_SHARING_POLICY_SLUG)
        .fetch_all(&pool)
        .await?;
        assert_eq!(consents, vec![reconsented, never_revoked]);
        // 取り消した利用者の、ほかの文書への同意は消さない。
        let other_documents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM cn_user.policy_consents
             WHERE subscriber_pubkey = $1 AND policy_slug = 'terms_of_service'",
        )
        .bind(&revoked)
        .fetch_one(&pool)
        .await?;
        assert_eq!(other_documents, 1);
        let remaining: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT to_regclass(name)::text FROM unnest(ARRAY[
                'cn_trust.observation_sharing_revocations',
                'cn_trust.observation_target_revisions',
                'cn_trust.observation_revision_seq'
             ]) AS name",
        )
        .fetch_all(&pool)
        .await?;
        assert_eq!(remaining, vec![None, None, None]);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}
