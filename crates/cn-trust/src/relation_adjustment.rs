//! 閲覧者別の relation 値 R と、利用者向けの信頼値 S の合成（ADR 0026 §8、#1061）。
//!
//! - T（trust 絶対値）は [`crate::build_trust_read`] の `trust`。閲覧者・観測に依存しない。
//! - R は、閲覧者 A と関係の深いユーザー U による対象 B へのブロック / ミュート観測から求める。
//!   重み `w(A,U)` は relation graph の proximity だけから求め、R / S を入力に戻さない。
//! - S = clamp(-1, 1, T + R)。クライアントの表示判断に使う唯一の評価値。
//!
//! すべて純関数で、入力の順序によらず同じ結果を返す（`relation_observation_is_idempotent_and_order_independent`）。

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
pub use kukuri_cn_protocol::{TrustEvaluation, TrustEvaluationReason, TrustReadView};

use crate::params::TrustParams;
use crate::score::{clamp_unit, decay_factor};

/// 観測の種別（kukuri-core の `TrustObservationKind` と対応。DB 層の文字列表現も同じ）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationObservationKind {
    Block,
    Mute,
}

/// 評価に使う active な観測 1 件（observer → 対象 B）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationObservation {
    pub observer_pubkey: String,
    pub kind: RelationObservationKind,
    pub observed_at: DateTime<Utc>,
}

/// 閲覧者 A から見た対象 B の relation 値。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelationAdjustment {
    /// R。`[-1, 0]`。
    pub value: f64,
    /// 集約した減点（`[0, 1]`、倍率適用前）。
    pub penalty: f64,
}

impl RelationAdjustment {
    pub const NONE: Self = Self {
        value: 0.0,
        penalty: 0.0,
    };
}

fn strength(params: &TrustParams, kind: RelationObservationKind) -> f64 {
    match kind {
        RelationObservationKind::Block => params.block_strength,
        RelationObservationKind::Mute => params.mute_strength,
    }
}

/// 閲覧者 A から見た B の relation 値 R を求める（ADR 0026 §8.2）。
///
/// - `observations`: B に対する active な観測（失効・取消・同意外は呼出側で除外済み）。
/// - `proximities`: A から各 observer への proximity（`[0, 1]`）。無い observer は 0。
///   observer == viewer の場合は proximity によらず重み 1。
pub fn compose_relation_adjustment(
    viewer_pubkey: &str,
    observations: &[RelationObservation],
    proximities: &BTreeMap<String, f64>,
    now: DateTime<Utc>,
    params: &TrustParams,
) -> RelationAdjustment {
    // 同一 observer の block / mute は強い方の寄与だけを採る。observer ごとに 1 値へ畳むので、
    // 再送・複数端末・両操作で増幅しない。
    let mut per_observer: BTreeMap<&str, f64> = BTreeMap::new();
    for observation in observations {
        let observer = observation.observer_pubkey.as_str();
        let weight = if observer == viewer_pubkey {
            1.0
        } else {
            proximities
                .get(observer)
                .copied()
                .filter(|value| value.is_finite())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0)
        };
        if weight < params.relation_min_weight || weight <= 0.0 {
            continue;
        }
        let decay = decay_factor(observation.observed_at, now, params.relative_half_life_days);
        let contribution = (weight * strength(params, observation.kind) * decay).clamp(0.0, 1.0);
        let slot = per_observer.entry(observer).or_insert(0.0);
        if contribution > *slot {
            *slot = contribution;
        }
    }

    // 寄与の大きい順（同点は observer 辞書順）に上位 K 件だけを noisy-OR で集約する。
    let mut contributions: Vec<(&str, f64)> = per_observer
        .into_iter()
        .filter(|(_, value)| *value > 0.0)
        .collect();
    contributions.sort_by(|(left_key, left), (right_key, right)| {
        right
            .partial_cmp(left)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left_key.cmp(right_key))
    });
    let survive: f64 = contributions
        .iter()
        .take(params.relation_top_k)
        .map(|(_, value)| 1.0 - value)
        .product();
    let penalty = (1.0 - survive).clamp(0.0, 1.0);
    if penalty <= 0.0 {
        return RelationAdjustment::NONE;
    }
    RelationAdjustment {
        value: (-(params.relation_penalty_scale * penalty)).clamp(-1.0, 0.0),
        penalty,
    }
}

/// S = clamp(-1, 1, T + R)（ADR 0026 §8.1）。
pub fn compose_viewer_trust(trust_absolute: f64, relation: f64) -> f64 {
    clamp_unit(clamp_unit(trust_absolute) + relation.clamp(-1.0, 0.0))
}

/// 入力の並びによらない digest（blake3 の先頭 16 文字）。
fn input_digest(mut parts: Vec<String>) -> String {
    parts.sort();
    let digest = blake3::hash(parts.join("\n").as_bytes()).to_hex();
    digest.as_str()[..16].to_string()
}

/// T の入力の digest（`trust_version`）。寄与する signal の識別と状態だけから決まり、
/// 時間減衰による値の変化では変わらない。
pub fn trust_version(view: &TrustReadView) -> String {
    let parts = view
        .basis
        .iter()
        .map(|entry| {
            format!(
                "{}|{:?}|{}|{}",
                entry.signal_id,
                entry.appeal_status,
                entry.operator_adjusted_at.as_deref().unwrap_or(""),
                entry.expires_at.as_deref().unwrap_or(""),
            )
        })
        .collect();
    format!("t-{}", input_digest(parts))
}

/// relation 側の版（`relation_version`）。直近で成功した relation 解析の id と、評価に使った観測の
/// digest の組（観測が無ければ `0`）。観測の集合が同じなら同じ版で、時間減衰では変わらない（#1699）。
pub fn relation_version(
    relation_snapshot_id: Option<i64>,
    observations: &[RelationObservation],
) -> String {
    let observations = if observations.is_empty() {
        "0".to_string()
    } else {
        input_digest(
            observations
                .iter()
                .map(|observation| {
                    format!(
                        "{}|{:?}|{}",
                        observation.observer_pubkey,
                        observation.kind,
                        observation.observed_at.timestamp_millis()
                    )
                })
                .collect(),
        )
    };
    match relation_snapshot_id {
        Some(snapshot) => format!("r-{snapshot}-{observations}"),
        None => format!("r-none-{observations}"),
    }
}

/// T の read view に R を合算し、S と評価 metadata を付けた利用者向け view を作る。
///
/// `trust_view` は [`crate::build_trust_read`] の出力（`trust` = T）。戻り値の `trust` は S で、
/// `absolute` / `relative` / `basis` は T の内訳のまま変えない。
pub fn apply_viewer_relation(
    mut trust_view: TrustReadView,
    adjustment: RelationAdjustment,
    relation_version: String,
    now: DateTime<Utc>,
    params: &TrustParams,
) -> TrustReadView {
    let evaluation = evaluate(&trust_view, adjustment, relation_version, now, params);
    trust_view.trust = compose_viewer_trust(trust_view.trust, adjustment.value);
    trust_view.evaluation = Some(evaluation);
    trust_view
}

fn evaluate(
    trust_view: &TrustReadView,
    adjustment: RelationAdjustment,
    relation_version: String,
    now: DateTime<Utc>,
    params: &TrustParams,
) -> TrustEvaluation {
    let trust = compose_viewer_trust(trust_view.trust, adjustment.value);
    let mut reasons = Vec::new();
    if trust_view.trust < 0.0 {
        reasons.push(TrustEvaluationReason::RiskSignals);
    }
    if adjustment.value < 0.0 {
        reasons.push(TrustEvaluationReason::RelatedUsersBlockOrMute);
    }
    let expires_at = now + Duration::seconds(i64::from(params.evaluation_ttl_seconds));
    TrustEvaluation {
        policy_version: params.policy_version(),
        trust_version: trust_version(trust_view),
        relation_version,
        computed_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        hide_recommended: !reasons.is_empty() && trust <= params.hide_threshold,
        reasons,
    }
}
