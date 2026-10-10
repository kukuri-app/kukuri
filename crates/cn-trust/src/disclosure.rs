//! cross-node pull への開示境界（ADR 0026 §6.3 Decision, #415）。
//!
//! read は pull 型: 他 node がこの CN に問い合わせると、この CN の視点の signal が返る。
//! cross-node pull に対しては **confirmed（basis = known-hash / provider-verdict）な絶対成分
//! のみ**を根拠つき（issuer / basis / confidence / expiry）で応答する。
//!
//! - **相対成分は返さない**（文化依存の指標を node 外へ拡散しない）。
//! - **suspected（classifier / local-policy 由来）は返さない**（誤検知を拡散しない）。
//! - `visibility` は pull へのアクセス範囲: `Local` は決して返さず（既定）、
//!   `SubscribedNodes` は subscriber と認証できた要求者のみ、`Public` は誰でも。
//! - relation は本 module に開示口が **存在しない**（`Local` 固定、
//!   `relation_defaults_local_and_not_cross_node_pullable` の構造的保証）。
//!
//! 開示できる signal の判定は、対象ごとの集計を作る `cn-core` の trigger が行ごとに持つ
//! （`trust_target_signals.disclosure`、ADR 0026 §10）。本 module は開示分の集計と 1 ページを並べる。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::inputs::TrustRiskInput;
use crate::read::{TrustBasisEntry, trust_basis};
use crate::totals::TrustTotals;

/// cross-node pull の要求者区分。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullAudience {
    /// 匿名 / 未認証の要求者（`Public` visibility のみ見える）。
    Public,
    /// この CN の subscriber として認証済みの要求者（`SubscribedNodes` 以上が見える）。
    SubscribedNodes,
}

/// cross-node pull への応答（confirmed 絶対成分のみ、根拠つき）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossNodeTrustDisclosure {
    pub target_id: String,
    /// 開示できる signal だけから求めた絶対成分（`[-1, 1]`）。開示しない signal の存在が値から
    /// 推測できないよう、全量からではなく開示分の集計から求める。
    pub absolute: f64,
    /// 計算時刻（RFC3339）。
    pub computed_at: String,
    /// 開示した signal の根拠の 1 ページ（新しい順。issuer / basis / confidence / expiry を同伴）。
    pub basis: Vec<TrustBasisEntry>,
    /// basis の続きを取る cursor（`?cursor=`）。最後のページでは欠落する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis_next_cursor: Option<String>,
}

/// cross-node pull への開示を組み立てる。`page` は `audience` へ開示できる signal の 1 ページ。
pub fn cross_node_trust_disclosure(
    target_id: &str,
    totals: &TrustTotals,
    page: &[TrustRiskInput],
    audience: PullAudience,
    now: DateTime<Utc>,
) -> CrossNodeTrustDisclosure {
    CrossNodeTrustDisclosure {
        target_id: target_id.to_string(),
        absolute: totals.disclosed_absolute(audience),
        computed_at: now.to_rfc3339(),
        // 開示するのは絶対成分だけなので減衰しない（半減期は使われない）。
        basis: trust_basis(page, now, totals.half_life_days),
        basis_next_cursor: None,
    }
}
