use super::*;

// capability registry の永続化はラッパー側の手動 persist ではなく、AppService へ注入した
// write-through callback(registry 変異時に発火)が担う(WP-C2 boundary)。
impl DesktopRuntime {
    pub async fn list_pending_dome_deletions(
        &self,
        context: kukuri_core::SpatialContextV1,
    ) -> Result<Vec<kukuri_app_api::PendingDomeDeletionView>> {
        self.app_service.list_pending_dome_deletions(context).await
    }

    pub async fn delete_dome(
        &self,
        request: crate::DeleteDomeRequest,
    ) -> Result<kukuri_app_api::DeleteDomeView> {
        let mut result = self.app_service.delete_dome(request.clone()).await?;
        if let Some((url, signed_close_json)) =
            self.app_service.dome_deletion_release(&request).await?
        {
            let signed_close = serde_json::from_str(&signed_close_json)?;
            result.cleanup_pending = self
                .release_dome_hosting_on_community_node(
                    &url,
                    &DomeHostingReleaseRequest {
                        instance_id: request.instance_id.clone(),
                        signed_close,
                    },
                )
                .await
                .is_err();
        }
        if !result.cleanup_pending {
            self.app_service
                .finish_dome_deletion_release(&request)
                .await?;
        }
        Ok(result)
    }

    pub async fn preview_dome_transition_access(
        &self,
        request: PrepareDomeTransitionRequest,
    ) -> Result<kukuri_core::DomeTransitionAccessDecisionV1> {
        if request.request.participant_pubkey != self.author_keys.public_key() {
            anyhow::bail!("Dome transition participant does not match local identity");
        }
        self.app_service
            .preview_dome_transition_access(PrepareDomeTransitionInput {
                request: request.request,
            })
            .await
    }

    pub async fn create_private_channel(
        &self,
        request: CreatePrivateChannelRequest,
    ) -> Result<JoinedPrivateChannelView> {
        self.app_service
            .create_private_channel(CreatePrivateChannelInput {
                topic_id: TopicId::new(request.topic),
                label: request.label,
                audience_kind: request.audience_kind,
            })
            .await
    }

    pub async fn export_private_channel_invite(
        &self,
        request: ExportPrivateChannelInviteRequest,
    ) -> Result<String> {
        self.app_service
            .export_private_channel_invite(
                request.topic.as_str(),
                request.channel_id.as_str(),
                request.expires_at,
            )
            .await
    }

    pub async fn import_private_channel_invite(
        &self,
        request: ImportPrivateChannelInviteRequest,
    ) -> Result<PrivateChannelInvitePreview> {
        self.app_service
            .import_private_channel_invite(request.token.as_str())
            .await
    }

    pub async fn export_channel_access_token(
        &self,
        request: ExportChannelAccessTokenRequest,
    ) -> Result<ChannelAccessTokenExport> {
        self.app_service
            .export_channel_access_token(
                request.topic.as_str(),
                request.channel_id.as_str(),
                request.expires_at,
            )
            .await
    }

    pub async fn import_channel_access_token(
        &self,
        request: ImportChannelAccessTokenRequest,
    ) -> Result<ChannelAccessTokenPreview> {
        self.app_service
            .import_channel_access_token(request.token.as_str())
            .await
    }

    pub async fn preview_channel_access_token(
        &self,
        request: PreviewChannelAccessTokenRequest,
    ) -> Result<ChannelAccessTokenPreview> {
        self.app_service
            .preview_channel_access_token(request.token.as_str())
            .await
    }

    pub async fn export_friend_only_grant(
        &self,
        request: ExportFriendOnlyGrantRequest,
    ) -> Result<String> {
        self.app_service
            .export_friend_only_grant(
                request.topic.as_str(),
                request.channel_id.as_str(),
                request.expires_at,
            )
            .await
    }

    pub async fn import_friend_only_grant(
        &self,
        request: ImportFriendOnlyGrantRequest,
    ) -> Result<FriendOnlyGrantPreview> {
        self.app_service
            .import_friend_only_grant(request.token.as_str())
            .await
    }

    pub async fn export_friend_plus_share(
        &self,
        request: ExportFriendPlusShareRequest,
    ) -> Result<String> {
        self.app_service
            .export_friend_plus_share(
                request.topic.as_str(),
                request.channel_id.as_str(),
                request.expires_at,
            )
            .await
    }

    pub async fn import_friend_plus_share(
        &self,
        request: ImportFriendPlusShareRequest,
    ) -> Result<FriendPlusSharePreview> {
        self.app_service
            .import_friend_plus_share(request.token.as_str())
            .await
    }

    pub async fn freeze_private_channel(
        &self,
        request: FreezePrivateChannelRequest,
    ) -> Result<JoinedPrivateChannelView> {
        self.app_service
            .freeze_private_channel(request.topic.as_str(), request.channel_id.as_str())
            .await
    }

    pub async fn rotate_private_channel(
        &self,
        request: RotatePrivateChannelRequest,
    ) -> Result<JoinedPrivateChannelView> {
        let rotated = self
            .app_service
            .rotate_private_channel(request.topic.as_str(), request.channel_id.as_str())
            .await?;
        for session in self.community_node_sessions.lock().await.values_mut() {
            session.rendezvous_refresh_deadline = 0;
        }
        Ok(rotated)
    }

    pub async fn leave_private_channel(&self, request: LeavePrivateChannelRequest) -> Result<()> {
        let _grant_guard = self.private_index_grant_guard.lock().await;
        self.app_service
            .leave_private_channel(request.topic.as_str(), request.channel_id.as_str())
            .await?;
        self.store
            .stop_private_index_grants_for_channel(
                request.topic.as_str(),
                request.channel_id.as_str(),
            )
            .await
    }

    pub async fn set_private_channel_entry_dome(
        &self,
        request: SetPrivateChannelEntryDomeRequest,
    ) -> Result<JoinedPrivateChannelView> {
        self.app_service
            .set_private_channel_entry_dome(
                request.topic.as_str(),
                request.channel_id.as_str(),
                request.entry_dome_instance_id,
            )
            .await
    }

    pub async fn list_joined_private_channels(
        &self,
        request: ListJoinedPrivateChannelsRequest,
    ) -> Result<Vec<JoinedPrivateChannelView>> {
        self.app_service
            .list_joined_private_channels(request.topic.as_str())
            .await
    }

    pub async fn update_game_room(&self, request: UpdateGameRoomRequest) -> Result<()> {
        self.app_service
            .update_game_room(
                request.topic.as_str(),
                request.room_id.as_str(),
                UpdateGameRoomInput {
                    status: request.status,
                    phase_label: request.phase_label,
                    scores: request.scores,
                },
            )
            .await
    }

    pub async fn create_metaverse_room(
        &self,
        request: CreateMetaverseRoomRequest,
    ) -> Result<String> {
        self.app_service
            .create_metaverse_room_in_channel(
                request.topic.as_str(),
                request.channel_ref,
                CreateMetaverseRoomInput {
                    title: request.title,
                    description: request.description,
                    max_peers: request.max_peers,
                },
            )
            .await
    }

    pub async fn update_metaverse_room(&self, request: UpdateMetaverseRoomRequest) -> Result<()> {
        self.app_service
            .update_metaverse_room(
                request.topic.as_str(),
                request.room_id.as_str(),
                UpdateMetaverseRoomInput {
                    status: request.status,
                    customization: request.customization,
                },
            )
            .await
    }

    pub async fn get_dome_hosting(
        &self,
        request: GetDomeHostingRequest,
    ) -> Result<DomeHostingView> {
        let mut view = self
            .app_service
            .get_dome_hosting(request.spatial_context, &request.instance_id)
            .await?;
        if let Some(DomeHostTargetV1::CommunityNode { api_base_url, .. }) =
            view.lease.as_ref().map(|lease| &lease.host)
        {
            let status = self
                .get_dome_hosting_status_from_community_node(api_base_url, &request.instance_id)
                .await;
            if let Ok(status) = status {
                let now = chrono::Utc::now().timestamp_millis();
                if let (Some(lease), Some(session_id), Some(heartbeat)) = (
                    view.lease.as_ref(),
                    status.session_id.as_deref(),
                    status.signed_heartbeat.as_ref(),
                ) {
                    verify_signed_dome_host_heartbeat(heartbeat, lease, session_id)?;
                    if heartbeat.heartbeat.sent_at > now.saturating_add(5_000) {
                        anyhow::bail!("Community Node Dome heartbeat is from the future");
                    }
                    let mut heartbeats = self.community_node_dome_heartbeats.lock().await;
                    let replace = heartbeats.get(&request.instance_id).is_none_or(|current| {
                        heartbeat.heartbeat.sequence > current.heartbeat.sequence
                    });
                    if replace {
                        heartbeats.insert(request.instance_id.clone(), heartbeat.clone());
                    }
                    view.state.last_heartbeat_at = Some(heartbeat.heartbeat.sent_at);
                    view.state.reason = None;
                    view.state.kind = match kukuri_core::resolve_dome_host_liveness(
                        Some(heartbeat.heartbeat.sent_at),
                        now,
                    ) {
                        kukuri_core::DomeHostLivenessV1::Online => status.state,
                        kukuri_core::DomeHostLivenessV1::OfflineGrace => {
                            kukuri_core::DomeHostingStateKindV1::GracePeriod
                        }
                        kukuri_core::DomeHostLivenessV1::Closed => {
                            kukuri_core::DomeHostingStateKindV1::Closed
                        }
                    };
                } else {
                    view.state.kind = match status.state {
                        kukuri_core::DomeHostingStateKindV1::OwnerHosted
                        | kukuri_core::DomeHostingStateKindV1::CommunityNodeHosted => {
                            view.state.reason = Some("heartbeat_missing".into());
                            kukuri_core::DomeHostingStateKindV1::GracePeriod
                        }
                        state => state,
                    };
                }
                view.state.session_id = status.session_id;
                view.participants = status.participants;
                view.sleeping = status.sleeping;
            } else if let Some(heartbeat) = self
                .community_node_dome_heartbeats
                .lock()
                .await
                .get(&request.instance_id)
                .cloned()
            {
                let liveness = kukuri_core::resolve_dome_host_liveness(
                    Some(heartbeat.heartbeat.sent_at),
                    chrono::Utc::now().timestamp_millis(),
                );
                view.state.kind = match liveness {
                    kukuri_core::DomeHostLivenessV1::Online => view.state.kind,
                    kukuri_core::DomeHostLivenessV1::OfflineGrace => {
                        kukuri_core::DomeHostingStateKindV1::GracePeriod
                    }
                    kukuri_core::DomeHostLivenessV1::Closed => {
                        kukuri_core::DomeHostingStateKindV1::Closed
                    }
                };
                view.state.last_heartbeat_at = Some(heartbeat.heartbeat.sent_at);
                view.state.reason = Some(if liveness == kukuri_core::DomeHostLivenessV1::Closed {
                    "heartbeat_timeout".into()
                } else {
                    "heartbeat_missing".into()
                });
                view.participants = heartbeat.heartbeat.participants;
                view.sleeping = heartbeat.heartbeat.sleeping;
            } else {
                view.state.kind = kukuri_core::DomeHostingStateKindV1::GracePeriod;
                view.state.reason = Some("heartbeat_missing".into());
            }
        }
        Ok(view)
    }

    pub async fn start_owner_dome_hosting(
        &self,
        request: StartOwnerDomeHostingRequest,
    ) -> Result<DomeHostingView> {
        self.app_service
            .start_owner_dome_hosting(StartOwnerDomeHostingInput {
                expected_generation: request.expected_generation,
                spatial_context: request.spatial_context,
                instance_id: request.instance_id,
                endpoint_id: request.endpoint_id,
                lease_duration_millis: request.lease_duration_millis,
            })
            .await
    }

    pub async fn delegate_dome_hosting(
        &self,
        request: DelegateDomeHostingRequest,
    ) -> Result<DomeHostingView> {
        let prepared = self
            .app_service
            .prepare_community_node_dome_hosting(PrepareCommunityNodeDomeHostingInput {
                expected_generation: request.expected_generation,
                spatial_context: request.spatial_context.clone(),
                instance_id: request.instance_id.clone(),
                node_id: request.node_id,
                api_base_url: request.base_url.clone(),
                lease_duration_millis: request.lease_duration_millis,
            })
            .await?;
        self.complete_community_node_dome_transfer(
            &request.base_url,
            request.spatial_context,
            request.instance_id,
            &prepared,
            (
                "prepared Dome hosting view has no signed lease",
                "activated Dome hosting view has no signed activation",
            ),
        )
        .await
    }

    pub async fn close_dome_hosting(
        &self,
        request: CloseDomeHostingRequest,
    ) -> Result<DomeHostingView> {
        let previous = self
            .app_service
            .get_dome_hosting(request.spatial_context.clone(), &request.instance_id)
            .await?;
        let closed = self
            .app_service
            .close_dome_hosting(CloseDomeHostingInput {
                expected_generation: request.expected_generation,
                spatial_context: request.spatial_context,
                instance_id: request.instance_id.clone(),
            })
            .await?;
        if let Some(DomeHostTargetV1::CommunityNode { api_base_url, .. }) =
            previous.lease.map(|lease| lease.host)
        {
            let signed_close: SignedDomeHostingCloseV1 = serde_json::from_str(
                closed
                    .signed_close_json
                    .as_deref()
                    .context("closed Dome hosting view has no signed close record")?,
            )?;
            self.release_dome_hosting_on_community_node(
                &api_base_url,
                &DomeHostingReleaseRequest {
                    instance_id: request.instance_id,
                    signed_close,
                },
            )
            .await?;
        }
        Ok(closed)
    }

    pub async fn submit_dome_session_input(
        &self,
        request: SubmitDomeSessionInputRequest,
    ) -> Result<kukuri_core::DomePhysicsSnapshotV1> {
        let hosting = self
            .get_dome_hosting(GetDomeHostingRequest {
                spatial_context: request.spatial_context.clone(),
                instance_id: request.instance_id.clone(),
            })
            .await?;
        let lease = hosting.lease.context("Dome is not currently hosted")?;
        if request
            .expected_generation
            .is_some_and(|generation| generation != lease.instance_generation)
        {
            anyhow::bail!("DOME_SESSION_STALE_INSTANCE");
        }
        let session_id = hosting
            .state
            .session_id
            .clone()
            .context("Dome session is not active")?;
        let signed_snapshot = match &lease.host {
            DomeHostTargetV1::OwnerDevice { host_pubkey, .. }
                if host_pubkey == &self.author_keys.public_key() =>
            {
                self.app_service
                    .submit_dome_session_input(SubmitDomeSessionInput {
                        expected_generation: Some(lease.instance_generation),
                        spatial_context: request.spatial_context,
                        instance_id: request.instance_id,
                        sequence: request.sequence,
                        input: request.input,
                    })
                    .await?
            }
            DomeHostTargetV1::CommunityNode { api_base_url, .. } => {
                let access_proof = if matches!(
                    &request.input,
                    kukuri_core::DomeSessionInputKindV1::Join { .. }
                        | kukuri_core::DomeSessionInputKindV1::KeepAlive
                ) {
                    Some(
                        self.app_service
                            .build_dome_access_proof(
                                request.spatial_context.clone(),
                                lease.owner_pubkey.clone(),
                            )
                            .await?,
                    )
                } else {
                    None
                };
                let signed_input = build_signed_dome_session_input(
                    self.author_keys.as_ref(),
                    kukuri_core::DomeSessionInputV1 {
                        input_id: format!("input-{}-{}", request.instance_id, request.sequence),
                        instance_id: request.instance_id,
                        instance_generation: lease.instance_generation,
                        lease_epoch: lease.epoch,
                        session_id: session_id.clone(),
                        participant_pubkey: self.author_keys.public_key(),
                        sequence: request.sequence,
                        sent_at: chrono::Utc::now().timestamp_millis(),
                        input: request.input,
                    },
                )?;
                self.submit_dome_hosting_input_to_community_node(
                    api_base_url,
                    &DomeHostingSessionInputRequest {
                        signed_input,
                        access_proof,
                    },
                )
                .await?
                .signed_snapshot
            }
            _ => anyhow::bail!("the active Dome host is not reachable from this device"),
        };
        verify_signed_dome_physics_snapshot(&signed_snapshot, &lease, &session_id)?;
        Ok(signed_snapshot.snapshot)
    }

    pub async fn prepare_dome_transition(
        &self,
        request: PrepareDomeTransitionRequest,
    ) -> Result<kukuri_core::DomeTransitionAdmissionTicketV1> {
        if request.request.participant_pubkey != self.author_keys.public_key() {
            anyhow::bail!("Dome transition participant does not match local identity");
        }
        let topology = self
            .list_dome_connection_topology(ListDomeConnectionTopologyRequest {
                spatial_context: request.request.spatial_context.clone(),
            })
            .await?;
        if topology.resolution.topology.topology_digest != request.request.topology_digest
            || !topology
                .resolution
                .topology
                .active_connection_ids
                .contains(&request.request.connection_id)
        {
            anyhow::bail!("DOME_TRANSITION_STALE_TOPOLOGY");
        }
        let connection = topology.connections.iter().find(|view| {
            let agreement = &view.record.agreement;
            agreement.connection_id == request.request.connection_id
                && ((agreement.proposer.instance_id == request.request.source_instance_id
                    && agreement.proposer.instance_generation
                        == request.request.source_instance_generation
                    && agreement.proposer.direction == request.request.direction
                    && agreement.receiver.instance_id == request.request.target_instance_id
                    && agreement.receiver.instance_generation
                        == request.request.target_instance_generation)
                    || (agreement.receiver.instance_id == request.request.source_instance_id
                        && agreement.receiver.instance_generation
                            == request.request.source_instance_generation
                        && agreement.receiver.direction == request.request.direction
                        && agreement.proposer.instance_id == request.request.target_instance_id
                        && agreement.proposer.instance_generation
                            == request.request.target_instance_generation))
        });
        let Some(connection) = connection else {
            anyhow::bail!("DOME_TRANSITION_STALE_TOPOLOGY");
        };
        let agreement = &connection.record.agreement;
        let target_owner_pubkey =
            if agreement.proposer.instance_id == request.request.target_instance_id {
                agreement.proposer.owner_pubkey.clone()
            } else {
                agreement.receiver.owner_pubkey.clone()
            };
        let hosting = self
            .get_dome_hosting(GetDomeHostingRequest {
                spatial_context: request.request.spatial_context.clone(),
                instance_id: request.request.target_instance_id.clone(),
            })
            .await?;
        let lease = hosting
            .lease
            .context("destination Dome is not currently hosted")?;
        match &lease.host {
            DomeHostTargetV1::OwnerDevice { host_pubkey, .. }
                if host_pubkey == &self.author_keys.public_key() =>
            {
                self.app_service
                    .prepare_dome_transition(PrepareDomeTransitionInput {
                        request: request.request,
                    })
                    .await
            }
            DomeHostTargetV1::CommunityNode { api_base_url, .. } => Ok(self
                .prepare_dome_transition_on_community_node(
                    api_base_url,
                    &DomeTransitionPrepareRequest {
                        access_proof: self
                            .app_service
                            .build_dome_access_proof(
                                request.request.spatial_context.clone(),
                                target_owner_pubkey,
                            )
                            .await?,
                        request: request.request,
                    },
                )
                .await?
                .ticket),
            _ => {
                anyhow::bail!("the active destination Dome host is not reachable from this device")
            }
        }
    }

    pub async fn commit_dome_transition(&self, request: CommitDomeTransitionRequest) -> Result<()> {
        if request.ticket.request.participant_pubkey != self.author_keys.public_key() {
            anyhow::bail!("Dome transition participant does not match local identity");
        }
        let hosting = self
            .get_dome_hosting(GetDomeHostingRequest {
                spatial_context: request.ticket.request.spatial_context.clone(),
                instance_id: request.ticket.request.target_instance_id.clone(),
            })
            .await?;
        let lease = hosting
            .lease
            .context("destination Dome is not currently hosted")?;
        match &lease.host {
            DomeHostTargetV1::OwnerDevice { host_pubkey, .. }
                if host_pubkey == &self.author_keys.public_key() =>
            {
                self.app_service
                    .commit_dome_transition(CommitDomeTransitionInput {
                        ticket: request.ticket,
                        position: request.position,
                        rotation: request.rotation,
                    })
                    .await
            }
            DomeHostTargetV1::CommunityNode { api_base_url, .. } => {
                self.commit_dome_transition_on_community_node(
                    api_base_url,
                    &DomeTransitionCommitRequest {
                        ticket: request.ticket,
                        position: request.position,
                        rotation: request.rotation,
                    },
                )
                .await?;
                Ok(())
            }
            _ => {
                anyhow::bail!("the active destination Dome host is not reachable from this device")
            }
        }
    }

    pub async fn abort_dome_transition(&self, request: AbortDomeTransitionRequest) -> Result<()> {
        if request.ticket.request.participant_pubkey != self.author_keys.public_key() {
            anyhow::bail!("Dome transition participant does not match local identity");
        }
        let hosting = self
            .get_dome_hosting(GetDomeHostingRequest {
                spatial_context: request.ticket.request.spatial_context.clone(),
                instance_id: request.ticket.request.target_instance_id.clone(),
            })
            .await?;
        let lease = hosting
            .lease
            .context("destination Dome is not currently hosted")?;
        match &lease.host {
            DomeHostTargetV1::OwnerDevice { host_pubkey, .. }
                if host_pubkey == &self.author_keys.public_key() =>
            {
                self.app_service
                    .abort_dome_transition(AbortDomeTransitionInput {
                        ticket: request.ticket,
                    })
                    .await
            }
            DomeHostTargetV1::CommunityNode { api_base_url, .. } => {
                self.abort_dome_transition_on_community_node(
                    api_base_url,
                    &DomeTransitionAbortRequest {
                        ticket: request.ticket,
                    },
                )
                .await?;
                Ok(())
            }
            _ => {
                anyhow::bail!("the active destination Dome host is not reachable from this device")
            }
        }
    }

    pub async fn commit_dome_layout(
        &self,
        request: CommitDomeLayoutRequest,
    ) -> Result<DomeLayoutCommitView> {
        let hosting = self
            .get_dome_hosting(GetDomeHostingRequest {
                spatial_context: request.spatial_context.clone(),
                instance_id: request.instance_id.clone(),
            })
            .await?;
        let candidate_json = match hosting.lease.as_ref().map(|lease| &lease.host) {
            Some(DomeHostTargetV1::CommunityNode { api_base_url, .. }) => {
                let response = self
                    .capture_dome_layout_candidate_from_community_node(
                        api_base_url,
                        &DomeHostingLayoutCandidateRequest {
                            instance_id: request.instance_id.clone(),
                            operation_id: request.operation_id.clone(),
                        },
                    )
                    .await?;
                Some(serde_json::to_string(&response.signed_candidate)?)
            }
            _ => None,
        };
        let mut committed = self
            .app_service
            .commit_dome_layout(CommitDomeLayoutInput {
                spatial_context: request.spatial_context.clone(),
                instance_id: request.instance_id.clone(),
                operation_id: request.operation_id,
                signed_candidate_json: candidate_json,
            })
            .await?;
        let Some(DomeHostTargetV1::CommunityNode { api_base_url, .. }) =
            committed.hosting.lease.as_ref().map(|lease| &lease.host)
        else {
            return Ok(committed);
        };
        if committed.hosting.state.kind != kukuri_core::DomeHostingStateKindV1::Transferring {
            return Ok(committed);
        }
        let api_base_url = api_base_url.clone();
        committed.hosting = self
            .complete_community_node_dome_transfer(
                &api_base_url,
                request.spatial_context,
                request.instance_id,
                &committed.hosting,
                (
                    "prepared layout commit has no signed lease",
                    "activated layout commit has no signed activation",
                ),
            )
            .await?;
        Ok(committed)
    }

    pub async fn resync_dome_snapshots(
        &self,
        request: ResyncDomeSnapshotsRequest,
    ) -> Result<Vec<kukuri_core::DomePhysicsSnapshotV1>> {
        let hosting = self
            .get_dome_hosting(GetDomeHostingRequest {
                spatial_context: request.spatial_context.clone(),
                instance_id: request.instance_id.clone(),
            })
            .await?;
        let lease = hosting.lease.context("Dome is not currently hosted")?;
        let session_id = hosting
            .state
            .session_id
            .context("Dome session is not active")?;
        let signed = match &lease.host {
            DomeHostTargetV1::OwnerDevice { .. } => {
                self.app_service
                    .resync_dome_snapshots(ResyncDomeSnapshotsInput {
                        spatial_context: request.spatial_context,
                        instance_id: request.instance_id,
                        after_sequence: request.after_sequence,
                    })
                    .await?
            }
            DomeHostTargetV1::CommunityNode { api_base_url, .. } => {
                self.resync_dome_snapshots_from_community_node(
                    api_base_url,
                    &DomeHostingSnapshotResyncRequest {
                        instance_id: request.instance_id,
                        after_sequence: request.after_sequence,
                    },
                )
                .await?
                .snapshots
            }
        };
        signed
            .into_iter()
            .map(|snapshot| {
                verify_signed_dome_physics_snapshot(&snapshot, &lease, &session_id)?;
                Ok(snapshot.snapshot)
            })
            .collect()
    }

    // Both entrypoints retain their own preparation/no-op rules. This method owns
    // assignment -> owner activation persistence -> remote activation in that order.
    async fn complete_community_node_dome_transfer(
        &self,
        base_url: &str,
        spatial_context: kukuri_core::SpatialContextV1,
        instance_id: String,
        prepared: &DomeHostingView,
        error_contexts: (&'static str, &'static str),
    ) -> Result<DomeHostingView> {
        let signed_lease: SignedDomeHostingLeaseV1 = serde_json::from_str(
            prepared
                .signed_lease_json
                .as_deref()
                .context(error_contexts.0)?,
        )?;
        let assignment = self
            .assign_dome_hosting_to_community_node(
                base_url,
                &self
                    .build_dome_hosting_assignment_request(
                        signed_lease,
                        &prepared.instance_manifest_json,
                        prepared
                            .preset_manifest_json
                            .as_deref()
                            .context("Dome preset manifest is unavailable")?,
                    )
                    .await?,
            )
            .await?;
        let activated = self
            .app_service
            .activate_community_node_dome_hosting(ActivateCommunityNodeDomeHostingInput {
                spatial_context,
                instance_id: instance_id.clone(),
                signed_acceptance_json: serde_json::to_string(&assignment.signed_acceptance)?,
            })
            .await?;
        let signed_activation: SignedDomeHostingActivationV1 = serde_json::from_str(
            activated
                .signed_activation_json
                .as_deref()
                .context(error_contexts.1)?,
        )?;
        self.activate_dome_hosting_on_community_node(
            base_url,
            &DomeHostingActivationRequest {
                instance_id,
                signed_activation,
            },
        )
        .await?;
        Ok(activated)
    }

    async fn build_dome_hosting_assignment_request(
        &self,
        signed_lease: SignedDomeHostingLeaseV1,
        instance_manifest_json: &str,
        preset_manifest_json: &str,
    ) -> Result<DomeHostingAssignmentRequest> {
        let instance_manifest: DomeInstanceManifestV1 =
            serde_json::from_str(instance_manifest_json)?;
        let preset_manifest: DomePresetManifestV1 = serde_json::from_str(preset_manifest_json)?;
        let mut asset_blobs = Vec::with_capacity(preset_manifest.asset_refs.len());
        for asset in &preset_manifest.asset_refs {
            let bytes = self
                .app_service
                .fetch_metaverse_blob_bytes(&asset.blob_hash)
                .await?
                .with_context(|| format!("Dome asset {} is unavailable", asset.blob_hash))?;
            asset_blobs.push(DomeHostingAssetBlob {
                blob_hash: asset.blob_hash.clone(),
                bytes,
            });
        }
        Ok(DomeHostingAssignmentRequest {
            signed_lease,
            instance_manifest,
            preset_manifest,
            asset_blobs,
        })
    }

    pub async fn move_dome(
        &self,
        request: MoveDomeRequest,
    ) -> Result<kukuri_app_api::DomeMoveView> {
        self.app_service
            .move_dome(
                request.source_topic.as_str(),
                MoveDomeInput {
                    move_id: request.move_id,
                    source_instance_id: request.source_instance_id,
                    target_context: request.target_context,
                },
            )
            .await
    }

    pub async fn list_dome_connection_topology(
        &self,
        request: ListDomeConnectionTopologyRequest,
    ) -> Result<kukuri_app_api::DomeConnectionTopologyView> {
        self.app_service
            .list_dome_connection_topology(request.spatial_context)
            .await
    }

    pub async fn create_dome_connection_proposal(
        &self,
        request: CreateDomeConnectionProposalRequest,
    ) -> Result<kukuri_app_api::DomeConnectionProposalView> {
        self.app_service
            .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
                proposal_id: request.proposal_id,
                spatial_context: request.spatial_context,
                proposer_instance_id: request.proposer_instance_id,
                receiver_instance_id: request.receiver_instance_id,
                proposer_direction: request.proposer_direction,
            })
            .await
    }

    pub async fn accept_dome_connection_proposal(
        &self,
        request: AcceptDomeConnectionProposalRequest,
    ) -> Result<kukuri_app_api::DomeConnectionView> {
        self.app_service
            .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
                spatial_context: request.spatial_context,
                proposal_id: request.proposal_id,
            })
            .await
    }

    pub async fn withdraw_dome_connection_proposal(
        &self,
        request: WithdrawDomeConnectionProposalRequest,
    ) -> Result<kukuri_app_api::DomeConnectionProposalView> {
        self.app_service
            .withdraw_dome_connection_proposal(WithdrawDomeConnectionProposalInput {
                spatial_context: request.spatial_context,
                proposal_id: request.proposal_id,
            })
            .await
    }

    pub async fn revoke_dome_connection(
        &self,
        request: RevokeDomeConnectionRequest,
    ) -> Result<kukuri_app_api::DomeConnectionView> {
        self.app_service
            .revoke_dome_connection(RevokeDomeConnectionInput {
                spatial_context: request.spatial_context,
                connection_id: request.connection_id,
            })
            .await
    }

    pub async fn publish_metaverse_room_event(
        &self,
        request: PublishMetaverseRoomEventRequest,
    ) -> Result<MetaverseRoomEventView> {
        self.app_service
            .publish_metaverse_room_event(
                request.topic.as_str(),
                PublishMetaverseRoomEventInput {
                    room_id: request.room_id,
                    peer_id: request.peer_id,
                    seq: request.seq,
                    event: request.event,
                },
            )
            .await
    }

    pub async fn list_metaverse_room_events(
        &self,
        request: ListMetaverseRoomEventsRequest,
    ) -> Result<Vec<MetaverseRoomEventView>> {
        self.app_service
            .list_metaverse_room_events(
                request.topic.as_str(),
                request.room_id.as_str(),
                request.after_envelope_id.as_deref(),
                request.limit,
            )
            .await
    }

    pub async fn import_metaverse_room_asset(
        &self,
        request: ImportMetaverseRoomAssetRequest,
    ) -> Result<MetaverseAssetRefView> {
        let bytes = BASE64_STANDARD
            .decode(request.data_base64.as_bytes())
            .context("failed to decode metaverse asset data")?;
        self.app_service
            .import_metaverse_room_asset(
                request.topic.as_str(),
                ImportMetaverseRoomAssetInput {
                    room_id: request.room_id,
                    kind: request.kind,
                    mime_type: request.mime_type,
                    name: request.name,
                    bytes,
                },
            )
            .await
    }

    pub async fn import_peer_ticket(&self, request: ImportPeerTicketRequest) -> Result<()> {
        self.app_service
            .import_peer_ticket(request.ticket.as_str())
            .await
    }

    pub async fn set_discovery_seeds(
        &self,
        request: SetDiscoverySeedsRequest,
    ) -> Result<DiscoveryConfig> {
        let mut next_config = self.discovery_config.lock().await.clone();
        if next_config.env_locked {
            bail!("discovery configuration is locked by environment variables");
        }
        next_config.seed_peers = parse_seed_entries(&request.seed_entries)?;
        save_discovery_config(&self.db_path, &next_config.stored())?;
        *self.discovery_config.lock().await = next_config.clone();
        self.apply_effective_seed_peers().await?;
        Ok(next_config)
    }

    pub async fn unsubscribe_topic(&self, request: UnsubscribeTopicRequest) -> Result<()> {
        self.app_service
            .unsubscribe_topic(request.topic.as_str())
            .await
    }

    pub async fn set_topic_gossip_enabled(
        &self,
        request: SetTopicGossipEnabledRequest,
    ) -> Result<()> {
        self.app_service
            .set_topic_gossip_enabled(request.topic.as_str(), request.enabled)
            .await?;
        self.persist_gossip_subscription_state_from_app().await
    }

    pub async fn set_channel_gossip_enabled(
        &self,
        request: SetChannelGossipEnabledRequest,
    ) -> Result<()> {
        self.app_service
            .set_channel_gossip_enabled(
                request.topic.as_str(),
                request.channel.as_str(),
                request.enabled,
            )
            .await?;
        self.persist_gossip_subscription_state_from_app().await
    }

    pub async fn local_peer_ticket(&self) -> Result<Option<String>> {
        self.app_service.peer_ticket().await
    }

    pub async fn get_blob_preview_url(
        &self,
        request: GetBlobPreviewRequest,
    ) -> Result<Option<String>> {
        if let Some(kind) = request.metaverse_kind.clone() {
            let Some(bytes) = self
                .app_service
                .fetch_metaverse_blob_bytes(request.hash.as_str())
                .await?
            else {
                return Ok(None);
            };
            let metadata = kukuri_core::inspect_metaverse_asset(kind.clone(), &bytes)?;
            let budget = self.app_service.metaverse_resource_budget();
            let (scope, resource, byte_limit) = match kind {
                kukuri_core::MetaverseAssetKind::Vrm => (
                    kukuri_core::MetaverseBudgetScope::Player,
                    kukuri_core::MetaverseBudgetResource::AvatarAssetBytes,
                    budget.player.max_avatar_asset_bytes,
                ),
                kukuri_core::MetaverseAssetKind::Texture => (
                    kukuri_core::MetaverseBudgetScope::Client,
                    kukuri_core::MetaverseBudgetResource::TextureBytes,
                    budget.dome.max_texture_bytes,
                ),
                _ => (
                    kukuri_core::MetaverseBudgetScope::Client,
                    kukuri_core::MetaverseBudgetResource::ModelBytes,
                    budget.dome.max_model_bytes,
                ),
            };
            if metadata.stored_bytes > byte_limit {
                return Err(kukuri_core::MetaverseResourceRejection::new(
                    scope,
                    resource,
                    kukuri_core::MetaverseResourceRejectionReason::LimitExceeded,
                    metadata.stored_bytes,
                    byte_limit,
                )
                .into());
            }
            if metadata.model_triangles > budget.client.max_rendered_triangles {
                return Err(kukuri_core::MetaverseResourceRejection::new(
                    kukuri_core::MetaverseBudgetScope::Client,
                    kukuri_core::MetaverseBudgetResource::RenderedTriangles,
                    kukuri_core::MetaverseResourceRejectionReason::LimitExceeded,
                    metadata.model_triangles,
                    budget.client.max_rendered_triangles,
                )
                .into());
            }
            if metadata.decoded_texture_bytes > budget.client.max_texture_memory_bytes {
                return Err(kukuri_core::MetaverseResourceRejection::new(
                    kukuri_core::MetaverseBudgetScope::Client,
                    kukuri_core::MetaverseBudgetResource::TextureMemory,
                    kukuri_core::MetaverseResourceRejectionReason::LimitExceeded,
                    metadata.decoded_texture_bytes,
                    budget.client.max_texture_memory_bytes,
                )
                .into());
            }
        }
        self.app_service
            .blob_preview_data_url(request.hash.as_str(), request.mime.as_str())
            .await
    }

    pub async fn get_blob_media_payload(
        &self,
        request: GetBlobMediaRequest,
    ) -> Result<Option<BlobMediaPayload>> {
        if request.hash.trim().is_empty() {
            tracing::warn!(mime = %request.mime, "blob media payload request skipped because hash was blank");
            return Ok(None);
        }
        self.app_service
            .blob_media_payload_for_post(
                request.hash.as_str(),
                request.mime.as_str(),
                request.source_object_id.as_deref(),
            )
            .await
    }

    pub async fn get_blob_media_file(
        &self,
        request: GetBlobMediaRequest,
        path: &std::path::Path,
    ) -> Result<Option<u64>> {
        self.app_service
            .blob_media_file_for_post(
                request.hash.as_str(),
                request.source_object_id.as_deref(),
                path,
            )
            .await
    }
}
