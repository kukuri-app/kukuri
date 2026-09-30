use anyhow::{Context, Result, bail};
use sqlx::Executor;
use sqlx::postgres::{PgPool, PgPoolOptions};
use url::Url;
use uuid::Uuid;

use crate::DatabaseInitMode;
use crate::admission::{AdmissionRejection, ensure_default_admission};
use crate::config::{COMMUNITY_NODE_AUTH_SERVICE_NAME, DATABASE_PREPARE_HINT};
use crate::rollout::ensure_default_auth_rollout;

pub async fn connect_postgres(database_url: &str) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .with_context(|| "failed to connect to Postgres")
}

pub async fn migrate_postgres(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// 指定 version 以下の migration だけを適用する（migration test 用）。
///
/// データ変換を伴う migration（例: #1050 の risk signal 圧縮）を、その直前の schema に seed した
/// 状態から検証するために使う。残りは後続の `migrate_postgres` が適用する。
pub async fn migrate_postgres_up_to(pool: &PgPool, max_version: i64) -> Result<()> {
    let mut migrator = sqlx::migrate!("./migrations");
    let migrations = migrator
        .migrations
        .iter()
        .filter(|migration| migration.version <= max_version)
        .cloned()
        .collect::<Vec<_>>();
    migrator.migrations = std::borrow::Cow::Owned(migrations);
    migrator.run(pool).await?;
    Ok(())
}

pub async fn initialize_database(pool: &PgPool) -> Result<()> {
    migrate_postgres(pool).await?;
    seed_default_policies(pool).await?;
    ensure_default_auth_rollout(pool).await?;
    ensure_default_admission(pool).await?;
    Ok(())
}

pub async fn initialize_database_for_runtime(
    pool: &PgPool,
    init_mode: DatabaseInitMode,
) -> Result<()> {
    match init_mode {
        DatabaseInitMode::RequireReady => ensure_database_ready(pool).await,
        DatabaseInitMode::Prepare => initialize_database(pool).await,
    }
}

pub async fn ensure_database_ready(pool: &PgPool) -> Result<()> {
    for (schema, table) in [
        ("cn_auth", "auth_challenges"),
        ("cn_user", "subscriber_accounts"),
        ("cn_user", "policy_consents"),
        ("cn_admin", "policies"),
        ("cn_admin", "service_configs"),
        ("cn_admin", "invite_codes"),
        ("cn_admin", "admission_allowlist"),
        ("cn_admin", "operator_actions"),
        ("cn_admin", "tester_feedback"),
        ("cn_legal", "rights_requests"),
        ("cn_legal", "rights_request_events"),
        ("cn_legal", "sensitive_items"),
        ("cn_legal", "legal_holds"),
        ("cn_legal", "transmission_preventions"),
        ("cn_bootstrap", "bootstrap_nodes"),
        ("cn_bootstrap", "peer_registrations"),
        ("cn_safety", "signed_moderation_events"),
        ("cn_safety", "risk_signals"),
        ("cn_safety", "scan_verdicts"),
        ("cn_index", "supported_topics"),
        ("cn_index", "indexing_requests"),
        ("cn_index", "channel_secrets"),
        ("cn_index", "index_entries"),
        ("cn_trust", "relation_optouts"),
        ("cn_metaverse", "dome_hosting_assignments"),
    ] {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1
                FROM information_schema.tables
                WHERE table_schema = $1
                  AND table_name = $2
            )",
        )
        .bind(schema)
        .bind(table)
        .fetch_one(pool)
        .await?;
        if !exists {
            bail!(
                "community-node database is not ready: missing `{schema}.{table}`; {DATABASE_PREPARE_HINT}"
            );
        }
    }

    let policy_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM cn_admin.policies")
        .fetch_one(pool)
        .await?;
    if policy_count == 0 {
        bail!(
            "community-node database is not ready: required policy seed is missing; {DATABASE_PREPARE_HINT}"
        );
    }

    let rollout_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM cn_admin.service_configs
            WHERE service_name = $1
        )",
    )
    .bind(COMMUNITY_NODE_AUTH_SERVICE_NAME)
    .fetch_one(pool)
    .await?;
    if !rollout_exists {
        bail!(
            "community-node database is not ready: auth rollout seed is missing; {DATABASE_PREPARE_HINT}"
        );
    }

    Ok(())
}

pub async fn seed_default_policies(pool: &PgPool) -> Result<()> {
    for (slug, title, body) in [
        (
            "terms_of_service",
            "Terms of Service",
            "You must follow the community node terms of service.",
        ),
        (
            "privacy_policy",
            "Privacy Policy",
            "You must acknowledge the community node privacy policy.",
        ),
    ] {
        sqlx::query(
            "INSERT INTO cn_admin.policies (
                policy_slug, policy_version, title, body_markdown, required,
                policy_snapshot_revision
             )
             SELECT $1, 1, $2, $3, TRUE, 'legacy:' || md5('default-policy-catalog-v1')
             WHERE NOT EXISTS (
                 SELECT 1 FROM cn_admin.policies WHERE policy_slug = $1
             )
             ON CONFLICT (policy_slug, policy_version, policy_snapshot_revision) DO NOTHING",
        )
        .bind(slug)
        .bind(title)
        .bind(body)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub(crate) async fn ensure_active_subscriber<'e, E>(executor: E, pubkey: &str) -> Result<()>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    // admission を通過して token 発行に至った subscriber は member として記録する（#383）。
    // `admitted = TRUE` により、後の mode 変更後も再認証を bypass できる現メンバーと、
    // 未参加のまま ban/unban されただけの pubkey を区別する。
    //
    // banned 行は active に戻さない。auth/verify と ban_subscriber が競合し admission チェック後に
    // ban が割り込んでも、WHERE 条件で banned 行を除外して ban を権威的に維持する（既存トークンも
    // require_bearer_identity で失効）。
    let status = sqlx::query_scalar::<_, String>(
        "INSERT INTO cn_user.subscriber_accounts
            (subscriber_pubkey, status, admitted, last_authenticated_at)
         VALUES ($1, 'active', TRUE, NOW())
         ON CONFLICT (subscriber_pubkey) DO UPDATE
         SET status = 'active',
             admitted = TRUE,
             last_authenticated_at = NOW()
         WHERE cn_user.subscriber_accounts.status <> 'banned'
         RETURNING status",
    )
    .bind(pubkey)
    .fetch_optional(executor)
    .await?;
    match status.as_deref() {
        Some("active") => Ok(()),
        // ON CONFLICT の WHERE が banned 行を除外した場合は RETURNING が空になる。
        // この場合も token を返さず、auth/verify の admission 拒否として扱う。
        None | Some("banned") => Err(AdmissionRejection::Banned.into()),
        Some(_) => Ok(()),
    }
}

#[derive(Clone, Debug)]
pub struct TestDatabase {
    admin_database_url: String,
    pub database_name: String,
    pub database_url: String,
}

impl TestDatabase {
    pub async fn create(admin_database_url: &str, prefix: &str) -> Result<Self> {
        let sanitized_prefix = prefix
            .chars()
            .map(|ch| match ch {
                'a'..='z' | '0'..='9' => ch,
                'A'..='Z' => ch.to_ascii_lowercase(),
                _ => '_',
            })
            .collect::<String>();
        let sanitized_prefix = sanitized_prefix.trim_matches('_');
        let prefix = if sanitized_prefix.is_empty() {
            "cn_test"
        } else {
            sanitized_prefix
        };
        let suffix = Uuid::new_v4().simple().to_string();
        let mut database_name = format!("{prefix}_{suffix}");
        database_name.truncate(63);

        let admin_pool = connect_postgres(admin_database_url)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to admin Postgres while creating test database `{database_name}`"
                )
            })?;
        let create_sql = format!("CREATE DATABASE \"{}\"", database_name.replace('"', "\"\""));
        admin_pool
            .execute(sqlx::AssertSqlSafe(create_sql))
            .await
            .with_context(|| format!("failed to create test database `{database_name}`"))?;

        let mut parsed =
            Url::parse(admin_database_url).context("failed to parse admin database url")?;
        parsed.set_path(format!("/{database_name}").as_str());
        Ok(Self {
            admin_database_url: admin_database_url.to_string(),
            database_name,
            database_url: parsed.to_string(),
        })
    }

    pub async fn cleanup(&self) -> Result<()> {
        let admin_pool = connect_postgres(self.admin_database_url.as_str())
            .await
            .with_context(|| {
                format!(
                    "failed to connect to admin Postgres while cleaning up test database `{}`",
                    self.database_name
                )
            })?;
        sqlx::query(
            "SELECT pg_terminate_backend(pid)
             FROM pg_stat_activity
             WHERE datname = $1
               AND pid <> pg_backend_pid()",
        )
        .bind(&self.database_name)
        .execute(&admin_pool)
        .await
        .with_context(|| {
            format!(
                "failed to terminate active connections for test database `{}`",
                self.database_name
            )
        })?;
        let drop_sql = format!(
            "DROP DATABASE IF EXISTS \"{}\"",
            self.database_name.replace('"', "\"\"")
        );
        admin_pool
            .execute(sqlx::AssertSqlSafe(drop_sql))
            .await
            .with_context(|| format!("failed to drop test database `{}`", self.database_name))?;
        Ok(())
    }
}
