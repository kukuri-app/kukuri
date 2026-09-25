use super::*;
use kukuri_core::{
    DomeSpatialAccessProofV1, DomeTransitionAccessDecisionV1, DomeTransitionAdmissionRequestV1,
    DomeTransitionDenialReasonV1, SpatialContextV1, build_dome_spatial_access_proof,
};

impl AppService {
    pub(crate) async fn remove_joined_private_channel_and_evict_dome_participant(
        &self,
        topic_id: &str,
        channel_id: &ChannelId,
    ) -> Result<()> {
        self.remove_joined_private_channel(topic_id, channel_id.as_str())
            .await?;
        let context = SpatialContextV1::Channel {
            topic_id: TopicId::new(topic_id),
            channel_id: channel_id.clone(),
        };
        let participant = self.services.keys.public_key();
        for runtime in self.dome_host_sessions.lock().await.values_mut() {
            if runtime.lease().spatial_context == context {
                runtime.evict_participant(&participant);
            }
        }
        Ok(())
    }

    pub(crate) async fn evaluate_dome_room_access(
        &self,
        spatial_context: &SpatialContextV1,
        owner_pubkey: &Pubkey,
        participant_pubkey: &Pubkey,
    ) -> Result<DomeTransitionAccessDecisionV1> {
        if self
            .owner_blocks_visitor(owner_pubkey.as_str(), participant_pubkey.as_str())
            .await?
        {
            return Ok(DomeTransitionAccessDecisionV1::Denied {
                reason: DomeTransitionDenialReasonV1::VisitorBlocked,
            });
        }
        self.evaluate_spatial_context_access(spatial_context, participant_pubkey)
            .await
    }

    pub async fn build_dome_access_proof(
        &self,
        spatial_context: SpatialContextV1,
        target_owner_pubkey: Pubkey,
    ) -> Result<DomeSpatialAccessProofV1> {
        let participant_pubkey = self.services.keys.public_key();
        if !matches!(
            self.evaluate_dome_room_access(
                &spatial_context,
                &target_owner_pubkey,
                &participant_pubkey,
            )
            .await?,
            DomeTransitionAccessDecisionV1::Allowed
        ) {
            anyhow::bail!(DomeTransitionDenialReasonV1::AccessDenied.code());
        }
        let (policy_envelope, participant_envelope) = match &spatial_context {
            SpatialContextV1::Topic { .. } => (None, None),
            SpatialContextV1::Channel {
                topic_id,
                channel_id,
            } => {
                let state = self
                    .joined_private_channel_state(topic_id.as_str(), channel_id.as_str())
                    .await
                    .ok_or_else(|| anyhow::anyhow!("private channel is not joined"))?;
                let replica = current_private_channel_replica_id(&state);
                let policy = self
                    .docs_sync()
                    .query_replica(
                        &replica,
                        DocQuery::Exact(stable_key("channels", "policy/envelope")),
                    )
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        anyhow::anyhow!("private channel policy proof is unavailable")
                    })?;
                let participant = self
                    .docs_sync()
                    .query_replica(
                        &replica,
                        DocQuery::Exact(stable_key(
                            "channels/participants",
                            &format!("{}/envelope", participant_pubkey.as_str()),
                        )),
                    )
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        anyhow::anyhow!("private channel participant proof is unavailable")
                    })?;
                (
                    Some(serde_json::from_slice(&policy.value)?),
                    Some(serde_json::from_slice(&participant.value)?),
                )
            }
        };
        build_dome_spatial_access_proof(
            self.services.keys.as_ref(),
            spatial_context,
            target_owner_pubkey,
            Utc::now().timestamp_millis(),
            policy_envelope,
            participant_envelope,
        )
    }

    pub(crate) async fn evaluate_spatial_context_access(
        &self,
        spatial_context: &SpatialContextV1,
        participant_pubkey: &Pubkey,
    ) -> Result<DomeTransitionAccessDecisionV1> {
        let allowed = match spatial_context {
            SpatialContextV1::Topic { topic_id } => self
                .subscription_registry
                .subscriptions
                .lock()
                .await
                .get(topic_id.as_str())
                .is_some_and(|handle| !handle.is_finished()),
            SpatialContextV1::Channel {
                topic_id,
                channel_id,
            } => {
                let Some(state) = self
                    .joined_private_channel_state(topic_id.as_str(), channel_id.as_str())
                    .await
                else {
                    return Ok(DomeTransitionAccessDecisionV1::Denied {
                        reason: DomeTransitionDenialReasonV1::AccessDenied,
                    });
                };
                // #1221 R5-C: 入場する参加者の record だけを key 指定で読む(参加者の全件は読まない)。
                let replica = &current_private_channel_replica_id(&state);
                self.read_joined_private_control(&state, |docs, policy| async move {
                    fetch_private_channel_participant_from_replica(
                        docs.as_ref(),
                        replica,
                        participant_pubkey.as_str(),
                        policy,
                    )
                    .await
                })
                .await?
                .is_some_and(|participant| {
                    participant.epoch_id == state.current_epoch_id && participant.left_at.is_none()
                })
            }
        };
        Ok(if allowed {
            DomeTransitionAccessDecisionV1::Allowed
        } else {
            DomeTransitionAccessDecisionV1::Denied {
                reason: DomeTransitionDenialReasonV1::AccessDenied,
            }
        })
    }

    pub(crate) async fn evaluate_dome_transition_access(
        &self,
        request: &DomeTransitionAdmissionRequestV1,
        source_owner_pubkey: &Pubkey,
        target_owner_pubkey: &Pubkey,
    ) -> Result<DomeTransitionAccessDecisionV1> {
        if self
            .authors_blocked_either_direction(
                source_owner_pubkey.as_str(),
                target_owner_pubkey.as_str(),
            )
            .await?
        {
            return Ok(DomeTransitionAccessDecisionV1::Denied {
                reason: DomeTransitionDenialReasonV1::OwnersBlocked,
            });
        }
        if self
            .owner_blocks_visitor(
                target_owner_pubkey.as_str(),
                request.participant_pubkey.as_str(),
            )
            .await?
        {
            return Ok(DomeTransitionAccessDecisionV1::Denied {
                reason: DomeTransitionDenialReasonV1::VisitorBlocked,
            });
        }
        self.evaluate_spatial_context_access(&request.spatial_context, &request.participant_pubkey)
            .await
    }
}
