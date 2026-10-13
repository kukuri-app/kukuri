use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    DomeCustomizationV1, DomeInstanceManifestV1, DomeInstanceStatusV1, KukuriEnvelope, KukuriKeys,
    MetaversePersistentPropV1, Pubkey, SpatialContextV1, sign_envelope_json,
    validate_dome_customization, validate_dome_instance_manifest,
};

pub const DOME_HOSTING_MAX_LEASE_MILLIS: i64 = 30 * 24 * 60 * 60 * 1_000;
pub const DOME_HOST_HEARTBEAT_INTERVAL_MILLIS: i64 = 5_000;
pub const DOME_HOSTING_HEARTBEAT_GRACE_MILLIS: i64 = 15_000;
pub const DOME_PARTICIPANT_KEEPALIVE_INTERVAL_MILLIS: i64 = 5_000;
pub const DOME_PARTICIPANT_TIMEOUT_MILLIS: i64 = 30_000;
pub const DOME_SNAPSHOT_RING_CAPACITY: usize = 100;

const LEASE_KIND: &str = "dome-hosting-lease";
const ACCEPTANCE_KIND: &str = "dome-hosting-acceptance";
const ACTIVATION_KIND: &str = "dome-hosting-activation";
const CLOSE_KIND: &str = "dome-hosting-close";
const INPUT_KIND: &str = "dome-session-input";
const SNAPSHOT_KIND: &str = "dome-physics-snapshot";
const HEARTBEAT_KIND: &str = "dome-host-heartbeat";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DomeHostTargetV1 {
    OwnerDevice {
        endpoint_id: String,
        #[cfg_attr(feature = "ts", ts(type = "string"))]
        host_pubkey: Pubkey,
    },
    CommunityNode {
        #[cfg_attr(feature = "ts", ts(type = "string"))]
        node_id: Pubkey,
        api_base_url: String,
    },
}

impl DomeHostTargetV1 {
    pub fn signing_pubkey(&self) -> &Pubkey {
        match self {
            Self::OwnerDevice { host_pubkey, .. } => host_pubkey,
            Self::CommunityNode { node_id, .. } => node_id,
        }
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::OwnerDevice {
                endpoint_id,
                host_pubkey,
            } => {
                if endpoint_id.trim().is_empty() || host_pubkey.as_str().trim().is_empty() {
                    bail!("owner device hosting target is incomplete");
                }
            }
            Self::CommunityNode {
                node_id,
                api_base_url,
            } => {
                if node_id.as_str().trim().is_empty()
                    || !(api_base_url.starts_with("https://")
                        || api_base_url.starts_with("http://localhost")
                        || api_base_url.starts_with("http://127.0.0.1"))
                {
                    bail!("Community Node hosting target is invalid");
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomeHostingLeaseV1 {
    pub lease_id: String,
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub instance_generation: u64,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub owner_pubkey: Pubkey,
    pub host: DomeHostTargetV1,
    pub manifest_blob_hash: String,
    pub manifest_version: u64,
    pub epoch: u64,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDomeHostingLeaseV1 {
    pub lease: DomeHostingLeaseV1,
    pub envelope: KukuriEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomeHostingAcceptanceV1 {
    pub lease_id: String,
    pub lease_digest: String,
    pub instance_id: String,
    pub instance_generation: u64,
    pub lease_epoch: u64,
    pub session_id: String,
    pub accepted_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDomeHostingAcceptanceV1 {
    pub acceptance: DomeHostingAcceptanceV1,
    pub envelope: KukuriEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomeHostingActivationV1 {
    pub lease_id: String,
    pub lease_digest: String,
    pub lease_epoch: u64,
    pub host_acceptance_envelope_id: String,
    pub activated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDomeHostingActivationV1 {
    pub activation: DomeHostingActivationV1,
    pub envelope: KukuriEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomeHostingCloseV1 {
    pub lease_id: String,
    pub lease_digest: String,
    pub lease_epoch: u64,
    pub closed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDomeHostingCloseV1 {
    pub close: DomeHostingCloseV1,
    pub envelope: KukuriEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomeHostingRecordV1 {
    LeaseIssued(SignedDomeHostingLeaseV1),
    HostAccepted(SignedDomeHostingAcceptanceV1),
    LeaseActivated(SignedDomeHostingActivationV1),
    LeaseClosed(SignedDomeHostingCloseV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeHostingStateKindV1 {
    Closed,
    OwnerHosted,
    CommunityNodeHosted,
    GracePeriod,
    Transferring,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct DomeHostingStateV1 {
    pub kind: DomeHostingStateKindV1,
    pub host: Option<DomeHostTargetV1>,
    pub lease_id: Option<String>,
    pub lease_epoch: Option<u64>,
    pub lease_expires_at: Option<i64>,
    pub session_id: Option<String>,
    pub reason: Option<String>,
    pub last_heartbeat_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DomeSessionInputKindV1 {
    Join {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        avatar_collider: Option<crate::MetaverseColliderV1>,
    },
    Leave,
    KeepAlive,
    Move {
        position: [i64; 3],
        rotation: [i64; 3],
        animation: String,
    },
    Grab {
        prop_id: String,
    },
    Throw {
        prop_id: String,
        impulse: [i64; 3],
    },
    Push {
        prop_id: String,
        impulse: [i64; 3],
    },
    Sit {
        prop_id: String,
    },
    PrepareTransition {
        transition_id: String,
        direction: crate::DomeDirection,
    },
    AbortTransition {
        transition_id: String,
    },
    CompleteTransition {
        transition_id: String,
    },
    SpawnGuestProp {
        prop: MetaversePersistentPropV1,
        expires_at: i64,
    },
    UpsertPersistentProp {
        prop: MetaversePersistentPropV1,
    },
    DeletePersistentProp {
        prop_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomeSessionInputV1 {
    pub input_id: String,
    pub instance_id: String,
    pub instance_generation: u64,
    pub lease_epoch: u64,
    pub session_id: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub participant_pubkey: Pubkey,
    pub sequence: u64,
    pub sent_at: i64,
    pub input: DomeSessionInputKindV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDomeSessionInputV1 {
    pub input: DomeSessionInputV1,
    pub envelope: KukuriEnvelope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomePhysicsBodyKindV1 {
    Avatar,
    PersistentProp,
    GuestProp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomePhysicsBodyV1 {
    pub entity_id: String,
    pub kind: DomePhysicsBodyKindV1,
    pub position: [i64; 3],
    pub rotation: [i64; 3],
    pub linear_velocity: [i64; 3],
    pub animation: Option<String>,
    pub grabbed_by: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomePhysicsSnapshotV1 {
    pub instance_id: String,
    pub instance_generation: u64,
    pub lease_epoch: u64,
    pub session_id: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub host_pubkey: Pubkey,
    pub sequence: u64,
    pub simulated_at: i64,
    pub sleeping: bool,
    pub bodies: Vec<DomePhysicsBodyV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDomePhysicsSnapshotV1 {
    pub snapshot: DomePhysicsSnapshotV1,
    pub envelope: KukuriEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomeHostHeartbeatV1 {
    pub instance_id: String,
    pub instance_generation: u64,
    pub lease_epoch: u64,
    pub session_id: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub host_pubkey: Pubkey,
    pub participants: u32,
    pub sleeping: bool,
    pub sequence: u64,
    pub sent_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDomeHostHeartbeatV1 {
    pub heartbeat: DomeHostHeartbeatV1,
    pub envelope: KukuriEnvelope,
}

impl DomeHostingStateV1 {
    pub fn closed(reason: impl Into<String>) -> Self {
        Self {
            kind: DomeHostingStateKindV1::Closed,
            host: None,
            lease_id: None,
            lease_epoch: None,
            lease_expires_at: None,
            session_id: None,
            reason: Some(reason.into()),
            last_heartbeat_at: None,
        }
    }
}

pub fn validate_dome_hosting_lease(
    lease: &DomeHostingLeaseV1,
    instance: &DomeInstanceManifestV1,
) -> Result<()> {
    validate_dome_instance_manifest(instance)?;
    if instance.status != DomeInstanceStatusV1::Active || instance.relationship_detach.is_some() {
        bail!("Dome Hosting Lease requires an active attached instance");
    }
    if lease.lease_id.trim().is_empty()
        || lease.instance_id.trim().is_empty()
        || lease.manifest_blob_hash.trim().is_empty()
        || lease.instance_generation == 0
        || lease.manifest_version == 0
        || lease.epoch == 0
    {
        bail!("Dome Hosting Lease identity is incomplete");
    }
    if lease.owner_pubkey != instance.owner_pubkey
        || lease.spatial_context != instance.spatial_context
        || lease.instance_id != instance.instance_id
        || lease.instance_generation != instance.generation
        || lease.manifest_blob_hash != instance.preset_ref.manifest_blob_hash
        || lease.manifest_version != instance.preset_ref.revision
    {
        bail!("Dome Hosting Lease does not match the current instance");
    }
    if lease.expires_at <= lease.issued_at
        || lease.expires_at - lease.issued_at > DOME_HOSTING_MAX_LEASE_MILLIS
    {
        bail!("Dome Hosting Lease expiry is outside the supported range");
    }
    lease.host.validate()
}

pub fn dome_hosting_lease_digest(lease: &DomeHostingLeaseV1) -> Result<String> {
    let bytes = serde_json::to_vec(lease).context("failed to encode Dome Hosting Lease")?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

pub fn build_signed_dome_hosting_lease(
    owner_keys: &KukuriKeys,
    lease: DomeHostingLeaseV1,
) -> Result<SignedDomeHostingLeaseV1> {
    if lease.owner_pubkey != owner_keys.public_key() {
        bail!("Dome Hosting Lease owner must match signer");
    }
    lease.host.validate()?;
    if lease.lease_id.trim().is_empty()
        || lease.epoch == 0
        || lease.manifest_version == 0
        || lease.expires_at <= lease.issued_at
        || lease.expires_at - lease.issued_at > DOME_HOSTING_MAX_LEASE_MILLIS
    {
        bail!("Dome Hosting Lease is invalid");
    }
    let envelope = sign_envelope_json(
        owner_keys,
        LEASE_KIND,
        hosting_tags(&lease.instance_id, lease.epoch, &lease.lease_id),
        &lease,
    )?;
    Ok(SignedDomeHostingLeaseV1 { lease, envelope })
}

pub fn verify_signed_dome_hosting_lease(
    signed: &SignedDomeHostingLeaseV1,
    instance: &DomeInstanceManifestV1,
) -> Result<()> {
    validate_dome_hosting_lease(&signed.lease, instance)?;
    verify_envelope_content(
        &signed.envelope,
        LEASE_KIND,
        &signed.lease.owner_pubkey,
        &signed.lease,
    )
}

pub fn accept_dome_hosting_lease(
    host_keys: &KukuriKeys,
    signed: &SignedDomeHostingLeaseV1,
    session_id: impl Into<String>,
    accepted_at: i64,
) -> Result<SignedDomeHostingAcceptanceV1> {
    if signed.lease.host.signing_pubkey() != &host_keys.public_key() {
        bail!("Dome host acceptance signer does not match lease target");
    }
    signed.envelope.verify()?;
    let acceptance = DomeHostingAcceptanceV1 {
        lease_id: signed.lease.lease_id.clone(),
        lease_digest: dome_hosting_lease_digest(&signed.lease)?,
        instance_id: signed.lease.instance_id.clone(),
        instance_generation: signed.lease.instance_generation,
        lease_epoch: signed.lease.epoch,
        session_id: session_id.into(),
        accepted_at,
    };
    if acceptance.session_id.trim().is_empty()
        || accepted_at < signed.lease.issued_at
        || accepted_at >= signed.lease.expires_at
    {
        bail!("Dome host acceptance is outside the lease lifetime");
    }
    let envelope = sign_envelope_json(
        host_keys,
        ACCEPTANCE_KIND,
        hosting_tags(
            &signed.lease.instance_id,
            signed.lease.epoch,
            &signed.lease.lease_id,
        ),
        &acceptance,
    )?;
    Ok(SignedDomeHostingAcceptanceV1 {
        acceptance,
        envelope,
    })
}

pub fn activate_dome_hosting_lease(
    owner_keys: &KukuriKeys,
    signed: &SignedDomeHostingLeaseV1,
    acceptance: &SignedDomeHostingAcceptanceV1,
    activated_at: i64,
) -> Result<SignedDomeHostingActivationV1> {
    if signed.lease.owner_pubkey != owner_keys.public_key() {
        bail!("Dome Hosting activation owner must match signer");
    }
    verify_acceptance(signed, acceptance)?;
    if activated_at < acceptance.acceptance.accepted_at || activated_at >= signed.lease.expires_at {
        bail!("Dome Hosting activation is outside the lease lifetime");
    }
    let activation = DomeHostingActivationV1 {
        lease_id: signed.lease.lease_id.clone(),
        lease_digest: dome_hosting_lease_digest(&signed.lease)?,
        lease_epoch: signed.lease.epoch,
        host_acceptance_envelope_id: acceptance.envelope.id.0.clone(),
        activated_at,
    };
    let envelope = sign_envelope_json(
        owner_keys,
        ACTIVATION_KIND,
        hosting_tags(
            &signed.lease.instance_id,
            signed.lease.epoch,
            &signed.lease.lease_id,
        ),
        &activation,
    )?;
    Ok(SignedDomeHostingActivationV1 {
        activation,
        envelope,
    })
}

pub fn close_dome_hosting_lease(
    owner_keys: &KukuriKeys,
    signed: &SignedDomeHostingLeaseV1,
    closed_at: i64,
) -> Result<SignedDomeHostingCloseV1> {
    if signed.lease.owner_pubkey != owner_keys.public_key() || closed_at < signed.lease.issued_at {
        bail!("Dome Hosting close is invalid");
    }
    let close = DomeHostingCloseV1 {
        lease_id: signed.lease.lease_id.clone(),
        lease_digest: dome_hosting_lease_digest(&signed.lease)?,
        lease_epoch: signed.lease.epoch,
        closed_at,
    };
    let envelope = sign_envelope_json(
        owner_keys,
        CLOSE_KIND,
        hosting_tags(
            &signed.lease.instance_id,
            signed.lease.epoch,
            &signed.lease.lease_id,
        ),
        &close,
    )?;
    Ok(SignedDomeHostingCloseV1 { close, envelope })
}

pub fn resolve_dome_hosting_state(
    instance: &DomeInstanceManifestV1,
    records: &[DomeHostingRecordV1],
    now_millis: i64,
    last_heartbeat_at: Option<i64>,
) -> Result<DomeHostingStateV1> {
    validate_dome_instance_manifest(instance)?;
    if instance.status != DomeInstanceStatusV1::Active || instance.relationship_detach.is_some() {
        return Ok(DomeHostingStateV1::closed("instance_inactive"));
    }

    let mut leases_by_epoch: BTreeMap<u64, BTreeMap<String, &SignedDomeHostingLeaseV1>> =
        BTreeMap::new();
    for record in records {
        if let DomeHostingRecordV1::LeaseIssued(signed) = record {
            // A tombstoned owner slot is reused with a higher generation. Keep
            // authenticated history for monotonic epochs, never as authority.
            if signed.lease.instance_generation < instance.generation {
                if signed.lease.instance_id != instance.instance_id
                    || signed.lease.owner_pubkey != instance.owner_pubkey
                    || signed.lease.spatial_context != instance.spatial_context
                {
                    bail!("historical Dome lease identity mismatch");
                }
                verify_envelope_content(
                    &signed.envelope,
                    LEASE_KIND,
                    &signed.lease.owner_pubkey,
                    &signed.lease,
                )?;
                continue;
            }
            if signed.lease.manifest_version < instance.preset_ref.revision {
                if signed.lease.instance_id != instance.instance_id
                    || signed.lease.instance_generation != instance.generation
                    || signed.lease.owner_pubkey != instance.owner_pubkey
                    || signed.lease.spatial_context != instance.spatial_context
                {
                    bail!("stale Dome Hosting Lease identity does not match the instance");
                }
                verify_envelope_content(
                    &signed.envelope,
                    LEASE_KIND,
                    &signed.lease.owner_pubkey,
                    &signed.lease,
                )?;
                continue;
            }
            verify_signed_dome_hosting_lease(signed, instance)?;
            leases_by_epoch
                .entry(signed.lease.epoch)
                .or_default()
                .insert(dome_hosting_lease_digest(&signed.lease)?, signed);
        }
    }
    let Some((epoch, leases)) = leases_by_epoch.last_key_value() else {
        return Ok(DomeHostingStateV1::closed("no_lease"));
    };
    if leases.len() != 1 {
        return Ok(DomeHostingStateV1 {
            kind: DomeHostingStateKindV1::GracePeriod,
            host: None,
            lease_id: None,
            lease_epoch: Some(*epoch),
            lease_expires_at: None,
            session_id: None,
            reason: Some("split_brain".into()),
            last_heartbeat_at,
        });
    }
    let signed = leases.values().next().expect("one lease was checked above");
    let lease = &signed.lease;
    let digest = dome_hosting_lease_digest(lease)?;
    if now_millis >= lease.expires_at {
        return Ok(DomeHostingStateV1::closed("lease_expired"));
    }

    let mut acceptances = BTreeMap::<String, &SignedDomeHostingAcceptanceV1>::new();
    let mut activation_ids = BTreeSet::new();
    let mut closed = false;
    for record in records {
        match record {
            DomeHostingRecordV1::HostAccepted(acceptance)
                if acceptance.acceptance.lease_epoch == *epoch
                    && acceptance.acceptance.lease_digest == digest =>
            {
                verify_acceptance(signed, acceptance)?;
                acceptances.insert(acceptance.envelope.id.0.clone(), acceptance);
            }
            DomeHostingRecordV1::LeaseActivated(activation)
                if activation.activation.lease_epoch == *epoch
                    && activation.activation.lease_digest == digest =>
            {
                verify_activation(signed, activation)?;
                activation_ids.insert(activation.activation.host_acceptance_envelope_id.clone());
            }
            DomeHostingRecordV1::LeaseClosed(close)
                if close.close.lease_epoch == *epoch && close.close.lease_digest == digest =>
            {
                verify_close(signed, close)?;
                closed = true;
            }
            _ => {}
        }
    }
    if closed {
        return Ok(DomeHostingStateV1::closed("owner_closed"));
    }
    let active = activation_ids
        .iter()
        .find_map(|id| acceptances.get(id.as_str()).copied());
    let Some(acceptance) = active else {
        return Ok(DomeHostingStateV1 {
            kind: DomeHostingStateKindV1::Transferring,
            host: Some(lease.host.clone()),
            lease_id: Some(lease.lease_id.clone()),
            lease_epoch: Some(lease.epoch),
            lease_expires_at: Some(lease.expires_at),
            session_id: None,
            reason: Some("awaiting_activation".into()),
            last_heartbeat_at,
        });
    };

    let liveness = crate::resolve_dome_host_liveness(last_heartbeat_at, now_millis);
    if liveness == crate::DomeHostLivenessV1::Closed {
        return Ok(DomeHostingStateV1 {
            kind: DomeHostingStateKindV1::Closed,
            host: Some(lease.host.clone()),
            lease_id: Some(lease.lease_id.clone()),
            lease_epoch: Some(lease.epoch),
            lease_expires_at: Some(lease.expires_at),
            session_id: Some(acceptance.acceptance.session_id.clone()),
            reason: Some("heartbeat_timeout".into()),
            last_heartbeat_at,
        });
    }
    let kind = if liveness == crate::DomeHostLivenessV1::OfflineGrace {
        DomeHostingStateKindV1::GracePeriod
    } else {
        match lease.host {
            DomeHostTargetV1::OwnerDevice { .. } => DomeHostingStateKindV1::OwnerHosted,
            DomeHostTargetV1::CommunityNode { .. } => DomeHostingStateKindV1::CommunityNodeHosted,
        }
    };
    Ok(DomeHostingStateV1 {
        kind,
        host: Some(lease.host.clone()),
        lease_id: Some(lease.lease_id.clone()),
        lease_epoch: Some(lease.epoch),
        lease_expires_at: Some(lease.expires_at),
        session_id: Some(acceptance.acceptance.session_id.clone()),
        reason: (liveness == crate::DomeHostLivenessV1::OfflineGrace)
            .then(|| "heartbeat_missing".into()),
        last_heartbeat_at,
    })
}

pub fn build_signed_dome_session_input(
    participant_keys: &KukuriKeys,
    input: DomeSessionInputV1,
) -> Result<SignedDomeSessionInputV1> {
    validate_dome_session_input(&input)?;
    if input.participant_pubkey != participant_keys.public_key() {
        bail!("Dome session input participant must match signer");
    }
    let envelope = sign_envelope_json(
        participant_keys,
        INPUT_KIND,
        session_tags(&input.instance_id, input.lease_epoch, &input.session_id),
        &input,
    )?;
    Ok(SignedDomeSessionInputV1 { input, envelope })
}

pub fn verify_signed_dome_session_input(
    signed: &SignedDomeSessionInputV1,
    lease: &DomeHostingLeaseV1,
    session_id: &str,
) -> Result<()> {
    validate_dome_session_input(&signed.input)?;
    let input = &signed.input;
    if input.instance_id != lease.instance_id
        || input.instance_generation != lease.instance_generation
        || input.lease_epoch != lease.epoch
        || input.session_id != session_id
    {
        bail!("Dome session input does not match active lease and session");
    }
    verify_envelope_content(
        &signed.envelope,
        INPUT_KIND,
        &input.participant_pubkey,
        input,
    )
}

pub fn build_signed_dome_physics_snapshot(
    host_keys: &KukuriKeys,
    lease: &DomeHostingLeaseV1,
    snapshot: DomePhysicsSnapshotV1,
) -> Result<SignedDomePhysicsSnapshotV1> {
    validate_dome_physics_snapshot(lease, &snapshot)?;
    if lease.host.signing_pubkey() != &host_keys.public_key()
        || snapshot.host_pubkey != host_keys.public_key()
    {
        bail!("Dome physics snapshot host must match lease target");
    }
    let envelope = sign_envelope_json(
        host_keys,
        SNAPSHOT_KIND,
        session_tags(
            &snapshot.instance_id,
            snapshot.lease_epoch,
            &snapshot.session_id,
        ),
        &snapshot,
    )?;
    Ok(SignedDomePhysicsSnapshotV1 { snapshot, envelope })
}

pub fn verify_signed_dome_physics_snapshot(
    signed: &SignedDomePhysicsSnapshotV1,
    lease: &DomeHostingLeaseV1,
    session_id: &str,
) -> Result<()> {
    validate_dome_physics_snapshot(lease, &signed.snapshot)?;
    if signed.snapshot.session_id != session_id {
        bail!("Dome physics snapshot session is stale");
    }
    verify_envelope_content(
        &signed.envelope,
        SNAPSHOT_KIND,
        lease.host.signing_pubkey(),
        &signed.snapshot,
    )
}

pub fn build_signed_dome_host_heartbeat(
    host_keys: &KukuriKeys,
    lease: &DomeHostingLeaseV1,
    heartbeat: DomeHostHeartbeatV1,
) -> Result<SignedDomeHostHeartbeatV1> {
    validate_dome_host_heartbeat(lease, &heartbeat)?;
    if lease.host.signing_pubkey() != &host_keys.public_key()
        || heartbeat.host_pubkey != host_keys.public_key()
    {
        bail!("Dome host heartbeat signer does not match lease target");
    }
    let envelope = sign_envelope_json(
        host_keys,
        HEARTBEAT_KIND,
        session_tags(
            &heartbeat.instance_id,
            heartbeat.lease_epoch,
            &heartbeat.session_id,
        ),
        &heartbeat,
    )?;
    Ok(SignedDomeHostHeartbeatV1 {
        heartbeat,
        envelope,
    })
}

pub fn verify_signed_dome_host_heartbeat(
    signed: &SignedDomeHostHeartbeatV1,
    lease: &DomeHostingLeaseV1,
    session_id: &str,
) -> Result<()> {
    validate_dome_host_heartbeat(lease, &signed.heartbeat)?;
    if signed.heartbeat.session_id != session_id {
        bail!("Dome host heartbeat session is stale");
    }
    verify_envelope_content(
        &signed.envelope,
        HEARTBEAT_KIND,
        lease.host.signing_pubkey(),
        &signed.heartbeat,
    )
}

fn validate_dome_session_input(input: &DomeSessionInputV1) -> Result<()> {
    if input.input_id.trim().is_empty()
        || input.instance_id.trim().is_empty()
        || input.session_id.trim().is_empty()
        || input.instance_generation == 0
        || input.lease_epoch == 0
        || input.sequence == 0
    {
        bail!("Dome session input identity is incomplete");
    }
    match &input.input {
        DomeSessionInputKindV1::Join {
            avatar_collider: Some(collider),
        } => crate::validate_metaverse_collider(collider)?,
        DomeSessionInputKindV1::Move { animation, .. } if animation.trim().is_empty() => {
            bail!("Dome avatar animation is required");
        }
        DomeSessionInputKindV1::Grab { prop_id } | DomeSessionInputKindV1::Sit { prop_id }
            if prop_id.trim().is_empty() =>
        {
            bail!("Dome interaction prop id is required");
        }
        DomeSessionInputKindV1::Throw { prop_id, impulse }
        | DomeSessionInputKindV1::Push { prop_id, impulse }
            if prop_id.trim().is_empty() || impulse.iter().all(|component| *component == 0) =>
        {
            bail!("Dome impulse input is incomplete");
        }
        DomeSessionInputKindV1::SpawnGuestProp { prop, expires_at } => {
            validate_session_prop(prop)?;
            if *expires_at <= input.sent_at {
                bail!("Dome guest prop expiry must be in the future");
            }
        }
        DomeSessionInputKindV1::UpsertPersistentProp { prop } => validate_session_prop(prop)?,
        DomeSessionInputKindV1::DeletePersistentProp { prop_id } if prop_id.trim().is_empty() => {
            bail!("Dome persistent prop id is required");
        }
        _ => {}
    }
    Ok(())
}

fn validate_session_prop(prop: &MetaversePersistentPropV1) -> Result<()> {
    let customization = DomeCustomizationV1 {
        persistent_props: vec![prop.clone()],
        ..DomeCustomizationV1::default()
    };
    validate_dome_customization(&customization)
}

fn validate_dome_physics_snapshot(
    lease: &DomeHostingLeaseV1,
    snapshot: &DomePhysicsSnapshotV1,
) -> Result<()> {
    if snapshot.instance_id != lease.instance_id
        || snapshot.instance_generation != lease.instance_generation
        || snapshot.lease_epoch != lease.epoch
        || snapshot.session_id.trim().is_empty()
        || snapshot.host_pubkey != *lease.host.signing_pubkey()
        || snapshot.sequence == 0
    {
        bail!("Dome physics snapshot does not match active lease");
    }
    let mut entity_ids = BTreeSet::new();
    if snapshot
        .bodies
        .iter()
        .any(|body| body.entity_id.trim().is_empty() || !entity_ids.insert(&body.entity_id))
    {
        bail!("Dome physics snapshot entity ids must be non-empty and unique");
    }
    Ok(())
}

fn validate_dome_host_heartbeat(
    lease: &DomeHostingLeaseV1,
    heartbeat: &DomeHostHeartbeatV1,
) -> Result<()> {
    if heartbeat.instance_id != lease.instance_id
        || heartbeat.instance_generation != lease.instance_generation
        || heartbeat.lease_epoch != lease.epoch
        || heartbeat.session_id.trim().is_empty()
        || heartbeat.host_pubkey != *lease.host.signing_pubkey()
        || heartbeat.sequence == 0
        || heartbeat.sent_at < lease.issued_at
        || heartbeat.sent_at >= lease.expires_at
    {
        bail!("Dome host heartbeat does not match active lease");
    }
    Ok(())
}

fn verify_acceptance(
    signed: &SignedDomeHostingLeaseV1,
    acceptance: &SignedDomeHostingAcceptanceV1,
) -> Result<()> {
    let content = &acceptance.acceptance;
    if content.lease_id != signed.lease.lease_id
        || content.lease_digest != dome_hosting_lease_digest(&signed.lease)?
        || content.instance_id != signed.lease.instance_id
        || content.instance_generation != signed.lease.instance_generation
        || content.lease_epoch != signed.lease.epoch
        || content.session_id.trim().is_empty()
        || content.accepted_at < signed.lease.issued_at
        || content.accepted_at >= signed.lease.expires_at
    {
        bail!("Dome host acceptance does not match lease");
    }
    verify_envelope_content(
        &acceptance.envelope,
        ACCEPTANCE_KIND,
        signed.lease.host.signing_pubkey(),
        content,
    )
}

fn verify_activation(
    signed: &SignedDomeHostingLeaseV1,
    activation: &SignedDomeHostingActivationV1,
) -> Result<()> {
    let content = &activation.activation;
    if content.lease_id != signed.lease.lease_id
        || content.lease_digest != dome_hosting_lease_digest(&signed.lease)?
        || content.lease_epoch != signed.lease.epoch
        || content.host_acceptance_envelope_id.trim().is_empty()
        || content.activated_at < signed.lease.issued_at
        || content.activated_at >= signed.lease.expires_at
    {
        bail!("Dome Hosting activation does not match lease");
    }
    verify_envelope_content(
        &activation.envelope,
        ACTIVATION_KIND,
        &signed.lease.owner_pubkey,
        content,
    )
}

fn verify_close(signed: &SignedDomeHostingLeaseV1, close: &SignedDomeHostingCloseV1) -> Result<()> {
    let content = &close.close;
    if content.lease_id != signed.lease.lease_id
        || content.lease_digest != dome_hosting_lease_digest(&signed.lease)?
        || content.lease_epoch != signed.lease.epoch
        || content.closed_at < signed.lease.issued_at
    {
        bail!("Dome Hosting close does not match lease");
    }
    verify_envelope_content(
        &close.envelope,
        CLOSE_KIND,
        &signed.lease.owner_pubkey,
        content,
    )
}

pub(crate) fn verify_envelope_content<T>(
    envelope: &KukuriEnvelope,
    expected_kind: &str,
    expected_signer: &Pubkey,
    expected_content: &T,
) -> Result<()>
where
    T: Serialize + DeserializeOwned + PartialEq,
{
    envelope.verify()?;
    if envelope.kind != expected_kind || &envelope.pubkey != expected_signer {
        bail!("Dome Hosting envelope authority mismatch");
    }
    let decoded: T =
        serde_json::from_str(&envelope.content).context("invalid Dome Hosting envelope content")?;
    if &decoded != expected_content {
        bail!("Dome Hosting envelope content mismatch");
    }
    Ok(())
}

pub(crate) fn hosting_tags(instance_id: &str, epoch: u64, lease_id: &str) -> Vec<Vec<String>> {
    vec![
        vec!["object".into(), "dome-hosting".into()],
        vec!["instance_id".into(), instance_id.into()],
        vec!["epoch".into(), epoch.to_string()],
        vec!["lease_id".into(), lease_id.into()],
    ]
}

pub(crate) fn session_tags(instance_id: &str, epoch: u64, session_id: &str) -> Vec<Vec<String>> {
    vec![
        vec!["object".into(), "dome-session".into()],
        vec!["instance_id".into(), instance_id.into()],
        vec!["epoch".into(), epoch.to_string()],
        vec!["session_id".into(), session_id.into()],
    ]
}
