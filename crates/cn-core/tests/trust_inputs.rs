//! trust / relation reads への risk signal 供給契約の contract テスト（#406 → #415 → #1702）。DB 不要。
//!
//! ADR 0026 §2.7（category による絶対 / 相対振り分け）・§6.2（appeal 状態の同伴）を、永続化された
//! risk signal 1 行を trust 入力にする純関数 `trust_risk_input` に対して固定する。失効した行・読めない
//! 失効時刻の行を集計と basis から外す規則は、集計の trigger が持つ（`trust_totals.rs` の結合試験）。

use chrono::Utc;

use kukuri_cn_core::{StoredRiskSignal, trust_risk_input};
use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal, Severity, Visibility,
};
use kukuri_cn_trust::{TrustComponentKind, TrustRiskInput, trust_component_for};

fn stored_signal(
    id: &str,
    category: SafetyCategory,
    appeal_status: Option<AppealStatus>,
) -> StoredRiskSignal {
    StoredRiskSignal {
        id: id.to_string(),
        issuer_node_id: "issuer-node".to_string(),
        signal: SafetyRiskSignal {
            target: RiskSignalTarget::UserPubkey,
            target_id: "pubkey-1".to_string(),
            category,
            severity: Severity::High,
            basis: Basis::KnownHashMatch,
            confidence: Some(97),
            visibility: Visibility::Local,
            expires_at: None,
            appeal_status,
        },
        persisted_at: Utc::now(),
        operator_adjusted_at: None,
        operator_origin_category: None,
    }
}

fn inputs(signals: &[StoredRiskSignal]) -> Vec<TrustRiskInput> {
    signals.iter().map(trust_risk_input).collect()
}

/// #1058 AC-3: operator が値を確定した signal の印は trust 入力へそのまま運ばれる。
#[test]
fn operator_adjustment_is_carried_to_trust_inputs() {
    let adjusted_at = Utc::now();
    let mut adjusted = stored_signal("sig-adjusted", SafetyCategory::Spam, None);
    adjusted.operator_adjusted_at = Some(adjusted_at);
    adjusted.operator_origin_category = Some(SafetyCategory::Nsfw);
    let plain = stored_signal("sig-plain", SafetyCategory::Spam, None);
    let inputs = inputs(&[adjusted, plain]);
    assert_eq!(inputs[0].operator_adjusted_at, Some(adjusted_at));
    assert_eq!(inputs[1].operator_adjusted_at, None);
}

// --- 絶対 / 相対振り分け（ADR 0026 §2.7） ---

#[test]
fn csam_risk_signal_feeds_absolute_component() {
    // critical safety（CSAM / CSE / grooming）は絶対成分。relation 非依存・report-bomb 不動。
    for category in [
        SafetyCategory::Csam,
        SafetyCategory::Cse,
        SafetyCategory::Grooming,
    ] {
        assert_eq!(trust_component_for(category), TrustComponentKind::Absolute);
    }
    let input = trust_risk_input(&stored_signal("sig-1", SafetyCategory::Csam, None));
    assert_eq!(input.component, TrustComponentKind::Absolute);
}

#[test]
fn general_moderation_signal_feeds_relative_component() {
    // nsfw / spam 等の文化圏依存の指標は相対成分。
    for category in [
        SafetyCategory::Nsfw,
        SafetyCategory::Spam,
        SafetyCategory::Malware,
        SafetyCategory::Phishing,
    ] {
        assert_eq!(trust_component_for(category), TrustComponentKind::Relative);
    }
    let inputs = inputs(&[
        stored_signal("sig-1", SafetyCategory::Nsfw, None),
        stored_signal("sig-2", SafetyCategory::Csam, None),
    ]);
    assert_eq!(inputs[0].component, TrustComponentKind::Relative);
    assert_eq!(inputs[1].component, TrustComponentKind::Absolute);
}

// --- ADR 0028 contract: 非決定論的（VLM）suspected の trust 振り分け（#420） ---

/// VLM scan 由来の suspected signal（basis = ClassifierScore）を模す。
fn classifier_signal(id: &str, category: SafetyCategory) -> StoredRiskSignal {
    let mut stored = stored_signal(id, category, None);
    stored.signal.basis = Basis::ClassifierScore;
    stored.signal.severity = if category.is_critical_safety() {
        Severity::Critical
    } else {
        Severity::High
    };
    stored
}

#[test]
fn critical_suspected_feeds_trust_absolute_component() {
    // ADR 0028 §2.5 / ADR 0026 §2.3: critical（CSAM / CSE / grooming）の suspected
    // （厳格非決定論）は relation で薄まらない絶対成分に入る。
    for input in inputs(&[
        classifier_signal("sig-csam", SafetyCategory::Csam),
        classifier_signal("sig-cse", SafetyCategory::Cse),
        classifier_signal("sig-grooming", SafetyCategory::Grooming),
    ]) {
        assert_eq!(input.component, TrustComponentKind::Absolute);
        // 断定ラベルではなく suspected（ClassifierScore）の根拠を同伴する。
        assert_eq!(input.basis, Basis::ClassifierScore);
    }
}

#[test]
fn spam_malware_phishing_feed_trust_relative_component() {
    // ADR 0028 §2.5 / ADR 0026 §2.3 / §7.1: spam / malware / phishing の suspected は相対成分に入る
    // （nsfw / objectionable は ADR 0026 §7 で advisory-only = 寄与 0）。
    for input in inputs(&[
        classifier_signal("sig-malware", SafetyCategory::Malware),
        classifier_signal("sig-spam", SafetyCategory::Spam),
        classifier_signal("sig-phishing", SafetyCategory::Phishing),
    ]) {
        assert_eq!(input.component, TrustComponentKind::Relative);
        assert_eq!(input.basis, Basis::ClassifierScore);
    }
}

// --- appeal 状態の同伴（ADR 0026 §6.2。Disputed = pending / Cleared = accepted） ---

#[test]
fn appeal_status_is_carried_for_explanation() {
    // Cleared は寄与 0 の説明用 basis として、Disputed は寄与据え置きのまま、状態を同伴する。
    let inputs = inputs(&[
        stored_signal(
            "sig-cleared",
            SafetyCategory::Csam,
            Some(AppealStatus::Cleared),
        ),
        stored_signal(
            "sig-disputed",
            SafetyCategory::Nsfw,
            Some(AppealStatus::Disputed),
        ),
        stored_signal("sig-kept", SafetyCategory::Csam, None),
    ]);
    assert_eq!(inputs[0].appeal_status, AppealStatus::Cleared);
    assert_eq!(inputs[1].appeal_status, AppealStatus::Disputed);
    assert_eq!(inputs[2].appeal_status, AppealStatus::None);
}

// --- 根拠つき advisory（断定ラベル化しない） ---

#[test]
fn trust_inputs_carry_basis_confidence_visibility() {
    let input = trust_risk_input(&stored_signal("sig-1", SafetyCategory::Csam, None));
    // 消費側が issuer / 根拠 / 有効期限を説明できるよう、根拠フィールドを欠落なく同伴する。
    assert_eq!(input.issuer_node_id, "issuer-node");
    assert_eq!(input.category, SafetyCategory::Csam);
    assert_eq!(input.severity, Severity::High);
    assert_eq!(input.basis, Basis::KnownHashMatch);
    assert_eq!(input.confidence, Some(97));
    assert_eq!(input.visibility, Visibility::Local);
    assert_eq!(input.appeal_status, AppealStatus::None);
    assert_eq!(input.expires_at, None);
}

#[test]
fn general_advisory_contributes_zero_to_trust() {
    // ADR 0026 §7: nsfw / objectionable は `Relative` の入力として basis / confidence / appeal を
    // 同伴したまま残り（利用者向け read で状態を説明できる）、寄与は常に 0（集計にも 0 で入る）。
    let mut nsfw = classifier_signal("sig-nsfw", SafetyCategory::Nsfw);
    nsfw.signal.severity = Severity::Low;
    let mut objectionable = classifier_signal("sig-objectionable", SafetyCategory::Objectionable);
    objectionable.signal.severity = Severity::Low;
    for input in inputs(&[nsfw, objectionable]) {
        assert_eq!(input.component, TrustComponentKind::Relative);
        assert!(input.category.is_advisory_only());
        assert_eq!(input.basis, Basis::ClassifierScore);
        assert_eq!(input.severity, Severity::Low);
        assert_eq!(input.appeal_status, AppealStatus::None);
        assert_eq!(kukuri_cn_trust::signal_contribution(&input), 0.0);
    }
}
