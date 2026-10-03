use anyhow::{Context, Result, bail};
use kukuri_core::{
    DOME_TRANSITION_TICKET_TTL_MILLIS, DomeDirection, DomePhysicsBodyKindV1,
    DomeTransitionAccessDecisionV1, DomeTransitionAdmissionRequestV1,
    DomeTransitionAdmissionTicketV1, fixed_dome_v1, opposite_dome_direction,
};
use rapier3d::prelude::*;

use super::DomeSessionRuntime;
use super::support::{centimeters_to_meters, milliradians_to_radians};

#[derive(Clone, Debug)]
pub(super) struct PreparedExit {
    pub(super) transition_id: String,
    pub(super) direction: DomeDirection,
}

impl DomeSessionRuntime {
    pub fn transition_reservation_count(&self) -> usize {
        self.transition_reservations.len()
    }

    pub fn revoke_transition_access(
        &mut self,
        participant_pubkey: &kukuri_core::Pubkey,
        connection_id: Option<&str>,
    ) -> usize {
        let before = self.transition_reservations.len();
        self.transition_reservations.retain(|_, ticket| {
            ticket.request.participant_pubkey != *participant_pubkey
                && connection_id.is_none_or(|id| ticket.request.connection_id != id)
        });
        self.committed_transitions.retain(|_, ticket| {
            ticket.request.participant_pubkey != *participant_pubkey
                && connection_id.is_none_or(|id| ticket.request.connection_id != id)
        });
        self.prepared_exits.remove(participant_pubkey.as_str());
        before.saturating_sub(self.transition_reservations.len())
    }

    pub fn drain_connection(&mut self, connection_id: &str) -> usize {
        let participants = self
            .transition_reservations
            .values()
            .filter(|ticket| ticket.request.connection_id == connection_id)
            .map(|ticket| ticket.request.participant_pubkey.as_str().to_string())
            .collect::<Vec<_>>();
        let before = self.transition_reservations.len();
        self.transition_reservations
            .retain(|_, ticket| ticket.request.connection_id != connection_id);
        for participant in participants {
            self.prepared_exits.remove(&participant);
        }
        before.saturating_sub(self.transition_reservations.len())
    }

    pub fn evict_participant(&mut self, participant_pubkey: &kukuri_core::Pubkey) -> bool {
        let participant_id = participant_pubkey.as_str();
        let evicted = self.participants.remove(participant_id);
        self.revoke_transition_access(participant_pubkey, None);
        self.transition_entries.remove(participant_id);
        self.seated_on.remove(participant_id);
        if evicted && participant_pubkey != &self.lease.lease.owner_pubkey {
            self.inputs
                .retire(participant_id, self.participant_limit as usize);
        }
        self.participant_last_seen_at.remove(participant_id);
        self.player_budgets.remove(participant_id);
        self.remove_body(&format!("avatar:{participant_id}"));
        for runtime_body in self.bodies_by_id.values_mut() {
            if runtime_body.grabbed_by.as_deref() == Some(participant_id) {
                runtime_body.grabbed_by = None;
            }
        }
        evicted
    }

    pub fn prepare_transition_admission(
        &mut self,
        request: DomeTransitionAdmissionRequestV1,
        access: DomeTransitionAccessDecisionV1,
        now_millis: i64,
    ) -> Result<DomeTransitionAdmissionTicketV1> {
        self.expire_transition_reservations(now_millis);
        request.validate()?;
        if let DomeTransitionAccessDecisionV1::Denied { reason } = access {
            bail!(reason.code());
        }
        if request.spatial_context != self.lease.lease.spatial_context
            || request.target_instance_id != self.lease.lease.instance_id
            || request.target_instance_generation != self.lease.lease.instance_generation
        {
            bail!("DOME_TRANSITION_STALE_TOPOLOGY");
        }
        if let Some(ticket) = self.committed_transitions.get(&request.transition_id) {
            if ticket.request == request {
                return Ok(ticket.clone());
            }
            bail!("DOME_TRANSITION_INVALID_TICKET");
        }
        if let Some(ticket) = self.transition_reservations.get(&request.transition_id) {
            if ticket.request == request {
                return Ok(ticket.clone());
            }
            bail!("DOME_TRANSITION_INVALID_TICKET");
        }
        if self
            .transition_reservations
            .values()
            .any(|ticket| ticket.request.participant_pubkey == request.participant_pubkey)
        {
            bail!("participant already has another Dome transition reservation");
        }
        let already_joined = self
            .participants
            .contains(request.participant_pubkey.as_str());
        let participant_slots = self.participants.len()
            + self.transition_reservations.len()
            + usize::from(!already_joined);
        if participant_slots > self.participant_limit as usize {
            bail!("DOME_TRANSITION_CAPACITY_FULL");
        }
        if !already_joined {
            self.check_body_capacity(1)?;
        }
        let ticket = DomeTransitionAdmissionTicketV1 {
            request,
            target_lease_epoch: self.lease.lease.epoch,
            target_session_id: self.session_id.clone(),
            expires_at: now_millis.saturating_add(DOME_TRANSITION_TICKET_TTL_MILLIS),
        };
        self.transition_reservations
            .insert(ticket.request.transition_id.clone(), ticket.clone());
        Ok(ticket)
    }

    pub fn commit_transition_admission(
        &mut self,
        ticket: &DomeTransitionAdmissionTicketV1,
        position: [i64; 3],
        rotation: [i64; 3],
        now_millis: i64,
    ) -> Result<()> {
        self.expire_transition_reservations(now_millis);
        if let Some(committed) = self
            .committed_transitions
            .get(&ticket.request.transition_id)
        {
            if committed == ticket
                && ticket.target_lease_epoch == self.lease.lease.epoch
                && ticket.target_session_id == self.session_id
            {
                return Ok(());
            }
            bail!("DOME_TRANSITION_INVALID_TICKET");
        }
        ticket.validate_for(
            &ticket.request,
            self.lease.lease.epoch,
            &self.session_id,
            now_millis,
        )?;
        if self
            .transition_reservations
            .get(&ticket.request.transition_id)
            != Some(ticket)
        {
            bail!("DOME_TRANSITION_INVALID_TICKET");
        }
        let participant_id = ticket.request.participant_pubkey.as_str().to_string();
        self.ensure_avatar(&participant_id, None)?;
        self.participants.insert(participant_id.clone());
        self.participant_last_seen_at
            .insert(participant_id.clone(), now_millis);
        self.set_avatar_transform(&participant_id, position, rotation)?;
        self.transition_entries.insert(
            participant_id.clone(),
            opposite_dome_direction(ticket.request.direction),
        );
        self.prepared_exits.remove(&participant_id);
        self.transition_reservations
            .remove(&ticket.request.transition_id);
        // commit の再送に応えるため、参加者ごとに最後の commit だけを残す。
        self.committed_transitions.retain(|_, committed| {
            committed.request.participant_pubkey != ticket.request.participant_pubkey
        });
        self.committed_transitions
            .insert(ticket.request.transition_id.clone(), ticket.clone());
        self.clamp_bodies_to_dome();
        self.participant_high_water = self
            .participant_high_water
            .max(self.participants.len().try_into().unwrap_or(u32::MAX));
        Ok(())
    }

    pub fn abort_transition_admission(
        &mut self,
        transition_id: &str,
        participant_pubkey: &kukuri_core::Pubkey,
        now_millis: i64,
    ) -> Result<()> {
        self.expire_transition_reservations(now_millis);
        if self.committed_transitions.contains_key(transition_id) {
            return Ok(());
        }
        if let Some(ticket) = self.transition_reservations.get(transition_id)
            && &ticket.request.participant_pubkey != participant_pubkey
        {
            bail!("DOME_TRANSITION_INVALID_TICKET");
        }
        self.transition_reservations.remove(transition_id);
        Ok(())
    }

    pub(super) fn prepare_transition_exit(
        &mut self,
        participant_id: &str,
        transition_id: &str,
        direction: DomeDirection,
    ) -> Result<()> {
        self.require_participant(participant_id)?;
        if transition_id.trim().is_empty() {
            bail!("Dome transition id is required");
        }
        if let Some(existing) = self.prepared_exits.get(participant_id) {
            if existing.transition_id == transition_id && existing.direction == direction {
                return Ok(());
            }
            bail!("participant already has another prepared Dome transition");
        }
        for runtime_body in self.bodies_by_id.values_mut() {
            if runtime_body.grabbed_by.as_deref() == Some(participant_id) {
                runtime_body.grabbed_by = None;
            }
        }
        self.seated_on.remove(participant_id);
        self.prepared_exits.insert(
            participant_id.to_string(),
            PreparedExit {
                transition_id: transition_id.to_string(),
                direction,
            },
        );
        Ok(())
    }

    pub(super) fn abort_transition_exit(
        &mut self,
        participant_id: &str,
        transition_id: &str,
    ) -> Result<()> {
        if let Some(prepared) = self.prepared_exits.get(participant_id) {
            if prepared.transition_id != transition_id {
                bail!("DOME_TRANSITION_INVALID_TICKET");
            }
            self.prepared_exits.remove(participant_id);
        }
        Ok(())
    }

    pub(super) fn complete_transition_exit(
        &mut self,
        participant_pubkey: &kukuri_core::Pubkey,
        transition_id: &str,
    ) -> Result<()> {
        // 応答を失った完了の再送は、退出済みなら何もせず成功とする。
        if !self.participants.contains(participant_pubkey.as_str()) {
            return Ok(());
        }
        let prepared = self
            .prepared_exits
            .get(participant_pubkey.as_str())
            .context("participant has no prepared Dome transition")?;
        if prepared.transition_id != transition_id {
            bail!("DOME_TRANSITION_INVALID_TICKET");
        }
        self.evict_participant(participant_pubkey);
        Ok(())
    }

    fn expire_transition_reservations(&mut self, now_millis: i64) {
        self.transition_reservations
            .retain(|_, ticket| ticket.expires_at > now_millis);
    }

    fn set_avatar_transform(
        &mut self,
        participant_id: &str,
        position: [i64; 3],
        rotation: [i64; 3],
    ) -> Result<()> {
        let entity_id = format!("avatar:{participant_id}");
        let runtime_body = self
            .bodies_by_id
            .get(&entity_id)
            .context("participant avatar body is missing")?;
        let position = centimeters_to_meters(position);
        let rotation = milliradians_to_radians(rotation);
        let body = self
            .rigid_bodies
            .get_mut(runtime_body.handle)
            .context("participant avatar rigid body is missing")?;
        body.set_translation(Vector::new(position[0], position[1], position[2]), true);
        body.set_rotation(
            Rotation::from_scaled_axis(Vector::new(rotation[0], rotation[1], rotation[2])),
            true,
        );
        Ok(())
    }

    pub(super) fn expire_guest_props(&mut self, now_millis: i64) {
        let expired: Vec<String> = self
            .bodies_by_id
            .iter()
            .filter(|(_, body)| {
                body.kind == DomePhysicsBodyKindV1::GuestProp
                    && body
                        .expires_at
                        .is_some_and(|expires_at| expires_at <= now_millis)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.remove_body(&id);
        }
    }

    pub(super) fn remove_body(&mut self, entity_id: &str) {
        if let Some(body) = self.bodies_by_id.remove(entity_id) {
            self.rigid_bodies.remove(
                body.handle,
                &mut self.island_manager,
                &mut self.colliders,
                &mut self.impulse_joints,
                &mut self.multibody_joints,
                &mut self.soft_bodies,
                true,
            );
        }
    }

    pub(super) fn clamp_bodies_to_dome(&mut self) {
        let spec = fixed_dome_v1();
        let radius = spec.inner_radius_cm as f32 / 100.0;
        let corridor_limit =
            (spec.connection_boundary_offset_cm + spec.connection_zone_depth_cm / 2) as f32 / 100.0;
        let opening_half_width = spec.opening_width_cm as f32 / 200.0;
        let opening_height = spec.opening_height_cm as f32 / 100.0;
        let mut completed_entries = Vec::new();
        for (entity_id, runtime_body) in &self.bodies_by_id {
            let Some(body) = self.rigid_bodies.get_mut(runtime_body.handle) else {
                continue;
            };
            let mut translation = body.translation();
            translation.y = translation.y.clamp(0.0, radius);
            let participant_id = entity_id.strip_prefix("avatar:");
            let permitted_direction = participant_id.and_then(|participant_id| {
                self.prepared_exits
                    .get(participant_id)
                    .map(|exit| exit.direction)
                    .or_else(|| self.transition_entries.get(participant_id).copied())
            });
            let (axis, tangent) = match permitted_direction {
                Some(DomeDirection::North) => (-translation.z, translation.x),
                Some(DomeDirection::East) => (translation.x, translation.z),
                Some(DomeDirection::South) => (translation.z, translation.x),
                Some(DomeDirection::West) => (-translation.x, translation.z),
                None => (0.0, 0.0),
            };
            let inside_opening = permitted_direction.is_some()
                && tangent.abs() <= opening_half_width
                && translation.y <= opening_height;
            if inside_opening && axis >= radius {
                let clamped_axis = axis.min(corridor_limit);
                match permitted_direction.expect("checked above") {
                    DomeDirection::North => translation.z = -clamped_axis,
                    DomeDirection::East => translation.x = clamped_axis,
                    DomeDirection::South => translation.z = clamped_axis,
                    DomeDirection::West => translation.x = -clamped_axis,
                }
            } else {
                let horizontal_limit = (radius * radius - translation.y * translation.y).sqrt();
                let horizontal_distance =
                    (translation.x * translation.x + translation.z * translation.z).sqrt();
                if horizontal_distance > horizontal_limit && horizontal_distance > 0.0 {
                    let scale = (horizontal_limit - 0.05).max(0.0) / horizontal_distance;
                    translation.x *= scale;
                    translation.z *= scale;
                }
                if let Some(participant_id) = participant_id
                    && self.transition_entries.contains_key(participant_id)
                {
                    completed_entries.push(participant_id.to_string());
                }
            }
            body.set_translation(translation, true);
        }
        for participant_id in completed_entries {
            self.transition_entries.remove(&participant_id);
        }
    }
}
