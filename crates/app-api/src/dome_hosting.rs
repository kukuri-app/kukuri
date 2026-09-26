use crate::service::*;
use crate::views::{
    AbortDomeTransitionInput, ActivateCommunityNodeDomeHostingInput, CloseDomeHostingInput,
    CommitDomeLayoutInput, CommitDomeTransitionInput, DomeHostingView, DomeLayoutCommitOutcome,
    DomeLayoutCommitView, PrepareCommunityNodeDomeHostingInput, PrepareDomeTransitionInput,
    ResyncDomeSnapshotsInput, StartOwnerDomeHostingInput, SubmitDomeSessionInput,
    UpdateMetaverseRoomInput,
};
use kukuri_core::{
    DOME_HOSTING_MAX_LEASE_MILLIS, DOME_LAYOUT_COMMIT_MIN_INTERVAL_MILLIS, DomeHostTargetV1,
    DomeHostingLeaseV1, DomeHostingRecordV1, DomeHostingStateKindV1, DomeInstanceStatusV1,
    DomeLayoutCommitV1, DomeSessionInputKindV1, DomeTransitionAccessDecisionV1,
    DomeTransitionAdmissionTicketV1, SignedDomeHostingAcceptanceV1, SignedDomeHostingLeaseV1,
    SignedDomeLayoutCandidateV1, SignedDomeLayoutCommitV1, SignedDomePhysicsSnapshotV1,
    SpatialContextV1, accept_dome_hosting_lease, activate_dome_hosting_lease,
    build_signed_dome_hosting_lease, build_signed_dome_layout_commit,
    build_signed_dome_session_input, close_dome_hosting_lease, dome_layout_candidate_digest,
    resolve_dome_hosting_state, verify_signed_dome_host_heartbeat,
    verify_signed_dome_layout_candidate,
};

const HOSTING_RECORD_PREFIX: &str = "metaverse/dome-hosting";
const LAYOUT_COMMIT_PREFIX: &str = "metaverse/dome-layout-commits";
const DEFAULT_LEASE_MILLIS: i64 = 24 * 60 * 60 * 1_000;

impl AppService {
    pub async fn get_dome_hosting(
        &self,
        spatial_context: SpatialContextV1,
        instance_id: &str,
    ) -> Result<DomeHostingView> {
        let replica = self.hosting_context_replica(&spatial_context).await?;
        let instance = self
            .hosting_instance(&replica, &spatial_context, instance_id)
            .await?
            .context("Dome instance was not found")?;
        self.hosting_view_for_instance(&replica, &instance).await
    }

    pub(crate) async fn hosting_view_for_instance(
        &self,
        replica: &ReplicaId,
        instance: &DomeInstanceManifestV1,
    ) -> Result<DomeHostingView> {
        let records = self
            .list_dome_hosting_records(replica, &instance.instance_id)
            .await?;
        self.hosting_view(instance, &records, Utc::now().timestamp_millis())
            .await
    }

    pub async fn start_owner_dome_hosting(
        &self,
        input: StartOwnerDomeHostingInput,
    ) -> Result<DomeHostingView> {
        let _guard = self.services.dome_mutations.lock().await;
        // hosting は topic(channel なら channel も)を参加の holder で取る。上限なら始めない(#1221 R2-C)。
        let holder = dome_holder(&input.instance_id);
        let context = &input.spatial_context;
        self.set_scope_holder(
            &holder,
            participation_keys(context.topic_id().as_str(), context.channel_id()),
        )
        .await?;
        let instance_id = input.instance_id.clone();
        let started = self.start_owner_dome_hosting_unlocked(input).await;
        if started.is_err()
            && !self
                .dome_host_sessions
                .lock()
                .await
                .contains_key(&instance_id)
        {
            self.release_scope_holder(&holder).await;
        }
        started
    }

    async fn start_owner_dome_hosting_unlocked(
        &self,
        input: StartOwnerDomeHostingInput,
    ) -> Result<DomeHostingView> {
        if input.endpoint_id.trim().is_empty() {
            anyhow::bail!("owner device endpoint id is required");
        }
        let replica = self.hosting_context_replica(&input.spatial_context).await?;
        let instance = self
            .hosting_instance(&replica, &input.spatial_context, &input.instance_id)
            .await?
            .context("Dome instance was not found")?;
        if input
            .expected_generation
            .is_some_and(|generation| generation != instance.generation)
        {
            anyhow::bail!("DOME_HOSTING_STALE_INSTANCE");
        }
        self.ensure_dome_hosting_owner(&instance)?;
        self.ensure_dome_not_deleting(
            &instance.spatial_context,
            &instance.instance_id,
            instance.generation,
        )
        .await?;
        let preset = self
            .fetch_dome_preset_manifest(&instance.preset_ref)
            .await?
            .ok_or(DomeReadUnavailable::Preset)?;
        let records = self
            .list_dome_hosting_records(&replica, &input.instance_id)
            .await?;
        let now = Utc::now().timestamp_millis();
        let epoch = next_hosting_epoch(&records);
        let lease = build_signed_dome_hosting_lease(
            self.services.keys.as_ref(),
            DomeHostingLeaseV1 {
                lease_id: format!("lease-{}-{epoch}", instance.instance_id),
                spatial_context: instance.spatial_context.clone(),
                instance_id: instance.instance_id.clone(),
                instance_generation: instance.generation,
                owner_pubkey: instance.owner_pubkey.clone(),
                host: DomeHostTargetV1::OwnerDevice {
                    endpoint_id: input.endpoint_id,
                    host_pubkey: self.services.keys.public_key(),
                },
                manifest_blob_hash: instance.preset_ref.manifest_blob_hash.clone(),
                manifest_version: instance.preset_ref.revision,
                epoch,
                issued_at: now,
                expires_at: lease_expiry(now, input.lease_duration_millis)?,
            },
        )?;
        let session_id = format!("dome-session-{}-{epoch}-{now}", instance.instance_id);
        let acceptance =
            accept_dome_hosting_lease(self.services.keys.as_ref(), &lease, &session_id, now)?;
        let activation =
            activate_dome_hosting_lease(self.services.keys.as_ref(), &lease, &acceptance, now)?;
        let runtime = DomeSessionRuntime::start_with_budget(
            lease.clone(),
            self.services.keys.as_ref().clone(),
            &instance,
            &preset,
            &session_id,
            now,
            self.metaverse_resource_budget.clone(),
        )?;
        let new_records = [
            DomeHostingRecordV1::LeaseIssued(lease),
            DomeHostingRecordV1::HostAccepted(acceptance),
            DomeHostingRecordV1::LeaseActivated(activation),
        ];
        for record in &new_records {
            self.persist_dome_hosting_record(&replica, &instance.instance_id, record)
                .await?;
        }
        self.dome_host_sessions
            .lock()
            .await
            .insert(instance.instance_id.clone(), runtime);
        self.spawn_owner_dome_heartbeat_task(
            instance.spatial_context.clone(),
            instance.instance_id.clone(),
            session_id.clone(),
        )
        .await;
        let mut all_records = records;
        all_records.extend(new_records);
        self.publish_dome_hosting_hint(
            &instance.spatial_context,
            &instance.instance_id,
            &session_id,
        )
        .await?;
        self.hosting_view(&instance, &all_records, now).await
    }

    pub async fn prepare_community_node_dome_hosting(
        &self,
        input: PrepareCommunityNodeDomeHostingInput,
    ) -> Result<DomeHostingView> {
        let _guard = self.services.dome_mutations.lock().await;
        self.prepare_community_node_dome_hosting_unlocked(input)
            .await
    }

    async fn prepare_community_node_dome_hosting_unlocked(
        &self,
        input: PrepareCommunityNodeDomeHostingInput,
    ) -> Result<DomeHostingView> {
        let replica = self.hosting_context_replica(&input.spatial_context).await?;
        let instance = self
            .hosting_instance(&replica, &input.spatial_context, &input.instance_id)
            .await?
            .context("Dome instance was not found")?;
        if input
            .expected_generation
            .is_some_and(|generation| generation != instance.generation)
        {
            anyhow::bail!("DOME_HOSTING_STALE_INSTANCE");
        }
        self.ensure_dome_hosting_owner(&instance)?;
        self.ensure_dome_not_deleting(
            &instance.spatial_context,
            &instance.instance_id,
            instance.generation,
        )
        .await?;
        let mut records = self
            .list_dome_hosting_records(&replica, &input.instance_id)
            .await?;
        let now = Utc::now().timestamp_millis();
        let epoch = next_hosting_epoch(&records);
        let lease = build_signed_dome_hosting_lease(
            self.services.keys.as_ref(),
            DomeHostingLeaseV1 {
                lease_id: format!("lease-{}-{epoch}", instance.instance_id),
                spatial_context: instance.spatial_context.clone(),
                instance_id: instance.instance_id.clone(),
                instance_generation: instance.generation,
                owner_pubkey: instance.owner_pubkey.clone(),
                host: DomeHostTargetV1::CommunityNode {
                    node_id: Pubkey::from(input.node_id),
                    api_base_url: input.api_base_url,
                },
                manifest_blob_hash: instance.preset_ref.manifest_blob_hash.clone(),
                manifest_version: instance.preset_ref.revision,
                epoch,
                issued_at: now,
                expires_at: lease_expiry(now, input.lease_duration_millis)?,
            },
        )?;
        let record = DomeHostingRecordV1::LeaseIssued(lease);
        self.persist_dome_hosting_record(&replica, &instance.instance_id, &record)
            .await?;
        records.push(record);
        self.stop_owner_dome_hosting(&instance.instance_id).await;
        self.publish_dome_hosting_hint(
            &instance.spatial_context,
            &instance.instance_id,
            "transfer",
        )
        .await?;
        self.hosting_view(&instance, &records, now).await
    }

    pub async fn activate_community_node_dome_hosting(
        &self,
        input: ActivateCommunityNodeDomeHostingInput,
    ) -> Result<DomeHostingView> {
        let _guard = self.services.dome_mutations.lock().await;
        let replica = self.hosting_context_replica(&input.spatial_context).await?;
        let instance = self
            .hosting_instance(&replica, &input.spatial_context, &input.instance_id)
            .await?
            .context("Dome instance was not found")?;
        self.ensure_dome_hosting_owner(&instance)?;
        self.ensure_dome_not_deleting(
            &instance.spatial_context,
            &instance.instance_id,
            instance.generation,
        )
        .await?;
        let mut records = self
            .list_dome_hosting_records(&replica, &input.instance_id)
            .await?;
        let lease = current_unique_lease(&records)?.context("no pending Hosting Lease")?;
        if !matches!(lease.lease.host, DomeHostTargetV1::CommunityNode { .. }) {
            anyhow::bail!("pending Hosting Lease does not target a Community Node");
        }
        let acceptance: SignedDomeHostingAcceptanceV1 =
            serde_json::from_str(&input.signed_acceptance_json)
                .context("invalid Community Node hosting acceptance")?;
        let now = Utc::now().timestamp_millis();
        let activation =
            activate_dome_hosting_lease(self.services.keys.as_ref(), &lease, &acceptance, now)?;
        let new_records = [
            DomeHostingRecordV1::HostAccepted(acceptance),
            DomeHostingRecordV1::LeaseActivated(activation),
        ];
        for record in &new_records {
            self.persist_dome_hosting_record(&replica, &instance.instance_id, record)
                .await?;
        }
        records.extend(new_records);
        self.publish_dome_hosting_hint(
            &instance.spatial_context,
            &instance.instance_id,
            "community-node-active",
        )
        .await?;
        self.hosting_view(&instance, &records, now).await
    }

    pub async fn close_dome_hosting(
        &self,
        input: CloseDomeHostingInput,
    ) -> Result<DomeHostingView> {
        let _guard = self.services.dome_mutations.lock().await;
        let context = input.spatial_context.clone();
        let id = input.instance_id.clone();
        self.close_dome_hosting_unlocked(input).await?;
        self.get_dome_hosting(context, &id).await
    }

    pub(crate) async fn close_dome_hosting_unlocked(
        &self,
        input: CloseDomeHostingInput,
    ) -> Result<DomeHostingView> {
        let replica = self.hosting_context_replica(&input.spatial_context).await?;
        let instance = self
            .hosting_instance(&replica, &input.spatial_context, &input.instance_id)
            .await?
            .context("Dome instance was not found")?;
        if input
            .expected_generation
            .is_some_and(|generation| generation != instance.generation)
        {
            anyhow::bail!("DOME_HOSTING_STALE_INSTANCE");
        }
        self.ensure_dome_hosting_owner(&instance)?;
        let mut records = self
            .list_dome_hosting_records(&replica, &input.instance_id)
            .await?;
        let lease = current_unique_lease(&records)?.context("no Hosting Lease to close")?;
        if lease.lease.instance_generation != instance.generation {
            anyhow::bail!("no current-generation Hosting Lease to close");
        }
        let now = Utc::now().timestamp_millis();
        let record = DomeHostingRecordV1::LeaseClosed(close_dome_hosting_lease(
            self.services.keys.as_ref(),
            &lease,
            now,
        )?);
        self.persist_dome_hosting_record(&replica, &instance.instance_id, &record)
            .await?;
        records.push(record);
        self.stop_owner_dome_hosting(&instance.instance_id).await;
        self.publish_dome_hosting_hint(&instance.spatial_context, &instance.instance_id, "closed")
            .await?;
        self.hosting_authority_view(&instance, &records, now).await
    }

    pub async fn submit_dome_session_input(
        &self,
        input: SubmitDomeSessionInput,
    ) -> Result<SignedDomePhysicsSnapshotV1> {
        let replica = self.hosting_context_replica(&input.spatial_context).await?;
        let instance = self
            .hosting_instance(&replica, &input.spatial_context, &input.instance_id)
            .await?
            .context("Dome instance was not found")?;
        if input
            .expected_generation
            .is_some_and(|generation| generation != instance.generation)
        {
            anyhow::bail!("DOME_SESSION_STALE_INSTANCE");
        }
        if instance.status != DomeInstanceStatusV1::Active || instance.relationship_detach.is_some()
        {
            anyhow::bail!("DOME_SESSION_STALE_INSTANCE");
        }
        if matches!(
            &input.input,
            DomeSessionInputKindV1::Join { .. } | DomeSessionInputKindV1::KeepAlive
        ) {
            match self
                .evaluate_dome_room_access(
                    &input.spatial_context,
                    &instance.owner_pubkey,
                    &self.services.keys.public_key(),
                )
                .await?
            {
                DomeTransitionAccessDecisionV1::Allowed => {}
                DomeTransitionAccessDecisionV1::Denied { reason } => {
                    anyhow::bail!(reason.code())
                }
            }
        }
        let now = Utc::now().timestamp_millis();
        let mut sessions = self.dome_host_sessions.lock().await;
        let runtime = sessions
            .get_mut(&input.instance_id)
            .context("this device is not the active Dome host")?;
        if runtime.lease().spatial_context != input.spatial_context
            || runtime.lease().instance_generation != instance.generation
        {
            anyhow::bail!("Dome session input SpatialContext mismatch");
        }
        let signed = build_signed_dome_session_input(
            self.services.keys.as_ref(),
            kukuri_core::DomeSessionInputV1 {
                input_id: format!("input-{}-{}", input.instance_id, input.sequence),
                instance_id: input.instance_id,
                instance_generation: runtime.lease().instance_generation,
                lease_epoch: runtime.lease().epoch,
                session_id: runtime.session_id().to_string(),
                participant_pubkey: self.services.keys.public_key(),
                sequence: input.sequence,
                sent_at: now,
                input: input.input,
            },
        )?;
        let admission = matches!(&signed.input.input, DomeSessionInputKindV1::Join { .. });
        runtime.apply_signed_input_at(&signed, now)?;
        if admission {
            runtime.signed_admission_snapshot(now)
        } else {
            runtime.signed_snapshot(now)
        }
    }

    pub async fn prepare_dome_transition(
        &self,
        input: PrepareDomeTransitionInput,
    ) -> Result<DomeTransitionAdmissionTicketV1> {
        input.request.validate()?;
        let topology = self
            .list_dome_connection_topology(input.request.spatial_context.clone())
            .await?;
        if topology.resolution.topology.topology_digest != input.request.topology_digest
            || !topology
                .resolution
                .topology
                .active_connection_ids
                .contains(&input.request.connection_id)
        {
            anyhow::bail!("DOME_TRANSITION_STALE_TOPOLOGY");
        }
        let connection = topology.connections.iter().find(|view| {
            let agreement = &view.record.agreement;
            agreement.connection_id == input.request.connection_id
                && ((agreement.proposer.instance_id == input.request.source_instance_id
                    && agreement.proposer.instance_generation
                        == input.request.source_instance_generation
                    && agreement.proposer.direction == input.request.direction
                    && agreement.receiver.instance_id == input.request.target_instance_id
                    && agreement.receiver.instance_generation
                        == input.request.target_instance_generation)
                    || (agreement.receiver.instance_id == input.request.source_instance_id
                        && agreement.receiver.instance_generation
                            == input.request.source_instance_generation
                        && agreement.receiver.direction == input.request.direction
                        && agreement.proposer.instance_id == input.request.target_instance_id
                        && agreement.proposer.instance_generation
                            == input.request.target_instance_generation))
        });
        let Some(connection) = connection else {
            anyhow::bail!("DOME_TRANSITION_STALE_TOPOLOGY");
        };
        let agreement = &connection.record.agreement;
        let (source_owner, target_owner) =
            if agreement.proposer.instance_id == input.request.source_instance_id {
                (
                    &agreement.proposer.owner_pubkey,
                    &agreement.receiver.owner_pubkey,
                )
            } else {
                (
                    &agreement.receiver.owner_pubkey,
                    &agreement.proposer.owner_pubkey,
                )
            };
        let access = self
            .evaluate_dome_transition_access(&input.request, source_owner, target_owner)
            .await?;
        let now = Utc::now().timestamp_millis();
        self.dome_host_sessions
            .lock()
            .await
            .get_mut(&input.request.target_instance_id)
            .context("this device is not the active destination Dome host")?
            .prepare_transition_admission(input.request, access, now)
    }

    pub async fn preview_dome_transition_access(
        &self,
        input: PrepareDomeTransitionInput,
    ) -> Result<kukuri_core::DomeTransitionAccessDecisionV1> {
        input.request.validate()?;
        let topology = self
            .list_dome_connection_topology(input.request.spatial_context.clone())
            .await?;
        if topology.resolution.topology.topology_digest != input.request.topology_digest
            || !topology
                .resolution
                .topology
                .active_connection_ids
                .contains(&input.request.connection_id)
        {
            return Ok(kukuri_core::DomeTransitionAccessDecisionV1::Denied {
                reason: kukuri_core::DomeTransitionDenialReasonV1::StaleTopology,
            });
        }
        let connection = topology.connections.iter().find(|view| {
            let agreement = &view.record.agreement;
            agreement.connection_id == input.request.connection_id
                && ((agreement.proposer.instance_id == input.request.source_instance_id
                    && agreement.receiver.instance_id == input.request.target_instance_id)
                    || (agreement.receiver.instance_id == input.request.source_instance_id
                        && agreement.proposer.instance_id == input.request.target_instance_id))
        });
        let Some(connection) = connection else {
            return Ok(kukuri_core::DomeTransitionAccessDecisionV1::Denied {
                reason: kukuri_core::DomeTransitionDenialReasonV1::StaleTopology,
            });
        };
        let agreement = &connection.record.agreement;
        let (source_owner, target_owner) =
            if agreement.proposer.instance_id == input.request.source_instance_id {
                (
                    &agreement.proposer.owner_pubkey,
                    &agreement.receiver.owner_pubkey,
                )
            } else {
                (
                    &agreement.receiver.owner_pubkey,
                    &agreement.proposer.owner_pubkey,
                )
            };
        self.evaluate_dome_transition_access(&input.request, source_owner, target_owner)
            .await
    }

    pub async fn commit_dome_transition(&self, input: CommitDomeTransitionInput) -> Result<()> {
        self.dome_host_sessions
            .lock()
            .await
            .get_mut(&input.ticket.request.target_instance_id)
            .context("this device is not the active destination Dome host")?
            .commit_transition_admission(
                &input.ticket,
                input.position,
                input.rotation,
                Utc::now().timestamp_millis(),
            )
    }

    pub async fn abort_dome_transition(&self, input: AbortDomeTransitionInput) -> Result<()> {
        self.dome_host_sessions
            .lock()
            .await
            .get_mut(&input.ticket.request.target_instance_id)
            .context("this device is not the active destination Dome host")?
            .abort_transition_admission(
                &input.ticket.request.transition_id,
                &input.ticket.request.participant_pubkey,
                Utc::now().timestamp_millis(),
            )
    }

    pub async fn resync_dome_snapshots(
        &self,
        input: ResyncDomeSnapshotsInput,
    ) -> Result<Vec<SignedDomePhysicsSnapshotV1>> {
        let sessions = self.dome_host_sessions.lock().await;
        let runtime = sessions
            .get(&input.instance_id)
            .context("this device is not the active Dome host")?;
        if runtime.lease().spatial_context != input.spatial_context {
            anyhow::bail!("Dome snapshot resync SpatialContext mismatch");
        }
        Ok(runtime.snapshots_after(input.after_sequence))
    }

    pub async fn commit_dome_layout(
        &self,
        input: CommitDomeLayoutInput,
    ) -> Result<DomeLayoutCommitView> {
        let _guard = self.services.dome_mutations.lock().await;
        if input.operation_id.trim().is_empty() {
            anyhow::bail!("Dome layout commit operation id is required");
        }
        let replica = self.hosting_context_replica(&input.spatial_context).await?;
        let instance = self
            .hosting_instance(&replica, &input.spatial_context, &input.instance_id)
            .await?
            .context("Dome instance was not found")?;
        self.ensure_dome_hosting_owner(&instance)?;
        self.ensure_dome_not_deleting(
            &instance.spatial_context,
            &instance.instance_id,
            instance.generation,
        )
        .await?;

        if let Some(existing) = self
            .find_dome_layout_commit(&replica, &input.instance_id, &input.operation_id)
            .await?
        {
            let hosting = self
                .get_dome_hosting(input.spatial_context, &input.instance_id)
                .await?;
            return Ok(DomeLayoutCommitView {
                outcome: DomeLayoutCommitOutcome::Committed,
                operation_id: existing.commit.operation_id.clone(),
                revision: existing.commit.next_manifest_revision,
                manifest_blob_hash: existing.commit.manifest_blob_hash.clone(),
                signed_commit_json: Some(serde_json::to_string(&existing)?),
                hosting,
            });
        }

        let records = self
            .list_dome_hosting_records(&replica, &input.instance_id)
            .await?;
        let lease = current_unique_lease(&records)?.context("no active Dome Hosting Lease")?;
        let now = Utc::now().timestamp_millis();
        let resolved = resolve_dome_hosting_state(&instance, &records, now, Some(now))?;
        if !matches!(
            resolved.kind,
            DomeHostingStateKindV1::OwnerHosted | DomeHostingStateKindV1::CommunityNodeHosted
        ) {
            anyhow::bail!("Dome layout commit requires an active host");
        }
        let candidate = match input.signed_candidate_json {
            Some(json) => serde_json::from_str::<SignedDomeLayoutCandidateV1>(&json)
                .context("invalid host-signed Dome layout candidate")?,
            None => {
                let mut sessions = self.dome_host_sessions.lock().await;
                let runtime = sessions
                    .get_mut(&input.instance_id)
                    .context("active Community Node layout candidate is required")?;
                runtime.signed_layout_candidate(&input.operation_id, now)?
            }
        };
        verify_signed_dome_layout_candidate(
            &candidate,
            &lease.lease,
            candidate.candidate.session_id.as_str(),
        )?;
        if candidate.candidate.operation_id != input.operation_id {
            anyhow::bail!("Dome layout candidate operation id mismatch");
        }

        let topic_id = input.spatial_context.topic_id().as_str().to_string();
        let (_, _, manifest) = self
            .fetch_game_room_state_and_manifest(&topic_id, &input.instance_id)
            .await?
            .context("metaverse room was not found")?;
        let current = manifest
            .metaverse
            .as_ref()
            .context("metaverse room state is missing")?;
        if current.spatial_context != input.spatial_context
            || current.preset_ref.revision != candidate.candidate.base_manifest_revision
            || current.preset_ref.manifest_blob_hash != lease.lease.manifest_blob_hash
        {
            anyhow::bail!("Dome layout candidate is stale");
        }

        if normalized_persistent_props(&current.dome.customization.persistent_props)
            == normalized_persistent_props(&candidate.candidate.persistent_props)
        {
            let hosting = self
                .get_dome_hosting(input.spatial_context, &input.instance_id)
                .await?;
            return Ok(DomeLayoutCommitView {
                outcome: DomeLayoutCommitOutcome::NoOp,
                operation_id: input.operation_id,
                revision: current.preset_ref.revision,
                manifest_blob_hash: current.preset_ref.manifest_blob_hash.clone(),
                signed_commit_json: None,
                hosting,
            });
        }

        if let Some(last_committed_at) = self
            .last_dome_layout_commit_at(&replica, &input.instance_id)
            .await?
            && now.saturating_sub(last_committed_at) < DOME_LAYOUT_COMMIT_MIN_INTERVAL_MILLIS
        {
            anyhow::bail!("Dome layout commit rate limit is active");
        }

        let mut customization = current.dome.customization.clone();
        customization.persistent_props = candidate.candidate.persistent_props.clone();
        self.update_metaverse_room_unlocked(
            &topic_id,
            &input.instance_id,
            UpdateMetaverseRoomInput {
                status: manifest.status,
                customization,
            },
        )
        .await?;
        let (_, _, updated_manifest) = self
            .fetch_game_room_state_and_manifest(&topic_id, &input.instance_id)
            .await?
            .context("updated metaverse room was not found")?;
        let updated = updated_manifest
            .metaverse
            .as_ref()
            .context("updated metaverse room state is missing")?;
        let signed_commit = build_signed_dome_layout_commit(
            self.services.keys.as_ref(),
            &lease.lease,
            &candidate,
            DomeLayoutCommitV1 {
                operation_id: input.operation_id.clone(),
                instance_id: input.instance_id.clone(),
                instance_generation: lease.lease.instance_generation,
                owner_pubkey: lease.lease.owner_pubkey.clone(),
                base_manifest_revision: candidate.candidate.base_manifest_revision,
                next_manifest_revision: updated.preset_ref.revision,
                candidate_digest: dome_layout_candidate_digest(&candidate.candidate)?,
                manifest_blob_hash: updated.preset_ref.manifest_blob_hash.clone(),
                committed_at: now,
            },
        )?;
        self.persist_dome_layout_commit(&replica, &signed_commit)
            .await?;

        let remaining_lease_millis = lease.lease.expires_at.saturating_sub(now).max(1);
        let hosting = match lease.lease.host.clone() {
            DomeHostTargetV1::OwnerDevice { endpoint_id, .. } => {
                self.start_owner_dome_hosting_unlocked(StartOwnerDomeHostingInput {
                    expected_generation: None,
                    spatial_context: input.spatial_context,
                    instance_id: input.instance_id.clone(),
                    endpoint_id,
                    lease_duration_millis: remaining_lease_millis,
                })
                .await?
            }
            DomeHostTargetV1::CommunityNode {
                node_id,
                api_base_url,
            } => {
                self.prepare_community_node_dome_hosting_unlocked(
                    PrepareCommunityNodeDomeHostingInput {
                        expected_generation: None,
                        spatial_context: input.spatial_context,
                        instance_id: input.instance_id.clone(),
                        node_id: node_id.as_str().to_string(),
                        api_base_url,
                        lease_duration_millis: remaining_lease_millis,
                    },
                )
                .await?
            }
        };
        Ok(DomeLayoutCommitView {
            outcome: DomeLayoutCommitOutcome::Committed,
            operation_id: input.operation_id,
            revision: updated.preset_ref.revision,
            manifest_blob_hash: updated.preset_ref.manifest_blob_hash.clone(),
            signed_commit_json: Some(serde_json::to_string(&signed_commit)?),
            hosting,
        })
    }

    pub(crate) async fn hosting_context_replica(
        &self,
        context: &SpatialContextV1,
    ) -> Result<ReplicaId> {
        let replica = match context {
            SpatialContextV1::Topic { topic_id } => topic_replica_id(topic_id.as_str()),
            SpatialContextV1::Channel {
                topic_id,
                channel_id,
            } => {
                let state = self
                    .private_channel_write_state(topic_id.as_str(), channel_id)
                    .await?;
                current_private_channel_replica_id(&state)
            }
        };
        self.services.docs_sync.open_replica(&replica).await?;
        Ok(replica)
    }

    pub(crate) async fn list_dome_hosting_records(
        &self,
        replica: &ReplicaId,
        instance_id: &str,
    ) -> Result<Vec<DomeHostingRecordV1>> {
        let prefix = stable_key(HOSTING_RECORD_PREFIX, &format!("{instance_id}/"));
        let records = self
            .services
            .docs_sync
            .query_replica(replica, DocQuery::Prefix(prefix))
            .await?;
        records
            .into_iter()
            .map(|record| serde_json::from_slice(&record.value).map_err(Into::into))
            .collect()
    }

    async fn persist_dome_hosting_record(
        &self,
        replica: &ReplicaId,
        instance_id: &str,
        record: &DomeHostingRecordV1,
    ) -> Result<()> {
        let (epoch, stage, envelope_id) = hosting_record_identity(record);
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key(
                        HOSTING_RECORD_PREFIX,
                        &format!("{instance_id}/{epoch:020}/{stage}/{envelope_id}"),
                    ),
                    value: serde_json::to_value(record)?,
                },
            )
            .await
    }

    async fn find_dome_layout_commit(
        &self,
        replica: &ReplicaId,
        instance_id: &str,
        operation_id: &str,
    ) -> Result<Option<SignedDomeLayoutCommitV1>> {
        let records = self
            .services
            .docs_sync
            .query_replica(
                replica,
                DocQuery::Exact(stable_key(
                    LAYOUT_COMMIT_PREFIX,
                    &format!("{instance_id}/{operation_id}"),
                )),
            )
            .await?;
        records
            .into_iter()
            .next()
            .map(|record| serde_json::from_slice(&record.value).map_err(Into::into))
            .transpose()
    }

    async fn last_dome_layout_commit_at(
        &self,
        replica: &ReplicaId,
        instance_id: &str,
    ) -> Result<Option<i64>> {
        let records = self
            .services
            .docs_sync
            .query_replica(
                replica,
                DocQuery::Prefix(stable_key(LAYOUT_COMMIT_PREFIX, &format!("{instance_id}/"))),
            )
            .await?;
        let mut latest: Option<i64> = None;
        for record in records {
            let commit: SignedDomeLayoutCommitV1 = serde_json::from_slice(&record.value)?;
            latest = Some(
                latest
                    .unwrap_or(commit.commit.committed_at)
                    .max(commit.commit.committed_at),
            );
        }
        Ok(latest)
    }

    async fn persist_dome_layout_commit(
        &self,
        replica: &ReplicaId,
        signed: &SignedDomeLayoutCommitV1,
    ) -> Result<()> {
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key(
                        LAYOUT_COMMIT_PREFIX,
                        &format!(
                            "{}/{}",
                            signed.commit.instance_id, signed.commit.operation_id
                        ),
                    ),
                    value: serde_json::to_value(signed)?,
                },
            )
            .await
    }

    async fn hosting_view(
        &self,
        instance: &DomeInstanceManifestV1,
        records: &[DomeHostingRecordV1],
        now: i64,
    ) -> Result<DomeHostingView> {
        let mut view = self.hosting_authority_view(instance, records, now).await?;
        view.preset_manifest_json = self
            .fetch_dome_preset_manifest(&instance.preset_ref)
            .await?
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        Ok(view)
    }

    pub(crate) async fn hosting_authority_view(
        &self,
        instance: &DomeInstanceManifestV1,
        records: &[DomeHostingRecordV1],
        now: i64,
    ) -> Result<DomeHostingView> {
        resolve_dome_hosting_state(instance, records, now, Some(now))?;
        let current_epochs = records
            .iter()
            .filter_map(|record| match record {
                DomeHostingRecordV1::LeaseIssued(signed)
                    if signed.lease.instance_generation == instance.generation =>
                {
                    Some(signed.lease.epoch)
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let generation_records = records
            .iter()
            .filter(|record| current_epochs.contains(&hosting_record_identity(record).0))
            .cloned()
            .collect::<Vec<_>>();
        let records = generation_records.as_slice();
        let (local_heartbeat, mut participants, mut sleeping, resource_metrics) = {
            let mut sessions = self.dome_host_sessions.lock().await;
            match sessions.get_mut(&instance.instance_id) {
                Some(runtime) => {
                    runtime.advance_to(now)?;
                    (
                        Some(now),
                        runtime.participant_count().try_into().unwrap_or(u32::MAX),
                        runtime.is_sleeping(),
                        runtime.resource_metrics(),
                    )
                }
                None => (None, 0, true, Default::default()),
            }
        };
        let mut heartbeat_at = local_heartbeat;
        if heartbeat_at.is_none() {
            let preliminary = resolve_dome_hosting_state(instance, records, now, Some(now))?;
            // lock は `if let` の前で手放す(条件式の中で取ると、下の else の取り直しが自分を待って止まる。#1252)。
            let heartbeats = self.dome_host_heartbeats.lock().await;
            let received_heartbeat = heartbeats.get(&instance.instance_id).cloned();
            drop(heartbeats);
            if let (Some(lease), Some(session_id), Some(signed)) = (
                current_unique_lease(records)?,
                preliminary.session_id.as_deref(),
                received_heartbeat,
            ) {
                if signed.heartbeat.sent_at <= now.saturating_add(5_000)
                    && verify_signed_dome_host_heartbeat(&signed, &lease.lease, session_id).is_ok()
                {
                    heartbeat_at = Some(signed.heartbeat.sent_at);
                    participants = signed.heartbeat.participants;
                    sleeping = signed.heartbeat.sleeping;
                } else {
                    let mut heartbeats = self.dome_host_heartbeats.lock().await;
                    if heartbeats
                        .get(&instance.instance_id)
                        .is_some_and(|current| current.envelope.id == signed.envelope.id)
                    {
                        heartbeats.remove(&instance.instance_id);
                    }
                }
            }
            if heartbeat_at.is_none()
                && matches!(
                    preliminary.kind,
                    DomeHostingStateKindV1::OwnerHosted
                        | DomeHostingStateKindV1::CommunityNodeHosted
                )
            {
                heartbeat_at = preliminary.lease_epoch.and_then(|epoch| {
                    records.iter().rev().find_map(|record| match record {
                        DomeHostingRecordV1::LeaseActivated(signed)
                            if signed.activation.lease_epoch == epoch =>
                        {
                            Some(signed.activation.activated_at)
                        }
                        _ => None,
                    })
                });
            }
        }
        let state = resolve_dome_hosting_state(instance, records, now, heartbeat_at)?;
        self.services
            .projection_store
            .upsert_dome_hosting_projection(DomeHostingProjectionRow {
                instance_id: instance.instance_id.clone(),
                context_id: instance.spatial_context.canonical_id(),
                topic_id: instance.spatial_context.topic_id().as_str().to_string(),
                channel_id: instance
                    .spatial_context
                    .channel_id()
                    .map(|channel_id| channel_id.as_str().to_string())
                    .unwrap_or_default(),
                state_json: serde_json::to_string(&state)?,
                lease_epoch: state.lease_epoch,
                session_id: state.session_id.clone(),
                derived_at: now,
                projection_version: 1,
            })
            .await?;
        let lease = current_unique_lease(records)?;
        let epoch = lease.as_ref().map(|signed| signed.lease.epoch);
        let activation = epoch.and_then(|epoch| {
            records.iter().rev().find_map(|record| match record {
                DomeHostingRecordV1::LeaseActivated(signed)
                    if signed.activation.lease_epoch == epoch =>
                {
                    Some(signed)
                }
                _ => None,
            })
        });
        let close = epoch.and_then(|epoch| {
            records.iter().rev().find_map(|record| match record {
                DomeHostingRecordV1::LeaseClosed(signed) if signed.close.lease_epoch == epoch => {
                    Some(signed)
                }
                _ => None,
            })
        });
        Ok(DomeHostingView {
            instance_id: instance.instance_id.clone(),
            state,
            signed_lease_json: lease.as_ref().map(serde_json::to_string).transpose()?,
            lease: lease.map(|signed| signed.lease),
            signed_activation_json: activation.map(serde_json::to_string).transpose()?,
            signed_close_json: close.map(serde_json::to_string).transpose()?,
            instance_manifest_json: serde_json::to_string(instance)?,
            preset_manifest_json: None,
            participants,
            sleeping,
            resource_budget: self.metaverse_resource_budget.clone(),
            resource_metrics,
        })
    }

    fn ensure_dome_hosting_owner(&self, instance: &DomeInstanceManifestV1) -> Result<()> {
        if instance.status != DomeInstanceStatusV1::Active
            || instance.relationship_detach.is_some()
            || instance.owner_pubkey.as_str() != self.current_author_pubkey()
        {
            anyhow::bail!("only the active attached Dome owner can change hosting");
        }
        Ok(())
    }

    async fn publish_dome_hosting_hint(
        &self,
        context: &SpatialContextV1,
        instance_id: &str,
        session_id: &str,
    ) -> Result<()> {
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(context.topic_id().as_str(), context.channel_id()),
                GossipHint::SessionChanged {
                    topic_id: context.topic_id().clone(),
                    session_id: session_id.to_string(),
                    object_kind: format!("dome-hosting:{instance_id}"),
                },
            )
            .await
    }
}

fn lease_expiry(now: i64, requested_duration: i64) -> Result<i64> {
    let duration = if requested_duration == 0 {
        DEFAULT_LEASE_MILLIS
    } else {
        requested_duration
    };
    if duration <= 0 || duration > DOME_HOSTING_MAX_LEASE_MILLIS {
        anyhow::bail!("Dome Hosting Lease duration is outside the supported range");
    }
    now.checked_add(duration)
        .context("Dome Hosting Lease expiry overflow")
}

fn normalized_persistent_props(
    props: &[kukuri_core::MetaversePersistentPropV1],
) -> Vec<kukuri_core::MetaversePersistentPropV1> {
    let mut props = props.to_vec();
    props.sort_by(|left, right| left.prop_id.cmp(&right.prop_id));
    props
}

fn next_hosting_epoch(records: &[DomeHostingRecordV1]) -> u64 {
    records
        .iter()
        .map(|record| match record {
            DomeHostingRecordV1::LeaseIssued(signed) => signed.lease.epoch,
            DomeHostingRecordV1::HostAccepted(signed) => signed.acceptance.lease_epoch,
            DomeHostingRecordV1::LeaseActivated(signed) => signed.activation.lease_epoch,
            DomeHostingRecordV1::LeaseClosed(signed) => signed.close.lease_epoch,
        })
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn current_unique_lease(
    records: &[DomeHostingRecordV1],
) -> Result<Option<SignedDomeHostingLeaseV1>> {
    let highest = records
        .iter()
        .filter_map(|record| match record {
            DomeHostingRecordV1::LeaseIssued(signed) => Some(signed.lease.epoch),
            _ => None,
        })
        .max();
    let Some(highest) = highest else {
        return Ok(None);
    };
    let leases = records
        .iter()
        .filter_map(|record| match record {
            DomeHostingRecordV1::LeaseIssued(signed) if signed.lease.epoch == highest => {
                Some(signed.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let digests = leases
        .iter()
        .map(|lease| kukuri_core::dome_hosting_lease_digest(&lease.lease))
        .collect::<Result<BTreeSet<_>>>()?;
    if digests.len() > 1 {
        anyhow::bail!("split-brain Hosting Leases require a higher owner epoch");
    }
    Ok(leases.into_iter().next())
}

fn hosting_record_identity(record: &DomeHostingRecordV1) -> (u64, &'static str, &str) {
    match record {
        DomeHostingRecordV1::LeaseIssued(signed) => {
            (signed.lease.epoch, "issued", signed.envelope.id.as_str())
        }
        DomeHostingRecordV1::HostAccepted(signed) => (
            signed.acceptance.lease_epoch,
            "accepted",
            signed.envelope.id.as_str(),
        ),
        DomeHostingRecordV1::LeaseActivated(signed) => (
            signed.activation.lease_epoch,
            "activated",
            signed.envelope.id.as_str(),
        ),
        DomeHostingRecordV1::LeaseClosed(signed) => (
            signed.close.lease_epoch,
            "closed",
            signed.envelope.id.as_str(),
        ),
    }
}
