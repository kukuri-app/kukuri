//! appeal 経路 / operator レビューの Postgres integration テスト（#420 / ADR 0028 §2.3 / §2.8）。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。
//!
//! ADR 0028 contract:
//! - `false_positive_appeal_path_exists`
//! - `appeal_cleared_propagates_and_reverts_trust_contribution`
//! - `operator_review_can_edit_detection_metadata`

use anyhow::Result;
use kukuri_cn_core::{
    DistributionAudience, PersistedRiskSignal, RiskSignalCorrection, RiskSignalMetadataEdit,
    TestDatabase, connect_postgres, dispute_risk_signal, edit_risk_signal_detection_metadata,
    get_risk_signal, initialize_database, list_distributable_risk_signals, list_trust_basis_page,
    load_trust_totals, persist_risk_signal, persist_risk_signal_deduplicated,
    persist_risk_signal_with_author, reissue_corrected_risk_signal,
    update_risk_signal_appeal_status,
};
use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal, Severity, Visibility,
};

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const ISSUER: &str = "issuer-node-1";
const NOW: &str = "2026-07-30T09:00:00Z";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

/// VLM 由来の suspected signal（basis = ClassifierScore、配布可 visibility）。
fn suspected_signal(target_id: &str, category: SafetyCategory) -> SafetyRiskSignal {
    SafetyRiskSignal {
        target: RiskSignalTarget::BlobCid,
        target_id: target_id.to_string(),
        category,
        severity: Severity::Critical,
        basis: Basis::ClassifierScore,
        confidence: Some(90),
        visibility: Visibility::SubscribedNodes,
        expires_at: None,
        appeal_status: Some(AppealStatus::None),
    }
}

#[tokio::test]
async fn false_positive_appeal_path_exists() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety appeals integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_appeal_path").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;

        // issuer node が発行した suspected advisory に対し、user / client が
        // （/v1/report の appeal 参照経由で）異議を申し立てられる。
        let stored = persist_risk_signal(
            &pool,
            ISSUER,
            &suspected_signal("bafy-fp", SafetyCategory::Csam),
        )
        .await?;
        assert_eq!(
            stored.signal.appeal_status,
            Some(AppealStatus::None),
            "advisory starts undisputed"
        );

        let disputed = dispute_risk_signal(&pool, &stored.id).await?;
        assert_eq!(disputed.signal.appeal_status, Some(AppealStatus::Disputed));

        // 冪等: 二重申し立ては同じ Disputed のまま成功する。
        let again = dispute_risk_signal(&pool, &stored.id).await?;
        assert_eq!(again.signal.appeal_status, Some(AppealStatus::Disputed));

        // operator は申し立て中の advisory を参照できる（レビュー導線）。
        let visible = get_risk_signal(&pool, &stored.id).await?.expect("exists");
        assert_eq!(visible.signal.appeal_status, Some(AppealStatus::Disputed));

        // 不正な遷移（None → Cleared の飛び越し）は拒否される。
        let other = persist_risk_signal(
            &pool,
            ISSUER,
            &suspected_signal("bafy-2", SafetyCategory::Nsfw),
        )
        .await?;
        assert!(
            update_risk_signal_appeal_status(&pool, &other.id, AppealStatus::Cleared)
                .await
                .is_err(),
            "None -> Cleared must be rejected"
        );

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn appeal_cleared_propagates_and_reverts_trust_contribution() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety appeals integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_appeal_cleared").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;

        let stored = persist_risk_signal_with_author(
            &pool,
            ISSUER,
            &suspected_signal("blob-target", SafetyCategory::Csam),
            Some("pubkey-author"),
        )
        .await?;
        let author = ["pubkey-author".to_string()];

        // Cleared 前は著者の信頼値に寄与している。
        assert!(load_trust_totals(&pool, &author).await?["pubkey-author"].absolute() < 0.0);

        // Disputed → Cleared（operator 認容）。
        dispute_risk_signal(&pool, &stored.id).await?;
        let cleared =
            update_risk_signal_appeal_status(&pool, &stored.id, AppealStatus::Cleared).await?;
        assert_eq!(cleared.signal.appeal_status, Some(AppealStatus::Cleared));

        // 伝播: Cleared の signal は失効させず、配布クエリに **残る**（受け手が
        // appeal_status を見て寄与を除外できるように配布し続ける）。
        let distributable = list_distributable_risk_signals(
            &pool,
            DistributionAudience::SubscribedNodes,
            NOW,
            50,
            0,
        )
        .await?;
        let found = distributable
            .iter()
            .find(|s| s.id == stored.id)
            .expect("cleared advisory keeps distributing the correction");
        assert_eq!(found.signal.appeal_status, Some(AppealStatus::Cleared));

        // trust 寄与の戻し: Cleared を説明用に basis へ残し、集計は負の寄与をゼロにする。
        assert_eq!(
            load_trust_totals(&pool, &author).await?["pubkey-author"].absolute(),
            0.0
        );
        let basis = list_trust_basis_page(&pool, "pubkey-author", None)
            .await?
            .inputs;
        assert_eq!(basis.len(), 1);
        assert_eq!(basis[0].signal_id, stored.id);
        assert_eq!(basis[0].appeal_status, AppealStatus::Cleared);

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn operator_review_can_edit_detection_metadata() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety appeals integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_operator_review").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;

        let stored = persist_risk_signal(
            &pool,
            ISSUER,
            &suspected_signal("bafy-edit", SafetyCategory::Csam),
        )
        .await?;

        // operator レビューが無効（既定）のままでは編集できない（optional の明示的有効化）。
        let edit = RiskSignalMetadataEdit {
            category: Some(SafetyCategory::Nsfw),
            severity: Some(Severity::Low),
            confidence: Some(20),
            expires_at: None,
        };
        assert!(
            edit_risk_signal_detection_metadata(&pool, &stored.id, &edit, false)
                .await
                .is_err(),
            "operator review must be explicitly enabled"
        );

        // 有効化すると検知メタデータを直接編集できる（node-local advisory の是正。
        // user canonical state は変更しない = risk_signals 行の更新のみ）。
        let edited = edit_risk_signal_detection_metadata(&pool, &stored.id, &edit, true).await?;
        assert_eq!(edited.signal.category, SafetyCategory::Nsfw);
        assert_eq!(edited.signal.severity, Severity::Low);
        assert_eq!(edited.signal.confidence, Some(20));

        // #700: 個別編集経路(cn-cli もここを通る)でも不正値は保存前に拒否される。
        let invalid_expiry = RiskSignalMetadataEdit {
            expires_at: Some("not-a-timestamp".to_string()),
            ..RiskSignalMetadataEdit::default()
        };
        assert!(
            edit_risk_signal_detection_metadata(&pool, &stored.id, &invalid_expiry, true)
                .await
                .is_err(),
            "RFC 3339 でない有効期限は拒否する"
        );
        let invalid_confidence = RiskSignalCorrection {
            confidence: Some(255),
            ..RiskSignalCorrection::default()
        };
        assert!(
            reissue_corrected_risk_signal(&pool, &stored.id, &invalid_confidence, NOW, true)
                .await
                .is_err(),
            "範囲外の確信度は拒否する"
        );
        let unchanged = get_risk_signal(&pool, &stored.id).await?.expect("exists");
        assert_eq!(unchanged.signal.expires_at, None);
        assert_eq!(unchanged.signal.confidence, Some(20));

        // 訂正 signal の再発行: 旧 signal は失効し、訂正版が同じ issuer で新規発行される。
        let correction = RiskSignalCorrection {
            category: Some(SafetyCategory::Spam),
            severity: None,
            confidence: Some(10),
            visibility: None,
        };
        let reissued =
            reissue_corrected_risk_signal(&pool, &stored.id, &correction, NOW, true).await?;
        assert_ne!(reissued.id, stored.id);
        assert_eq!(reissued.issuer_node_id, ISSUER);
        assert_eq!(reissued.signal.category, SafetyCategory::Spam);
        assert_eq!(reissued.signal.confidence, Some(10));
        assert!(reissued.signal.expires_at.is_none());

        let old = get_risk_signal(&pool, &stored.id).await?.expect("exists");
        assert_eq!(old.signal.expires_at.as_deref(), Some(NOW));

        // 失効した旧 signal は配布からも trust 入力からも消える（NOW より後で判定）。
        let later = "2026-07-30T10:00:00Z";
        let distributable = list_distributable_risk_signals(
            &pool,
            DistributionAudience::SubscribedNodes,
            later,
            50,
            0,
        )
        .await?;
        assert!(distributable.iter().all(|s| s.id != stored.id));

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

/// 構成変更後の再 scan（scanner 経路）。operator の訂正値と異なる値で保存を試みる。
async fn rescan(
    pool: &sqlx::PgPool,
    target_id: &str,
    category: SafetyCategory,
) -> Result<PersistedRiskSignal> {
    let mut signal = suspected_signal(target_id, category);
    signal.severity = Severity::High;
    signal.confidence = Some(99);
    signal.visibility = Visibility::Public;
    persist_risk_signal_deduplicated(pool, ISSUER, &signal, None).await
}

/// #1058 AC-1 / AC-2: cn-cli の編集・再発行で operator が確定した値は、再 scan の集約更新で
/// 上書きされず、元の鍵での新規 insert も起きない。
#[tokio::test]
async fn operator_adjusted_signal_survives_rescan() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-core safety appeals integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_core_operator_rescan").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let nsfw = SafetyCategory::Nsfw;

        // 1. 同じ category の編集: 値が再 scan で戻らない。
        let stored =
            persist_risk_signal(&pool, ISSUER, &suspected_signal("edit-same", nsfw)).await?;
        let edit = RiskSignalMetadataEdit {
            severity: Some(Severity::Low),
            confidence: Some(20),
            ..RiskSignalMetadataEdit::default()
        };
        let edited = edit_risk_signal_detection_metadata(&pool, &stored.id, &edit, true).await?;
        let again = rescan(&pool, "edit-same", nsfw).await?;
        assert!(!again.newly_created);
        assert_eq!(again.stored.id, stored.id);
        let after = get_risk_signal(&pool, &stored.id).await?.expect("exists");
        assert_eq!(after.signal.severity, Severity::Low);
        assert_eq!(after.signal.confidence, Some(20));
        assert_eq!(after.signal.visibility, Visibility::SubscribedNodes);
        assert_eq!(after.signal.expires_at, None);
        assert_eq!(count_signals(&pool, "edit-same").await?, 1);
        assert!(edited.operator_adjusted_at.is_some());
        assert_eq!(after.operator_adjusted_at, edited.operator_adjusted_at);

        // 2. category を変える編集: 元の category の再 scan でも新しい行を作らない。
        let stored =
            persist_risk_signal(&pool, ISSUER, &suspected_signal("edit-category", nsfw)).await?;
        let edit = RiskSignalMetadataEdit {
            category: Some(SafetyCategory::Spam),
            confidence: Some(15),
            ..RiskSignalMetadataEdit::default()
        };
        edit_risk_signal_detection_metadata(&pool, &stored.id, &edit, true).await?;
        let again = rescan(&pool, "edit-category", nsfw).await?;
        assert!(!again.newly_created);
        assert_eq!(again.stored.id, stored.id);
        // 訂正後の category で再 scan されても値は変わらない。
        let again = rescan(&pool, "edit-category", SafetyCategory::Spam).await?;
        assert!(!again.newly_created);
        let after = get_risk_signal(&pool, &stored.id).await?.expect("exists");
        assert_eq!(after.signal.category, SafetyCategory::Spam);
        assert_eq!(after.signal.confidence, Some(15));
        assert_eq!(after.signal.severity, Severity::Critical);
        assert_eq!(count_signals(&pool, "edit-category").await?, 1);

        // 3. 期限を付ける編集: 失効後も再 scan で新しい行を作らず、期限も変えない。
        let stored =
            persist_risk_signal(&pool, ISSUER, &suspected_signal("edit-expiry", nsfw)).await?;
        let edit = RiskSignalMetadataEdit {
            expires_at: Some(NOW.to_string()),
            ..RiskSignalMetadataEdit::default()
        };
        edit_risk_signal_detection_metadata(&pool, &stored.id, &edit, true).await?;
        let again = rescan(&pool, "edit-expiry", nsfw).await?;
        assert!(!again.newly_created);
        assert_eq!(again.stored.id, stored.id);
        let after = get_risk_signal(&pool, &stored.id).await?.expect("exists");
        assert_eq!(after.signal.expires_at.as_deref(), Some(NOW));
        assert_eq!(after.signal.confidence, Some(90));
        assert_eq!(count_signals(&pool, "edit-expiry").await?, 1);

        // 4. 同じ category の再発行: 訂正版の値が再 scan で戻らない。
        let stored =
            persist_risk_signal(&pool, ISSUER, &suspected_signal("reissue-same", nsfw)).await?;
        let correction = RiskSignalCorrection {
            confidence: Some(20),
            ..RiskSignalCorrection::default()
        };
        let reissued =
            reissue_corrected_risk_signal(&pool, &stored.id, &correction, NOW, true).await?;
        let again = rescan(&pool, "reissue-same", nsfw).await?;
        assert!(!again.newly_created);
        assert_eq!(again.stored.id, reissued.id);
        let after = get_risk_signal(&pool, &reissued.id).await?.expect("exists");
        assert_eq!(after.signal.confidence, Some(20));
        assert_eq!(after.signal.severity, Severity::Critical);
        assert_eq!(after.signal.visibility, Visibility::SubscribedNodes);
        assert_eq!(after.signal.expires_at, None);
        assert_eq!(count_signals(&pool, "reissue-same").await?, 2);
        assert!(reissued.operator_adjusted_at.is_some());

        // 訂正済みの行も再発行でき、新しい訂正版が作られる（scanner の抑止に掛からない）。
        let correction = RiskSignalCorrection {
            confidence: Some(5),
            ..RiskSignalCorrection::default()
        };
        let second =
            reissue_corrected_risk_signal(&pool, &reissued.id, &correction, NOW, true).await?;
        assert_ne!(second.id, reissued.id);
        assert_eq!(second.signal.confidence, Some(5));
        assert!(second.operator_adjusted_at.is_some());
        assert_eq!(count_signals(&pool, "reissue-same").await?, 3);

        // 5. category を変える再発行: 元の category の再 scan でも新しい行を作らない。
        let stored =
            persist_risk_signal(&pool, ISSUER, &suspected_signal("reissue-category", nsfw)).await?;
        let correction = RiskSignalCorrection {
            category: Some(SafetyCategory::Spam),
            ..RiskSignalCorrection::default()
        };
        let reissued =
            reissue_corrected_risk_signal(&pool, &stored.id, &correction, NOW, true).await?;
        let again = rescan(&pool, "reissue-category", nsfw).await?;
        assert!(!again.newly_created);
        assert_eq!(count_signals(&pool, "reissue-category").await?, 2);
        let after = get_risk_signal(&pool, &reissued.id).await?.expect("exists");
        assert_eq!(after.signal.category, SafetyCategory::Spam);
        assert_eq!(after.signal.confidence, Some(90));

        // INVAR-1: 訂正されていない行は #1050 の集約更新を維持する。別 category の新しい判定も
        // 訂正の抑止に巻き込まれず新規行になる。
        let untouched =
            persist_risk_signal(&pool, ISSUER, &suspected_signal("untouched", nsfw)).await?;
        assert!(untouched.operator_adjusted_at.is_none());
        let again = rescan(&pool, "untouched", nsfw).await?;
        assert!(!again.newly_created);
        assert_eq!(again.stored.signal.confidence, Some(99));
        let csam = rescan(&pool, "edit-category", SafetyCategory::Csam).await?;
        assert!(
            csam.newly_created,
            "訂正されていない category の判定は抑止しない"
        );

        Ok::<(), anyhow::Error>(())
    }
    .await;
    database.cleanup().await?;
    result
}
