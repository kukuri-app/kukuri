//! 対象ごとの集計から求める T の成分（ADR 0026 §10、#1702）。
//!
//! 集計は `cn-core` の `cn_safety.trust_target_totals` で、risk signal の変更を trigger が差分で足し引き
//! する。寄与は 1/1000 単位の整数（[`crate::signal_contribution`] の大きさ × 1000）で持ち、相対成分は
//! `relative_at` まで半減期 `half_life_days` で減衰させた和を、照会の時刻まで減衰させる。照会は対象の
//! 行数によらず、この 1 行だけを読む。

use chrono::{DateTime, Utc};

use crate::disclosure::PullAudience;
use crate::score::clamp_unit;

/// 集計の寄与の単位（1 寄与 = 1000 単位）。
pub const TRUST_UNITS_PER_CONTRIBUTION: f64 = 1000.0;

/// 対象 1 つの集計。risk signal が 1 行も無い対象は [`Default`]（成分 0）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrustTotals {
    /// 絶対成分の寄与の大きさの和。
    pub absolute_units: i64,
    /// 相対成分の寄与の大きさを `relative_at` まで減衰させた和。
    pub relative_units: f64,
    pub relative_at: DateTime<Utc>,
    /// `relative_units` を作った半減期（日）。運営者が半減期を変えた後、作り直すまでは前の値。
    pub half_life_days: f64,
    /// pull で `Public` に開示できる絶対成分の寄与の大きさの和。
    pub disclosed_public_units: i64,
    /// pull で `SubscribedNodes` だけに開示できる絶対成分の寄与の大きさの和。
    pub disclosed_subscribed_units: i64,
    /// 寄与する signal ごとの hash（signal id・appeal 状態・operator の確定時刻・失効時刻）の XOR。
    pub digest: i64,
}

impl TrustTotals {
    /// 絶対成分（減衰しない、`[-1, 1]`）。
    pub fn absolute(&self) -> f64 {
        units_to_component(self.absolute_units as f64)
    }

    /// 相対成分（`now` まで減衰させる、`[-1, 1]`）。基準時刻は秒未満まで持つので、秒未満も減衰に入れる。
    pub fn relative(&self, now: DateTime<Utc>) -> f64 {
        if self.relative_units == 0.0 {
            return 0.0;
        }
        let age_days = (now - self.relative_at).as_seconds_f64().max(0.0) / 86_400.0;
        let decay = 0.5_f64.powf(age_days / self.half_life_days);
        units_to_component(self.relative_units * decay)
    }

    /// `audience` へ開示できる絶対成分（ADR 0026 §6.3）。開示しない signal は値に入らない。
    pub fn disclosed_absolute(&self, audience: PullAudience) -> f64 {
        let units = match audience {
            PullAudience::Public => self.disclosed_public_units,
            PullAudience::SubscribedNodes => {
                self.disclosed_public_units + self.disclosed_subscribed_units
            }
        };
        units_to_component(units as f64)
    }

    /// `trust_version`（ADR 0026 §8.4）。寄与する signal の集合と状態が同じなら同じ値。
    pub fn version(&self) -> String {
        format!("t-{:016x}", self.digest as u64)
    }
}

/// 寄与は負の evidence。`0.0 - units` は units が 0 のとき +0 になる（wire に -0 を出さない）。
fn units_to_component(units: f64) -> f64 {
    clamp_unit((0.0 - units) / TRUST_UNITS_PER_CONTRIBUTION)
}
