//! relation opt-out「見えない」（ADR 0026 §2.6 / §6.3, #415）の Postgres integration テスト。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。
//! - `relation_visibility_choice_is_user_controlled_and_reversible`: set → clear の往復・冪等性。
//! - opt-out は信頼値の集計と basis へ影響しない（troll 判定回避の手段にしない）。

use anyhow::Result;

use kukuri_cn_core::{
    TestDatabase, clear_relation_optout, connect_postgres, filter_relation_visible,
    get_relation_optout, initialize_database, is_relation_opted_out, list_trust_basis_page,
    load_trust_totals, persist_risk_signal, set_relation_optout,
};
use kukuri_cn_safety::{
    Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal, Severity, Visibility,
};
use kukuri_cn_trust::{EdgeFeatures, FEATURE_SHARED_TOPICS, MemoryRelationStore, RelationStore};

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

const PUBKEY_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PUBKEY_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const PUBKEY_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[tokio::test]
async fn relation_visibility_choice_is_user_controlled_and_reversible() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core relation optout test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_relation_optouts").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;

        // 既定は opt-out していない（既定で隠さない, §2.6）。
        assert!(!is_relation_opted_out(&pool, PUBKEY_A).await?);

        let relation = MemoryRelationStore::new();
        relation
            .upsert_edge(
                PUBKEY_C,
                PUBKEY_A,
                &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 0.1),
            )
            .await?;
        relation
            .upsert_edge(
                PUBKEY_C,
                PUBKEY_B,
                &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 3.0),
            )
            .await?;

        // 未選択なら遠距離も自動抑制しない。
        let candidates = [PUBKEY_A.to_string(), PUBKEY_B.to_string()];
        let visible = filter_relation_visible(&pool, &relation, PUBKEY_C, &candidates, 0.5).await?;
        assert_eq!(visible, candidates);

        // set は冪等で、opted_out_at（選択時刻）を説明できる。
        set_relation_optout(&pool, PUBKEY_A).await?;
        set_relation_optout(&pool, PUBKEY_A).await?;
        assert!(is_relation_opted_out(&pool, PUBKEY_A).await?);
        assert!(get_relation_optout(&pool, PUBKEY_A).await?.is_some());

        // target側が選択したfar pairだけが落ち、close pairは残る。
        let visible = filter_relation_visible(&pool, &relation, PUBKEY_C, &candidates, 0.5).await?;
        assert_eq!(visible, vec![PUBKEY_B.to_string()]);

        // 可逆: clear で見え直す（canonical 削除ではなく node-local な選択の解除）。
        clear_relation_optout(&pool, PUBKEY_A).await?;
        clear_relation_optout(&pool, PUBKEY_A).await?; // 冪等
        assert!(!is_relation_opted_out(&pool, PUBKEY_A).await?);
        let visible = filter_relation_visible(&pool, &relation, PUBKEY_C, &candidates, 0.5).await?;
        assert_eq!(visible.len(), 2);

        // viewer側の選択でも同じfar pairを抑制し、close pairは隠さない。
        set_relation_optout(&pool, PUBKEY_C).await?;
        let visible = filter_relation_visible(&pool, &relation, PUBKEY_C, &candidates, 0.5).await?;
        assert_eq!(visible, vec![PUBKEY_B.to_string()]);

        // 不正 pubkey は拒否（黙って通さない）。
        assert!(set_relation_optout(&pool, "not-a-pubkey").await.is_err());
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn relation_optout_does_not_affect_trust_inputs() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core relation optout test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_optout_trust").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;

        persist_risk_signal(
            &pool,
            "issuer-node",
            &SafetyRiskSignal {
                target: RiskSignalTarget::UserPubkey,
                target_id: PUBKEY_A.to_string(),
                category: SafetyCategory::Spam,
                severity: Severity::Medium,
                basis: Basis::ClassifierScore,
                confidence: Some(80),
                visibility: Visibility::Local,
                expires_at: None,
                appeal_status: None,
            },
        )
        .await?;

        let targets = [PUBKEY_A.to_string()];
        let before = (
            load_trust_totals(&pool, &targets).await?,
            list_trust_basis_page(&pool, PUBKEY_A, None).await?,
        );
        assert_eq!(before.1.inputs.len(), 1);
        assert!(before.0[PUBKEY_A].relative_units > 0.0);

        // opt-out しても集計と basis は 1 件も減らない（troll 判定回避の手段にしない, §6.3）。
        set_relation_optout(&pool, PUBKEY_A).await?;
        let after = (
            load_trust_totals(&pool, &targets).await?,
            list_trust_basis_page(&pool, PUBKEY_A, None).await?,
        );
        assert_eq!(before, after);
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}
