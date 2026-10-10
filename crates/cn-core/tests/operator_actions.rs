//! #382 Community Node admin write contract の Postgres integration テスト。

use anyhow::Result;
use kukuri_cn_core::{
    AdminOperation, AdmissionMode, LegalDataCipher, NewCommunityNodeReport, OperatorReportStatus,
    TestDatabase, apply_operator_action, connect_postgres, get_community_node_report,
    initialize_database, insert_community_node_report, is_topic_supported, list_operator_actions,
    load_admission_config,
};

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

#[tokio::test]
async fn admin_operations_commit_state_and_append_only_audit_together() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping operator action test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_operator_actions").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    let cipher = LegalDataCipher::from_key_material("test-legal-data-key-0123456789abcdef")?;
    let report = insert_community_node_report(
        &pool,
        &NewCommunityNodeReport {
            subject_kind: "post".to_string(),
            subject_id: "post-1".to_string(),
            capability: "moderation".to_string(),
            reason: "spam".to_string(),
            details: Some("must not enter audit".to_string()),
            reporter_contact: Some("private@example.com".to_string()),
            appeal_risk_signal_id: None,
        },
        Some(&cipher),
    )
    .await?;

    apply_operator_action(
        &pool,
        "ops@kukuri.app",
        &AdminOperation::SetAdmissionMode {
            mode: AdmissionMode::Invite,
        },
    )
    .await?;
    apply_operator_action(
        &pool,
        "ops@kukuri.app",
        &AdminOperation::AddSupportedPublicTopic {
            topic_id: "admin-smoke".to_string(),
        },
    )
    .await?;
    apply_operator_action(
        &pool,
        "ops@kukuri.app",
        &AdminOperation::SetReportStatus {
            report_id: report.id.clone(),
            status: OperatorReportStatus::Reviewing,
        },
    )
    .await?;

    assert_eq!(
        load_admission_config(&pool).await?.mode,
        AdmissionMode::Invite
    );
    assert!(
        is_topic_supported(
            &pool,
            kukuri_cn_core::IndexScopeKind::PublicTopic,
            "admin-smoke",
        )
        .await?
    );
    assert_eq!(
        get_community_node_report(&pool, report.id.as_str())
            .await?
            .expect("report")
            .status,
        "reviewing"
    );

    let audit = list_operator_actions(&pool, 10, 0).await?;
    assert_eq!(audit.len(), 3);
    assert!(audit.iter().all(|entry| entry.actor == "ops@kukuri.app"));
    let topic_audit = audit
        .iter()
        .find(|entry| entry.action == "supported_topic.add")
        .expect("topic audit");
    assert_eq!(topic_audit.before, serde_json::json!({ "present": false }));
    assert_eq!(topic_audit.after, serde_json::json!({ "present": true }));
    let serialized = serde_json::to_string(&audit)?;
    assert!(!serialized.contains("must not enter audit"));
    assert!(!serialized.contains("private@example.com"));

    let update_error =
        sqlx::query("UPDATE cn_admin.operator_actions SET actor = 'tampered' WHERE id = $1")
            .bind(&audit[0].id)
            .execute(&pool)
            .await
            .expect_err("append-only audit must reject updates");
    assert!(update_error.to_string().contains("append-only"));

    let delete_error = sqlx::query("DELETE FROM cn_admin.operator_actions WHERE id = $1")
        .bind(&audit[0].id)
        .execute(&pool)
        .await
        .expect_err("append-only audit must reject deletes");
    assert!(delete_error.to_string().contains("append-only"));

    database.cleanup().await
}

#[tokio::test]
async fn admin_operation_rejects_an_empty_deployment_actor() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping operator action test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_operator_actor").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    let error = apply_operator_action(
        &pool,
        "  ",
        &AdminOperation::SetAdmissionMode {
            mode: AdmissionMode::Invite,
        },
    )
    .await
    .expect_err("empty actor must fail closed");
    assert!(error.to_string().contains("actor"));
    assert!(list_operator_actions(&pool, 10, 0).await?.is_empty());

    database.cleanup().await
}
