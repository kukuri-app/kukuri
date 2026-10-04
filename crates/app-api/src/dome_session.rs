//! 所有者の端末の Dome host の入力の検証と適用。別の端末の participant から P2P で届く要求も、
//! 所有者本人の入力と同じ確認点で処理する(ADR 0038、#1527)。

use crate::service::*;
use crate::views::CommitDomeTransitionInput;
use kukuri_core::{
    DOME_SESSION_RESYNC_MAX_BYTES, DomeInstanceStatusV1, DomeSessionInputKindV1,
    DomeSessionInputV1, DomeSessionRequestV1, DomeSessionResponseV1, DomeSpatialAccessProofV1,
    DomeTransitionAccessDecisionV1, DomeTransitionAdmissionRequestV1,
    DomeTransitionAdmissionTicketV1, MetaverseResourceRejection, SignedDomePhysicsSnapshotV1,
    SignedDomeSessionInputV1, SpatialContextV1, build_signed_dome_session_input,
};

pub(crate) enum DomeInputSource {
    /// 所有者本人の端末の input。host の lease と session に束縛して、この端末の鍵で署名する。
    Local {
        sequence: u64,
        input: Box<DomeSessionInputKindV1>,
    },
    /// 別の端末の participant が署名した input。
    Remote(Box<SignedDomeSessionInputV1>),
}

impl AppService {
    /// 別の端末から届いた要求の bytes を処理し、応答の bytes を返す。処理できない要求は拒否の応答にする。
    pub async fn serve_dome_session_request(&self, request: &[u8]) -> Vec<u8> {
        let response = match serde_json::from_slice::<DomeSessionRequestV1>(request) {
            Ok(DomeSessionRequestV1::Input { signed_input }) => {
                let context = self
                    .dome_host_sessions
                    .lock()
                    .await
                    .get(&signed_input.input.instance_id)
                    .map(|runtime| runtime.lease().spatial_context.clone());
                match context {
                    Some(context) => {
                        let instance_id = signed_input.input.instance_id.clone();
                        let generation = Some(signed_input.input.instance_generation);
                        self.apply_dome_session_input(
                            &context,
                            &instance_id,
                            generation,
                            DomeInputSource::Remote(Box::new(signed_input)),
                        )
                        .await
                    }
                    None => Err(anyhow::anyhow!("this device is not the active Dome host")),
                }
                .map(|signed_snapshot| DomeSessionResponseV1::Snapshot {
                    signed_snapshot: Box::new(signed_snapshot),
                })
            }
            Ok(DomeSessionRequestV1::ResyncSnapshots {
                instance_id,
                after_sequence,
                access_proof,
            }) => self
                .resync_remote_dome_snapshots(&instance_id, after_sequence, &access_proof)
                .await
                .map(|snapshots| DomeSessionResponseV1::Snapshots { snapshots }),
            Ok(DomeSessionRequestV1::PrepareTransition {
                request,
                access_proof,
            }) => self
                .prepare_remote_dome_transition(request, &access_proof)
                .await
                .map(|ticket| DomeSessionResponseV1::Ticket {
                    ticket: Box::new(ticket),
                }),
            // commit / abort は host が発行した ticket と予約の一致で判定する(ADR 0042)。
            Ok(DomeSessionRequestV1::CommitTransition {
                ticket,
                position,
                rotation,
            }) => self
                .commit_dome_transition(CommitDomeTransitionInput {
                    ticket,
                    position,
                    rotation,
                })
                .await
                .map(|()| DomeSessionResponseV1::Accepted),
            Ok(DomeSessionRequestV1::AbortTransition { ticket }) => self
                .abort_remote_dome_transition(&ticket)
                .await
                .map(|()| DomeSessionResponseV1::Accepted),
            Err(error) => Err(error.into()),
        }
        .unwrap_or_else(|error| DomeSessionResponseV1::Rejected {
            resource_rejection: error.downcast_ref::<MetaverseResourceRejection>().cloned(),
            message: error.to_string(),
        });
        serde_json::to_vec(&response).unwrap_or_default()
    }

    /// 入力の共通の検証と適用(Community Node host と同じ確認点。署名・lease・sequence・budget は共通 runtime)。
    pub(crate) async fn apply_dome_session_input(
        &self,
        context: &SpatialContextV1,
        instance_id: &str,
        expected_generation: Option<u64>,
        source: DomeInputSource,
    ) -> Result<SignedDomePhysicsSnapshotV1> {
        let (participant, kind) = match &source {
            DomeInputSource::Local { input, .. } => (self.services.keys.public_key(), &**input),
            DomeInputSource::Remote(signed) => {
                (signed.input.participant_pubkey.clone(), &signed.input.input)
            }
        };
        let join = matches!(kind, DomeSessionInputKindV1::Join { .. });
        let keep_alive = matches!(kind, DomeSessionInputKindV1::KeepAlive);
        self.hosting_context_replica(context).await?;
        let instance = self
            .hosting_instance(context, instance_id)
            .await?
            .context("Dome instance was not found")?;
        if expected_generation.is_some_and(|generation| generation != instance.generation)
            || instance.status != DomeInstanceStatusV1::Active
            || instance.relationship_detach.is_some()
        {
            anyhow::bail!("DOME_SESSION_STALE_INSTANCE");
        }
        if join || keep_alive {
            self.ensure_dome_room_access(context, &instance.owner_pubkey, &participant)
                .await?;
        }
        let now = Utc::now().timestamp_millis();
        let mut sessions = self.dome_host_sessions.lock().await;
        let runtime = sessions
            .get_mut(instance_id)
            .context("this device is not the active Dome host")?;
        if runtime.lease().spatial_context != *context
            || runtime.lease().instance_generation != instance.generation
        {
            anyhow::bail!("Dome session input SpatialContext mismatch");
        }
        let signed = match source {
            DomeInputSource::Local { sequence, input } => build_signed_dome_session_input(
                self.services.keys.as_ref(),
                DomeSessionInputV1 {
                    input_id: format!("input-{instance_id}-{sequence}"),
                    instance_id: instance_id.to_string(),
                    instance_generation: runtime.lease().instance_generation,
                    lease_epoch: runtime.lease().epoch,
                    session_id: runtime.session_id().to_string(),
                    participant_pubkey: participant,
                    sequence,
                    sent_at: now,
                    input: *input,
                },
            )?,
            // 別の端末の Join 以外の input は、現在の participant だけを受け付ける(状態を変えず snapshot も返さない)。
            DomeInputSource::Remote(signed) => {
                if !join && !runtime.is_participant(&participant) {
                    anyhow::bail!("DOME_SESSION_NOT_JOINED");
                }
                *signed
            }
        };
        runtime.apply_signed_input_at(&signed, now)?;
        if join {
            runtime.signed_admission_snapshot(now)
        } else {
            runtime.signed_snapshot(now)
        }
    }

    /// 別の端末の participant の再同期。access proof で本人を確かめ、現在の participant で access と block を
    /// 満たすときだけ ring を返す(ADR 0038)。
    async fn resync_remote_dome_snapshots(
        &self,
        instance_id: &str,
        after_sequence: u64,
        proof: &DomeSpatialAccessProofV1,
    ) -> Result<Vec<SignedDomePhysicsSnapshotV1>> {
        let participant = &proof.statement.participant_pubkey;
        let lease = self
            .dome_host_sessions
            .lock()
            .await
            .get(instance_id)
            .map(|runtime| runtime.lease().clone())
            .context("this device is not the active Dome host")?;
        proof.verify_for(
            participant,
            &lease.spatial_context,
            &lease.owner_pubkey,
            Utc::now().timestamp_millis(),
        )?;
        self.ensure_dome_room_access(&lease.spatial_context, &lease.owner_pubkey, participant)
            .await?;
        let sessions = self.dome_host_sessions.lock().await;
        let runtime = sessions
            .get(instance_id)
            .filter(|runtime| runtime.lease().epoch == lease.epoch)
            .context("this device is not the active Dome host")?;
        if !runtime.is_participant(participant) {
            anyhow::bail!("DOME_SESSION_NOT_JOINED");
        }
        Ok(newest_within(
            runtime.snapshots_after(after_sequence),
            DOME_SESSION_RESYNC_MAX_BYTES,
        ))
    }

    /// 別の端末の participant の遷移の予約(Community Node と同じ確認。2026-10-04 ユーザー判断)。access proof で本人と
    /// Spatial Context・遷移先の owner への束縛を確かめ、owner 間と visitor の block、access、定員を key 指定で確かめる。
    /// topology 全体は照合しない(要求ごとの処理を件数に依存させない。ADR 0038)。Spatial Context・遷移先・世代と
    /// lease の一致は runtime が確かめる。
    async fn prepare_remote_dome_transition(
        &self,
        request: DomeTransitionAdmissionRequestV1,
        proof: &DomeSpatialAccessProofV1,
    ) -> Result<DomeTransitionAdmissionTicketV1> {
        let now = Utc::now().timestamp_millis();
        let target_owner = self
            .dome_host_sessions
            .lock()
            .await
            .get(&request.target_instance_id)
            .map(|runtime| runtime.lease().owner_pubkey.clone())
            .context("this device is not the active destination Dome host")?;
        proof.verify_for(
            &request.participant_pubkey,
            &request.spatial_context,
            &target_owner,
            now,
        )?;
        // 遷移元の owner を導けなければ、owner 間の block を確かめられないので拒否する(fail-closed。ADR 0042)。
        let source_owner = self
            .dome_instance_owner(&request.spatial_context, &request.source_instance_id)
            .await?
            .context("DOME_TRANSITION_STALE_TOPOLOGY")?;
        let access = self
            .evaluate_dome_transition_access(&request, &source_owner, &target_owner)
            .await?;
        self.dome_host_sessions
            .lock()
            .await
            .get_mut(&request.target_instance_id)
            .context("this device is not the active destination Dome host")?
            .prepare_transition_admission(request, access, now)
    }

    /// 別の端末の取消。要求者を識別しない経路なので、予約が残っていれば host が発行した ticket との一致を求める
    /// (ticket の所持で判定する。ADR 0038)。
    async fn abort_remote_dome_transition(
        &self,
        ticket: &DomeTransitionAdmissionTicketV1,
    ) -> Result<()> {
        let mut sessions = self.dome_host_sessions.lock().await;
        let runtime = sessions
            .get_mut(&ticket.request.target_instance_id)
            .context("this device is not the active destination Dome host")?;
        if runtime
            .transition_reservation(&ticket.request.transition_id)
            .is_some_and(|reserved| reserved != ticket)
        {
            anyhow::bail!("DOME_TRANSITION_INVALID_TICKET");
        }
        runtime.abort_transition_admission(
            &ticket.request.transition_id,
            &ticket.request.participant_pubkey,
            Utc::now().timestamp_millis(),
        )
    }

    async fn ensure_dome_room_access(
        &self,
        context: &SpatialContextV1,
        owner_pubkey: &Pubkey,
        participant_pubkey: &Pubkey,
    ) -> Result<()> {
        match self
            .evaluate_dome_room_access(context, owner_pubkey, participant_pubkey)
            .await?
        {
            DomeTransitionAccessDecisionV1::Allowed => Ok(()),
            DomeTransitionAccessDecisionV1::Denied { reason } => anyhow::bail!(reason.code()),
        }
    }
}

/// 応答の上限に収まる新しい側だけを、sequence 順のまま残す(ADR 0038)。
fn newest_within(
    mut snapshots: Vec<SignedDomePhysicsSnapshotV1>,
    limit: usize,
) -> Vec<SignedDomePhysicsSnapshotV1> {
    // 応答の型の外側(type tag と配列の括弧)の分を残しておく。
    let mut remaining = limit.saturating_sub(64);
    let keep = snapshots
        .iter()
        .rev()
        .take_while(|snapshot| {
            let size = serde_json::to_vec(snapshot).map_or(usize::MAX, |bytes| bytes.len() + 1);
            remaining
                .checked_sub(size)
                .map(|left| remaining = left)
                .is_some()
        })
        .count();
    snapshots.split_off(snapshots.len() - keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resync_keeps_the_newest_snapshots_that_fit() {
        let keys = generate_keys();
        let lease = kukuri_core::DomeHostingLeaseV1 {
            lease_id: "lease-dome-1".into(),
            spatial_context: SpatialContextV1::Topic {
                topic_id: TopicId::new("kukuri:topic:resync"),
            },
            instance_id: "dome".into(),
            instance_generation: 1,
            owner_pubkey: keys.public_key(),
            host: kukuri_core::DomeHostTargetV1::OwnerDevice {
                endpoint_id: "endpoint".into(),
                host_pubkey: keys.public_key(),
            },
            manifest_blob_hash: "manifest".into(),
            manifest_version: 1,
            epoch: 1,
            issued_at: 1,
            expires_at: 10_000,
        };
        let snapshots = (1..=3)
            .map(|sequence| {
                kukuri_core::build_signed_dome_physics_snapshot(
                    &keys,
                    &lease,
                    kukuri_core::DomePhysicsSnapshotV1 {
                        instance_id: "dome".into(),
                        instance_generation: 1,
                        lease_epoch: 1,
                        session_id: "session".into(),
                        host_pubkey: keys.public_key(),
                        sequence,
                        simulated_at: 1,
                        sleeping: true,
                        bodies: Vec::new(),
                    },
                )
                .expect("snapshot")
            })
            .collect::<Vec<_>>();
        let one = serde_json::to_vec(&snapshots[0]).expect("encode").len() + 1;

        assert_eq!(newest_within(snapshots.clone(), usize::MAX), snapshots);
        let trimmed = newest_within(snapshots.clone(), 64 + one * 2);
        assert_eq!(
            trimmed
                .iter()
                .map(|s| s.snapshot.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3],
            "the newest ones, in sequence order"
        );
        assert!(newest_within(snapshots, 64).is_empty());
    }
}
