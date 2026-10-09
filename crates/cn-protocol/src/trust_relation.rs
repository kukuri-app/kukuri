//! trust / relation の共有 wire 契約。

pub use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, Severity, Visibility,
};
use serde::{Deserialize, Serialize};

/// risk signal category の trust 成分振り分け先。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TrustComponentKind {
    Absolute,
    Relative,
}

/// 寄与 signal 1 件の説明（basis entry）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct TrustBasisEntry {
    pub signal_id: String,
    pub issuer_node_id: String,
    pub target: RiskSignalTarget,
    pub target_id: String,
    pub component: TrustComponentKind,
    pub category: SafetyCategory,
    pub severity: Severity,
    pub basis: Basis,
    pub confidence: Option<u8>,
    pub visibility: Visibility,
    pub appeal_status: AppealStatus,
    pub expires_at: Option<String>,
    /// operator が審査・運用是正で値を確定した時刻（RFC3339、#1058）。`None` は scanner 由来の
    /// 未訂正判定。旧 node の応答では欠落する。
    #[serde(default)]
    pub operator_adjusted_at: Option<String>,
    pub raw_contribution: f64,
    pub decay_factor: f64,
    pub relation_weight: f64,
    pub contribution: f64,
}

/// trust read の応答形（node-local advisory）。
///
/// `trust` は CN が合算した利用者向けの信頼値 S（ADR 0026 §8.1）。`absolute` / `relative` /
/// `w_abs_applied` / `basis` は閲覧者に依存しない trust 絶対値 T の内訳（説明用）であり、
/// クライアントはこれらから評価値を再合成しない。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct TrustReadView {
    pub target_id: String,
    pub absolute: f64,
    pub relative: f64,
    pub trust: f64,
    pub w_abs_applied: f64,
    pub computed_at: String,
    pub basis: Vec<TrustBasisEntry>,
    /// 評価の版・期限・表示 policy（#1061）。旧 node の応答では欠落し、クライアントは未評価として扱う。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation: Option<TrustEvaluation>,
    /// basis の続きを取る cursor（`?cursor=`）。最後のページと旧 node の応答では欠落する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis_next_cursor: Option<String>,
}

/// 信頼値が負になった理由の種類（ADR 0026 §8.4）。数値・observer は含めない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TrustEvaluationReason {
    /// trust 絶対値 T が負（risk signal 由来）。
    RiskSignals,
    /// relation 値 R が負（閲覧者と関係の深いユーザー群のブロック / ミュート）。
    RelatedUsersBlockOrMute,
}

/// CN 側で合算した評価の版・期限と表示 policy（ADR 0026 §8.4）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TrustEvaluation {
    /// 合算・表示 policy の parameter から決まる識別子。
    pub policy_version: String,
    /// T に寄与する入力の digest。
    pub trust_version: String,
    /// relation snapshot と、評価に使った対象への観測の digest の組（観測が無ければ `0`）。
    pub relation_version: String,
    pub computed_at: String,
    /// クライアントが結果を再利用してよい期限（RFC3339）。
    pub expires_at: String,
    /// node-local な表示 policy による非表示推奨（`trust <= hide_threshold`）。
    pub hide_recommended: bool,
    pub reasons: Vec<TrustEvaluationReason>,
}

/// 一括評価の要求（`POST /v1/trust/evaluations`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TrustEvaluationsRequest {
    pub targets: Vec<String>,
}

/// 一括評価の 1 件。`trust` は合算済みの S。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TrustEvaluationItem {
    pub target_pubkey: String,
    pub trust: f64,
    pub evaluation: TrustEvaluation,
}

/// 一括評価の応答。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TrustEvaluationsResponse {
    pub viewer_pubkey: String,
    pub evaluations: Vec<TrustEvaluationItem>,
}

/// 一括評価で一度に指定できる対象数の上限。
pub const TRUST_EVALUATIONS_MAX_TARGETS: usize = 100;

/// ブロック / ミュート観測の提供（`POST /v1/trust/observations`、ADR 0026 §8.3）。
///
/// `envelopes` は observer 本人が署名した `block-edge` / `mute-observation` envelope。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustObservationsSubmitRequest {
    pub envelopes: Vec<kukuri_core::KukuriEnvelope>,
}

/// 観測提供の結果。`stored` は保存済みより新しく置き換えた件数、`ignored` は古い・重複のため
/// 変更しなかった件数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TrustObservationsSubmitResponse {
    pub stored: u32,
    pub ignored: u32,
}

/// 観測の削除と提供同意の取消（`DELETE /v1/trust/observations`）の結果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TrustObservationsRevokeResponse {
    pub deleted: u64,
}

/// 一度に提供できる観測 envelope 数の上限。
pub const TRUST_OBSERVATIONS_MAX_ENVELOPES: usize = 100;

/// 観測提供の同意に使う任意文書の slug（ADR 0026 §8.5）。
pub const TRUST_OBSERVATION_SHARING_POLICY_SLUG: &str = "trust_observation_sharing";

/// viewer を含む per-user trust read wire 応答。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TrustUserReadResponse {
    pub viewer_pubkey: String,
    #[serde(flatten)]
    #[cfg_attr(feature = "ts", ts(flatten))]
    pub view: TrustReadView,
}

/// proximity の feature ごとの根拠。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProximityBasisEntry {
    pub feature: String,
    pub value: f64,
    pub weight: f64,
    pub contribution: f64,
}

/// pairwise cluster proximity（根拠つき）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Proximity {
    pub score: f64,
    pub basis: Vec<ProximityBasisEntry>,
}

/// viewer と target を含む pairwise relation read wire 応答。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RelationReadResponse {
    pub viewer_pubkey: String,
    pub target_pubkey: String,
    #[serde(flatten)]
    #[cfg_attr(feature = "ts", ts(flatten))]
    pub proximity: Proximity,
}

/// viewer の近接近傍一覧。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RelationNeighborsResponse {
    pub viewer_pubkey: String,
    pub neighbors: Vec<String>,
}

/// 本人向け distance opt-out 状態。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct RelationOptoutResponse {
    pub pubkey: String,
    pub opted_out: bool,
    pub opted_out_at: Option<String>,
    pub min_proximity: f64,
}
