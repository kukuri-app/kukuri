use std::collections::BTreeMap;

use anyhow::{Result, bail};
use kukuri_core::{
    DOME_ACCESS_PROOF_TTL_MILLIS, DomeSessionInputV1, MetaverseBudgetResource,
    MetaverseBudgetScope, MetaverseColliderV1, MetaverseResourceRejection,
    MetaverseResourceRejectionReason,
};
use rapier3d::prelude::{ColliderBuilder, Vector};

use crate::RateWindow;

#[derive(Clone, Copy, Debug, Default)]
struct InputMark {
    sequence: u64,
    sent_at: i64,
}

/// 署名者ごとに最後に受け付けた input。同じ session での古い署名済み input の再送を拒否する。
///
/// input は署名時刻から access proof と同じ期間を過ぎたら受け付けない。参加中の署名者と
/// 所有者の記録は保ち、退出した署名者の記録は受け付けた input が期限を過ぎるまで、
/// 参加者の上限の件数だけ残す（超えたら退出の古い順に捨てる）。
#[derive(Debug, Default)]
pub(crate) struct InputLedger {
    active: BTreeMap<String, InputMark>,
    departed: BTreeMap<String, (u64, InputMark)>,
    departures: u64,
}

impl InputLedger {
    pub(crate) fn check(&mut self, input: &DomeSessionInputV1, now_millis: i64) -> Result<()> {
        self.departed
            .retain(|_, (_, mark)| !input_expired(mark.sent_at, now_millis));
        if input_expired(input.sent_at, now_millis) {
            bail!("DOME_SESSION_INPUT_EXPIRED");
        }
        let id = input.participant_pubkey.as_str();
        if input.sequence <= self.mark(id).map_or(0, |mark| mark.sequence) {
            bail!("stale Dome session input sequence");
        }
        Ok(())
    }

    /// `active` は参加中の署名者または所有者。それ以外は退出後の記録だけを更新する。
    pub(crate) fn record(&mut self, input: &DomeSessionInputV1, active: bool) {
        let id = input.participant_pubkey.as_str();
        let mark = InputMark {
            sequence: input.sequence,
            sent_at: self
                .mark(id)
                .map_or(input.sent_at, |mark| mark.sent_at.max(input.sent_at)),
        };
        if active {
            self.departed.remove(id);
            self.active.insert(id.to_string(), mark);
        } else if let Some((_, departed)) = self.departed.get_mut(id) {
            *departed = mark;
        }
    }

    /// 退出した参加者の記録を、退出した署名者の側へ移す。まだ input を受け付けていない
    /// 参加者（遷移で到着した直後）にも行を作り、退出を起こした input を後から記録する。
    pub(crate) fn retire(&mut self, participant_id: &str, limit: usize) {
        let mark = self.mark(participant_id).unwrap_or_default();
        self.active.remove(participant_id);
        self.departures += 1;
        self.departed
            .insert(participant_id.to_string(), (self.departures, mark));
        if self.departed.len() > limit
            && let Some(oldest) = self
                .departed
                .iter()
                .min_by_key(|(_, (order, _))| *order)
                .map(|(id, _)| id.clone())
        {
            self.departed.remove(&oldest);
        }
    }

    #[cfg(test)]
    pub(crate) fn sequence(&self, participant_id: &str) -> Option<u64> {
        self.mark(participant_id).map(|mark| mark.sequence)
    }

    fn mark(&self, participant_id: &str) -> Option<InputMark> {
        self.active
            .get(participant_id)
            .or_else(|| self.departed.get(participant_id).map(|(_, mark)| mark))
            .copied()
    }
}

fn input_expired(sent_at: i64, now_millis: i64) -> bool {
    sent_at.saturating_add(DOME_ACCESS_PROOF_TTL_MILLIS) <= now_millis
}

pub(crate) fn rejection(
    scope: MetaverseBudgetScope,
    resource: MetaverseBudgetResource,
    reason: MetaverseResourceRejectionReason,
    observed: u64,
    limit: u64,
) -> anyhow::Error {
    MetaverseResourceRejection::new(scope, resource, reason, observed, limit).into()
}

pub(crate) fn check_limit(
    scope: MetaverseBudgetScope,
    resource: MetaverseBudgetResource,
    observed: u64,
    limit: u64,
) -> Result<()> {
    if observed > limit {
        return Err(rejection(
            scope,
            resource,
            MetaverseResourceRejectionReason::LimitExceeded,
            observed,
            limit,
        ));
    }
    Ok(())
}

pub(crate) fn window_allows(
    window: &mut RateWindow,
    now_millis: i64,
    duration_millis: i64,
    amount: u64,
    limit: u64,
) -> bool {
    if window.started_at == 0 || now_millis.saturating_sub(window.started_at) >= duration_millis {
        window.started_at = now_millis;
        window.count = 0;
    }
    window.count = window.count.saturating_add(amount);
    window.count <= limit
}

pub(crate) fn collider_builder(collider: Option<&MetaverseColliderV1>) -> ColliderBuilder {
    let builder = match collider {
        Some(MetaverseColliderV1::Capsule {
            radius,
            half_height,
            ..
        }) => ColliderBuilder::capsule_y(*half_height as f32 / 100.0, *radius as f32 / 100.0),
        Some(MetaverseColliderV1::Cuboid { half_extents, .. }) => ColliderBuilder::cuboid(
            half_extents[0] as f32 / 100.0,
            half_extents[1] as f32 / 100.0,
            half_extents[2] as f32 / 100.0,
        ),
        None => ColliderBuilder::capsule_y(0.5, 0.25),
    };
    match collider {
        Some(MetaverseColliderV1::Capsule { center, .. })
        | Some(MetaverseColliderV1::Cuboid { center, .. }) => builder.translation(Vector::new(
            center[0] as f32 / 100.0,
            center[1] as f32 / 100.0,
            center[2] as f32 / 100.0,
        )),
        None => builder,
    }
}

pub(crate) fn centimeters_to_meters(value: [i64; 3]) -> [f32; 3] {
    value.map(|component| component as f32 / 100.0)
}

pub(crate) fn meters_to_centimeters(value: [f32; 3]) -> [i64; 3] {
    value.map(|component| (component * 100.0).round() as i64)
}

pub(crate) fn milliradians_to_radians(value: [i64; 3]) -> [f32; 3] {
    value.map(|component| component as f32 / 1_000.0)
}

pub(crate) fn radians_to_milliradians(value: [f32; 3]) -> [i64; 3] {
    value.map(|component| (component * 1_000.0).round() as i64)
}
