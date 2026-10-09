//! cross-node pull 開示の組み立て（ADR 0026 §6.3 / §10, Issue #415 / #1702）の contract テスト。DB 不要。
//!
//! 開示できる signal の判定（confirmed の絶対成分で `Local` でなく cleared でない）は集計の trigger が
//! 行ごとに持ち、`cn-core` の結合試験（`trust_totals.rs`）が同じ contract 名で固定する。ここでは
//! 開示分の集計と 1 ページから応答を組み立てる部分を固定する。

use chrono::{DateTime, Duration, Utc};

use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, Severity, Visibility,
};
use kukuri_cn_trust::{
    PullAudience, TrustComponentKind, TrustRiskInput, TrustTotals, cross_node_trust_disclosure,
};

#[allow(clippy::unwrap_used)] // test fixture helper
fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-07-02T09:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn confirmed(id: &str, visibility: Visibility) -> TrustRiskInput {
    TrustRiskInput {
        signal_id: id.to_string(),
        issuer_node_id: "issuer-node".to_string(),
        target: RiskSignalTarget::UserPubkey,
        target_id: "pubkey-1".to_string(),
        component: TrustComponentKind::Absolute,
        category: SafetyCategory::Csam,
        severity: Severity::Critical,
        basis: Basis::KnownHashMatch,
        confidence: Some(100),
        visibility,
        appeal_status: AppealStatus::None,
        expires_at: Some("2026-12-31T00:00:00Z".to_string()),
        persisted_at: now() - Duration::days(400),
        operator_adjusted_at: None,
    }
}

#[test]
fn disclosure_uses_only_the_audience_share_of_the_totals() {
    // 開示値は開示分の集計だけから求める（Local・suspected・相対成分の存在が値から漏れない）。
    let totals = TrustTotals {
        absolute_units: 2_000,
        relative_units: 5_000.0,
        relative_at: now(),
        half_life_days: 30.0,
        disclosed_public_units: 400,
        disclosed_subscribed_units: 300,
        digest: 7,
    };
    let page = vec![confirmed("abs-public", Visibility::Public)];
    let public =
        cross_node_trust_disclosure("pubkey-1", &totals, &page, PullAudience::Public, now());
    assert!((public.absolute - -0.4).abs() < 1e-12);
    let subscribed = cross_node_trust_disclosure(
        "pubkey-1",
        &totals,
        &page,
        PullAudience::SubscribedNodes,
        now(),
    );
    assert!((subscribed.absolute - -0.7).abs() < 1e-12);

    // 開示する行が無ければ 0（相対成分は開示口を持たない）。
    let none = cross_node_trust_disclosure(
        "pubkey-1",
        &TrustTotals {
            disclosed_public_units: 0,
            disclosed_subscribed_units: 0,
            ..totals
        },
        &[],
        PullAudience::SubscribedNodes,
        now(),
    );
    assert_eq!(none.absolute, 0.0);
    assert!(none.basis.is_empty());
}

#[test]
fn disclosure_basis_explains_each_confirmed_signal_without_decay() {
    let page = vec![confirmed("abs-public", Visibility::Public)];
    let disclosure = cross_node_trust_disclosure(
        "pubkey-1",
        &TrustTotals::default(),
        &page,
        PullAudience::Public,
        now(),
    );
    // 根拠（issuer / basis / confidence / expiry）を同伴し、絶対成分なので時間で薄めない。
    let entry = &disclosure.basis[0];
    assert_eq!(entry.signal_id, "abs-public");
    assert_eq!(entry.issuer_node_id, "issuer-node");
    assert_eq!(entry.basis, Basis::KnownHashMatch);
    assert_eq!(entry.confidence, Some(100));
    assert_eq!(entry.expires_at, Some("2026-12-31T00:00:00Z".to_string()));
    assert_eq!(entry.decay_factor, 1.0);
    assert_eq!(entry.contribution, -1.0);
    assert_eq!(disclosure.basis_next_cursor, None);
}
