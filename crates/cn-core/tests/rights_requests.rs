use anyhow::Result;
use kukuri_cn_core::{
    LEGAL_DATA_KEY_CHECK_SQL, LegalDataCipher, RetentionPolicy, TestDatabase,
    TransmissionPreventionCapability, action_rights_request, cleanup_expired, connect_postgres,
    export_legal_hold, get_active_transmission_prevention, get_public_rights_request_status,
    get_rights_request, get_rights_request_with_sensitive, initialize_database,
    insert_rights_request, list_operator_actions, release_legal_hold, start_legal_hold,
    transition_rights_request, verify_legal_data_key,
};
use kukuri_cn_protocol::{
    RightsCategory, RightsRequestCreateRequest, RightsRequestScopeStatus, RightsRequestStatus,
    RightsRequesterKind,
};
use serde_json::Value;
use sqlx::{PgPool, Row};

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn request() -> RightsRequestCreateRequest {
    RightsRequestCreateRequest {
        scope_revision: "scope-v1".to_string(),
        scope_acknowledged: true,
        requester_kind: RightsRequesterKind::RightsHolder,
        requester_name: "権利者".to_string(),
        organization: None,
        address: None,
        email: "rights@example.com".to_string(),
        phone: None,
        represented_rights_holder: None,
        authority_basis: None,
        rights_category: RightsCategory::Copyright,
        rights_basis: "著作権者である".to_string(),
        original_work_description: None,
        original_work_reference: None,
        subject_kind: "post".to_string(),
        subject_id: "post-760".to_string(),
        subject_url: None,
        infringement_description: "無断複製".to_string(),
        no_permission_statement: true,
        evidence_references: Vec::new(),
        requested_capabilities: vec!["moderation".to_string()],
    }
}

#[tokio::test]
async fn accountless_tracking_and_action_are_durable_and_redacted() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping rights-request integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_rights_requests").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let cipher =
            LegalDataCipher::from_key_material("unit-test-legal-data-key-0123456789abcdef")?;
        let created = insert_rights_request(
            &pool,
            &request(),
            RightsRequestScopeStatus::UnverifiedScope,
            &cipher,
            chrono::Utc::now(),
        )
        .await?;
        assert_eq!(created.record.status, RightsRequestStatus::NeedsInformation);
        assert!(
            get_public_rights_request_status(&pool, &created.record.id, "wrong")
                .await?
                .is_none()
        );
        assert!(
            get_public_rights_request_status(&pool, &created.record.id, &created.tracking_secret)
                .await?
                .is_some()
        );
        let hash: String =
            sqlx::query("SELECT tracking_secret_hash FROM cn_legal.rights_requests WHERE id = $1")
                .bind(&created.record.id)
                .fetch_one(&pool)
                .await?
                .try_get("tracking_secret_hash")?;
        assert_ne!(hash, created.tracking_secret);

        let reviewing = transition_rights_request(
            &pool,
            &created.record.id,
            created.record.version,
            "legal@node.example",
            RightsRequestStatus::Reviewing,
            Some("審査を開始しました"),
            "status_surface",
            chrono::Utc::now(),
        )
        .await?;
        let actioned = action_rights_request(
            &pool,
            &reviewing.id,
            reviewing.version,
            "legal@node.example",
            vec![TransmissionPreventionCapability::Moderation],
            "このノードの moderation 対象から除外しました",
            chrono::Utc::now(),
        )
        .await?;
        assert_eq!(actioned.request.status, RightsRequestStatus::Actioned);
        assert_eq!(
            actioned.prevention.decision.related_report_id.as_deref(),
            Some(created.record.id.as_str())
        );
        assert!(
            get_active_transmission_prevention(&pool, "post", "post-760")
                .await?
                .is_some()
        );
        assert_eq!(
            get_rights_request(&pool, &created.record.id)
                .await?
                .unwrap()
                .status,
            RightsRequestStatus::Actioned
        );
        let audit = list_operator_actions(&pool, 20, 0).await?;
        assert!(
            audit
                .iter()
                .any(|row| row.action == "rights_request.transition")
        );
        assert!(
            audit
                .iter()
                .all(|row| !row.before.to_string().contains("rights@example.com")
                    && !row.after.to_string().contains("rights@example.com"))
        );
        assert!(
            sqlx::query("DELETE FROM cn_legal.rights_request_events WHERE request_id = $1")
                .bind(&created.record.id)
                .execute(&pool)
                .await
                .is_err(),
            "rights request events must be append-only"
        );
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn expired_held_case_is_hidden_exportable_and_deleted_after_release() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping legal-hold integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_legal_hold").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let cipher =
            LegalDataCipher::from_key_material("unit-test-legal-data-key-0123456789abcdef")?;
        let retention = RetentionPolicy::default();
        let now = chrono::Utc::now();
        let created = insert_rights_request(
            &pool,
            &request(),
            RightsRequestScopeStatus::UnverifiedScope,
            &cipher,
            now - chrono::Duration::days(800),
        )
        .await?;
        let categories = vec![
            "rights_request".to_string(),
            "rights_request_contact".to_string(),
            "rights_request_identity".to_string(),
            "rights_request_evidence".to_string(),
            "rights_request_history".to_string(),
            "operator_audit".to_string(),
        ];
        let hold = start_legal_hold(
            &pool,
            "rights_request",
            &created.record.id,
            &categories,
            "court preservation order",
            "final disposition",
            "legal@node.example",
            now,
        )
        .await?;

        cleanup_expired(&pool, now).await?;
        assert!(
            get_rights_request(&pool, &created.record.id)
                .await?
                .is_none()
        );
        let physical_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM cn_legal.rights_requests WHERE id = $1")
                .bind(&created.record.id)
                .fetch_one(&pool)
                .await?;
        assert_eq!(physical_count, 1, "hold must prevent physical deletion");

        let export =
            export_legal_hold(&pool, &cipher, &hold.id, "reviewer@node.example", now).await?;
        // export の期限は、最終の状態遷移の時刻に状態の区分（未解決）の日数を足した値。
        assert_eq!(
            serde_json::from_value::<chrono::DateTime<chrono::Utc>>(
                export.data["rights_request"]["expires_at"].clone()
            )?,
            created.record.updated_at
                + chrono::Duration::days(i64::from(retention.rights_request_active_days))
        );
        let exported = serde_json::to_string(&export)?;
        assert!(exported.contains("rights@example.com"));
        for forbidden in [
            "tracking_secret_hash",
            "ciphertext",
            "nonce",
            "unit-test-legal-data-key",
        ] {
            assert!(!exported.contains(forbidden), "export leaked {forbidden}");
        }

        release_legal_hold(&pool, &hold.id, "legal@node.example", now).await?;
        let holds = "SELECT COUNT(*) FROM cn_legal.legal_holds WHERE id = $1";
        assert_eq!(
            count(&pool, holds, &hold.id).await?,
            0,
            "release deletes the hold"
        );
        let audit = list_operator_actions(&pool, 20, 0).await?;
        for action in ["legal_hold.start", "legal_hold.release"] {
            assert!(
                audit.iter().any(|row| row.action == action
                    && row.target_id == created.record.id
                    && row.after["data_categories"] == serde_json::json!(categories)),
                "operator audit must keep {action} with the hold's categories"
            );
        }
        assert!(
            release_legal_hold(&pool, &hold.id, "legal@node.example", now)
                .await
                .is_err(),
            "double release must fail"
        );
        assert!(
            export_legal_hold(&pool, &cipher, &hold.id, "reviewer@node.example", now)
                .await
                .is_err(),
            "released hold cannot be exported"
        );
        cleanup_expired(&pool, now).await?;
        let physical_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM cn_legal.rights_requests WHERE id = $1")
                .bind(&created.record.id)
                .fetch_one(&pool)
                .await?;
        assert_eq!(physical_count, 0, "released expired case must be deleted");
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

async fn count(pool: &sqlx::PgPool, sql: &'static str, id: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(sql).bind(id).fetch_one(pool).await?)
}

/// 却下等の本体は区分の期限（終了から 180 日）で消え、履歴は記録から 365 日まで残る（#1706）。
#[tokio::test]
async fn declined_request_body_is_deleted_before_its_history_expires() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping rights-request retention integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_rights_retention").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let cipher =
            LegalDataCipher::from_key_material("unit-test-legal-data-key-0123456789abcdef")?;
        let now = chrono::Utc::now();
        let declined_at = now - chrono::Duration::days(200);
        let created = insert_rights_request(
            &pool,
            &request(),
            RightsRequestScopeStatus::VerifiedScope,
            &cipher,
            declined_at - chrono::Duration::days(10),
        )
        .await?;
        transition_rights_request(
            &pool,
            &created.record.id,
            created.record.version,
            "legal@node.example",
            RightsRequestStatus::Declined,
            Some("対象外のため却下しました"),
            "status_surface",
            declined_at,
        )
        .await?;
        let id = created.record.id.as_str();
        let bodies = "SELECT COUNT(*) FROM cn_legal.rights_requests WHERE id = $1";
        let history = "SELECT COUNT(*) FROM cn_legal.rights_request_events WHERE request_id = $1";

        cleanup_expired(&pool, now).await?;
        assert_eq!(
            count(&pool, bodies, id).await?,
            0,
            "expired body must be deleted"
        );
        assert_eq!(
            count(&pool, history, id).await?,
            2,
            "history keeps its retention"
        );

        cleanup_expired(&pool, declined_at + chrono::Duration::days(365)).await?;
        assert_eq!(
            count(&pool, history, id).await?,
            0,
            "history expires on its own"
        );
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// #1705: 起動時の鍵の照合は暗号文を 1 行だけ読む。照合が読まない行の改ざんは、その行を読むときに失敗する。
#[tokio::test]
async fn legal_data_key_check_reads_one_row_and_tampering_fails_on_read() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping legal data key check integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_legal_key_check").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let cipher =
            LegalDataCipher::from_key_material("unit-test-legal-data-key-0123456789abcdef")?;
        let wrong_cipher =
            LegalDataCipher::from_key_material("wrong-unit-test-legal-key-0123456789abcdef")?;
        verify_legal_data_key(&pool, &wrong_cipher).await?;
        let now = chrono::Utc::now();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let created = insert_rights_request(
                &pool,
                &request(),
                RightsRequestScopeStatus::UnverifiedScope,
                &cipher,
                now,
            )
            .await?;
            ids.push(created.record.id);
        }
        assert!(verify_legal_data_key(&pool, &wrong_cipher).await.is_err());
        verify_legal_data_key(&pool, &cipher).await?;

        let tampered: String = sqlx::query_scalar(
            "UPDATE cn_legal.sensitive_items
             SET ciphertext = set_byte(ciphertext, 0, get_byte(ciphertext, 0) # 1)
             WHERE id = (SELECT id FROM cn_legal.sensitive_items ORDER BY id DESC LIMIT 1)
             RETURNING owner_id",
        )
        .fetch_one(&pool)
        .await?;
        verify_legal_data_key(&pool, &cipher).await?;
        assert!(
            get_rights_request_with_sensitive(&pool, &cipher, &tampered, now)
                .await
                .is_err(),
            "tampered ciphertext must fail closed on read"
        );
        let intact = ids.iter().find(|id| **id != tampered).expect("intact id");
        let intact = get_rights_request_with_sensitive(&pool, &cipher, intact, now)
            .await?
            .expect("intact request");
        assert_eq!(intact.request.email, "rights@example.com");

        assert_eq!(key_check_rows_read(&pool, 2_000).await?, 1.0);
        assert_eq!(key_check_rows_read(&pool, 20_000).await?, 1.0);
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// 暗号文を `total` 行まで足して ANALYZE し、照合の SQL が `cn_legal.sensitive_items` から読む行数を返す。
async fn key_check_rows_read(pool: &PgPool, total: i64) -> Result<f64> {
    sqlx::query(
        "INSERT INTO cn_legal.sensitive_items
            (id, owner_kind, owner_id, data_category, nonce, ciphertext)
         SELECT 'seed-' || lpad(n::text, 6, '0'), 'report', 'seed-' || n, 'report_contact',
                decode(repeat('00', 24), 'hex'), decode(repeat('ab', 32), 'hex')
         FROM generate_series((SELECT COUNT(*) FROM cn_legal.sensitive_items) + 1, $1) AS n",
    )
    .bind(total)
    .execute(pool)
    .await?;
    sqlx::query("ANALYZE cn_legal.sensitive_items")
        .execute(pool)
        .await?;
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE, FORMAT JSON) {LEGAL_DATA_KEY_CHECK_SQL}"
    )))
    .fetch_one(pool)
    .await?;
    Ok(rows_read(&plan, "sensitive_items"))
}

/// 計画の各節点の (Actual Rows + Rows Removed by Filter) × Actual Loops を、対象の表について足す。
fn rows_read(node: &Value, relation: &str) -> f64 {
    match node {
        Value::Array(items) => items.iter().map(|item| rows_read(item, relation)).sum(),
        Value::Object(fields) => {
            let number = |key: &str| fields.get(key).and_then(Value::as_f64);
            let own = if fields.get("Relation Name").and_then(Value::as_str) == Some(relation) {
                (number("Actual Rows").unwrap_or(0.0)
                    + number("Rows Removed by Filter").unwrap_or(0.0))
                    * number("Actual Loops").unwrap_or(1.0)
            } else {
                0.0
            };
            own + fields
                .values()
                .map(|value| rows_read(value, relation))
                .sum::<f64>()
        }
        _ => 0.0,
    }
}
