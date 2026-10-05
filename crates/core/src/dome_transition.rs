use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    DomeDirection, KukuriEnvelope, KukuriKeys, PrivateChannelParticipantDocV1,
    PrivateChannelPolicyDocV1, Pubkey, SpatialContextV1, fixed_dome_v1,
    parse_private_channel_participant, parse_private_channel_policy,
};

pub const DOME_TRANSITION_TICKET_TTL_MILLIS: i64 = 15_000;
pub const DOME_TRANSITION_CROSSING_HYSTERESIS_CM: i64 = 10;
pub const DOME_ACCESS_PROOF_TTL_MILLIS: i64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeTransitionDenialReasonV1 {
    HostUnavailable,
    AccessDenied,
    OwnersBlocked,
    VisitorBlocked,
    CapacityFull,
    AssetsUnavailable,
    StaleTopology,
    StaleSession,
    InvalidTicket,
}

impl DomeTransitionDenialReasonV1 {
    pub const fn code(self) -> &'static str {
        match self {
            Self::HostUnavailable => "DOME_TRANSITION_HOST_UNAVAILABLE",
            Self::AccessDenied => "DOME_TRANSITION_ACCESS_DENIED",
            Self::OwnersBlocked => "DOME_TRANSITION_OWNERS_BLOCKED",
            Self::VisitorBlocked => "DOME_TRANSITION_VISITOR_BLOCKED",
            Self::CapacityFull => "DOME_TRANSITION_CAPACITY_FULL",
            Self::AssetsUnavailable => "DOME_TRANSITION_ASSETS_UNAVAILABLE",
            Self::StaleTopology => "DOME_TRANSITION_STALE_TOPOLOGY",
            Self::StaleSession => "DOME_TRANSITION_STALE_SESSION",
            Self::InvalidTicket => "DOME_TRANSITION_INVALID_TICKET",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeBoundaryStateV1 {
    Closed,
    Offline,
    Draining,
    Blocked,
    Loading,
    Ready,
    Denied,
    Full,
    Unhosted,
    Error,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeTransitionPhaseV1 {
    Closed,
    Loading,
    Ready,
    Preparing,
    Provisional,
    Committing,
    Committed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomeTransitionAdmissionRequestV1 {
    pub transition_id: String,
    pub connection_id: String,
    pub topology_digest: String,
    pub spatial_context: SpatialContextV1,
    pub source_instance_id: String,
    pub source_instance_generation: u64,
    pub target_instance_id: String,
    pub target_instance_generation: u64,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub participant_pubkey: Pubkey,
    pub direction: DomeDirection,
    pub requested_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomeTransitionAdmissionTicketV1 {
    pub request: DomeTransitionAdmissionRequestV1,
    pub target_lease_epoch: u64,
    pub target_session_id: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum DomeTransitionAccessDecisionV1 {
    Allowed,
    Denied {
        reason: DomeTransitionDenialReasonV1,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomeSpatialAccessStatementV1 {
    pub spatial_context: SpatialContextV1,
    pub participant_pubkey: Pubkey,
    pub target_owner_pubkey: Pubkey,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomeSpatialAccessProofV1 {
    pub statement: DomeSpatialAccessStatementV1,
    pub participant_signature: KukuriEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_policy_signature: Option<KukuriEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_participant_signature: Option<KukuriEnvelope>,
}

pub fn build_dome_spatial_access_proof(
    participant_keys: &KukuriKeys,
    spatial_context: SpatialContextV1,
    target_owner_pubkey: Pubkey,
    issued_at: i64,
    channel_policy_signature: Option<KukuriEnvelope>,
    channel_participant_signature: Option<KukuriEnvelope>,
) -> Result<DomeSpatialAccessProofV1> {
    let statement = DomeSpatialAccessStatementV1 {
        spatial_context,
        participant_pubkey: participant_keys.public_key(),
        target_owner_pubkey,
        issued_at,
        expires_at: issued_at.saturating_add(DOME_ACCESS_PROOF_TTL_MILLIS),
    };
    let participant_signature = crate::sign_envelope_json(
        participant_keys,
        "dome-access-proof",
        vec![
            vec![
                "participant".into(),
                statement.participant_pubkey.as_str().to_string(),
            ],
            vec!["context".into(), statement.spatial_context.canonical_id()],
            vec![
                "target_owner".into(),
                statement.target_owner_pubkey.as_str().to_string(),
            ],
        ],
        &statement,
    )?;
    let proof = DomeSpatialAccessProofV1 {
        statement,
        participant_signature,
        channel_policy_signature,
        channel_participant_signature,
    };
    proof.verify_at(issued_at)?;
    Ok(proof)
}

impl DomeSpatialAccessProofV1 {
    /// 要求者・Spatial Context・owner への束縛と、署名・有効期限を確かめる(Community Node と owner device host で共通)。
    pub fn verify_for(
        &self,
        participant_pubkey: &Pubkey,
        spatial_context: &SpatialContextV1,
        target_owner_pubkey: &Pubkey,
        now_millis: i64,
    ) -> Result<()> {
        if self.statement.participant_pubkey != *participant_pubkey
            || self.statement.spatial_context != *spatial_context
            || self.statement.target_owner_pubkey != *target_owner_pubkey
        {
            bail!("Dome access proof is bound to another participant, context, or owner");
        }
        self.verify_at(now_millis)
    }

    pub fn verify_at(&self, now_millis: i64) -> Result<()> {
        self.participant_signature.verify()?;
        if self.participant_signature.kind != "dome-access-proof"
            || self.participant_signature.pubkey != self.statement.participant_pubkey
        {
            bail!("Dome access proof participant signature is invalid");
        }
        let signed_statement: DomeSpatialAccessStatementV1 =
            serde_json::from_str(&self.participant_signature.content)?;
        if signed_statement != self.statement
            || self.statement.issued_at <= 0
            || self.statement.expires_at <= now_millis
            || self.statement.expires_at
                > self
                    .statement
                    .issued_at
                    .saturating_add(DOME_ACCESS_PROOF_TTL_MILLIS)
        {
            bail!("Dome access proof is invalid or expired");
        }

        match &self.statement.spatial_context {
            SpatialContextV1::Topic { .. } => {
                if self.channel_policy_signature.is_some()
                    || self.channel_participant_signature.is_some()
                {
                    bail!("public topic Dome access proof must not include channel evidence");
                }
            }
            SpatialContextV1::Channel {
                topic_id,
                channel_id,
            } => {
                let policy =
                    verify_channel_policy_evidence(self.channel_policy_signature.as_ref())?;
                let participant = verify_channel_participant_evidence(
                    self.channel_participant_signature.as_ref(),
                    &self.statement.participant_pubkey,
                )?;
                if policy.topic_id != *topic_id
                    || policy.channel_id != *channel_id
                    || participant.topic_id != *topic_id
                    || participant.channel_id != *channel_id
                    || participant.epoch_id != policy.epoch_id
                    || participant.left_at.is_some()
                {
                    bail!("private channel Dome access evidence is stale or mismatched");
                }
            }
        }
        Ok(())
    }
}

fn verify_channel_policy_evidence(
    envelope: Option<&KukuriEnvelope>,
) -> Result<PrivateChannelPolicyDocV1> {
    let envelope = envelope.ok_or_else(|| anyhow::anyhow!("channel policy proof is required"))?;
    envelope.verify()?;
    let policy = parse_private_channel_policy(envelope)?
        .ok_or_else(|| anyhow::anyhow!("channel policy proof is invalid"))?;
    Ok(policy)
}

fn verify_channel_participant_evidence(
    envelope: Option<&KukuriEnvelope>,
    participant_pubkey: &Pubkey,
) -> Result<PrivateChannelParticipantDocV1> {
    let envelope =
        envelope.ok_or_else(|| anyhow::anyhow!("channel participant proof is required"))?;
    envelope.verify()?;
    let participant = parse_private_channel_participant(envelope)?
        .ok_or_else(|| anyhow::anyhow!("channel participant proof is invalid"))?;
    if participant.participant_pubkey != *participant_pubkey {
        bail!("channel participant proof does not match transition participant");
    }
    Ok(participant)
}

impl DomeTransitionAdmissionRequestV1 {
    pub fn validate(&self) -> Result<()> {
        if self.transition_id.trim().is_empty()
            || self.connection_id.trim().is_empty()
            || self.topology_digest.trim().is_empty()
            || self.source_instance_id.trim().is_empty()
            || self.target_instance_id.trim().is_empty()
            || self.source_instance_id == self.target_instance_id
            || self.source_instance_generation == 0
            || self.target_instance_generation == 0
            || self.requested_at <= 0
        {
            bail!("Dome transition admission request is invalid");
        }
        Ok(())
    }
}

impl DomeTransitionAdmissionTicketV1 {
    pub fn validate_for(
        &self,
        request: &DomeTransitionAdmissionRequestV1,
        lease_epoch: u64,
        session_id: &str,
        now_millis: i64,
    ) -> Result<()> {
        request.validate()?;
        if self.request != *request
            || self.target_lease_epoch != lease_epoch
            || self.target_session_id != session_id
            || self.expires_at <= now_millis
        {
            bail!("Dome transition admission ticket is invalid or expired");
        }
        Ok(())
    }
}

pub fn advance_dome_transition_phase(
    current: DomeTransitionPhaseV1,
    next: DomeTransitionPhaseV1,
) -> Result<DomeTransitionPhaseV1> {
    use DomeTransitionPhaseV1::*;
    let valid = matches!(
        (current, next),
        (Closed, Loading)
            | (Loading, Ready)
            | (Ready, Preparing)
            | (Preparing, Provisional)
            | (Provisional, Committing)
            | (Committing, Committed)
            | (
                Loading | Ready | Preparing | Provisional | Committing,
                Failed
            )
    );
    if current == next {
        return Ok(current);
    }
    if !valid {
        bail!("invalid Dome transition phase change");
    }
    Ok(next)
}

pub fn dome_transition_component_position_cm(
    dome_coordinate_cm: [i64; 3],
    local_position_cm: [i64; 3],
) -> [i64; 3] {
    [
        dome_coordinate_cm[0] + local_position_cm[0],
        dome_coordinate_cm[1] + local_position_cm[1],
        dome_coordinate_cm[2] + local_position_cm[2],
    ]
}

pub fn dome_transition_local_position_cm(
    component_position_cm: [i64; 3],
    dome_coordinate_cm: [i64; 3],
) -> [i64; 3] {
    [
        component_position_cm[0] - dome_coordinate_cm[0],
        component_position_cm[1] - dome_coordinate_cm[1],
        component_position_cm[2] - dome_coordinate_cm[2],
    ]
}

pub fn transform_avatar_between_domes_cm(
    source_local_position_cm: [i64; 3],
    source_coordinate_cm: [i64; 3],
    target_coordinate_cm: [i64; 3],
) -> [i64; 3] {
    dome_transition_local_position_cm(
        dome_transition_component_position_cm(source_coordinate_cm, source_local_position_cm),
        target_coordinate_cm,
    )
}

pub fn dome_transition_axis_cm(position_cm: [i64; 3], direction: DomeDirection) -> i64 {
    match direction {
        DomeDirection::North => -position_cm[2],
        DomeDirection::East => position_cm[0],
        DomeDirection::South => position_cm[2],
        DomeDirection::West => -position_cm[0],
    }
}

pub fn dome_transition_progress_millionths(position_cm: [i64; 3], direction: DomeDirection) -> u32 {
    let spec = fixed_dome_v1();
    let start = spec.connection_boundary_offset_cm - spec.connection_zone_depth_cm / 2;
    let axis = dome_transition_axis_cm(position_cm, direction);
    let travelled = axis
        .saturating_sub(start)
        .clamp(0, spec.connection_zone_depth_cm);
    ((travelled * 1_000_000) / spec.connection_zone_depth_cm) as u32
}

pub fn crossed_dome_transition_center(
    previous_position_cm: [i64; 3],
    current_position_cm: [i64; 3],
    direction: DomeDirection,
) -> bool {
    let center = fixed_dome_v1().connection_boundary_offset_cm;
    dome_transition_axis_cm(previous_position_cm, direction)
        <= center - DOME_TRANSITION_CROSSING_HYSTERESIS_CM
        && dome_transition_axis_cm(current_position_cm, direction)
            >= center + DOME_TRANSITION_CROSSING_HYSTERESIS_CM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_avatar_between_each_adjacent_dome_without_moving_component_position() {
        let cases = [
            (DomeDirection::North, [0, 0, -5_700]),
            (DomeDirection::East, [5_700, 0, 0]),
            (DomeDirection::South, [0, 0, 5_700]),
            (DomeDirection::West, [-5_700, 0, 0]),
        ];
        for (_, target) in cases {
            let source_local = [125, 90, -2_860];
            let target_local = transform_avatar_between_domes_cm(source_local, [0, 0, 0], target);
            assert_eq!(
                dome_transition_component_position_cm(target, target_local),
                source_local
            );
        }
    }

    #[test]
    fn progress_uses_the_fifteen_meter_zone_and_center_is_half() {
        assert_eq!(
            dome_transition_progress_millionths([0, 0, -2_100], DomeDirection::North),
            0
        );
        assert_eq!(
            dome_transition_progress_millionths([0, 0, -2_850], DomeDirection::North),
            500_000
        );
        assert_eq!(
            dome_transition_progress_millionths([0, 0, -3_600], DomeDirection::North),
            1_000_000
        );
    }

    #[test]
    fn crossing_requires_hysteresis_in_the_travel_direction() {
        assert!(crossed_dome_transition_center(
            [2_830, 0, 0],
            [2_870, 0, 0],
            DomeDirection::East
        ));
        assert!(!crossed_dome_transition_center(
            [2_870, 0, 0],
            [2_830, 0, 0],
            DomeDirection::East
        ));
        assert!(!crossed_dome_transition_center(
            [2_845, 0, 0],
            [2_855, 0, 0],
            DomeDirection::East
        ));
    }

    #[test]
    fn transition_phase_is_idempotent_and_rejects_skips() {
        assert_eq!(
            advance_dome_transition_phase(
                DomeTransitionPhaseV1::Loading,
                DomeTransitionPhaseV1::Loading
            )
            .expect("idempotent"),
            DomeTransitionPhaseV1::Loading
        );
        assert!(
            advance_dome_transition_phase(
                DomeTransitionPhaseV1::Ready,
                DomeTransitionPhaseV1::Committed
            )
            .is_err()
        );
    }

    #[test]
    fn public_topic_access_proof_is_bound_and_expires() {
        let participant = crate::generate_keys();
        let owner = crate::generate_keys().public_key();
        let context = SpatialContextV1::Topic {
            topic_id: crate::TopicId::new("kukuri:topic:access-proof"),
        };
        let proof = build_dome_spatial_access_proof(
            &participant,
            context.clone(),
            owner.clone(),
            1_000,
            None,
            None,
        )
        .expect("public proof");
        proof.verify_at(1_001).expect("valid proof");
        assert_eq!(proof.statement.spatial_context, context);
        assert_eq!(proof.statement.target_owner_pubkey, owner);
        assert!(proof.verify_at(11_000).is_err());

        let me = participant.public_key();
        let other = crate::generate_keys().public_key();
        let other_context = SpatialContextV1::Topic {
            topic_id: crate::TopicId::new("kukuri:topic:other"),
        };
        proof
            .verify_for(&me, &context, &owner, 1_001)
            .expect("bound proof");
        assert!(proof.verify_for(&other, &context, &owner, 1_001).is_err());
        assert!(
            proof
                .verify_for(&me, &other_context, &owner, 1_001)
                .is_err()
        );
        assert!(proof.verify_for(&me, &context, &other, 1_001).is_err());
        assert!(proof.verify_for(&me, &context, &owner, 11_000).is_err());
    }

    #[test]
    fn private_channel_access_proof_requires_current_active_participant() {
        let owner = crate::generate_keys();
        let participant = crate::generate_keys();
        let topic_id = crate::TopicId::new("kukuri:topic:private-access-proof");
        let channel_id = crate::ChannelId::new("channel-1");
        let policy = PrivateChannelPolicyDocV1 {
            channel_id: channel_id.clone(),
            topic_id: topic_id.clone(),
            audience_kind: crate::ChannelAudienceKind::InviteOnly,
            owner_pubkey: owner.public_key(),
            epoch_id: "epoch-2".into(),
            sharing_state: crate::ChannelSharingState::Open,
            rotated_at: None,
            previous_epoch_id: Some("epoch-1".into()),
            entry_dome_instance_id: None,
        };
        let participant_doc = PrivateChannelParticipantDocV1 {
            channel_id: channel_id.clone(),
            topic_id: topic_id.clone(),
            epoch_id: "epoch-2".into(),
            participant_pubkey: participant.public_key(),
            joined_at: 900,
            is_owner: false,
            join_mode: Some(crate::PrivateChannelJoinMode::InviteToken),
            sponsor_pubkey: Some(owner.public_key()),
            share_token_id: None,
            left_at: None,
        };
        let proof = build_dome_spatial_access_proof(
            &participant,
            SpatialContextV1::Channel {
                topic_id,
                channel_id,
            },
            owner.public_key(),
            1_000,
            Some(
                crate::build_private_channel_policy_envelope(&owner, &policy)
                    .expect("policy envelope"),
            ),
            Some(
                crate::build_private_channel_participant_envelope(&participant, &participant_doc)
                    .expect("participant envelope"),
            ),
        )
        .expect("private proof");
        proof.verify_at(1_001).expect("valid private proof");

        let mut stale = proof;
        let mut stale_doc = participant_doc;
        stale_doc.epoch_id = "epoch-1".into();
        stale.channel_participant_signature = Some(
            crate::build_private_channel_participant_envelope(&participant, &stale_doc)
                .expect("stale participant envelope"),
        );
        assert!(stale.verify_at(1_001).is_err());
    }
}
