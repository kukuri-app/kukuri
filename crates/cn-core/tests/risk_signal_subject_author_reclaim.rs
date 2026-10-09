//! #1699 AC-1: risk signal の著者の対応（`cn_safety.risk_signal_subject_authors`）の回収の
//! Postgres integration テスト。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。
//! - 内容の最後の risk signal の行が期限で消えると、その内容の対応も同じ取引で消える。
//!   ほかの risk signal の行が残る内容の対応は残る。
//! - 保存済み判定の再利用は、risk signal の行が無い内容に対応を作らない。
//! - 期限削除と、同じ内容への関連付け・risk signal の保存が重なっても、参照先のある対応を
//!   消さず、参照先の無い対応を残さない。
//! - migration が、参照先の無い既存の対応を 1 回消す。

use std::time::Duration;

use anyhow::{Result, bail};
use chrono::Utc;
use kukuri_cn_core::{
    TestDatabase, attribute_risk_signal_subject_author, cleanup_expired, connect_postgres,
    initialize_database, list_trust_risk_inputs, migrate_postgres, migrate_postgres_up_to,
    persist_risk_signal_deduplicated,
};
use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal, Severity, Visibility,
};
use sqlx::PgPool;
use tokio::task::JoinHandle;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const ISSUER: &str = "issuer-node";
/// 回収の migration の直前の版（`202609260003_drop_legacy_scope_cursor.sql`）。
const PREVIOUS_MIGRATION_VERSION: i64 = 202609260003;

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn post_signal(target_id: &str, category: SafetyCategory) -> SafetyRiskSignal {
    SafetyRiskSignal {
        target: RiskSignalTarget::PostId,
        target_id: target_id.to_string(),
        category,
        severity: Severity::Medium,
        basis: Basis::ClassifierScore,
        confidence: Some(80),
        visibility: Visibility::Local,
        expires_at: None,
        appeal_status: Some(AppealStatus::None),
    }
}

/// 投稿への risk signal を著者つきで保存し、行の id を返す。
async fn persist(
    pool: &PgPool,
    target_id: &str,
    category: SafetyCategory,
    author: &str,
) -> Result<String> {
    let persisted = persist_risk_signal_deduplicated(
        pool,
        ISSUER,
        &post_signal(target_id, category),
        Some(author),
    )
    .await?;
    Ok(persisted.stored.id)
}

async fn authors_of(pool: &PgPool, target_id: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT author_pubkey FROM cn_safety.risk_signal_subject_authors
         WHERE target = 'post_id' AND target_id = $1
         ORDER BY author_pubkey",
    )
    .bind(target_id)
    .fetch_all(pool)
    .await?)
}

/// risk signal の行の保持期限を過去にして、期限削除の対象にする。
async fn expire(pool: &PgPool, signal_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE cn_safety.risk_signals
         SET retention_expires_at = NOW() - INTERVAL '1 day'
         WHERE id = $1",
    )
    .bind(signal_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn relative_signal_ids(pool: &PgPool, author: &str) -> Result<Vec<String>> {
    let inputs = list_trust_risk_inputs(
        pool,
        RiskSignalTarget::UserPubkey,
        author,
        Utc::now().to_rfc3339().as_str(),
    )
    .await?;
    Ok(inputs
        .relative
        .into_iter()
        .map(|input| input.signal_id)
        .collect())
}

/// 別の接続の取引が行の lock を待つまで、または `task` が終わるまで待つ。lock を待ったら true。
async fn blocked_on_lock<T>(pool: &PgPool, task: &JoinHandle<T>) -> Result<bool> {
    for _ in 0..500 {
        if task.is_finished() {
            return Ok(false);
        }
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM pg_stat_activity
                WHERE datname = current_database() AND wait_event_type = 'Lock'
             )",
        )
        .fetch_one(pool)
        .await?;
        if waiting {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("the concurrent transaction neither finished nor waited on a lock")
}

#[tokio::test]
async fn subject_authors_are_removed_with_the_last_risk_signal_of_the_content() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core subject author reclaim test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database =
        TestDatabase::create(admin_url.as_str(), "cn_core_subject_author_reclaim").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        // post-single の risk signal は 1 行だけ。post-double は category の違う 2 行。
        let single = persist(&pool, "post-single", SafetyCategory::Spam, "author-a").await?;
        let double_old = persist(&pool, "post-double", SafetyCategory::Spam, "author-a").await?;
        let double_new =
            persist(&pool, "post-double", SafetyCategory::Phishing, "author-b").await?;
        expire(&pool, &single).await?;
        expire(&pool, &double_old).await?;

        cleanup_expired(&pool, Utc::now()).await?;

        assert!(
            authors_of(&pool, "post-single").await?.is_empty(),
            "the last risk signal of the content takes its subject authors with it"
        );
        assert_eq!(
            authors_of(&pool, "post-double").await?,
            vec!["author-a", "author-b"],
            "a content that still has a risk signal keeps its subject authors"
        );
        for author in ["author-a", "author-b"] {
            assert_eq!(
                relative_signal_ids(&pool, author).await?,
                vec![double_new.clone()],
                "{author}"
            );
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn reused_verdict_does_not_attribute_a_content_without_risk_signal() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core subject author reclaim test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database =
        TestDatabase::create(admin_url.as_str(), "cn_core_subject_author_without_signal").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        attribute_risk_signal_subject_author(
            &pool,
            RiskSignalTarget::PostId,
            "post-without-signal",
            "author-c",
        )
        .await?;
        assert!(authors_of(&pool, "post-without-signal").await?.is_empty());

        // risk signal の行がある内容には、今までどおり 2 人目の著者を関連付ける。
        persist(&pool, "post-with-signal", SafetyCategory::Spam, "author-a").await?;
        attribute_risk_signal_subject_author(
            &pool,
            RiskSignalTarget::PostId,
            "post-with-signal",
            "author-c",
        )
        .await?;
        assert_eq!(
            authors_of(&pool, "post-with-signal").await?,
            vec!["author-a", "author-c"]
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn attribution_waits_for_the_deletion_of_the_last_risk_signal() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core subject author reclaim test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database =
        TestDatabase::create(admin_url.as_str(), "cn_core_subject_author_racing_reuse").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let signal = persist(&pool, "post-racing", SafetyCategory::Spam, "author-a").await?;

        // 期限削除の取引を、最後の risk signal の行を消したところで止めておく。
        let mut deletion = pool.begin().await?;
        sqlx::query("DELETE FROM cn_safety.risk_signals WHERE id = $1")
            .bind(&signal)
            .execute(&mut *deletion)
            .await?;
        let attribution = tokio::spawn({
            let pool = pool.clone();
            async move {
                attribute_risk_signal_subject_author(
                    &pool,
                    RiskSignalTarget::PostId,
                    "post-racing",
                    "author-c",
                )
                .await
            }
        });
        assert!(
            blocked_on_lock(&pool, &attribution).await?,
            "the attribution waits until the deletion settles"
        );
        deletion.commit().await?;
        attribution.await??;

        assert!(
            authors_of(&pool, "post-racing").await?.is_empty(),
            "no subject author is left without a risk signal"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn new_risk_signal_keeps_its_author_while_the_old_one_is_deleted() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core subject author reclaim test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database =
        TestDatabase::create(admin_url.as_str(), "cn_core_subject_author_racing_signal").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let old = persist(&pool, "post-racing", SafetyCategory::Spam, "author-a").await?;

        let mut deletion = pool.begin().await?;
        sqlx::query("DELETE FROM cn_safety.risk_signals WHERE id = $1")
            .bind(&old)
            .execute(&mut *deletion)
            .await?;
        let persisting = tokio::spawn({
            let pool = pool.clone();
            async move { persist(&pool, "post-racing", SafetyCategory::Phishing, "author-a").await }
        });
        assert!(
            blocked_on_lock(&pool, &persisting).await?,
            "the new risk signal waits until the deletion settles"
        );
        deletion.commit().await?;
        let new = persisting.await??;

        assert_eq!(authors_of(&pool, "post-racing").await?, vec!["author-a"]);
        assert_eq!(relative_signal_ids(&pool, "author-a").await?, vec![new]);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn migration_removes_subject_authors_without_risk_signal() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core subject author reclaim test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database =
        TestDatabase::create(admin_url.as_str(), "cn_core_subject_author_migration").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        migrate_postgres_up_to(&pool, PREVIOUS_MIGRATION_VERSION).await?;
        sqlx::query(
            "INSERT INTO cn_safety.risk_signals
                (id, issuer_node_id, target, target_id, category, severity, basis, visibility)
             VALUES ('kept', $1, 'post_id', 'post-kept', 'spam', 'medium',
                     'classifier_score', 'local')",
        )
        .bind(ISSUER)
        .execute(&pool)
        .await?;
        // blob_cid の post-kept は、同じ id でも target が違うので参照先が無い。
        sqlx::query(
            "INSERT INTO cn_safety.risk_signal_subject_authors (target, target_id, author_pubkey)
             VALUES ('post_id', 'post-kept', 'author-a'),
                    ('post_id', 'post-orphan', 'author-b'),
                    ('blob_cid', 'post-kept', 'author-c')",
        )
        .execute(&pool)
        .await?;

        migrate_postgres(&pool).await?;

        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT target, target_id, author_pubkey
             FROM cn_safety.risk_signal_subject_authors
             ORDER BY target, target_id, author_pubkey",
        )
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            rows,
            vec![(
                "post_id".to_string(),
                "post-kept".to_string(),
                "author-a".to_string()
            )]
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}
