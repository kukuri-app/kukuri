use crate::service::*;
use kukuri_core::{
    DOME_CONNECTION_MAX_OPEN_OUTBOUND, DOME_CONNECTION_MAX_PER_PEER_SLOT,
    DOME_CONNECTION_MAX_RECEIVER_QUEUE, DomeConnectionAgreementV1, DomeDirection,
    SignedDomeConnectionAgreementV1, SpatialContextV1, derive_dome_proposal_status,
    opposite_dome_direction, resolve_dome_topology, resolve_dome_topology_candidates,
    sign_envelope_json, verify_signed_dome_connection_agreement,
};

impl AppService {
    pub async fn list_dome_connection_topology(
        &self,
        spatial_context: SpatialContextV1,
    ) -> Result<DomeConnectionTopologyView> {
        let replica = self.dome_connection_read_replica(&spatial_context).await?;
        let instances = self.list_context_dome_instances(&replica).await?;
        let proposal_states = self.list_dome_proposal_states(&replica).await?;
        let selections = self.list_dome_selections(&replica).await?;
        let connection_states = self.list_dome_connection_states(&replica).await?;
        let connections = connection_states
            .iter()
            .map(|state| state.record.clone())
            .collect::<Vec<_>>();
        let resolution =
            if instances.is_empty() {
                let digest = blake3::hash(&serde_json::to_vec(&serde_json::json!({
                "spatial_context": spatial_context, "components": [], "active_connection_ids": [],
            }))?).to_hex().to_string();
                kukuri_core::DomeTopologyResolutionV1 {
                    topology: kukuri_core::DomeTopologyV1 {
                        spatial_context: spatial_context.clone(),
                        components: vec![],
                        active_connection_ids: vec![],
                        topology_digest: digest,
                    },
                    rejected_connections: connections
                        .iter()
                        .map(|record| kukuri_core::DomeRejectedConnectionV1 {
                            connection_id: record.agreement.connection_id.clone(),
                            reason: "instance unavailable".into(),
                        })
                        .collect(),
                }
            } else {
                resolve_dome_topology_candidates(&instances, &connections)?
            };
        let active_ids = resolution
            .topology
            .active_connection_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let effective_connections = connections
            .iter()
            .map(|record| {
                let mut record = record.clone();
                if matches!(record.status, DomeConnectionStatusV1::Active)
                    && !active_ids.contains(&record.agreement.connection_id)
                {
                    record.status = DomeConnectionStatusV1::Accepted;
                }
                record
            })
            .collect::<Vec<_>>();
        let selected_by_proposal = selected_proposals(&selections);
        let proposals = proposal_states
            .into_iter()
            .map(|state| {
                let selection = selected_by_proposal
                    .get(&state.proposal.proposal_id)
                    .cloned();
                let terminal_reason = state
                    .terminal_reason
                    .or_else(|| proposal_instance_invalidation(&state.proposal, &instances));
                DomeConnectionProposalView {
                    status: derive_dome_proposal_status(
                        &state.proposal,
                        selection.as_ref(),
                        &effective_connections,
                        terminal_reason,
                    ),
                    proposal: state.proposal,
                    selection,
                    terminal_reason,
                    connection_id: state.connection_id,
                }
            })
            .collect();
        let view = DomeConnectionTopologyView {
            proposals,
            connections: effective_connections
                .into_iter()
                .map(|record| DomeConnectionView { record })
                .collect(),
            resolution,
        };
        let now = Utc::now().timestamp_millis();
        self.services
            .projection_store
            .upsert_dome_connection_projection(DomeConnectionProjectionRow {
                context_id: spatial_context.canonical_id(),
                topic_id: spatial_context.topic_id().as_str().to_string(),
                channel_id: spatial_context
                    .channel_id()
                    .map(|channel_id| channel_id.as_str().to_string())
                    .unwrap_or_default(),
                snapshot_json: serde_json::to_string(&view)?,
                topology_digest: view.resolution.topology.topology_digest.clone(),
                derived_at: now,
                projection_version: 1,
            })
            .await?;
        Ok(view)
    }

    pub async fn create_dome_connection_proposal(
        &self,
        input: CreateDomeConnectionProposalInput,
    ) -> Result<DomeConnectionProposalView> {
        if !is_valid_dome_operation_id(&input.proposal_id, 128) {
            anyhow::bail!("Dome Connection proposal id is required");
        }
        let replica = self
            .dome_connection_context_replica(&input.spatial_context)
            .await?;
        if let Some(existing) = self
            .fetch_dome_proposal_state(&replica, input.proposal_id.as_str())
            .await?
        {
            if existing.proposal.proposer.owner_pubkey.as_str() != self.current_author_pubkey() {
                anyhow::bail!("Dome Connection proposal id is already used");
            }
            if existing.proposal.spatial_context != input.spatial_context
                || existing.proposal.proposer.instance_id != input.proposer_instance_id
                || existing.proposal.receiver.instance_id != input.receiver_instance_id
                || existing.proposal.proposer.direction != input.proposer_direction
            {
                anyhow::bail!("Dome Connection proposal idempotency payload mismatch");
            }
            return self.proposal_view_from_state(&replica, existing).await;
        }
        let instances = self.list_context_dome_instances(&replica).await?;
        let proposer = instances
            .iter()
            .find(|instance| instance.instance_id == input.proposer_instance_id)
            .context("Dome Connection proposer instance was not found")?;
        let receiver = instances
            .iter()
            .find(|instance| instance.instance_id == input.receiver_instance_id)
            .context("Dome Connection receiver instance was not found")?;
        if proposer.owner_pubkey.as_str() != self.current_author_pubkey() {
            anyhow::bail!("only the Dome owner can create a Connection proposal");
        }
        let existing = self.list_dome_proposal_states(&replica).await?;
        let connected_proposal_ids = self
            .list_dome_connection_states(&replica)
            .await?
            .into_iter()
            .map(|state| state.record.agreement.proposal_id)
            .collect::<BTreeSet<_>>();
        let open_by_proposer = existing
            .iter()
            .filter(|state| {
                state.terminal_reason.is_none()
                    && !connected_proposal_ids.contains(&state.proposal.proposal_id)
                    && state.proposal.proposer.instance_id == proposer.instance_id
            })
            .count();
        if open_by_proposer >= DOME_CONNECTION_MAX_OPEN_OUTBOUND {
            anyhow::bail!("Dome Connection outbound proposal limit reached");
        }
        let same_peer_slot = existing
            .iter()
            .filter(|state| {
                state.terminal_reason.is_none()
                    && !connected_proposal_ids.contains(&state.proposal.proposal_id)
                    && state.proposal.proposer.instance_id == proposer.instance_id
                    && state.proposal.receiver.instance_id == receiver.instance_id
                    && state.proposal.receiver.direction
                        == opposite_dome_direction(input.proposer_direction)
            })
            .count();
        if same_peer_slot >= DOME_CONNECTION_MAX_PER_PEER_SLOT {
            anyhow::bail!("Dome Connection peer slot proposal limit reached");
        }
        let receiver_queue = existing
            .iter()
            .filter(|state| {
                state.terminal_reason.is_none()
                    && !connected_proposal_ids.contains(&state.proposal.proposal_id)
                    && state.proposal.receiver.instance_id == receiver.instance_id
                    && state.proposal.receiver.direction
                        == opposite_dome_direction(input.proposer_direction)
            })
            .count();
        if receiver_queue >= DOME_CONNECTION_MAX_RECEIVER_QUEUE {
            anyhow::bail!("Dome Connection receiver queue limit reached");
        }
        let now = Utc::now().timestamp_millis();
        let recent = existing
            .iter()
            .filter(|state| {
                state.proposal.proposer.owner_pubkey == proposer.owner_pubkey
                    && state.proposal.receiver.instance_id == receiver.instance_id
                    && state.proposal.receiver.direction
                        == opposite_dome_direction(input.proposer_direction)
                    && state.proposal.created_at >= now - LOCAL_PROPOSAL_RATE_WINDOW_MS
            })
            .count();
        let proposal_limit = self
            .metaverse_resource_budget
            .player
            .max_proposals_per_ten_minutes_per_slot as usize;
        if recent >= proposal_limit {
            return Err(kukuri_core::MetaverseResourceRejection::new(
                kukuri_core::MetaverseBudgetScope::Player,
                kukuri_core::MetaverseBudgetResource::ProposalRate,
                kukuri_core::MetaverseResourceRejectionReason::RateExceeded,
                (recent + 1) as u64,
                proposal_limit as u64,
            )
            .into());
        }
        let sequence = existing
            .iter()
            .filter(|state| state.proposal.proposer.owner_pubkey == proposer.owner_pubkey)
            .map(|state| state.proposal.sequence)
            .max()
            .unwrap_or(0)
            + 1;
        let proposal = DomeConnectionProposalV1 {
            proposal_id: input.proposal_id,
            spatial_context: input.spatial_context,
            proposer: DomeConnectionEndpointV1::from_instance(proposer, input.proposer_direction),
            receiver: DomeConnectionEndpointV1::from_instance(
                receiver,
                opposite_dome_direction(input.proposer_direction),
            ),
            sequence,
            created_at: now,
        };
        validate_dome_connection_proposal(&proposal, proposer, receiver)?;
        let connection_id = format!("connection-{}", proposal.proposal_id);
        let agreement = DomeConnectionAgreementV1::from_proposal(&connection_id, &proposal);
        validate_dome_connection_agreement(&agreement, proposer, receiver)?;
        let proposal_envelope =
            build_dome_connection_proposal_envelope(self.services.keys.as_ref(), &proposal)?;
        let agreement_envelope =
            build_dome_connection_agreement_envelope(self.services.keys.as_ref(), &agreement)?;
        self.persist_connection_envelope(&replica, &proposal_envelope)
            .await?;
        self.persist_connection_envelope(&replica, &agreement_envelope)
            .await?;
        let state = DomeProposalStateDocV1 {
            proposal,
            connection_id,
            proposal_envelope_id: proposal_envelope.id,
            proposer_agreement_envelope_id: agreement_envelope.id,
            terminal_reason: None,
            terminal_event_envelope_id: None,
        };
        self.persist_dome_proposal_state(&replica, &state).await?;
        self.publish_dome_topology_hint(&state.proposal.spatial_context, &state.connection_id)
            .await?;
        self.proposal_view_from_state(&replica, state).await
    }

    pub async fn accept_dome_connection_proposal(
        &self,
        input: AcceptDomeConnectionProposalInput,
    ) -> Result<DomeConnectionView> {
        if !is_valid_dome_operation_id(&input.proposal_id, 128) {
            anyhow::bail!("invalid Dome Connection proposal id");
        }
        let replica = self
            .dome_connection_context_replica(&input.spatial_context)
            .await?;
        let proposal_state = self
            .fetch_dome_proposal_state(&replica, input.proposal_id.as_str())
            .await?
            .context("Dome Connection proposal was not found")?;
        if proposal_state.terminal_reason.is_some() {
            anyhow::bail!("Dome Connection proposal is no longer active");
        }
        if proposal_state.proposal.receiver.owner_pubkey.as_str() != self.current_author_pubkey() {
            anyhow::bail!("only the receiver Dome owner can accept this proposal");
        }
        if self
            .authors_blocked_either_direction(
                proposal_state.proposal.proposer.owner_pubkey.as_str(),
                proposal_state.proposal.receiver.owner_pubkey.as_str(),
            )
            .await?
        {
            self.terminate_dome_connection_proposal_with_reason(
                &proposal_state.proposal.spatial_context,
                proposal_state.proposal.proposal_id.as_str(),
                DomeConnectionTerminalReasonV1::OwnersBlocked,
            )
            .await?;
            anyhow::bail!("DOME_CONNECTION_OWNERS_BLOCKED");
        }
        if let Some(existing) = self
            .fetch_dome_connection_state(&replica, &proposal_state.connection_id)
            .await?
        {
            return Ok(DomeConnectionView {
                record: existing.record,
            });
        }
        let instances = self.list_context_dome_instances(&replica).await?;
        let proposer =
            current_instance_for_endpoint(&instances, &proposal_state.proposal.proposer)?;
        let receiver =
            current_instance_for_endpoint(&instances, &proposal_state.proposal.receiver)?;
        validate_dome_connection_proposal(&proposal_state.proposal, proposer, receiver)?;
        let selections = self.list_dome_selections(&replica).await?;
        let slot_generation = selections
            .iter()
            .filter(|selection| {
                selection.selection.receiver.instance_id
                    == proposal_state.proposal.receiver.instance_id
                    && selection.selection.receiver.direction
                        == proposal_state.proposal.receiver.direction
            })
            .map(|selection| selection.selection.slot_generation)
            .max()
            .unwrap_or(0)
            + 1;
        let current_connections = self
            .list_dome_connection_states(&replica)
            .await?
            .into_iter()
            .map(|state| state.record)
            .collect::<Vec<_>>();
        let current_resolution =
            resolve_dome_topology_candidates(&instances, &current_connections)?;
        let effective_active_ids = current_resolution
            .topology
            .active_connection_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let selection = DomeProposalSelectionV1 {
            selection_id: format!("selection-{}-{slot_generation}", input.proposal_id),
            proposal_id: input.proposal_id,
            spatial_context: input.spatial_context,
            receiver: proposal_state.proposal.receiver.clone(),
            slot_generation,
            observed_active_connection_ids: current_resolution
                .topology
                .active_connection_ids
                .clone(),
            selected_at: Utc::now().timestamp_millis(),
        };
        validate_dome_connection_selection(&selection, &proposal_state.proposal)?;
        let agreement = DomeConnectionAgreementV1::from_proposal(
            proposal_state.connection_id.clone(),
            &proposal_state.proposal,
        );
        let receiver_signature =
            build_dome_connection_agreement_envelope(self.services.keys.as_ref(), &agreement)?;
        let proposer_signature = self
            .fetch_connection_envelope(&replica, &proposal_state.proposer_agreement_envelope_id)
            .await?;
        verify_signed_dome_connection_agreement(&SignedDomeConnectionAgreementV1 {
            agreement: agreement.clone(),
            proposer_signature,
            receiver_signature: receiver_signature.clone(),
        })?;
        let record = DomeConnectionRecordV1 {
            agreement,
            receiver_slot_generation: slot_generation,
            observed_active_connection_ids: selection.observed_active_connection_ids.clone(),
            status: DomeConnectionStatusV1::Active,
            lifecycle_generation: 1,
            lifecycle_actor: None,
            lifecycle_reason: None,
            lifecycle_deadline_at: None,
        };
        validate_dome_connection_record(&record)?;
        let mut prospective = current_connections
            .into_iter()
            .filter(|connection| effective_active_ids.contains(&connection.agreement.connection_id))
            .collect::<Vec<_>>();
        prospective.push(record.clone());
        resolve_dome_topology(&instances, &prospective)?;
        let selection_envelope =
            build_dome_connection_selection_envelope(self.services.keys.as_ref(), &selection)?;
        let lifecycle_envelope = sign_envelope_json(
            self.services.keys.as_ref(),
            "dome-connection-lifecycle",
            connection_tags_for_state(&record, self.current_author_pubkey().as_str()),
            &record,
        )?;
        for envelope in [
            &selection_envelope,
            &receiver_signature,
            &lifecycle_envelope,
        ] {
            self.persist_connection_envelope(&replica, envelope).await?;
        }
        self.persist_dome_selection(
            &replica,
            &DomeSelectionStateDocV1 {
                selection,
                envelope_id: selection_envelope.id,
            },
        )
        .await?;
        let state = DomeConnectionStateDocV1 {
            record: record.clone(),
            proposer_agreement_envelope_id: proposal_state.proposer_agreement_envelope_id,
            receiver_agreement_envelope_id: receiver_signature.id,
            lifecycle_envelope_id: lifecycle_envelope.id,
        };
        self.persist_dome_connection_state(&replica, &state).await?;
        self.publish_dome_topology_hint(
            &record.agreement.spatial_context,
            &record.agreement.connection_id,
        )
        .await?;
        Ok(DomeConnectionView { record })
    }

    pub async fn withdraw_dome_connection_proposal(
        &self,
        input: WithdrawDomeConnectionProposalInput,
    ) -> Result<DomeConnectionProposalView> {
        if !is_valid_dome_operation_id(&input.proposal_id, 128) {
            anyhow::bail!("invalid Dome Connection proposal id");
        }
        let replica = self
            .dome_connection_context_replica(&input.spatial_context)
            .await?;
        let state = self
            .fetch_dome_proposal_state(&replica, input.proposal_id.as_str())
            .await?
            .context("Dome Connection proposal was not found")?;
        if state.proposal.proposer.owner_pubkey.as_str() != self.current_author_pubkey() {
            anyhow::bail!("only the proposer can withdraw this proposal");
        }
        self.terminate_dome_connection_proposal_with_reason(
            &input.spatial_context,
            input.proposal_id.as_str(),
            DomeConnectionTerminalReasonV1::ProposerWithdrew,
        )
        .await
    }

    pub(crate) async fn terminate_dome_connection_proposal_with_reason(
        &self,
        spatial_context: &SpatialContextV1,
        proposal_id: &str,
        reason: DomeConnectionTerminalReasonV1,
    ) -> Result<DomeConnectionProposalView> {
        let replica = self
            .dome_connection_context_replica(spatial_context)
            .await?;
        let mut state = self
            .fetch_dome_proposal_state(&replica, proposal_id)
            .await?
            .context("Dome Connection proposal was not found")?;
        if state.terminal_reason.is_some()
            || self
                .fetch_dome_connection_state(&replica, &state.connection_id)
                .await?
                .is_some()
        {
            return self.proposal_view_from_state(&replica, state).await;
        }
        let actor = Pubkey::from(self.current_author_pubkey());
        validate_dome_proposal_terminal_actor(&state.proposal, &actor, reason)?;
        let event = DomeProposalTerminalEventV1 {
            proposal_id: state.proposal.proposal_id.clone(),
            spatial_context: state.proposal.spatial_context.clone(),
            actor_pubkey: actor,
            reason,
            updated_at: Utc::now().timestamp_millis(),
        };
        let envelope = sign_envelope_json(
            self.services.keys.as_ref(),
            "dome-connection-proposal-terminal",
            vec![vec!["proposal".into(), event.proposal_id.clone()]],
            &event,
        )?;
        self.persist_connection_envelope(&replica, &envelope)
            .await?;
        state.terminal_reason = Some(event.reason);
        state.terminal_event_envelope_id = Some(envelope.id);
        self.persist_dome_proposal_state(&replica, &state).await?;
        self.publish_dome_topology_hint(&state.proposal.spatial_context, &state.connection_id)
            .await?;
        self.proposal_view_from_state(&replica, state).await
    }

    pub async fn revoke_dome_connection(
        &self,
        input: RevokeDomeConnectionInput,
    ) -> Result<DomeConnectionView> {
        if !is_valid_dome_operation_id(&input.connection_id, 160) {
            anyhow::bail!("invalid Dome Connection id");
        }
        self.terminate_dome_connection_with_reason(
            &input.spatial_context,
            input.connection_id.as_str(),
            DomeConnectionTerminalReasonV1::OwnerRevoked,
        )
        .await
    }

    pub(crate) async fn dome_connection_context_replica(
        &self,
        context: &SpatialContextV1,
    ) -> Result<ReplicaId> {
        let replica = match context {
            SpatialContextV1::Topic { topic_id } => topic_replica_id(topic_id.as_str()),
            SpatialContextV1::Channel {
                topic_id,
                channel_id,
            } => {
                self.ensure_private_channel_access(topic_id.as_str(), channel_id)
                    .await?;
                let state = self
                    .private_channel_write_state(topic_id.as_str(), channel_id)
                    .await?;
                current_private_channel_replica_id(&state)
            }
        };
        self.ensure_topic_subscription(context.topic_id().as_str())
            .await?;
        self.services.docs_sync.open_replica(&replica).await?;
        Ok(replica)
    }

    async fn list_context_dome_instances(
        &self,
        replica: &ReplicaId,
    ) -> Result<Vec<DomeInstanceManifestV1>> {
        let records = self
            .services
            .docs_sync
            .query_replica(
                replica,
                DocQuery::Prefix(stable_key("metaverse/dome-instances", "")),
            )
            .await?;
        let mut instances = Vec::new();
        for record in records {
            if !record.key.ends_with("/state") {
                continue;
            }
            let state: DomeInstanceStateDocV1 = serde_json::from_slice(&record.value)?;
            let resolved = match self
                .fetch_dome_instance_manifest(replica, &state.owner_pubkey)
                .await
            {
                Ok(value) => value,
                Err(error) if error.downcast_ref::<DomeReadUnavailable>().is_some() => continue,
                Err(error) => return Err(error),
            };
            if let Some((_, manifest)) = resolved
                && manifest.status == kukuri_core::DomeInstanceStatusV1::Active
                && manifest.relationship_detach.is_none()
            {
                instances.push(manifest);
            }
        }
        instances.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        instances.dedup_by(|left, right| left.instance_id == right.instance_id);
        Ok(instances)
    }

    async fn persist_connection_envelope(
        &self,
        replica: &ReplicaId,
        envelope: &KukuriEnvelope,
    ) -> Result<()> {
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key("envelopes", envelope.id.as_str()),
                    value: serde_json::to_value(envelope)?,
                },
            )
            .await
    }

    async fn fetch_connection_envelope(
        &self,
        replica: &ReplicaId,
        envelope_id: &EnvelopeId,
    ) -> Result<KukuriEnvelope> {
        let record = self
            .services
            .docs_sync
            .query_replica(
                replica,
                DocQuery::Exact(stable_key("envelopes", envelope_id.as_str())),
            )
            .await?
            .into_iter()
            .next()
            .context("Dome Connection envelope is unavailable")?;
        let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)?;
        envelope.verify()?;
        Ok(envelope)
    }

    async fn persist_dome_proposal_state(
        &self,
        replica: &ReplicaId,
        state: &DomeProposalStateDocV1,
    ) -> Result<()> {
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key(
                        PROPOSAL_PREFIX,
                        &format!("{}/state", state.proposal.proposal_id),
                    ),
                    value: serde_json::to_value(state)?,
                },
            )
            .await
    }

    async fn fetch_dome_proposal_state(
        &self,
        replica: &ReplicaId,
        proposal_id: &str,
    ) -> Result<Option<DomeProposalStateDocV1>> {
        let state = self
            .services
            .docs_sync
            .query_replica(
                replica,
                DocQuery::Exact(stable_key(PROPOSAL_PREFIX, &format!("{proposal_id}/state"))),
            )
            .await?
            .into_iter()
            .next()
            .map(|record| serde_json::from_slice(&record.value))
            .transpose()?;
        if let Some(state) = &state {
            self.verify_dome_proposal_state(replica, state).await?;
        }
        Ok(state)
    }

    async fn list_dome_proposal_states(
        &self,
        replica: &ReplicaId,
    ) -> Result<Vec<DomeProposalStateDocV1>> {
        let records = self
            .services
            .docs_sync
            .query_replica(replica, DocQuery::Prefix(stable_key(PROPOSAL_PREFIX, "")))
            .await?;
        let mut states = Vec::new();
        for record in records {
            if record.key.ends_with("/state") {
                let state: DomeProposalStateDocV1 = serde_json::from_slice(&record.value)?;
                self.verify_dome_proposal_state(replica, &state).await?;
                states.push(state);
            }
        }
        states.sort_by(|left, right| left.proposal.proposal_id.cmp(&right.proposal.proposal_id));
        Ok(states)
    }

    async fn verify_dome_proposal_state(
        &self,
        replica: &ReplicaId,
        state: &DomeProposalStateDocV1,
    ) -> Result<()> {
        let proposal: DomeConnectionProposalV1 = fetch_verified_dome_envelope(
            self.services.docs_sync.as_ref(),
            replica,
            &state.proposal_envelope_id,
            "dome-connection-proposal",
            &state.proposal.proposer.owner_pubkey,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?;
        if proposal != state.proposal {
            anyhow::bail!("signed Dome Connection proposal does not match state");
        }
        let agreement: DomeConnectionAgreementV1 = fetch_verified_dome_envelope(
            self.services.docs_sync.as_ref(),
            replica,
            &state.proposer_agreement_envelope_id,
            "dome-connection-agreement",
            &state.proposal.proposer.owner_pubkey,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?;
        if agreement
            != DomeConnectionAgreementV1::from_proposal(&state.connection_id, &state.proposal)
        {
            anyhow::bail!("signed Dome Connection agreement does not match proposal");
        }
        match (
            state.terminal_reason,
            state.terminal_event_envelope_id.as_ref(),
        ) {
            (Some(reason), Some(envelope_id)) => {
                let envelope = self.fetch_connection_envelope(replica, envelope_id).await?;
                verify_dome_proposal_terminal_event(state, envelope_id, reason, &envelope)?;
            }
            (None, None) => {}
            _ => anyhow::bail!("Dome Connection terminal state is incomplete"),
        }
        Ok(())
    }

    async fn persist_dome_selection(
        &self,
        replica: &ReplicaId,
        state: &DomeSelectionStateDocV1,
    ) -> Result<()> {
        let direction = direction_key(state.selection.receiver.direction);
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key(
                        SELECTION_PREFIX,
                        &format!(
                            "{}/{}/{}",
                            state.selection.receiver.instance_id,
                            direction,
                            state.selection.selection_id
                        ),
                    ),
                    value: serde_json::to_value(state)?,
                },
            )
            .await
    }

    async fn list_dome_selections(
        &self,
        replica: &ReplicaId,
    ) -> Result<Vec<DomeSelectionStateDocV1>> {
        let records = self
            .services
            .docs_sync
            .query_replica(replica, DocQuery::Prefix(stable_key(SELECTION_PREFIX, "")))
            .await?;
        let mut states = Vec::new();
        for record in records {
            let state: DomeSelectionStateDocV1 = serde_json::from_slice(&record.value)?;
            let signed: DomeProposalSelectionV1 = fetch_verified_dome_envelope(
                self.services.docs_sync.as_ref(),
                replica,
                &state.envelope_id,
                "dome-connection-selection",
                &state.selection.receiver.owner_pubkey,
                DocFetchPolicy::LocalThenRemote,
            )
            .await?;
            if signed != state.selection {
                anyhow::bail!("signed Dome Connection selection does not match state");
            }
            states.push(state);
        }
        Ok(states)
    }

    async fn persist_dome_connection_state(
        &self,
        replica: &ReplicaId,
        state: &DomeConnectionStateDocV1,
    ) -> Result<()> {
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key(
                        CONNECTION_PREFIX,
                        &format!("{}/state", state.record.agreement.connection_id),
                    ),
                    value: serde_json::to_value(state)?,
                },
            )
            .await
    }

    pub(crate) async fn fetch_dome_connection_state(
        &self,
        replica: &ReplicaId,
        connection_id: &str,
    ) -> Result<Option<DomeConnectionStateDocV1>> {
        let state = self
            .services
            .docs_sync
            .query_replica(
                replica,
                DocQuery::Exact(stable_key(
                    CONNECTION_PREFIX,
                    &format!("{connection_id}/state"),
                )),
            )
            .await?
            .into_iter()
            .next()
            .map(|record| serde_json::from_slice(&record.value))
            .transpose()?;
        if let Some(state) = &state {
            self.verify_dome_connection_state(replica, state).await?;
        }
        Ok(state)
    }

    async fn list_dome_connection_states(
        &self,
        replica: &ReplicaId,
    ) -> Result<Vec<DomeConnectionStateDocV1>> {
        let records = self
            .services
            .docs_sync
            .query_replica(replica, DocQuery::Prefix(stable_key(CONNECTION_PREFIX, "")))
            .await?;
        let mut states = Vec::new();
        for record in records {
            if record.key.ends_with("/state") {
                let state: DomeConnectionStateDocV1 = serde_json::from_slice(&record.value)?;
                self.verify_dome_connection_state(replica, &state).await?;
                states.push(state);
            }
        }
        states.sort_by(|left, right| {
            left.record
                .agreement
                .connection_id
                .cmp(&right.record.agreement.connection_id)
        });
        Ok(states)
    }

    async fn verify_dome_connection_state(
        &self,
        replica: &ReplicaId,
        state: &DomeConnectionStateDocV1,
    ) -> Result<()> {
        validate_dome_connection_record(&state.record)?;
        let signed = SignedDomeConnectionAgreementV1 {
            agreement: state.record.agreement.clone(),
            proposer_signature: self
                .fetch_connection_envelope(replica, &state.proposer_agreement_envelope_id)
                .await?,
            receiver_signature: self
                .fetch_connection_envelope(replica, &state.receiver_agreement_envelope_id)
                .await?,
        };
        verify_signed_dome_connection_agreement(&signed)?;
        let lifecycle = self
            .fetch_connection_envelope(replica, &state.lifecycle_envelope_id)
            .await?;
        if lifecycle.kind != "dome-connection-lifecycle" {
            anyhow::bail!("Dome Connection lifecycle envelope kind mismatch");
        }
        let lifecycle_record: DomeConnectionRecordV1 = serde_json::from_str(&lifecycle.content)?;
        if lifecycle_record != state.record {
            anyhow::bail!("signed Dome Connection lifecycle does not match state");
        }
        if lifecycle.pubkey != state.record.agreement.receiver.owner_pubkey
            && lifecycle.pubkey != state.record.agreement.proposer.owner_pubkey
        {
            anyhow::bail!("Dome Connection lifecycle signer is not an endpoint owner");
        }
        Ok(())
    }

    pub(crate) async fn persist_connection_lifecycle(
        &self,
        replica: &ReplicaId,
        state: &mut DomeConnectionStateDocV1,
    ) -> Result<()> {
        validate_dome_connection_record(&state.record)?;
        let envelope = sign_envelope_json(
            self.services.keys.as_ref(),
            "dome-connection-lifecycle",
            connection_tags_for_state(&state.record, self.current_author_pubkey().as_str()),
            &state.record,
        )?;
        self.persist_connection_envelope(replica, &envelope).await?;
        state.lifecycle_envelope_id = envelope.id;
        self.persist_dome_connection_state(replica, state).await
    }

    async fn proposal_view_from_state(
        &self,
        replica: &ReplicaId,
        state: DomeProposalStateDocV1,
    ) -> Result<DomeConnectionProposalView> {
        let selections = selected_proposals(&self.list_dome_selections(replica).await?);
        let selection = selections.get(&state.proposal.proposal_id).cloned();
        let connections = self
            .list_dome_connection_states(replica)
            .await?
            .into_iter()
            .map(|state| state.record)
            .collect::<Vec<_>>();
        Ok(DomeConnectionProposalView {
            status: derive_dome_proposal_status(
                &state.proposal,
                selection.as_ref(),
                &connections,
                state.terminal_reason,
            ),
            proposal: state.proposal,
            selection,
            terminal_reason: state.terminal_reason,
            connection_id: state.connection_id,
        })
    }

    pub(crate) async fn publish_dome_topology_hint(
        &self,
        context: &SpatialContextV1,
        connection_id: &str,
    ) -> Result<()> {
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(context.topic_id().as_str(), context.channel_id()),
                GossipHint::SessionChanged {
                    topic_id: context.topic_id().clone(),
                    session_id: connection_id.to_string(),
                    object_kind: "dome-topology".into(),
                },
            )
            .await
    }
}

fn current_instance_for_endpoint<'a>(
    instances: &'a [DomeInstanceManifestV1],
    endpoint: &DomeConnectionEndpointV1,
) -> Result<&'a DomeInstanceManifestV1> {
    instances
        .iter()
        .find(|instance| {
            instance.instance_id == endpoint.instance_id
                && instance.generation == endpoint.instance_generation
                && instance.owner_pubkey == endpoint.owner_pubkey
        })
        .context("Dome Connection endpoint instance is stale or unavailable")
}

fn is_valid_dome_operation_id(id: &str, max_len: usize) -> bool {
    !id.is_empty()
        && id.len() <= max_len
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn selected_proposals(
    selections: &[DomeSelectionStateDocV1],
) -> BTreeMap<String, DomeProposalSelectionV1> {
    let mut winner_by_slot: BTreeMap<(String, DomeDirection), &DomeSelectionStateDocV1> =
        BTreeMap::new();
    for selection in selections {
        let slot = (
            selection.selection.receiver.instance_id.clone(),
            selection.selection.receiver.direction,
        );
        let replace = winner_by_slot.get(&slot).is_none_or(|current| {
            selection.selection.slot_generation > current.selection.slot_generation
                || (selection.selection.slot_generation == current.selection.slot_generation
                    && selection.envelope_id < current.envelope_id)
        });
        if replace {
            winner_by_slot.insert(slot, selection);
        }
    }
    winner_by_slot
        .into_values()
        .map(|state| (state.selection.proposal_id.clone(), state.selection.clone()))
        .collect()
}

fn proposal_instance_invalidation(
    proposal: &DomeConnectionProposalV1,
    instances: &[DomeInstanceManifestV1],
) -> Option<DomeConnectionTerminalReasonV1> {
    let proposer_exists = instances.iter().any(|instance| {
        instance.instance_id == proposal.proposer.instance_id
            && instance.generation == proposal.proposer.instance_generation
    });
    let receiver_exists = instances.iter().any(|instance| {
        instance.instance_id == proposal.receiver.instance_id
            && instance.generation == proposal.receiver.instance_generation
    });
    (!proposer_exists || !receiver_exists)
        .then_some(DomeConnectionTerminalReasonV1::InstanceDetached)
}

fn direction_key(direction: DomeDirection) -> &'static str {
    match direction {
        DomeDirection::North => "north",
        DomeDirection::East => "east",
        DomeDirection::South => "south",
        DomeDirection::West => "west",
    }
}

fn connection_tags_for_state(record: &DomeConnectionRecordV1, author: &str) -> Vec<Vec<String>> {
    vec![
        vec!["author".into(), author.into()],
        vec!["object".into(), "dome-connection-lifecycle".into()],
        vec!["connection".into(), record.agreement.connection_id.clone()],
        vec![
            "context".into(),
            record.agreement.spatial_context.canonical_id(),
        ],
        vec!["generation".into(), record.lifecycle_generation.to_string()],
    ]
}
