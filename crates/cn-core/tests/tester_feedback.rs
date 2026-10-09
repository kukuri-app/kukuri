//! #802 / ADR 0039 テスターフィードバック storage contract の Postgres integration テスト。

use anyhow::Result;
use chrono::{Duration, Utc};
use kukuri_cn_core::{
    NewTesterFeedback, RetentionPolicy, TestDatabase, cleanup_expired, connect_postgres,
    get_tester_feedback, initialize_database, insert_tester_feedback, list_tester_feedback,
};

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn feedback(prefix: &str) -> NewTesterFeedback {
    NewTesterFeedback {
        what_attempted: format!("{prefix}: 投稿を作成しようとした"),
        what_happened: format!("{prefix}: 送信ボタンを押しても反応がなかった"),
        what_seemed_wrong: format!("{prefix}: エラーも成功も表示されないのが変だと思った"),
        client_version: "0.1.7".to_string(),
        os: "linux".to_string(),
    }
}

#[tokio::test]
async fn tester_feedback_inserts_lists_newest_first_and_gets_by_id() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping tester feedback test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_tester_feedback").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    let older = insert_tester_feedback(&pool, &feedback("older")).await?;
    let newer = insert_tester_feedback(&pool, &feedback("newer")).await?;
    assert_ne!(older.id, newer.id);
    assert_eq!(older.client_version, "0.1.7");
    assert_eq!(older.os, "linux");

    let listed = list_tester_feedback(&pool, 50, 0).await?;
    assert_eq!(
        listed.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        vec![newer.id.as_str(), older.id.as_str()],
        "一覧は新着順"
    );
    assert_eq!(listed[1].what_attempted, "older: 投稿を作成しようとした");
    assert_eq!(
        listed[1].what_happened,
        "older: 送信ボタンを押しても反応がなかった"
    );
    assert_eq!(
        listed[1].what_seemed_wrong,
        "older: エラーも成功も表示されないのが変だと思った"
    );

    let fetched = get_tester_feedback(&pool, &older.id).await?;
    assert_eq!(fetched.as_ref(), Some(&older));
    assert_eq!(get_tester_feedback(&pool, "missing-id").await?, None);

    let paged = list_tester_feedback(&pool, 1, 1).await?;
    assert_eq!(paged.len(), 1);
    assert_eq!(paged[0].id, older.id);
    Ok(())
}

#[tokio::test]
async fn tester_feedback_expires_via_retention_policy_and_cleanup() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping tester feedback retention test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_tester_feedback_ret").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    let retention = RetentionPolicy::default();
    let now = Utc::now();
    let stored = insert_tester_feedback(&pool, &feedback("expiring")).await?;

    // 期限内の sweep では消えない。
    let counts = cleanup_expired(&pool, now).await?;
    assert_eq!(counts.tester_feedback, 0);
    assert!(get_tester_feedback(&pool, &stored.id).await?.is_some());

    // 保持期限(tester_feedback_days)を過ぎると list / get から消え、cleanup が削除する。
    let after_expiry = now + Duration::days(i64::from(retention.tester_feedback_days) + 1);
    let counts = cleanup_expired(&pool, after_expiry).await?;
    assert_eq!(counts.tester_feedback, 1);
    assert!(get_tester_feedback(&pool, &stored.id).await?.is_none());
    assert!(list_tester_feedback(&pool, 50, 0).await?.is_empty());
    Ok(())
}
