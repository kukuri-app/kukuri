use anyhow::Result;
use kukuri_cn_core::{
    TestDatabase, accept_consents, connect_postgres, get_policy_revision,
    get_policy_snapshot_revision, initialize_database, list_policies, list_policy_revisions,
    sync_policies,
};
use kukuri_cn_protocol::CommunityNodePolicyDocument;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn policy(version: i32, body: &str) -> CommunityNodePolicyDocument {
    CommunityNodePolicyDocument {
        policy_slug: "terms_of_service".to_string(),
        policy_version: version,
        title: "KingYoSun Node 利用規約".to_string(),
        body_markdown: body.to_string(),
        required: true,
        policy_kind: None,
        effective_date: Some("2026-09-02".to_string()),
        language: Some("ja".to_string()),
        policy_snapshot_revision: Some(format!("snapshot-{version}")),
        authoritative_language: Some("ja".to_string()),
        reference_translation: false,
        translation_revision: None,
        translation_of_version: None,
        fallback: false,
        requested_language: None,
        material_change: false,
        requires_reconsent: false,
        is_current: true,
        publication_status: Some("current".to_string()),
        published_at: None,
        retired_at: None,
        previous_policy_version: None,
        previous_policy_snapshot_revision: None,
        next_policy_version: None,
        next_policy_snapshot_revision: None,
    }
}

#[tokio::test]
async fn operator_policy_sync_appends_snapshot_history_and_preserves_exact_consent() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_database_url.as_str(), "cn_policy_sync").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    let pubkey = "a".repeat(64);
    sqlx::query("INSERT INTO cn_user.subscriber_accounts (subscriber_pubkey) VALUES ($1)")
        .bind(&pubkey)
        .execute(&pool)
        .await?;
    let legacy_snapshot = sqlx::query_scalar::<_, String>(
        "SELECT policy_snapshot_revision FROM cn_admin.policies
         WHERE policy_slug = 'terms_of_service' AND is_current = TRUE",
    )
    .fetch_one(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO cn_user.policy_consents
            (subscriber_pubkey, policy_slug, policy_version, policy_snapshot_revision)
         VALUES ($1, 'terms_of_service', 1, $2)",
    )
    .bind(&pubkey)
    .bind(legacy_snapshot)
    .execute(&pool)
    .await?;

    let v1 = policy(
        1,
        "# KingYoSun Node 利用規約\n\n現行の運用実態に基づく本文です。",
    );
    sync_policies(&pool, std::slice::from_ref(&v1)).await?;
    let stored = list_policies(&pool).await?;
    let terms = stored
        .iter()
        .find(|document| document.policy_slug == "terms_of_service")
        .expect("terms policy exists");
    assert_eq!(terms.body_markdown, v1.body_markdown);
    assert_eq!(terms.effective_date.as_deref(), Some("2026-09-02"));
    assert_eq!(terms.language.as_deref(), Some("ja"));
    let legacy_consent_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM cn_user.policy_consents
         WHERE subscriber_pubkey = $1",
    )
    .bind(&pubkey)
    .fetch_one(&pool)
    .await?;
    assert_eq!(legacy_consent_count, 1, "過去同意は監査履歴として保持する");

    let mut snapshot_only_change = v1.clone();
    snapshot_only_change.policy_snapshot_revision = Some("snapshot-catalog-2".to_string());
    sync_policies(&pool, &[snapshot_only_change]).await?;
    let snapshot_revisions = list_policy_revisions(&pool, "terms_of_service").await?;
    assert_eq!(
        snapshot_revisions.len(),
        3,
        "legacy正文を含め同一表示版でも snapshot 履歴を残す"
    );
    assert!(snapshot_revisions[0].material_change);
    assert!(snapshot_revisions[0].requires_reconsent);
    assert_eq!(
        snapshot_revisions[0]
            .previous_policy_snapshot_revision
            .as_deref(),
        Some("snapshot-1")
    );
    assert_eq!(
        snapshot_revisions[0].publication_status.as_deref(),
        Some("current")
    );
    assert_eq!(
        snapshot_revisions[1].publication_status.as_deref(),
        Some("retired")
    );
    assert!(snapshot_revisions[0].published_at.is_some());
    assert!(snapshot_revisions[1].retired_at.is_some());

    let historical = get_policy_snapshot_revision(&pool, "terms_of_service", "snapshot-1", None)
        .await?
        .expect("old snapshot remains retrievable");
    assert_eq!(historical.body_markdown, v1.body_markdown);

    let mut changed_without_bump = policy(1, "本文だけを変更");
    changed_without_bump.policy_snapshot_revision = Some("snapshot-content-3".to_string());
    sync_policies(&pool, &[changed_without_bump]).await?;
    assert_eq!(
        list_policy_revisions(&pool, "terms_of_service")
            .await?
            .len(),
        4,
        "同一表示版の法的変更も append-only revision にする"
    );

    let v2 = policy(2, "# KingYoSun Node 利用規約 v2");
    sync_policies(&pool, std::slice::from_ref(&v2)).await?;
    let revisions = list_policy_revisions(&pool, "terms_of_service").await?;
    assert_eq!(revisions.len(), 5, "公開済み正文は削除しない");
    assert_eq!(revisions[0].policy_version, 2);
    assert_eq!(revisions[1].policy_version, 1);
    assert!(revisions[0].material_change);
    assert!(revisions[0].requires_reconsent);

    let mut translation = v2.clone();
    translation.title = "Terms of Service".to_string();
    translation.body_markdown = "English reference translation.".to_string();
    translation.language = Some("en".to_string());
    translation.reference_translation = true;
    translation.translation_revision = Some(1);
    translation.translation_of_version = Some(2);
    sync_policies(&pool, &[v2.clone(), translation]).await?;
    let localized = get_policy_revision(&pool, "terms_of_service", 2, Some("en"))
        .await?
        .expect("localized revision");
    assert!(localized.reference_translation);
    assert!(!localized.fallback);
    assert_eq!(localized.translation_of_version, Some(2));
    let fallback = get_policy_revision(&pool, "terms_of_service", 2, Some("fr"))
        .await?
        .expect("authoritative fallback");
    assert!(!fallback.reference_translation);
    assert!(fallback.fallback);
    assert_eq!(fallback.language.as_deref(), Some("ja"));

    let stale_accept = accept_consents(&pool, &pubkey, &[], Some("snapshot-1"))
        .await
        .expect_err("stale snapshot must not be accepted");
    assert!(stale_accept.to_string().contains("policy snapshot changed"));
    let rollback_error = sync_policies(&pool, &[v1])
        .await
        .expect_err("version rollback must fail");
    assert!(rollback_error.to_string().contains("version rollback"));

    pool.close().await;
    database.cleanup().await
}
