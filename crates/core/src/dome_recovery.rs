use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::{DOME_HOST_HEARTBEAT_INTERVAL_MILLIS, DOME_HOSTING_HEARTBEAT_GRACE_MILLIS};

/// 所有者の端末が heartbeat を送る間隔。offline の判定(最後の heartbeat から `DOME_HOST_HEARTBEAT_INTERVAL_MILLIS`
/// 超)の半分にして、届くまでの遅れと端末の時計のずれで、動いている host を offline と読ませない(#1803、ADR 0045)。
pub const DOME_HOST_HEARTBEAT_SEND_INTERVAL_MILLIS: i64 = DOME_HOST_HEARTBEAT_INTERVAL_MILLIS / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeHostLivenessV1 {
    Online,
    OfflineGrace,
    Closed,
}

pub fn resolve_dome_host_liveness(
    last_heartbeat_at: Option<i64>,
    now_millis: i64,
) -> DomeHostLivenessV1 {
    let Some(last_heartbeat_at) = last_heartbeat_at else {
        return DomeHostLivenessV1::OfflineGrace;
    };
    let age = now_millis.saturating_sub(last_heartbeat_at);
    if age > DOME_HOSTING_HEARTBEAT_GRACE_MILLIS {
        DomeHostLivenessV1::Closed
    } else if age > DOME_HOST_HEARTBEAT_INTERVAL_MILLIS {
        DomeHostLivenessV1::OfflineGrace
    } else {
        DomeHostLivenessV1::Online
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeEvacuationReasonV1 {
    HostOffline,
    AccessRevoked,
    Blocked,
    TopologyInvalid,
    UserRequested,
}

impl DomeEvacuationReasonV1 {
    pub const fn code(self) -> &'static str {
        match self {
            Self::HostOffline => "DOME_EVACUATION_HOST_OFFLINE",
            Self::AccessRevoked => "DOME_EVACUATION_ACCESS_REVOKED",
            Self::Blocked => "DOME_EVACUATION_BLOCKED",
            Self::TopologyInvalid => "DOME_EVACUATION_TOPOLOGY_INVALID",
            Self::UserRequested => "DOME_EVACUATION_USER_REQUESTED",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeEvacuationPhaseV1 {
    Idle,
    Selecting,
    Admitting,
    Confirming,
    CleaningSource,
    Complete,
    NoCandidate,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeEvacuationCandidateKindV1 {
    EstablishedTransition,
    ActiveAdjacent,
    OwnHosted,
    LastVisited,
    ChannelEntry,
    StableFallback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomeEvacuationCandidateV1 {
    pub instance_id: String,
    pub kind: DomeEvacuationCandidateKindV1,
    pub available: bool,
}

pub fn order_dome_evacuation_candidates(
    current_instance_id: &str,
    candidates: &[DomeEvacuationCandidateV1],
) -> Vec<DomeEvacuationCandidateV1> {
    let mut ordered = candidates
        .iter()
        .filter(|candidate| candidate.available && candidate.instance_id != current_instance_id)
        .cloned()
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });
    let mut seen_instance_ids = BTreeSet::new();
    ordered.retain(|candidate| seen_instance_ids.insert(candidate.instance_id.clone()));
    ordered
}
