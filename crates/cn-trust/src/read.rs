//! 根拠つき trust read view（ADR 0026 §2.5 / trust-semantics §4）。
//!
//! read は断定ラベルではなく **根拠つき advisory**: 絶対 / 相対成分を分離して返し
//! （`trust_is_not_single_absolute_scalar` / `trust_separates_absolute_and_relative_indicators`）、
//! 寄与 signal ごとに issuer / basis / confidence / visibility / expiry / appeal と
//! 実効寄与（decay 込み）を説明できる形にする（`trust_read_is_explainable_with_basis`）。
//!
//! 成分は対象ごとの集計（[`TrustTotals`]）から求め、basis は 1 ページ分だけを並べる（ADR 0026 §10）。

use chrono::{DateTime, SecondsFormat, Utc};
pub use kukuri_cn_protocol::{TrustBasisEntry, TrustReadView};

use crate::inputs::{TrustComponentKind, TrustRiskInput};
use crate::params::TrustParams;
use crate::score::{compose_trust, contributes, decay_factor, signal_contribution};
use crate::totals::TrustTotals;

fn basis_entry(input: &TrustRiskInput, decay: f64) -> TrustBasisEntry {
    let raw = signal_contribution(input);
    TrustBasisEntry {
        signal_id: input.signal_id.clone(),
        issuer_node_id: input.issuer_node_id.clone(),
        target: input.target,
        target_id: input.target_id.clone(),
        component: input.component,
        category: input.category,
        severity: input.severity,
        basis: input.basis,
        confidence: input.confidence,
        visibility: input.visibility,
        appeal_status: input.appeal_status,
        expires_at: input.expires_at.clone(),
        operator_adjusted_at: input
            .operator_adjusted_at
            .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true)),
        raw_contribution: raw,
        decay_factor: decay,
        // 閲覧者別の relation は T に入れない（§8 の R が担う）。
        relation_weight: 1.0,
        contribution: if contributes(input) { raw * decay } else { 0.0 },
    }
}

/// basis の 1 ページ分の説明。絶対成分は減衰させず（decay 1.0）、相対成分は集計と同じ半減期で減衰させる。
/// `Cleared` は寄与 0 の説明用 basis として残す（§6.2）。
pub fn trust_basis(
    page: &[TrustRiskInput],
    now: DateTime<Utc>,
    half_life_days: f64,
) -> Vec<TrustBasisEntry> {
    page.iter()
        .map(|input| {
            let decay = match input.component {
                TrustComponentKind::Absolute => 1.0,
                TrustComponentKind::Relative => {
                    decay_factor(input.persisted_at, now, half_life_days)
                }
            };
            basis_entry(input, decay)
        })
        .collect()
}

/// 集計と basis の 1 ページから根拠つき trust read view を組み立てる。
///
/// 戻り値の `trust` は閲覧者に依存しない trust 絶対値 T（ADR 0026 §8.1）。利用者向けの S は
/// [`crate::apply_viewer_relation`] で合算する。成分は §6.2 の式（[`compose_trust`]）で合成する。
pub fn build_trust_read(
    target_id: &str,
    totals: &TrustTotals,
    page: &[TrustRiskInput],
    now: DateTime<Utc>,
    params: &TrustParams,
) -> TrustReadView {
    let absolute = totals.absolute();
    let relative = totals.relative(now);
    let composed = compose_trust(params, absolute, relative);
    TrustReadView {
        target_id: target_id.to_string(),
        absolute,
        relative,
        trust: composed.trust,
        w_abs_applied: composed.w_abs_applied,
        computed_at: now.to_rfc3339(),
        basis: trust_basis(page, now, totals.half_life_days),
        // 閲覧者別の合算（S）と評価 metadata は `apply_viewer_relation` が付ける。
        evaluation: None,
        basis_next_cursor: None,
    }
}
