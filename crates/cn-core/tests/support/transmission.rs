//! 送信防止・malware の制限の結合試験が共有する補助（#761、#1062）。

use anyhow::Result;
use kukuri_cn_safety::{ReasonCode, SafetyAction, SafetyVerdict};
use sqlx::PgPool;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

pub fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

pub fn allow_verdict() -> SafetyVerdict {
    SafetyVerdict {
        action: SafetyAction::Allow,
        labels: Vec::new(),
        advisory_labels: Vec::new(),
        critical: false,
        reason_code: ReasonCode::NoKnownMatch,
        confidence: None,
        provider: Some("mock-known-csam".to_string()),
        provider_capability: None,
        policy_version: "test".to_string(),
        scanned_at: "2026-08-25T00:00:00Z".to_string(),
    }
}

pub async fn add_other_indexed_posts(pool: &PgPool, from: i64, to: i64) -> Result<()> {
    for start in (from..to).step_by(500) {
        let end = (start + 500).min(to);
        sqlx::query(
            "INSERT INTO cn_safety.scan_verdicts
                (id, subject_kind, subject_id, action, critical, reason_code, policy_version,
                 scanned_at)
             SELECT 'verdict-' || g, 'post', encode(sha256(convert_to('post-' || g, 'UTF8')), 'hex'),
                    'allow', false, 'no_known_match', 'test', '2026-10-09T00:00:00Z'
             FROM generate_series($1::bigint, $2::bigint - 1) AS g",
        )
        .bind(start)
        .bind(end)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO cn_index.index_entries
                (scope_kind, scope_id, object_id, author_pubkey, created_at, source_replica_id,
                 verdict_id, verdict_action, critical)
             SELECT 'public_topic', 'topic-' || (g % 100),
                    encode(sha256(convert_to('post-' || g, 'UTF8')), 'hex'), 'author-' || (g % 1000),
                    g, 'replica', 'verdict-' || g, 'allow', false
             FROM generate_series($1::bigint, $2::bigint - 1) AS g",
        )
        .bind(start)
        .bind(end)
        .execute(pool)
        .await?;
    }
    Ok(())
}
