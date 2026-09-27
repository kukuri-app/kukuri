//! Dome の接続の記録(提案・選択・合意)の保存と読取り(#1221 R5-H)。
//!
//! 接続の記録は、提案した Dome の anchor(Dome の継続状態の置き場所。ADR 0054 §2)に置く。読むのは、context で
//! 知っている Dome の anchor と、更新前の旧 context replica。切替後は旧 replica へ書かない。

use super::dome_connection_support::{CONNECTION_PREFIX, PROPOSAL_PREFIX, SELECTION_PREFIX};
use super::*;
use crate::dome_connections::{connection_tags_for_state, direction_key};
use kukuri_core::{
    DomeConnectionAgreementV1, SignedDomeConnectionAgreementV1, SpatialContextV1,
    sign_envelope_json, verify_signed_dome_connection_agreement,
};

impl AppService {
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
        self.services.docs_sync.open_replica(&replica).await?;
        Ok(replica)
    }

    /// 接続の記録を読む replica。context で知っている Dome(と `extra`)の anchor と、更新前の旧 context replica。
    pub(crate) async fn dome_connection_stores(
        &self,
        context: &SpatialContextV1,
        legacy: ReplicaId,
        extra: &[ReplicaId],
    ) -> Result<Vec<ReplicaId>> {
        let mut stores = extra.to_vec();
        for instance in self.list_context_dome_instances(context, []).await? {
            for anchor in self
                .dome_anchors(context, &instance.instance_id, &instance.owner_pubkey)
                .await?
            {
                if !stores.contains(&anchor) {
                    stores.push(anchor);
                }
            }
        }
        if !stores.contains(&legacy) {
            stores.push(legacy);
        }
        Ok(stores)
    }

    /// 新しい接続の記録を書く replica。提案した Dome の anchor(#1221 R5-H)。
    pub(crate) async fn dome_connection_write_replica(
        &self,
        proposer: &DomeInstanceManifestV1,
    ) -> Result<ReplicaId> {
        let replica = self
            .dome_anchors(
                &proposer.spatial_context,
                &proposer.instance_id,
                &proposer.owner_pubkey,
            )
            .await?
            .into_iter()
            .next()
            .context("the proposer Dome's anchor is unknown")?;
        self.dome_connection_legacy_guard(replica)
    }

    /// 既にある接続の記録を更新する replica。記録を見つけた replica で、それが更新前の旧 replica なら提案した Dome の
    /// anchor(切替後は旧 replica へ書かない)。
    pub(crate) async fn dome_connection_record_replica(
        &self,
        found: &ReplicaId,
        proposer: &DomeInstanceManifestV1,
    ) -> Result<ReplicaId> {
        if found.as_str().starts_with("bucket::") || !self.services.writes_buckets() {
            return Ok(found.clone());
        }
        self.dome_connection_write_replica(proposer).await
    }

    /// 切替後に旧 replica へ書かないことの確認(#1221 R5-H)。
    pub(crate) fn dome_connection_legacy_guard(&self, replica: ReplicaId) -> Result<ReplicaId> {
        anyhow::ensure!(
            !self.services.writes_buckets() || replica.as_str().starts_with("bucket::"),
            "Dome records are not written to a legacy replica after the writer switch"
        );
        Ok(replica)
    }

    pub(crate) async fn persist_connection_envelope(
        &self,
        replica: &ReplicaId,
        envelope: &KukuriEnvelope,
    ) -> Result<()> {
        self.services.docs_sync.open_replica(replica).await?;
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

    pub(crate) async fn fetch_connection_envelope(
        &self,
        stores: &[ReplicaId],
        envelope_id: &EnvelopeId,
    ) -> Result<KukuriEnvelope> {
        for replica in stores {
            let record = self
                .services
                .docs_sync
                .query_replica(
                    replica,
                    DocQuery::Exact(stable_key("envelopes", envelope_id.as_str())),
                )
                .await?
                .into_iter()
                .next();
            if let Some(record) = record {
                let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)?;
                envelope.verify()?;
                anyhow::ensure!(
                    envelope.id == *envelope_id,
                    "Dome Connection envelope id mismatch"
                );
                return Ok(envelope);
            }
        }
        Err(DomeReadUnavailable::Envelope.into())
    }

    /// 署名者と種類を確かめた接続の記録の envelope の中身。
    pub(crate) async fn fetch_signed_connection_content<T: serde::de::DeserializeOwned>(
        &self,
        stores: &[ReplicaId],
        envelope_id: &EnvelopeId,
        kind: &str,
        signer: &Pubkey,
    ) -> Result<T> {
        let envelope = self.fetch_connection_envelope(stores, envelope_id).await?;
        if envelope.kind != kind || envelope.pubkey != *signer {
            anyhow::bail!("signed Dome envelope identity does not match state");
        }
        Ok(serde_json::from_str(&envelope.content)?)
    }

    pub(crate) async fn persist_dome_proposal_state(
        &self,
        replica: &ReplicaId,
        state: &DomeProposalStateDocV1,
    ) -> Result<()> {
        self.services.docs_sync.open_replica(replica).await?;
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

    /// 提案の state。読む replica を順に探し、見つけた replica と返す。
    pub(crate) async fn fetch_dome_proposal_state(
        &self,
        stores: &[ReplicaId],
        proposal_id: &str,
    ) -> Result<Option<(ReplicaId, DomeProposalStateDocV1)>> {
        let key = stable_key(PROPOSAL_PREFIX, &format!("{proposal_id}/state"));
        for replica in stores {
            let record = self
                .services
                .docs_sync
                .query_replica(replica, DocQuery::Exact(key.clone()))
                .await?
                .into_iter()
                .next();
            if let Some(record) = record {
                let state: DomeProposalStateDocV1 = serde_json::from_slice(&record.value)?;
                self.verify_dome_proposal_state(stores, &state).await?;
                return Ok(Some((replica.clone(), state)));
            }
        }
        Ok(None)
    }

    pub(crate) async fn list_dome_proposal_states(
        &self,
        stores: &[ReplicaId],
    ) -> Result<Vec<DomeProposalStateDocV1>> {
        let mut states: Vec<DomeProposalStateDocV1> = Vec::new();
        for replica in stores {
            let records = self
                .services
                .docs_sync
                .query_replica(replica, DocQuery::Prefix(stable_key(PROPOSAL_PREFIX, "")))
                .await?;
            for record in records {
                if record.key.ends_with("/state") {
                    let state: DomeProposalStateDocV1 = serde_json::from_slice(&record.value)?;
                    if states
                        .iter()
                        .any(|known| known.proposal.proposal_id == state.proposal.proposal_id)
                    {
                        continue;
                    }
                    self.verify_dome_proposal_state(stores, &state).await?;
                    states.push(state);
                }
            }
        }
        states.sort_by(|left, right| left.proposal.proposal_id.cmp(&right.proposal.proposal_id));
        Ok(states)
    }

    pub(crate) async fn verify_dome_proposal_state(
        &self,
        stores: &[ReplicaId],
        state: &DomeProposalStateDocV1,
    ) -> Result<()> {
        let proposal: DomeConnectionProposalV1 = self
            .fetch_signed_connection_content(
                stores,
                &state.proposal_envelope_id,
                "dome-connection-proposal",
                &state.proposal.proposer.owner_pubkey,
            )
            .await?;
        if proposal != state.proposal {
            anyhow::bail!("signed Dome Connection proposal does not match state");
        }
        let agreement: DomeConnectionAgreementV1 = self
            .fetch_signed_connection_content(
                stores,
                &state.proposer_agreement_envelope_id,
                "dome-connection-agreement",
                &state.proposal.proposer.owner_pubkey,
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
                let envelope = self.fetch_connection_envelope(stores, envelope_id).await?;
                verify_dome_proposal_terminal_event(state, envelope_id, reason, &envelope)?;
            }
            (None, None) => {}
            _ => anyhow::bail!("Dome Connection terminal state is incomplete"),
        }
        Ok(())
    }

    pub(crate) async fn persist_dome_selection(
        &self,
        replica: &ReplicaId,
        state: &DomeSelectionStateDocV1,
    ) -> Result<()> {
        let direction = direction_key(state.selection.receiver.direction);
        self.services.docs_sync.open_replica(replica).await?;
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

    pub(crate) async fn list_dome_selections(
        &self,
        stores: &[ReplicaId],
    ) -> Result<Vec<DomeSelectionStateDocV1>> {
        let mut records = Vec::new();
        for replica in stores {
            records.extend(
                self.services
                    .docs_sync
                    .query_replica(replica, DocQuery::Prefix(stable_key(SELECTION_PREFIX, "")))
                    .await?,
            );
        }
        let mut states: Vec<DomeSelectionStateDocV1> = Vec::new();
        for record in records {
            let state: DomeSelectionStateDocV1 = serde_json::from_slice(&record.value)?;
            if states
                .iter()
                .any(|known| known.selection.selection_id == state.selection.selection_id)
            {
                continue;
            }
            let signed: DomeProposalSelectionV1 = self
                .fetch_signed_connection_content(
                    stores,
                    &state.envelope_id,
                    "dome-connection-selection",
                    &state.selection.receiver.owner_pubkey,
                )
                .await?;
            if signed != state.selection {
                anyhow::bail!("signed Dome Connection selection does not match state");
            }
            states.push(state);
        }
        Ok(states)
    }

    pub(crate) async fn persist_dome_connection_state(
        &self,
        replica: &ReplicaId,
        state: &DomeConnectionStateDocV1,
    ) -> Result<()> {
        self.services.docs_sync.open_replica(replica).await?;
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

    /// 接続の state。読む replica を順に探し、見つけた replica と返す。
    pub(crate) async fn fetch_dome_connection_state(
        &self,
        stores: &[ReplicaId],
        connection_id: &str,
    ) -> Result<Option<(ReplicaId, DomeConnectionStateDocV1)>> {
        let key = stable_key(CONNECTION_PREFIX, &format!("{connection_id}/state"));
        for replica in stores {
            let record = self
                .services
                .docs_sync
                .query_replica(replica, DocQuery::Exact(key.clone()))
                .await?
                .into_iter()
                .next();
            if let Some(record) = record {
                let state: DomeConnectionStateDocV1 = serde_json::from_slice(&record.value)?;
                self.verify_dome_connection_state(stores, &state).await?;
                return Ok(Some((replica.clone(), state)));
            }
        }
        Ok(None)
    }

    pub(crate) async fn list_dome_connection_states(
        &self,
        stores: &[ReplicaId],
    ) -> Result<Vec<DomeConnectionStateDocV1>> {
        let mut states: Vec<DomeConnectionStateDocV1> = Vec::new();
        for replica in stores {
            let records = self
                .services
                .docs_sync
                .query_replica(replica, DocQuery::Prefix(stable_key(CONNECTION_PREFIX, "")))
                .await?;
            for record in records {
                if record.key.ends_with("/state") {
                    let state: DomeConnectionStateDocV1 = serde_json::from_slice(&record.value)?;
                    if states.iter().any(|known| {
                        known.record.agreement.connection_id == state.record.agreement.connection_id
                    }) {
                        continue;
                    }
                    self.verify_dome_connection_state(stores, &state).await?;
                    states.push(state);
                }
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

    pub(crate) async fn verify_dome_connection_state(
        &self,
        stores: &[ReplicaId],
        state: &DomeConnectionStateDocV1,
    ) -> Result<()> {
        validate_dome_connection_record(&state.record)?;
        let signed = SignedDomeConnectionAgreementV1 {
            agreement: state.record.agreement.clone(),
            proposer_signature: self
                .fetch_connection_envelope(stores, &state.proposer_agreement_envelope_id)
                .await?,
            receiver_signature: self
                .fetch_connection_envelope(stores, &state.receiver_agreement_envelope_id)
                .await?,
        };
        verify_signed_dome_connection_agreement(&signed)?;
        let lifecycle = self
            .fetch_connection_envelope(stores, &state.lifecycle_envelope_id)
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
}
