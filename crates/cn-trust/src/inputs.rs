//! trust read への入力型（#406 の供給契約が生成し、本 crate の scoring が消費する）。
//!
//! 型はここ（pure domain）に置き、永続化 risk signal からの組み立て（`trust_risk_input`）と
//! 対象ごとの集計・basis のページの読取りは `cn-core` が担う。
//! ADR 0026 §2.7 に従い、signal の category で trust の **絶対成分**（CSAM 等 critical safety。
//! relation 非依存・report-bomb 不動）と **相対成分**（spam 等）へ振り分ける。
//!
//! 断定ラベルにしない（ADR 0026 §2.5 / trust-semantics）: 入力は必ず basis / confidence /
//! visibility / expiry / appeal を同伴し、read surface が説明可能性を落とせない形にする。

use chrono::{DateTime, Utc};
pub use kukuri_cn_protocol::TrustComponentKind;
use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, Severity, Visibility,
};

/// category → trust 成分の振り分け（初期規則）。
///
/// critical safety（`SafetyCategory::is_critical_safety()` = Csam / Cse / Grooming）は絶対成分、
/// それ以外（spam / malware / phishing 等）は相対成分（ADR 0026 §2.7）。
/// nsfw / objectionable（advisory-only。ADR 0026 §7）も wire 上は `Relative` のまま basis に
/// 残るが、寄与は `signal_contribution` が常に 0 にする（評価値には入らない）。
pub fn trust_component_for(category: SafetyCategory) -> TrustComponentKind {
    if category.is_critical_safety() {
        TrustComponentKind::Absolute
    } else {
        TrustComponentKind::Relative
    }
}

/// trust / relation read への入力 1 件（根拠つき advisory）。
///
/// 断定ラベルではない。basis / confidence / visibility / expiry / appeal を必ず同伴し、
/// 消費側が issuer / 根拠 / 有効期限を説明できる状態を保つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustRiskInput {
    /// 永続化側が採番した risk signal id。
    pub signal_id: String,
    /// この signal を発行した issuer node。
    pub issuer_node_id: String,
    /// 判定元の対象種別。
    pub target: RiskSignalTarget,
    /// 判定元の対象識別子。
    pub target_id: String,
    /// 振り分け先の trust 成分。
    pub component: TrustComponentKind,
    pub category: SafetyCategory,
    pub severity: Severity,
    pub basis: Basis,
    pub confidence: Option<u8>,
    /// cross-node 開示の判定材料（§6.3。開示判定は消費側の責務）。
    pub visibility: Visibility,
    /// `Disputed`（= pending）は寄与据え置きのまま含まれる。消費側が状態を説明できるよう同伴する。
    pub appeal_status: AppealStatus,
    pub expires_at: Option<String>,
    pub persisted_at: DateTime<Utc>,
    /// operator が値を確定した時刻（#1058）。`None` は scanner 由来の未訂正判定。
    pub operator_adjusted_at: Option<DateTime<Utc>>,
}

/// 観測者つき node-local 観測の seam（ADR 0026 §2.3 相対成分の将来入力）。
///
/// 相対成分は本来「非決定論 moderation + node-local 観測を relation で重み付け」した値だが、
/// 観測者（observer）を持つ観測の producer は非決定論的 moderation（ADR 0028 / #420 系）の
/// 実装まで存在しない。本型はその接続点のみを固定する: 観測者 pubkey を持つことで、
/// 消費側が observer↔viewer / observer↔target の relation（cluster 近接度）で重み付けでき、
/// 特定 cluster からの大量観測が raw count として効かない（report-bombing 耐性、§2.3）。
///
/// 通報（`cn_admin.reports`, #370）は reporter identity を保持しないため本型の producer に
/// **ならない**（raw count を trust に入れない構造的保証）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedSignal {
    /// 観測者（relation 重み付けの対象となる pubkey）。
    pub observer_pubkey: String,
    pub category: SafetyCategory,
    pub severity: Severity,
    pub observed_at: DateTime<Utc>,
}
