//! #680 異議申し立て受理・運営者審査の Postgres 結合試験。

use anyhow::Result;
use kukuri_cn_core::{
    AppealReviewOperation, AppealReviewVersion, NewCommunityNodeReport, PersistedRiskSignal,
    RiskSignalCorrection, RiskSignalMetadataEdit, TestDatabase, apply_appeal_review_action,
    connect_postgres, get_appeal_review, get_community_node_report, get_risk_signal,
    initialize_database, insert_community_node_appeal, list_appeal_reviews, list_operator_actions,
    list_trust_basis_page, persist_risk_signal, persist_risk_signal_deduplicated,
    sweep_expired_trust_signals,
};
use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal, Severity, Visibility,
};

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const ISSUER: &str = "issuer-node-1";

/// 利用者の trust read の basis（審査の結果を確かめる）。期限を過ぎた行は掃除の後に外れる。
async fn basis(pool: &sqlx::PgPool, target: &str) -> Result<Vec<kukuri_cn_trust::TrustRiskInput>> {
    sweep_expired_trust_signals(pool, chrono::Utc::now()).await?;
    Ok(list_trust_basis_page(pool, target, None).await?.inputs)
}

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn signal(target_id: &str, issuer_status: AppealStatus) -> SafetyRiskSignal {
    SafetyRiskSignal {
        target: RiskSignalTarget::UserPubkey,
        target_id: target_id.to_string(),
        category: SafetyCategory::Nsfw,
        severity: Severity::High,
        basis: Basis::ClassifierScore,
        confidence: Some(90),
        visibility: Visibility::Local,
        expires_at: None,
        appeal_status: Some(issuer_status),
    }
}

fn report(target_id: &str, details: &str) -> NewCommunityNodeReport {
    NewCommunityNodeReport {
        subject_kind: "profile".to_string(),
        subject_id: target_id.to_string(),
        capability: "moderation".to_string(),
        reason: "false_positive".to_string(),
        details: Some(details.to_string()),
        reporter_contact: Some("must-not-be-stored@example.com".to_string()),
        appeal_risk_signal_id: None,
    }
}

#[tokio::test]
async fn appeal_intake_is_linked_atomic_anonymous_and_grouped() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping appeal review integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_appeal_intake_680").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let stored =
            persist_risk_signal(&pool, ISSUER, &signal("alice", AppealStatus::None)).await?;

        let first = insert_community_node_appeal(
            &pool,
            ISSUER,
            &stored.id,
            &report("alice", "一件目の説明"),
        )
        .await?;
        let second = insert_community_node_appeal(
            &pool,
            ISSUER,
            &stored.id,
            &report("alice", "二件目の説明"),
        )
        .await?;
        assert_ne!(first.id, second.id);
        assert_eq!(
            first.appeal_risk_signal_id.as_deref(),
            Some(stored.id.as_str())
        );
        assert!(first.reporter_contact.is_none());

        let reviews = list_appeal_reviews(&pool, 50, 0).await?;
        assert_eq!(reviews.len(), 1, "同じ判定は一つの審査対象にまとめる");
        assert_eq!(reviews[0].reports.len(), 2);

        let foreign = persist_risk_signal(
            &pool,
            "foreign-node",
            &signal("mallory", AppealStatus::None),
        )
        .await?;
        assert!(
            insert_community_node_appeal(
                &pool,
                ISSUER,
                &foreign.id,
                &report("mallory", "拒否される説明"),
            )
            .await
            .is_err()
        );
        let foreign_after = get_risk_signal(&pool, &foreign.id)
            .await?
            .expect("foreign signal");
        assert_eq!(foreign_after.signal.appeal_status, Some(AppealStatus::None));
        assert!(get_appeal_review(&pool, &foreign.id).await?.is_some());
        assert!(
            get_appeal_review(&pool, &foreign.id)
                .await?
                .expect("foreign review shell")
                .reports
                .is_empty()
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn operator_review_revalidates_and_commits_state_reports_and_audit_together() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping appeal review integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_appeal_actions_680").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let stored =
            persist_risk_signal(&pool, ISSUER, &signal("alice", AppealStatus::None)).await?;
        let first = insert_community_node_appeal(
            &pool,
            ISSUER,
            &stored.id,
            &report("alice", "監査へ入れてはならない本文"),
        )
        .await?;
        let stale = get_appeal_review(&pool, &stored.id)
            .await?
            .expect("review")
            .version();
        insert_community_node_appeal(
            &pool,
            ISSUER,
            &stored.id,
            &report("alice", "確認後に届いた本文"),
        )
        .await?;
        assert!(
            apply_appeal_review_action(
                &pool,
                "ops@kukuri.app",
                &stored.id,
                &AppealReviewOperation::Accept { expected: stale },
                true,
            )
            .await
            .is_err(),
            "古い確認内容は拒否する"
        );
        assert!(list_operator_actions(&pool, 50, 0).await?.is_empty());

        let current = get_appeal_review(&pool, &stored.id)
            .await?
            .expect("review")
            .version();
        assert!(
            apply_appeal_review_action(
                &pool,
                "ops@kukuri.app",
                &stored.id,
                &AppealReviewOperation::Accept {
                    expected: current.clone(),
                },
                false,
            )
            .await
            .is_err(),
            "機能無効時は拒否する"
        );
        apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &stored.id,
            &AppealReviewOperation::Accept { expected: current },
            true,
        )
        .await?;
        let cleared = get_risk_signal(&pool, &stored.id).await?.expect("signal");
        assert_eq!(cleared.signal.appeal_status, Some(AppealStatus::Cleared));
        assert_eq!(
            get_community_node_report(&pool, &first.id)
                .await?
                .expect("report")
                .status,
            "actioned"
        );
        let trust = basis(&pool, "alice").await?;
        assert_eq!(trust.len(), 1);
        assert_eq!(
            trust[0].component,
            kukuri_cn_trust::TrustComponentKind::Relative
        );
        assert_eq!(trust[0].appeal_status, AppealStatus::Cleared);

        let rejected =
            persist_risk_signal(&pool, ISSUER, &signal("bob", AppealStatus::None)).await?;
        let rejected_report =
            insert_community_node_appeal(&pool, ISSUER, &rejected.id, &report("bob", "棄却対象"))
                .await?;
        let expected = get_appeal_review(&pool, &rejected.id)
            .await?
            .expect("review")
            .version();
        apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &rejected.id,
            &AppealReviewOperation::Reject { expected },
            true,
        )
        .await?;
        assert_eq!(
            get_risk_signal(&pool, &rejected.id)
                .await?
                .expect("signal")
                .signal
                .appeal_status,
            Some(AppealStatus::None)
        );
        assert_eq!(
            get_community_node_report(&pool, &rejected_report.id)
                .await?
                .expect("report")
                .status,
            "dismissed"
        );
        assert_eq!(basis(&pool, "bob").await?.len(), 1);

        let edited =
            persist_risk_signal(&pool, ISSUER, &signal("carol", AppealStatus::None)).await?;
        let edited_report =
            insert_community_node_appeal(&pool, ISSUER, &edited.id, &report("carol", "調整対象"))
                .await?;
        let expected = get_appeal_review(&pool, &edited.id)
            .await?
            .expect("review")
            .version();
        apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &edited.id,
            &AppealReviewOperation::Edit {
                expected,
                edit: RiskSignalMetadataEdit {
                    category: Some(SafetyCategory::Spam),
                    severity: Some(Severity::Low),
                    confidence: Some(20),
                    expires_at: None,
                },
            },
            true,
        )
        .await?;
        let expected = get_appeal_review(&pool, &edited.id)
            .await?
            .expect("review")
            .version();
        let before_ids =
            sqlx::query_scalar::<_, String>("SELECT id FROM cn_safety.risk_signals ORDER BY id")
                .fetch_all(&pool)
                .await?;
        apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &edited.id,
            &AppealReviewOperation::Reissue {
                expected,
                correction: RiskSignalCorrection {
                    category: None,
                    severity: None,
                    confidence: Some(10),
                    visibility: Some(Visibility::Public),
                },
            },
            true,
        )
        .await?;
        let after_ids =
            sqlx::query_scalar::<_, String>("SELECT id FROM cn_safety.risk_signals ORDER BY id")
                .fetch_all(&pool)
                .await?;
        assert_eq!(after_ids.len(), before_ids.len() + 1);
        // #710(案A): 旧判定は失効させず、同一取引で cleared にして終結させる。
        // 利用者は再取得で「係争中だった判定が認容として終結した」ことを確認できる。
        let old_signal = get_risk_signal(&pool, &edited.id)
            .await?
            .expect("old signal");
        assert_eq!(
            old_signal.signal.expires_at, None,
            "旧判定は失効させない(失効すると根拠一覧から消え、終結を確認できない)"
        );
        assert_eq!(
            old_signal.signal.appeal_status,
            Some(AppealStatus::Cleared),
            "旧判定は認容として終結する"
        );
        assert_eq!(
            get_community_node_report(&pool, &edited_report.id)
                .await?
                .expect("report")
                .status,
            "actioned",
            "関連通報は訂正版発行という対応を取った処理済みになる"
        );
        // 再取得(trust 入力): 旧判定が cleared(寄与 0)で残り、訂正版の新判定が現れる。
        let trust = basis(&pool, "carol").await?;
        assert_eq!(trust.len(), 2);
        assert!(
            trust.iter().any(|input| input.signal_id == edited.id
                && input.appeal_status == AppealStatus::Cleared),
            "旧判定は cleared として根拠一覧に残る"
        );
        assert!(
            trust
                .iter()
                .any(|input| input.signal_id != edited.id
                    && input.appeal_status == AppealStatus::None),
            "訂正版の新判定が根拠一覧に現れる"
        );

        let audit = list_operator_actions(&pool, 50, 0).await?;
        assert_eq!(audit.len(), 4);
        let serialized = serde_json::to_string(&audit)?;
        assert!(!serialized.contains("監査へ入れてはならない本文"));
        assert!(!serialized.contains("must-not-be-stored@example.com"));
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

async fn count_signals(pool: &sqlx::PgPool, target_id: &str) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM cn_safety.risk_signals WHERE target_id = $1",
    )
    .bind(target_id)
    .fetch_one(pool)
    .await?)
}

/// 構成変更後の再 scan（scanner 経路）。審査の訂正値と異なる値で保存を試みる。
async fn rescan(
    pool: &sqlx::PgPool,
    target_id: &str,
    category: SafetyCategory,
) -> Result<PersistedRiskSignal> {
    let mut rescanned = signal(target_id, AppealStatus::None);
    rescanned.category = category;
    rescanned.severity = Severity::Critical;
    rescanned.confidence = Some(84);
    rescanned.visibility = Visibility::Public;
    persist_risk_signal_deduplicated(pool, ISSUER, &rescanned, None).await
}

async fn open_review(
    pool: &sqlx::PgPool,
    signal_id: &str,
    target_id: &str,
) -> Result<AppealReviewVersion> {
    insert_community_node_appeal(pool, ISSUER, signal_id, &report(target_id, "誤検知")).await?;
    Ok(get_appeal_review(pool, signal_id)
        .await?
        .expect("review")
        .version())
}

/// #1058 AC-1 / AC-2: 審査の訂正版再発行・検知メタデータ編集で確定した値は、構成変更後の
/// 再 scan で上書きされず、signal の新規 insert も起きない。
#[tokio::test]
async fn appeal_review_adjustments_survive_rescan() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping appeal review integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_appeal_rescan_1058").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;

        // Issue の再現 sequence: 申し立て → 審査で confidence 20 に訂正して再発行 → 再 scan。
        let stored =
            persist_risk_signal(&pool, ISSUER, &signal("frank", AppealStatus::None)).await?;
        let expected = open_review(&pool, &stored.id, "frank").await?;
        apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &stored.id,
            &AppealReviewOperation::Reissue {
                expected,
                correction: RiskSignalCorrection {
                    confidence: Some(20),
                    ..RiskSignalCorrection::default()
                },
            },
            true,
        )
        .await?;
        let corrected_id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM cn_safety.risk_signals WHERE target_id = 'frank' AND id <> $1",
        )
        .bind(&stored.id)
        .fetch_one(&pool)
        .await?;
        let again = rescan(&pool, "frank", SafetyCategory::Nsfw).await?;
        assert!(!again.newly_created);
        assert_eq!(again.stored.id, corrected_id);
        let after = get_risk_signal(&pool, &corrected_id)
            .await?
            .expect("corrected");
        assert_eq!(after.signal.confidence, Some(20));
        assert_eq!(after.signal.severity, Severity::High);
        assert_eq!(after.signal.visibility, Visibility::Local);
        assert_eq!(after.signal.expires_at, None);
        assert_eq!(after.signal.appeal_status, Some(AppealStatus::None));
        assert_eq!(count_signals(&pool, "frank").await?, 2);
        assert!(after.operator_adjusted_at.is_some());
        let old = get_risk_signal(&pool, &stored.id).await?.expect("old");
        assert_eq!(old.signal.appeal_status, Some(AppealStatus::Cleared));
        assert!(
            old.operator_adjusted_at.is_none(),
            "cleared で終結した旧判定は訂正の印を持たない"
        );

        // 審査の編集で category を変えた行: 元の category の再 scan でも新しい行を作らず、
        // その後の訂正版再発行も元の category を引き継いで保護される。
        let stored =
            persist_risk_signal(&pool, ISSUER, &signal("grace", AppealStatus::None)).await?;
        let expected = open_review(&pool, &stored.id, "grace").await?;
        apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &stored.id,
            &AppealReviewOperation::Edit {
                expected,
                edit: RiskSignalMetadataEdit {
                    category: Some(SafetyCategory::Spam),
                    confidence: Some(30),
                    ..RiskSignalMetadataEdit::default()
                },
            },
            true,
        )
        .await?;
        let again = rescan(&pool, "grace", SafetyCategory::Nsfw).await?;
        assert!(!again.newly_created);
        assert_eq!(count_signals(&pool, "grace").await?, 1);
        let again = rescan(&pool, "grace", SafetyCategory::Spam).await?;
        assert!(!again.newly_created);
        let after = get_risk_signal(&pool, &stored.id).await?.expect("edited");
        assert_eq!(after.signal.confidence, Some(30));
        assert_eq!(after.signal.severity, Severity::High);
        assert_eq!(after.signal.appeal_status, Some(AppealStatus::Disputed));
        assert!(after.operator_adjusted_at.is_some());

        let expected = get_appeal_review(&pool, &stored.id)
            .await?
            .expect("review")
            .version();
        apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &stored.id,
            &AppealReviewOperation::Reissue {
                expected,
                correction: RiskSignalCorrection {
                    confidence: Some(10),
                    ..RiskSignalCorrection::default()
                },
            },
            true,
        )
        .await?;
        assert_eq!(count_signals(&pool, "grace").await?, 2);
        let again = rescan(&pool, "grace", SafetyCategory::Nsfw).await?;
        assert!(!again.newly_created);
        let again = rescan(&pool, "grace", SafetyCategory::Spam).await?;
        assert!(!again.newly_created);
        assert_eq!(again.stored.signal.confidence, Some(10));
        assert_eq!(count_signals(&pool, "grace").await?, 2);

        // INVAR-2: 棄却は訂正の印を付けず、#1050 の集約更新を維持する。
        let stored =
            persist_risk_signal(&pool, ISSUER, &signal("heidi", AppealStatus::None)).await?;
        let expected = open_review(&pool, &stored.id, "heidi").await?;
        apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &stored.id,
            &AppealReviewOperation::Reject { expected },
            true,
        )
        .await?;
        let again = rescan(&pool, "heidi", SafetyCategory::Nsfw).await?;
        assert!(!again.newly_created);
        assert!(again.stored.operator_adjusted_at.is_none());
        assert_eq!(again.stored.signal.confidence, Some(84));
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// #700: 中核処理へ不正な入力値(RFC 3339 でない有効期限・範囲外の確信度)を直接渡すと
/// 保存前に拒否され、`risk_signals` / `reports` / `operator_actions` が一切変化しない。
#[tokio::test]
async fn invalid_review_inputs_are_rejected_without_state_change() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping appeal review integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_appeal_invalid_700").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let stored =
            persist_risk_signal(&pool, ISSUER, &signal("dave", AppealStatus::None)).await?;
        let appeal_report = insert_community_node_appeal(
            &pool,
            ISSUER,
            &stored.id,
            &report("dave", "不正入力の対象"),
        )
        .await?;
        let expected = get_appeal_review(&pool, &stored.id)
            .await?
            .expect("review")
            .version();
        let before_signal = get_risk_signal(&pool, &stored.id).await?.expect("signal");
        let before_signal_ids =
            sqlx::query_scalar::<_, String>("SELECT id FROM cn_safety.risk_signals ORDER BY id")
                .fetch_all(&pool)
                .await?;

        let invalid_operations = vec![
            AppealReviewOperation::Edit {
                expected: expected.clone(),
                edit: RiskSignalMetadataEdit {
                    expires_at: Some("not-a-timestamp".to_string()),
                    ..RiskSignalMetadataEdit::default()
                },
            },
            AppealReviewOperation::Edit {
                expected: expected.clone(),
                edit: RiskSignalMetadataEdit {
                    confidence: Some(101),
                    ..RiskSignalMetadataEdit::default()
                },
            },
            AppealReviewOperation::Edit {
                expected: expected.clone(),
                edit: RiskSignalMetadataEdit {
                    confidence: Some(255),
                    ..RiskSignalMetadataEdit::default()
                },
            },
            AppealReviewOperation::Reissue {
                expected: expected.clone(),
                correction: RiskSignalCorrection {
                    confidence: Some(101),
                    ..RiskSignalCorrection::default()
                },
            },
        ];
        for operation in &invalid_operations {
            assert!(
                apply_appeal_review_action(&pool, "ops@kukuri.app", &stored.id, operation, true)
                    .await
                    .is_err(),
                "不正な入力値は拒否されるべき: {operation:?}"
            );
        }

        // 拒否された操作でデータベースが一切変化していないこと。
        let after_signal = get_risk_signal(&pool, &stored.id).await?.expect("signal");
        assert_eq!(after_signal.signal, before_signal.signal);
        let after_signal_ids =
            sqlx::query_scalar::<_, String>("SELECT id FROM cn_safety.risk_signals ORDER BY id")
                .fetch_all(&pool)
                .await?;
        assert_eq!(
            after_signal_ids, before_signal_ids,
            "再発行の新規行が残ってはならない"
        );
        assert_eq!(
            get_community_node_report(&pool, &appeal_report.id)
                .await?
                .expect("report")
                .status,
            "received"
        );
        assert!(list_operator_actions(&pool, 50, 0).await?.is_empty());

        // 妥当な入力(過去時刻を含む RFC 3339)は従来どおり適用できる。
        apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &stored.id,
            &AppealReviewOperation::Edit {
                expected,
                edit: RiskSignalMetadataEdit {
                    expires_at: Some("2000-01-01T00:00:00Z".to_string()),
                    ..RiskSignalMetadataEdit::default()
                },
            },
            true,
        )
        .await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// #1050 INV-9: 検知メタデータ編集で category を変えた結果、別の活性 signal と鍵が衝突する場合は
/// 部分 UNIQUE index が拒否し、取引全体が巻き戻る（部分書込なし）。
#[tokio::test]
async fn edit_detection_category_collision_is_rejected_without_partial_write() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping appeal review integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_appeal_edit_collision").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let nsfw = persist_risk_signal(&pool, ISSUER, &signal("erin", AppealStatus::None)).await?;
        let mut spam_signal = signal("erin", AppealStatus::None);
        spam_signal.category = SafetyCategory::Spam;
        let spam = persist_risk_signal(&pool, ISSUER, &spam_signal).await?;
        assert_ne!(nsfw.id, spam.id);

        insert_community_node_appeal(&pool, ISSUER, &spam.id, &report("erin", "衝突")).await?;
        let expected = get_appeal_review(&pool, &spam.id)
            .await?
            .expect("review")
            .version();
        let collision = apply_appeal_review_action(
            &pool,
            "ops@kukuri.app",
            &spam.id,
            &AppealReviewOperation::Edit {
                expected,
                edit: RiskSignalMetadataEdit {
                    category: Some(SafetyCategory::Nsfw),
                    severity: Some(Severity::Low),
                    confidence: Some(20),
                    expires_at: None,
                },
            },
            true,
        )
        .await;
        assert!(collision.is_err(), "same active key must be rejected");

        let unchanged = get_risk_signal(&pool, &spam.id)
            .await?
            .expect("spam signal");
        assert_eq!(unchanged.signal.category, SafetyCategory::Spam);
        assert_eq!(unchanged.signal.severity, Severity::High);
        let other = get_risk_signal(&pool, &nsfw.id)
            .await?
            .expect("nsfw signal");
        assert_eq!(other.signal.category, SafetyCategory::Nsfw);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}
