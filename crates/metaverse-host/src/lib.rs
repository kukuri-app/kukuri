// ブラウザでも動く共用 crate（ADR 0056 §3）。tokio・std の時刻と task を直接使わない（native では
// n0_future・web_time がそれらの再公開なので、wasm32 の clippy で確かめる）。
#![cfg_attr(
    all(target_family = "wasm", not(test)),
    warn(clippy::disallowed_methods)
)]
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use kukuri_core::{
    DOME_SNAPSHOT_RING_CAPACITY, DomeDirection, DomeHostHeartbeatV1, DomeHostingLeaseV1,
    DomeInstanceManifestV1, DomeLayoutCandidateV1, DomePhysicsBodyKindV1, DomePhysicsBodyV1,
    DomePhysicsSnapshotV1, DomePresetManifestV1, DomeSessionInputKindV1, DomeSessionInputV1,
    DomeTransitionAdmissionTicketV1, KukuriKeys, MetaverseAssetRef, MetaverseBudgetResource,
    MetaverseBudgetScope, MetaversePersistentPropV1, MetaverseResourceBudgetConfig,
    MetaverseResourceMetricCountV1, MetaverseResourceMetricsV1, MetaverseResourceRejection,
    MetaverseResourceRejectionReason, SignedDomeHostHeartbeatV1, SignedDomeHostingLeaseV1,
    SignedDomeLayoutCandidateV1, SignedDomePhysicsSnapshotV1, SignedDomeSessionInputV1,
    build_signed_dome_host_heartbeat, build_signed_dome_layout_candidate,
    build_signed_dome_physics_snapshot, validate_dome_asset_budget,
    validate_dome_instance_manifest, validate_dome_preset_manifest,
    validate_metaverse_asset_metadata, verify_signed_dome_hosting_lease,
    verify_signed_dome_session_input,
};
use rapier3d::prelude::*;
use uuid::Uuid;

mod support;
mod transition;

use transition::PreparedExit;

use support::{
    centimeters_to_meters, check_limit, collider_builder, meters_to_centimeters,
    milliradians_to_radians, radians_to_milliradians, rejection, window_allows,
};

pub const DOME_SIMULATION_HZ: u32 = 30;
pub const DOME_SNAPSHOT_HZ: u32 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestPropSpec {
    pub prop_id: String,
    pub position: [i64; 3],
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
struct RuntimeBody {
    handle: RigidBodyHandle,
    kind: DomePhysicsBodyKindV1,
    animation: Option<String>,
    grabbed_by: Option<String>,
    expires_at: Option<i64>,
    persistent_definition: Option<MetaversePersistentPropV1>,
    created_by: Option<String>,
    asset_bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RateWindow {
    started_at: i64,
    count: u64,
}

#[derive(Clone, Debug, Default)]
struct PlayerBudgetState {
    input_bytes: RateWindow,
    spawns: RateWindow,
    interactions: RateWindow,
}

pub struct DomeSessionRuntime {
    lease: SignedDomeHostingLeaseV1,
    host_keys: KukuriKeys,
    session_id: String,
    participants: BTreeSet<String>,
    transition_reservations: BTreeMap<String, DomeTransitionAdmissionTicketV1>,
    committed_transitions: BTreeMap<String, DomeTransitionAdmissionTicketV1>,
    prepared_exits: BTreeMap<String, PreparedExit>,
    transition_entries: BTreeMap<String, DomeDirection>,
    seated_on: BTreeMap<String, String>,
    last_input_sequence: BTreeMap<String, u64>,
    participant_last_seen_at: BTreeMap<String, i64>,
    bodies_by_id: BTreeMap<String, RuntimeBody>,
    pipeline: PhysicsPipeline,
    integration_parameters: IntegrationParameters,
    island_manager: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    rigid_bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    soft_bodies: SoftBodySet,
    ccd_solver: CCDSolver,
    tick: u64,
    snapshot_sequence: u64,
    heartbeat_sequence: AtomicU64,
    snapshot_ring: VecDeque<SignedDomePhysicsSnapshotV1>,
    last_simulated_at: i64,
    budget: MetaverseResourceBudgetConfig,
    verified_assets: BTreeMap<String, MetaverseAssetRef>,
    participant_limit: u32,
    default_spawn: kukuri_core::MetaverseRoomSpawnV1,
    player_budgets: BTreeMap<String, PlayerBudgetState>,
    rejection_counts: BTreeMap<String, u64>,
    rejected_total: u64,
    participant_high_water: u32,
    rigid_body_high_water: u32,
    snapshot_bytes: u64,
    snapshot_throttled: u64,
    last_snapshot_at: Option<i64>,
    snapshot_bandwidth: RateWindow,
}

impl DomeSessionRuntime {
    pub fn start(
        lease: SignedDomeHostingLeaseV1,
        host_keys: KukuriKeys,
        instance: &DomeInstanceManifestV1,
        preset: &DomePresetManifestV1,
        started_at: i64,
    ) -> Result<Self> {
        Self::start_with_session_id(
            lease,
            host_keys,
            instance,
            preset,
            format!("dome-session-{}", Uuid::new_v4()),
            started_at,
        )
    }

    pub fn start_with_session_id(
        lease: SignedDomeHostingLeaseV1,
        host_keys: KukuriKeys,
        instance: &DomeInstanceManifestV1,
        preset: &DomePresetManifestV1,
        session_id: impl Into<String>,
        started_at: i64,
    ) -> Result<Self> {
        Self::start_with_budget(
            lease,
            host_keys,
            instance,
            preset,
            session_id,
            started_at,
            MetaverseResourceBudgetConfig::default(),
        )
    }

    pub fn start_with_budget(
        lease: SignedDomeHostingLeaseV1,
        host_keys: KukuriKeys,
        instance: &DomeInstanceManifestV1,
        preset: &DomePresetManifestV1,
        session_id: impl Into<String>,
        started_at: i64,
        budget: MetaverseResourceBudgetConfig,
    ) -> Result<Self> {
        budget.validate()?;
        validate_dome_instance_manifest(instance)?;
        validate_dome_preset_manifest(preset)?;
        validate_dome_asset_budget(&preset.asset_refs, &budget)?;
        let persistent_props = preset.dome.customization.persistent_props.len() as u64;
        check_limit(
            MetaverseBudgetScope::Dome,
            MetaverseBudgetResource::PersistentProps,
            persistent_props,
            u64::from(budget.dome.max_persistent_props),
        )?;
        check_limit(
            MetaverseBudgetScope::Dome,
            MetaverseBudgetResource::Colliders,
            persistent_props,
            u64::from(budget.dome.max_colliders),
        )?;
        check_limit(
            MetaverseBudgetScope::Dome,
            MetaverseBudgetResource::RigidBodies,
            persistent_props,
            u64::from(budget.dome.max_rigid_bodies),
        )?;
        check_limit(
            MetaverseBudgetScope::Host,
            MetaverseBudgetResource::RigidBodies,
            persistent_props,
            u64::from(budget.host.max_simulated_rigid_bodies),
        )?;
        verify_signed_dome_hosting_lease(&lease, instance)?;
        if lease.lease.host.signing_pubkey() != &host_keys.public_key() {
            bail!("Dome session host key does not match lease target");
        }
        if instance.preset_ref.preset_id != preset.preset_id
            || instance.preset_ref.manifest_blob_hash != lease.lease.manifest_blob_hash
            || started_at < lease.lease.issued_at
            || started_at >= lease.lease.expires_at
        {
            bail!("Dome session manifest or start time does not match lease");
        }
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            bail!("Dome session id is required");
        }

        let participant_limit = instance
            .max_peers
            .unwrap_or(budget.host.max_participants)
            .min(budget.host.max_participants);
        let mut runtime = Self {
            lease,
            host_keys,
            session_id,
            participants: BTreeSet::new(),
            transition_reservations: BTreeMap::new(),
            committed_transitions: BTreeMap::new(),
            prepared_exits: BTreeMap::new(),
            transition_entries: BTreeMap::new(),
            seated_on: BTreeMap::new(),
            last_input_sequence: BTreeMap::new(),
            participant_last_seen_at: BTreeMap::new(),
            bodies_by_id: BTreeMap::new(),
            pipeline: PhysicsPipeline::new(),
            integration_parameters: IntegrationParameters {
                dt: 1.0 / DOME_SIMULATION_HZ as f32,
                ..IntegrationParameters::default()
            },
            island_manager: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            rigid_bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            soft_bodies: SoftBodySet::default(),
            ccd_solver: CCDSolver::new(),
            tick: 0,
            snapshot_sequence: 0,
            heartbeat_sequence: AtomicU64::new(0),
            snapshot_ring: VecDeque::with_capacity(DOME_SNAPSHOT_RING_CAPACITY),
            last_simulated_at: started_at,
            budget,
            verified_assets: preset
                .asset_refs
                .iter()
                .cloned()
                .map(|asset| (asset.blob_hash.clone(), asset))
                .collect(),
            participant_limit,
            default_spawn: instance.default_spawn.clone(),
            player_budgets: BTreeMap::new(),
            rejection_counts: BTreeMap::new(),
            rejected_total: 0,
            participant_high_water: 0,
            rigid_body_high_water: 0,
            snapshot_bytes: 0,
            snapshot_throttled: 0,
            last_snapshot_at: None,
            snapshot_bandwidth: RateWindow::default(),
        };
        runtime.insert_dome_boundaries();
        for prop in &preset.dome.customization.persistent_props {
            runtime.insert_prop(
                prop.clone(),
                DomePhysicsBodyKindV1::PersistentProp,
                None,
                None,
            )?;
        }
        Ok(runtime)
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn lease(&self) -> &DomeHostingLeaseV1 {
        &self.lease.lease
    }

    pub fn participant_count(&self) -> usize {
        self.participants.len()
    }

    pub fn is_sleeping(&self) -> bool {
        self.participants.is_empty()
    }

    pub fn budget(&self) -> &MetaverseResourceBudgetConfig {
        &self.budget
    }

    pub fn resource_metrics(&self) -> MetaverseResourceMetricsV1 {
        MetaverseResourceMetricsV1 {
            rejected_total: self.rejected_total,
            rejection_counts: self
                .rejection_counts
                .iter()
                .map(|(code, count)| MetaverseResourceMetricCountV1 {
                    code: code.clone(),
                    count: *count,
                })
                .collect(),
            participant_high_water: self.participant_high_water,
            rigid_body_high_water: self.rigid_body_high_water,
            snapshot_bytes: self.snapshot_bytes,
            snapshot_throttled: self.snapshot_throttled,
        }
    }

    pub fn add_guest_prop(&mut self, spec: GuestPropSpec) -> Result<()> {
        if spec.prop_id.trim().is_empty()
            || spec.expires_at <= self.last_simulated_at
            || self.bodies_by_id.contains_key(&spec.prop_id)
        {
            bail!("guest prop specification is invalid");
        }
        self.insert_prop(
            MetaversePersistentPropV1 {
                prop_id: spec.prop_id,
                asset_ref: None,
                primitive_fallback: kukuri_core::MetaversePrimitive::Cube,
                position: spec.position,
                rotation: [0, 0, 0],
                scale: [100, 100, 100],
                visual_only: false,
                interactions: Vec::new(),
                collider: None,
            },
            DomePhysicsBodyKindV1::GuestProp,
            Some(spec.expires_at),
            None,
        )
    }

    pub fn apply_signed_input(&mut self, signed: &SignedDomeSessionInputV1) -> Result<()> {
        self.apply_signed_input_at(signed, signed.input.sent_at)
    }

    pub fn apply_signed_input_at(
        &mut self,
        signed: &SignedDomeSessionInputV1,
        now_millis: i64,
    ) -> Result<()> {
        verify_signed_dome_session_input(signed, &self.lease.lease, &self.session_id)?;
        if let Err(error) = self.preflight_input(&signed.input, now_millis) {
            if let Some(rejection) = error.downcast_ref::<MetaverseResourceRejection>() {
                self.record_rejection(rejection.clone());
            }
            return Err(error);
        }
        self.apply_input(&signed.input)?;
        let participant_id = signed.input.participant_pubkey.as_str().to_string();
        if matches!(signed.input.input, DomeSessionInputKindV1::Leave) {
            self.participant_last_seen_at.remove(&participant_id);
        } else if self.participants.contains(&participant_id) {
            self.participant_last_seen_at
                .insert(participant_id, now_millis);
        }
        self.clamp_bodies_to_dome();
        Ok(())
    }

    pub fn advance_to(&mut self, now_millis: i64) -> Result<()> {
        if now_millis < self.last_simulated_at {
            bail!("Dome session clock cannot move backwards");
        }
        self.expire_guest_props(now_millis);
        self.expire_stale_participants(now_millis);
        if self.participants.is_empty() {
            self.last_simulated_at = now_millis;
            return Ok(());
        }

        let elapsed_millis = now_millis.saturating_sub(self.last_simulated_at);
        let step_millis = 1_000 / i64::from(DOME_SIMULATION_HZ);
        let steps = (elapsed_millis / step_millis).min(i64::from(DOME_SIMULATION_HZ) * 5);
        for _ in 0..steps {
            self.pipeline.step(
                Vector::new(0.0, -9.81, 0.0),
                &self.integration_parameters,
                &mut self.island_manager,
                &mut self.broad_phase,
                &mut self.narrow_phase,
                &mut self.rigid_bodies,
                &mut self.colliders,
                &mut self.impulse_joints,
                &mut self.multibody_joints,
                &mut self.soft_bodies,
                &mut self.ccd_solver,
                &(),
                &(),
            );
            self.clamp_bodies_to_dome();
            self.tick = self.tick.saturating_add(1);
        }
        self.last_simulated_at = now_millis;
        Ok(())
    }

    pub fn signed_snapshot(&mut self, now_millis: i64) -> Result<SignedDomePhysicsSnapshotV1> {
        self.signed_snapshot_inner(now_millis, true)
    }

    /// Admission confirmation is a control-plane receipt. It must contain the Join that was just
    /// committed, so it cannot reuse a rate-throttled older streaming snapshot.
    pub fn signed_admission_snapshot(
        &mut self,
        now_millis: i64,
    ) -> Result<SignedDomePhysicsSnapshotV1> {
        self.signed_snapshot_inner(now_millis, false)
    }

    fn signed_snapshot_inner(
        &mut self,
        now_millis: i64,
        apply_stream_throttle: bool,
    ) -> Result<SignedDomePhysicsSnapshotV1> {
        self.advance_to(now_millis)?;
        let minimum_interval = 1_000 / i64::from(self.budget.dome.max_snapshot_hz);
        if apply_stream_throttle
            && self
                .last_snapshot_at
                .is_some_and(|last| now_millis.saturating_sub(last) < minimum_interval)
            && let Some(latest) = self.snapshot_ring.back()
        {
            self.snapshot_throttled = self.snapshot_throttled.saturating_add(1);
            return Ok(latest.clone());
        }
        let next_sequence = self.snapshot_sequence.saturating_add(1);
        let bodies = self
            .bodies_by_id
            .iter()
            .filter_map(|(entity_id, runtime_body)| {
                let body = self.rigid_bodies.get(runtime_body.handle)?;
                let translation = body.translation();
                let rotation = body.rotation().to_scaled_axis();
                let velocity = body.linvel();
                Some(DomePhysicsBodyV1 {
                    entity_id: entity_id.clone(),
                    kind: runtime_body.kind,
                    position: meters_to_centimeters([translation.x, translation.y, translation.z]),
                    rotation: radians_to_milliradians([rotation.x, rotation.y, rotation.z]),
                    linear_velocity: meters_to_centimeters([velocity.x, velocity.y, velocity.z]),
                    animation: runtime_body.animation.clone(),
                    grabbed_by: runtime_body.grabbed_by.clone(),
                    expires_at: runtime_body.expires_at,
                })
            })
            .collect();
        let signed = build_signed_dome_physics_snapshot(
            &self.host_keys,
            &self.lease.lease,
            DomePhysicsSnapshotV1 {
                instance_id: self.lease.lease.instance_id.clone(),
                instance_generation: self.lease.lease.instance_generation,
                lease_epoch: self.lease.lease.epoch,
                session_id: self.session_id.clone(),
                host_pubkey: self.host_keys.public_key(),
                sequence: next_sequence,
                simulated_at: now_millis,
                sleeping: self.is_sleeping(),
                bodies,
            },
        )?;
        let snapshot_bytes = serde_json::to_vec(&signed)?.len() as u64;
        if apply_stream_throttle
            && !window_allows(
                &mut self.snapshot_bandwidth,
                now_millis,
                1_000,
                snapshot_bytes,
                self.budget.host.max_snapshot_bytes_per_second,
            )
        {
            self.snapshot_throttled = self.snapshot_throttled.saturating_add(1);
            if let Some(latest) = self.snapshot_ring.back() {
                return Ok(latest.clone());
            }
            let rejection = MetaverseResourceRejection::new(
                MetaverseBudgetScope::Host,
                MetaverseBudgetResource::SnapshotBandwidth,
                MetaverseResourceRejectionReason::LimitExceeded,
                snapshot_bytes,
                self.budget.host.max_snapshot_bytes_per_second,
            );
            self.record_rejection(rejection.clone());
            return Err(rejection.into());
        }
        self.snapshot_sequence = next_sequence;
        self.last_snapshot_at = Some(now_millis);
        self.snapshot_bytes = self.snapshot_bytes.saturating_add(snapshot_bytes);
        self.snapshot_ring.push_back(signed.clone());
        while self.snapshot_ring.len() > DOME_SNAPSHOT_RING_CAPACITY {
            self.snapshot_ring.pop_front();
        }
        Ok(signed)
    }

    pub fn snapshots_after(&self, after_sequence: u64) -> Vec<SignedDomePhysicsSnapshotV1> {
        let Some(first) = self.snapshot_ring.front() else {
            return Vec::new();
        };
        if after_sequence != 0 && after_sequence < first.snapshot.sequence {
            return self.snapshot_ring.back().cloned().into_iter().collect();
        }
        let snapshots = self
            .snapshot_ring
            .iter()
            .filter(|snapshot| snapshot.snapshot.sequence > after_sequence)
            .cloned()
            .collect::<Vec<_>>();
        if !snapshots.is_empty() {
            return snapshots;
        }
        Vec::new()
    }

    pub fn snapshot_ring_len(&self) -> usize {
        self.snapshot_ring.len()
    }

    pub fn signed_layout_candidate(
        &mut self,
        operation_id: impl Into<String>,
        now_millis: i64,
    ) -> Result<SignedDomeLayoutCandidateV1> {
        let snapshot = self.signed_snapshot(now_millis)?;
        let mut persistent_props = Vec::new();
        for runtime_body in self.bodies_by_id.values() {
            let Some(mut prop) = runtime_body.persistent_definition.clone() else {
                continue;
            };
            let body = self
                .rigid_bodies
                .get(runtime_body.handle)
                .context("persistent prop rigid body is missing")?;
            let translation = body.translation();
            let rotation = body.rotation().to_scaled_axis();
            prop.position = meters_to_centimeters([translation.x, translation.y, translation.z]);
            prop.rotation = radians_to_milliradians([rotation.x, rotation.y, rotation.z]);
            persistent_props.push(prop);
        }
        persistent_props.sort_by(|left, right| left.prop_id.cmp(&right.prop_id));
        build_signed_dome_layout_candidate(
            &self.host_keys,
            &self.lease.lease,
            DomeLayoutCandidateV1 {
                operation_id: operation_id.into(),
                instance_id: self.lease.lease.instance_id.clone(),
                instance_generation: self.lease.lease.instance_generation,
                lease_epoch: self.lease.lease.epoch,
                session_id: self.session_id.clone(),
                host_pubkey: self.host_keys.public_key(),
                base_manifest_revision: self.lease.lease.manifest_version,
                snapshot_sequence: snapshot.snapshot.sequence,
                captured_at: now_millis,
                persistent_props,
            },
        )
    }

    pub fn signed_heartbeat(&self, now_millis: i64) -> Result<SignedDomeHostHeartbeatV1> {
        let sequence = self
            .heartbeat_sequence
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        build_signed_dome_host_heartbeat(
            &self.host_keys,
            &self.lease.lease,
            DomeHostHeartbeatV1 {
                instance_id: self.lease.lease.instance_id.clone(),
                instance_generation: self.lease.lease.instance_generation,
                lease_epoch: self.lease.lease.epoch,
                session_id: self.session_id.clone(),
                host_pubkey: self.host_keys.public_key(),
                participants: self.participant_count().try_into().unwrap_or(u32::MAX),
                sleeping: self.is_sleeping(),
                sequence,
                sent_at: now_millis,
            },
        )
    }

    fn apply_input(&mut self, input: &DomeSessionInputV1) -> Result<()> {
        let participant_id = input.participant_pubkey.as_str().to_string();
        let previous_sequence = self
            .last_input_sequence
            .get(&participant_id)
            .copied()
            .unwrap_or(0);
        if input.sequence <= previous_sequence {
            bail!("stale Dome session input sequence");
        }
        match &input.input {
            DomeSessionInputKindV1::Join { avatar_collider } => {
                self.ensure_avatar(&participant_id, avatar_collider.as_ref())?;
                self.participants.insert(participant_id.clone());
                self.participant_high_water = self
                    .participant_high_water
                    .max(self.participants.len().try_into().unwrap_or(u32::MAX));
            }
            DomeSessionInputKindV1::Leave => {
                self.participants.remove(&participant_id);
                self.prepared_exits.remove(&participant_id);
                self.transition_entries.remove(&participant_id);
                self.seated_on.remove(&participant_id);
                self.remove_body(&format!("avatar:{participant_id}"));
                for runtime_body in self.bodies_by_id.values_mut() {
                    if runtime_body.grabbed_by.as_deref() == Some(participant_id.as_str()) {
                        runtime_body.grabbed_by = None;
                    }
                }
            }
            DomeSessionInputKindV1::KeepAlive => {
                self.require_participant(&participant_id)?;
            }
            DomeSessionInputKindV1::Move {
                position,
                rotation,
                animation,
            } => {
                self.require_participant(&participant_id)?;
                let entity_id = format!("avatar:{participant_id}");
                let runtime_body = self
                    .bodies_by_id
                    .get_mut(&entity_id)
                    .context("participant avatar body is missing")?;
                let body = self
                    .rigid_bodies
                    .get_mut(runtime_body.handle)
                    .context("participant avatar rigid body is missing")?;
                let position = centimeters_to_meters(*position);
                let rotation = milliradians_to_radians(*rotation);
                body.set_translation(Vector::new(position[0], position[1], position[2]), true);
                body.set_rotation(
                    Rotation::from_scaled_axis(Vector::new(rotation[0], rotation[1], rotation[2])),
                    true,
                );
                runtime_body.animation = Some(animation.clone());
            }
            DomeSessionInputKindV1::Grab { prop_id } => {
                self.require_participant(&participant_id)?;
                let body = self
                    .bodies_by_id
                    .get_mut(prop_id)
                    .context("Dome prop does not exist")?;
                body.grabbed_by = Some(participant_id.clone());
            }
            DomeSessionInputKindV1::Throw { prop_id, impulse }
            | DomeSessionInputKindV1::Push { prop_id, impulse } => {
                self.require_participant(&participant_id)?;
                let runtime_body = self
                    .bodies_by_id
                    .get_mut(prop_id)
                    .context("Dome prop does not exist")?;
                if matches!(input.input, DomeSessionInputKindV1::Throw { .. })
                    && runtime_body.grabbed_by.as_deref() != Some(participant_id.as_str())
                {
                    bail!("participant cannot throw a prop it is not grabbing");
                }
                let impulse = centimeters_to_meters(*impulse);
                self.rigid_bodies
                    .get_mut(runtime_body.handle)
                    .context("Dome prop rigid body is missing")?
                    .apply_impulse(Vector::new(impulse[0], impulse[1], impulse[2]), true);
                runtime_body.grabbed_by = None;
            }
            DomeSessionInputKindV1::Sit { prop_id } => {
                self.require_participant(&participant_id)?;
                if !self.bodies_by_id.contains_key(prop_id) {
                    bail!("Dome seat prop does not exist");
                }
                self.seated_on
                    .insert(participant_id.clone(), prop_id.clone());
            }
            DomeSessionInputKindV1::PrepareTransition {
                transition_id,
                direction,
            } => {
                self.prepare_transition_exit(&participant_id, transition_id, *direction)?;
            }
            DomeSessionInputKindV1::AbortTransition { transition_id } => {
                self.abort_transition_exit(&participant_id, transition_id)?;
            }
            DomeSessionInputKindV1::CompleteTransition { transition_id } => {
                self.complete_transition_exit(&participant_id, transition_id)?;
            }
            DomeSessionInputKindV1::SpawnGuestProp { prop, expires_at } => {
                self.require_participant(&participant_id)?;
                if *expires_at <= self.last_simulated_at {
                    bail!("Dome guest prop is already expired");
                }
                self.insert_prop(
                    prop.clone(),
                    DomePhysicsBodyKindV1::GuestProp,
                    Some(*expires_at),
                    Some(participant_id.clone()),
                )?;
            }
            DomeSessionInputKindV1::UpsertPersistentProp { prop } => {
                self.require_owner(input)?;
                self.remove_body(&prop.prop_id);
                self.insert_prop(
                    prop.clone(),
                    DomePhysicsBodyKindV1::PersistentProp,
                    None,
                    None,
                )?;
            }
            DomeSessionInputKindV1::DeletePersistentProp { prop_id } => {
                self.require_owner(input)?;
                if self
                    .bodies_by_id
                    .get(prop_id)
                    .is_none_or(|body| body.kind != DomePhysicsBodyKindV1::PersistentProp)
                {
                    bail!("Dome persistent prop does not exist");
                }
                self.remove_body(prop_id);
            }
        }
        self.last_input_sequence
            .insert(participant_id, input.sequence);
        Ok(())
    }

    fn require_participant(&self, participant_id: &str) -> Result<()> {
        if !self.participants.contains(participant_id) {
            bail!("Dome session input requires a joined participant");
        }
        Ok(())
    }

    fn expire_stale_participants(&mut self, now_millis: i64) {
        let stale = self
            .participant_last_seen_at
            .iter()
            .filter(|(_, last_seen)| {
                now_millis.saturating_sub(**last_seen)
                    > kukuri_core::DOME_PARTICIPANT_TIMEOUT_MILLIS
            })
            .map(|(participant_id, _)| participant_id.clone())
            .collect::<Vec<_>>();
        for participant_id in stale {
            self.evict_participant(&kukuri_core::Pubkey::from(participant_id));
        }
    }

    fn require_owner(&self, input: &DomeSessionInputV1) -> Result<()> {
        if input.participant_pubkey != self.lease.lease.owner_pubkey {
            bail!("only the Dome owner can change persistent props");
        }
        Ok(())
    }

    fn preflight_input(&mut self, input: &DomeSessionInputV1, now_millis: i64) -> Result<()> {
        let participant_id = input.participant_pubkey.as_str().to_string();
        let input_bytes = serde_json::to_vec(input)?.len() as u64;
        {
            let player = self
                .player_budgets
                .entry(participant_id.clone())
                .or_default();
            if !window_allows(
                &mut player.input_bytes,
                now_millis,
                1_000,
                input_bytes,
                self.budget.player.max_input_bytes_per_second,
            ) {
                return Err(rejection(
                    MetaverseBudgetScope::Player,
                    MetaverseBudgetResource::InputBandwidth,
                    MetaverseResourceRejectionReason::RateExceeded,
                    player.input_bytes.count,
                    self.budget.player.max_input_bytes_per_second,
                ));
            }
        }
        if self.prepared_exits.contains_key(&participant_id)
            && matches!(
                input.input,
                DomeSessionInputKindV1::Join { .. }
                    | DomeSessionInputKindV1::Grab { .. }
                    | DomeSessionInputKindV1::Throw { .. }
                    | DomeSessionInputKindV1::Push { .. }
                    | DomeSessionInputKindV1::Sit { .. }
            )
        {
            bail!("DOME_TRANSITION_SOURCE_INPUT_FENCED");
        }
        match &input.input {
            DomeSessionInputKindV1::Join { .. } if !self.participants.contains(&participant_id) => {
                check_limit(
                    MetaverseBudgetScope::Host,
                    MetaverseBudgetResource::Participants,
                    self.participants.len() as u64 + 1,
                    u64::from(self.participant_limit),
                )?;
                self.check_body_capacity(1)?;
            }
            DomeSessionInputKindV1::SpawnGuestProp { prop, .. } => {
                {
                    let player = self
                        .player_budgets
                        .entry(participant_id.clone())
                        .or_default();
                    if !window_allows(
                        &mut player.spawns,
                        now_millis,
                        60_000,
                        1,
                        u64::from(self.budget.player.max_prop_spawns_per_minute),
                    ) {
                        return Err(rejection(
                            MetaverseBudgetScope::Player,
                            MetaverseBudgetResource::PropSpawnRate,
                            MetaverseResourceRejectionReason::RateExceeded,
                            player.spawns.count,
                            u64::from(self.budget.player.max_prop_spawns_per_minute),
                        ));
                    }
                }
                let guest_bodies = self.bodies_by_id.values().filter(|body| {
                    body.kind == DomePhysicsBodyKindV1::GuestProp
                        && body.created_by.as_deref() == Some(participant_id.as_str())
                });
                let (guest_count, guest_bytes) = guest_bodies
                    .fold((0_u64, 0_u64), |state, body| {
                        (state.0 + 1, state.1.saturating_add(body.asset_bytes))
                    });
                check_limit(
                    MetaverseBudgetScope::Player,
                    MetaverseBudgetResource::GuestProps,
                    guest_count + 1,
                    u64::from(self.budget.player.max_guest_props),
                )?;
                let asset_bytes = prop
                    .asset_ref
                    .as_ref()
                    .and_then(|asset| asset.size_bytes)
                    .unwrap_or_default();
                self.check_verified_asset(prop.asset_ref.as_ref())?;
                check_limit(
                    MetaverseBudgetScope::Player,
                    MetaverseBudgetResource::ModelBytes,
                    guest_bytes.saturating_add(asset_bytes),
                    self.budget.player.max_guest_prop_bytes,
                )?;
                self.check_body_capacity(1)?;
            }
            DomeSessionInputKindV1::UpsertPersistentProp { prop } => {
                self.check_verified_asset(prop.asset_ref.as_ref())?;
                let replacing = self.bodies_by_id.contains_key(&prop.prop_id);
                let persistent = self
                    .bodies_by_id
                    .values()
                    .filter(|body| body.kind == DomePhysicsBodyKindV1::PersistentProp)
                    .count() as u64;
                check_limit(
                    MetaverseBudgetScope::Dome,
                    MetaverseBudgetResource::PersistentProps,
                    persistent + u64::from(!replacing),
                    u64::from(self.budget.dome.max_persistent_props),
                )?;
                if !replacing {
                    self.check_body_capacity(1)?;
                }
            }
            DomeSessionInputKindV1::Grab { .. }
            | DomeSessionInputKindV1::Throw { .. }
            | DomeSessionInputKindV1::Push { .. }
            | DomeSessionInputKindV1::Sit { .. } => {
                {
                    let player = self.player_budgets.entry(participant_id).or_default();
                    if !window_allows(
                        &mut player.interactions,
                        now_millis,
                        1_000,
                        1,
                        u64::from(self.budget.player.max_interactions_per_second),
                    ) {
                        return Err(rejection(
                            MetaverseBudgetScope::Player,
                            MetaverseBudgetResource::InteractionRate,
                            MetaverseResourceRejectionReason::RateExceeded,
                            player.interactions.count,
                            u64::from(self.budget.player.max_interactions_per_second),
                        ));
                    }
                }
                if let DomeSessionInputKindV1::Throw { impulse, .. }
                | DomeSessionInputKindV1::Push { impulse, .. } = &input.input
                {
                    let observed = impulse
                        .iter()
                        .map(|value| value.unsigned_abs())
                        .max()
                        .unwrap_or_default();
                    check_limit(
                        MetaverseBudgetScope::Player,
                        MetaverseBudgetResource::Impulse,
                        observed,
                        self.budget.player.max_impulse_centimeters as u64,
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn check_body_capacity(&self, added: u64) -> Result<()> {
        let observed = self.bodies_by_id.len() as u64 + added;
        check_limit(
            MetaverseBudgetScope::Dome,
            MetaverseBudgetResource::RigidBodies,
            observed,
            u64::from(self.budget.dome.max_rigid_bodies),
        )?;
        check_limit(
            MetaverseBudgetScope::Dome,
            MetaverseBudgetResource::Colliders,
            observed,
            u64::from(self.budget.dome.max_colliders),
        )?;
        check_limit(
            MetaverseBudgetScope::Host,
            MetaverseBudgetResource::RigidBodies,
            observed,
            u64::from(self.budget.host.max_simulated_rigid_bodies),
        )
    }

    fn check_verified_asset(&self, asset: Option<&MetaverseAssetRef>) -> Result<()> {
        let Some(asset) = asset else {
            return Ok(());
        };
        validate_metaverse_asset_metadata(
            &asset.kind,
            asset.size_bytes,
            asset.budget_metadata.as_ref(),
        )?;
        if self.verified_assets.get(&asset.blob_hash) != Some(asset) {
            return Err(rejection(
                MetaverseBudgetScope::Host,
                MetaverseBudgetResource::AssetFormat,
                MetaverseResourceRejectionReason::UnverifiedAsset,
                1,
                0,
            ));
        }
        Ok(())
    }

    fn record_rejection(&mut self, rejection: MetaverseResourceRejection) {
        self.rejected_total = self.rejected_total.saturating_add(1);
        *self.rejection_counts.entry(rejection.code()).or_default() += 1;
    }

    fn insert_dome_boundaries(&mut self) {
        let floor = RigidBodyBuilder::fixed()
            .translation(Vector::new(0.0, -0.1, 0.0))
            .build();
        let floor_handle = self.rigid_bodies.insert(floor);
        self.colliders.insert_with_parent(
            ColliderBuilder::cuboid(20.0, 0.1, 20.0)
                .friction(0.8)
                .build(),
            floor_handle,
            &mut self.rigid_bodies,
        );
    }

    fn insert_prop(
        &mut self,
        prop: MetaversePersistentPropV1,
        kind: DomePhysicsBodyKindV1,
        expires_at: Option<i64>,
        created_by: Option<String>,
    ) -> Result<()> {
        let prop_id = prop.prop_id.clone();
        if self.bodies_by_id.contains_key(&prop_id) {
            bail!("Dome physics entity id already exists");
        }
        let position = centimeters_to_meters(prop.position);
        let rotation = milliradians_to_radians(prop.rotation);
        let rigid_body = RigidBodyBuilder::dynamic()
            .translation(Vector::new(position[0], position[1], position[2]))
            .rotation(Vector::new(rotation[0], rotation[1], rotation[2]))
            .ccd_enabled(true)
            .build();
        let handle = self.rigid_bodies.insert(rigid_body);
        let collider = collider_builder(prop.collider.as_ref())
            .friction(0.7)
            .restitution(0.15)
            .build();
        self.colliders
            .insert_with_parent(collider, handle, &mut self.rigid_bodies);
        self.bodies_by_id.insert(
            prop_id,
            RuntimeBody {
                handle,
                kind,
                animation: None,
                grabbed_by: None,
                expires_at,
                persistent_definition: (kind == DomePhysicsBodyKindV1::PersistentProp)
                    .then_some(prop.clone()),
                created_by,
                asset_bytes: prop
                    .asset_ref
                    .as_ref()
                    .and_then(|asset| asset.size_bytes)
                    .unwrap_or_default(),
            },
        );
        self.rigid_body_high_water = self
            .rigid_body_high_water
            .max(self.bodies_by_id.len().try_into().unwrap_or(u32::MAX));
        Ok(())
    }

    fn ensure_avatar(
        &mut self,
        participant_id: &str,
        collider: Option<&kukuri_core::MetaverseColliderV1>,
    ) -> Result<()> {
        let entity_id = format!("avatar:{participant_id}");
        if self.bodies_by_id.contains_key(&entity_id) {
            return Ok(());
        }
        let fallback = kukuri_core::fallback_capsule_collider([-25, 0, -25], [25, 180, 25])?;
        let collider = collider.unwrap_or(&fallback);
        let spawn = self.safe_avatar_spawn(collider)?;
        let position = centimeters_to_meters(spawn.position);
        let body = RigidBodyBuilder::kinematic_position_based()
            .translation(Vector::new(position[0], position[1], position[2]))
            .build();
        let handle = self.rigid_bodies.insert(body);
        self.colliders.insert_with_parent(
            collider_builder(Some(collider)).build(),
            handle,
            &mut self.rigid_bodies,
        );
        self.bodies_by_id.insert(
            entity_id,
            RuntimeBody {
                handle,
                kind: DomePhysicsBodyKindV1::Avatar,
                animation: Some("idle".into()),
                grabbed_by: None,
                expires_at: None,
                persistent_definition: None,
                created_by: None,
                asset_bytes: 0,
            },
        );
        self.rigid_body_high_water = self
            .rigid_body_high_water
            .max(self.bodies_by_id.len().try_into().unwrap_or(u32::MAX));
        Ok(())
    }

    fn safe_avatar_spawn(
        &self,
        collider: &kukuri_core::MetaverseColliderV1,
    ) -> Result<kukuri_core::MetaverseRoomSpawnV1> {
        const STEP_CM: i64 = 150;
        const SAFETY_MARGIN_CM: i64 = 25;
        const OFFSETS: [[i64; 2]; 25] = [
            [0, 0],
            [1, 0],
            [0, 1],
            [-1, 0],
            [0, -1],
            [1, 1],
            [-1, 1],
            [-1, -1],
            [1, -1],
            [2, 0],
            [0, 2],
            [-2, 0],
            [0, -2],
            [2, 1],
            [1, 2],
            [-1, 2],
            [-2, 1],
            [-2, -1],
            [-1, -2],
            [1, -2],
            [2, -1],
            [2, 2],
            [-2, 2],
            [-2, -2],
            [2, -2],
        ];
        for [offset_x, offset_z] in OFFSETS {
            let candidate = kukuri_core::MetaverseRoomSpawnV1 {
                position: [
                    self.default_spawn.position[0] + offset_x * STEP_CM,
                    self.default_spawn.position[1],
                    self.default_spawn.position[2] + offset_z * STEP_CM,
                ],
                rotation: self.default_spawn.rotation,
            };
            let bounds = collider_bounds_cm(collider, candidate.position, SAFETY_MARGIN_CM);
            if !spawn_bounds_inside_dome(bounds) || self.spawn_bounds_overlap_body(bounds) {
                continue;
            }
            return Ok(candidate);
        }
        bail!("DOME_ENTRY_NO_SAFE_SPAWN")
    }

    fn spawn_bounds_overlap_body(&self, candidate: ([i64; 3], [i64; 3])) -> bool {
        self.bodies_by_id.values().any(|runtime_body| {
            let Some(body) = self.rigid_bodies.get(runtime_body.handle) else {
                return true;
            };
            body.colliders().iter().any(|handle| {
                let Some(collider) = self.colliders.get(*handle) else {
                    return true;
                };
                let aabb = collider.compute_aabb();
                let minimum = meters_to_centimeters([aabb.mins.x, aabb.mins.y, aabb.mins.z]);
                let maximum = meters_to_centimeters([aabb.maxs.x, aabb.maxs.y, aabb.maxs.z]);
                aabb_overlaps(candidate, (minimum, maximum))
            })
        })
    }
}

fn collider_bounds_cm(
    collider: &kukuri_core::MetaverseColliderV1,
    origin: [i64; 3],
    margin: i64,
) -> ([i64; 3], [i64; 3]) {
    let (center, extents) = match collider {
        kukuri_core::MetaverseColliderV1::Capsule {
            center,
            radius,
            half_height,
        } => (*center, [*radius, half_height + radius, *radius]),
        kukuri_core::MetaverseColliderV1::Cuboid {
            center,
            half_extents,
        } => (*center, *half_extents),
    };
    let center = [
        origin[0] + center[0],
        origin[1] + center[1],
        origin[2] + center[2],
    ];
    (
        [
            center[0] - extents[0] - margin,
            center[1] - extents[1],
            center[2] - extents[2] - margin,
        ],
        [
            center[0] + extents[0] + margin,
            center[1] + extents[1],
            center[2] + extents[2] + margin,
        ],
    )
}

fn spawn_bounds_inside_dome(bounds: ([i64; 3], [i64; 3])) -> bool {
    let spec = kukuri_core::fixed_dome_v1();
    if bounds.0[1] < 0 {
        return false;
    }
    for x in [bounds.0[0], bounds.1[0]] {
        for z in [bounds.0[2], bounds.1[2]] {
            let horizontal_squared = i128::from(x) * i128::from(x) + i128::from(z) * i128::from(z);
            let radius_squared =
                i128::from(spec.inner_radius_cm) * i128::from(spec.inner_radius_cm);
            if horizontal_squared >= radius_squared {
                return false;
            }
            let ceiling_squared = radius_squared - horizontal_squared;
            if i128::from(bounds.1[1]) * i128::from(bounds.1[1]) > ceiling_squared {
                return false;
            }
        }
    }
    true
}

fn aabb_overlaps(left: ([i64; 3], [i64; 3]), right: ([i64; 3], [i64; 3])) -> bool {
    (0..3).all(|axis| left.0[axis] < right.1[axis] && left.1[axis] > right.0[axis])
}

#[cfg(test)]
mod tests;
