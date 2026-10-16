//! Public node-local transmission-prevention status contract (#761).

use anyhow::Result;
use kukuri_cn_core::{
    AdminOperation, MalwareSubject, NewCommunityNodeReport, NewTransmissionPrevention,
    TransmissionPreventionBasis, TransmissionPreventionCapability, apply_operator_action,
    apply_transmission_prevention, connect_postgres, insert_community_node_report,
    release_transmission_prevention,
};
use reqwest::{Client, StatusCode};

mod support;
use support::{TestServer, integration_test_admin_database_url};

#[tokio::test]
async fn public_status_exposes_scope_and_appeal_path_without_operator_identity() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping transmission-prevention user-api integration test");
        return Ok(());
    };
    let server = TestServer::spawn(admin_url.as_str(), "transmission-prevention-status").await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    apply_transmission_prevention(
        &pool,
        "operator@example.net",
        &NewTransmissionPrevention {
            subject_kind: "post".to_string(),
            subject_id: "post-761-status".to_string(),
            basis: TransmissionPreventionBasis::Privacy,
            capabilities: vec![
                TransmissionPreventionCapability::CommunityIndex,
                TransmissionPreventionCapability::Search,
            ],
            expires_at: None,
            related_report_id: Some("private-report-reference".to_string()),
        },
    )
    .await?;

    let client = Client::new();
    let response = client
        .get(format!(
            "{}/v1/transmission-preventions/post/post-761-status",
            server.base_url
        ))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.json::<serde_json::Value>().await?;
    assert_eq!(body["active"], true);
    assert_eq!(body["basis"], "privacy");
    assert_eq!(body["appeal_path"], "/v1/report");
    assert_eq!(
        body["capabilities"],
        serde_json::json!(["community_index", "search"])
    );
    assert!(body.get("decided_by").is_none());
    assert!(body.get("related_report_id").is_none());

    release_transmission_prevention(
        &pool,
        "operator@example.net",
        "post",
        "post-761-status",
        "claim resolved",
    )
    .await?;
    let body = client
        .get(format!(
            "{}/v1/transmission-preventions/post/post-761-status",
            server.base_url
        ))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(body["active"], false);
    assert_eq!(body["appeal_path"], "/v1/report");

    server.shutdown().await
}

/// #1062: malware の通報から制限した投稿の状態は、根拠 `malware` と止めた提供面だけを返し、運営者と通報を返さない。
/// 解除すると有効でなくなる。
#[tokio::test]
async fn public_status_explains_a_malware_restriction_until_it_is_released() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping transmission-prevention user-api integration test");
        return Ok(());
    };
    let server = TestServer::spawn(admin_url.as_str(), "malware-restriction-status").await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    // 投稿（AC-1）と添付の blob hash（AC-2）の判断を、同じ公開の状態取得で説明する。
    let hash = "b".repeat(64);
    for (subject, subject_id) in [
        (MalwareSubject::Post, "post-1062-status".to_string()),
        (MalwareSubject::Blob, hash),
    ] {
        let report = insert_community_node_report(
            &pool,
            &NewCommunityNodeReport {
                subject_kind: subject.report_kind().to_string(),
                subject_id: subject_id.clone(),
                capability: "community_index".to_string(),
                reason: "malware".to_string(),
                ..NewCommunityNodeReport::default()
            },
            None,
        )
        .await?;
        let status = || async {
            anyhow::Ok(
                Client::new()
                    .get(format!(
                        "{}/v1/transmission-preventions/{}/{subject_id}",
                        server.base_url,
                        subject.as_str()
                    ))
                    .send()
                    .await?
                    .json::<serde_json::Value>()
                    .await?,
            )
        };
        assert_eq!(status().await?["active"], false);
        apply_operator_action(
            &pool,
            "ops@example.net",
            &AdminOperation::RestrictMalware {
                subject,
                report_id: report.id.clone(),
            },
        )
        .await?;
        let body = status().await?;
        assert_eq!(body["active"], true);
        assert_eq!(body["basis"], "malware");
        assert_eq!(
            body["capabilities"],
            serde_json::json!(["community_index", "search", "discovery", "recommendation"])
        );
        assert!(!body.to_string().contains(&report.id));
        assert!(!body.to_string().contains("ops@example.net"));

        apply_operator_action(
            &pool,
            "ops@example.net",
            &AdminOperation::ReleaseMalwareRestriction {
                subject,
                subject_id: subject_id.clone(),
                reason: "cleared".to_string(),
            },
        )
        .await?;
        assert_eq!(status().await?["active"], false);
    }
    server.shutdown().await
}
