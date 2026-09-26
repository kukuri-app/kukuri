//! Owner-confirmed, generation-bound Dome deletion. The signed journal lives in
//! the same Context replica as the Instance, including for private channels.
use crate::CloseDomeHostingInput;
use crate::game::dome_instance_manifest_from_game_manifest;
use crate::service::*;
use kukuri_core::{DomeHostTargetV1, DomeInstanceStatusV1, SpatialContextV1};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DeleteDomeInput {
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub expected_generation: u64,
    pub operation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DeleteDomeView {
    pub instance_id: String,
    pub generation: u64,
    pub deleted: bool,
    pub cleanup_pending: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PendingDomeDeletionView {
    pub request: DeleteDomeInput,
    pub title: String,
    pub deleted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DomeDeletion {
    request: DeleteDomeInput,
    owner: Pubkey,
    manifest: GameRoomManifestBlobV1,
    created_at: i64,
    completed: bool,
    pub node_url: Option<String>,
    pub signed_close_json: Option<String>,
}

impl AppService {
    pub async fn list_pending_dome_deletions(
        &self,
        context: SpatialContextV1,
    ) -> Result<Vec<PendingDomeDeletionView>> {
        let replica = self.dome_deletion_replica(&context).await?;
        let records = self
            .services
            .docs_sync
            .query_replica(
                &replica,
                DocQuery::Prefix("metaverse/dome-deletions/".into()),
            )
            .await?;
        let mut pending = vec![];
        for record in records {
            let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)?;
            if envelope.pubkey != self.services.keys.public_key() {
                continue;
            }
            envelope.verify()?;
            if envelope.kind != "dome-deletion" {
                anyhow::bail!("invalid deletion record");
            }
            let operation: DomeDeletion = serde_json::from_str(&envelope.content)?;
            if operation.owner != envelope.pubkey
                || operation.request.spatial_context != context
                || record.key != deletion_key(&operation.request)
            {
                anyhow::bail!("invalid deletion identity");
            }
            if !operation.completed || operation.node_url.is_some() {
                pending.push(PendingDomeDeletionView {
                    request: operation.request,
                    title: operation.manifest.title,
                    deleted: operation.completed,
                });
            }
        }
        Ok(pending)
    }

    pub async fn finish_dome_deletion_release(&self, input: &DeleteDomeInput) -> Result<()> {
        let _guard = self.services.dome_mutations.lock().await;
        let replica = self.dome_deletion_replica(&input.spatial_context).await?;
        let mut record = self
            .load_dome_deletion(&replica, input)
            .await?
            .context("Dome deletion not found")?;
        if record.request != *input || !record.completed {
            anyhow::bail!("DOME_DELETE_OPERATION_MISMATCH");
        }
        record.node_url = None;
        self.save_dome_deletion(&replica, &record).await
    }

    pub async fn delete_dome(&self, input: DeleteDomeInput) -> Result<DeleteDomeView> {
        let _guard = self.services.dome_mutations.lock().await;
        if input.operation_id.is_empty()
            || input.operation_id.len() > 128
            || !input
                .operation_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            anyhow::bail!("DOME_DELETE_INVALID_OPERATION");
        }
        let replica = self.dome_deletion_replica(&input.spatial_context).await?;
        let owner = self.services.keys.public_key();
        let mut operation = match self.load_dome_deletion(&replica, &input).await? {
            Some(record) => {
                if record.request != input || record.owner != owner {
                    anyhow::bail!("DOME_DELETE_OPERATION_MISMATCH");
                }
                if record.completed {
                    return Ok(deletion_view(&record));
                }
                record
            }
            None => {
                let (state, instance) = self
                    .fetch_dome_instance_manifest(&replica, &owner)
                    .await?
                    .context("DOME_DELETE_OWNER_REQUIRED")?;
                if instance.instance_id != input.instance_id
                    || instance.spatial_context != input.spatial_context
                    || instance.generation != input.expected_generation
                    || instance.status != DomeInstanceStatusV1::Active
                    || instance.relationship_detach.is_some()
                {
                    anyhow::bail!("DOME_DELETE_STALE_INSTANCE");
                }
                let manifest = crate::dome_management::instance_management_manifest(&instance);
                let hosting = self
                    .get_dome_hosting_authority(input.spatial_context.clone(), &input.instance_id)
                    .await?;
                let node_url = hosting.lease.as_ref().and_then(|lease| match &lease.host {
                    DomeHostTargetV1::CommunityNode { api_base_url, .. } => {
                        Some(api_base_url.clone())
                    }
                    _ => None,
                });
                let record = DomeDeletion {
                    request: input.clone(),
                    owner: owner.clone(),
                    manifest,
                    created_at: state.created_at,
                    completed: false,
                    node_url,
                    signed_close_json: None,
                };
                self.save_dome_deletion(&replica, &record).await?;
                record
            }
        };
        let (_, instance) = self
            .fetch_dome_instance_manifest(&replica, &owner)
            .await?
            .context("DOME_DELETE_STALE_INSTANCE")?;
        if instance.instance_id != input.instance_id
            || instance.generation != input.expected_generation
        {
            anyhow::bail!("DOME_DELETE_STALE_INSTANCE");
        }
        if instance.status == DomeInstanceStatusV1::Active {
            let hosting = self
                .get_dome_hosting_authority(input.spatial_context.clone(), &input.instance_id)
                .await?;
            if hosting.lease.is_some() && operation.signed_close_json.is_none() {
                let closed = self
                    .close_dome_hosting_unlocked(CloseDomeHostingInput {
                        expected_generation: Some(input.expected_generation),
                        spatial_context: input.spatial_context.clone(),
                        instance_id: input.instance_id.clone(),
                    })
                    .await?;
                operation.signed_close_json = closed.signed_close_json;
                self.save_dome_deletion(&replica, &operation).await?;
            }
        }
        let topology = self
            .list_dome_connection_topology(input.spatial_context.clone())
            .await?;
        for connection in topology.connections {
            let agreement = &connection.record.agreement;
            if [&agreement.proposer, &agreement.receiver]
                .iter()
                .any(|endpoint| {
                    endpoint.instance_id == input.instance_id
                        && endpoint.instance_generation == input.expected_generation
                        && endpoint.owner_pubkey == owner
                })
            {
                self.terminate_dome_connection_with_reason(
                    &input.spatial_context,
                    &agreement.connection_id,
                    DomeConnectionTerminalReasonV1::InstanceDeleted,
                )
                .await?;
            }
        }
        self.stop_owner_dome_hosting(&input.instance_id).await;
        self.dome_host_heartbeats
            .lock()
            .await
            .remove(&input.instance_id);
        let mut manifest = operation.manifest.clone();
        let metaverse = manifest.metaverse.as_mut().context("Dome state missing")?;
        metaverse.instance_status = DomeInstanceStatusV1::Tombstoned;
        metaverse.replacement_instance_id = None;
        manifest.status = GameRoomStatus::Ended;
        manifest.updated_at = Utc::now().timestamp_millis();
        let tombstone = dome_instance_manifest_from_game_manifest(&manifest)?;
        self.persist_dome_instance_manifest(&replica, &tombstone, operation.created_at)
            .await?;
        let state = self
            .persist_game_room_manifest(
                &replica,
                input.spatial_context.topic_id().as_str(),
                manifest.clone(),
                operation.created_at,
            )
            .await?;
        self.services
            .projection_store
            .upsert_game_room_cache(game_projection_row(&state))
            .await?;
        // Topology resolution discards endpoints of the tombstoned generation.
        self.list_dome_connection_topology(input.spatial_context.clone())
            .await?;
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(
                    input.spatial_context.topic_id().as_str(),
                    manifest.channel_id.as_ref(),
                ),
                GossipHint::SessionChanged {
                    topic_id: input.spatial_context.topic_id().clone(),
                    session_id: input.instance_id.clone(),
                    object_kind: "game-session".into(),
                },
            )
            .await?;
        operation.completed = true;
        self.save_dome_deletion(&replica, &operation).await?;
        Ok(deletion_view(&operation))
    }

    pub async fn dome_deletion_release(
        &self,
        input: &DeleteDomeInput,
    ) -> Result<Option<(String, String)>> {
        let replica = self.dome_deletion_replica(&input.spatial_context).await?;
        let record = self
            .load_dome_deletion(&replica, input)
            .await?
            .context("Dome deletion not found")?;
        if record.request != *input || !record.completed {
            anyhow::bail!("DOME_DELETE_OPERATION_MISMATCH");
        }
        Ok(record.node_url.zip(record.signed_close_json))
    }

    pub(crate) async fn ensure_dome_not_deleting(
        &self,
        context: &SpatialContextV1,
        id: &str,
        generation: u64,
    ) -> Result<()> {
        let replica = self.dome_deletion_replica(context).await?;
        let input = DeleteDomeInput {
            spatial_context: context.clone(),
            instance_id: id.into(),
            expected_generation: generation,
            operation_id: String::new(),
        };
        if self.load_dome_deletion(&replica, &input).await?.is_some() {
            anyhow::bail!("DOME_DELETE_IN_PROGRESS");
        }
        Ok(())
    }

    async fn dome_deletion_replica(&self, context: &SpatialContextV1) -> Result<ReplicaId> {
        match context {
            SpatialContextV1::Topic { topic_id } => {
                // 公開 topic は gossip を止めていなければ扱える(購読の有無は問わない。#1221 R2-C)。
                if self.is_topic_gossip_disabled(topic_id.as_str()).await {
                    anyhow::bail!("DOME_DELETE_CONTEXT_ACCESS_REQUIRED");
                }
                Ok(topic_replica_id(topic_id.as_str()))
            }
            SpatialContextV1::Channel {
                topic_id,
                channel_id,
            } => {
                let state = self
                    .private_channel_write_state(topic_id.as_str(), channel_id)
                    .await?;
                Ok(current_private_channel_replica_id(&state))
            }
        }
    }

    pub(crate) async fn ensure_dome_deletion_finished(
        &self,
        context: &SpatialContextV1,
        id: &str,
        generation: u64,
    ) -> Result<()> {
        let replica = self.dome_deletion_replica(context).await?;
        let input = DeleteDomeInput {
            spatial_context: context.clone(),
            instance_id: id.into(),
            expected_generation: generation,
            operation_id: String::new(),
        };
        if self
            .load_dome_deletion(&replica, &input)
            .await?
            .is_some_and(|r| !r.completed)
        {
            anyhow::bail!("DOME_DELETE_IN_PROGRESS");
        }
        Ok(())
    }

    async fn load_dome_deletion(
        &self,
        replica: &ReplicaId,
        input: &DeleteDomeInput,
    ) -> Result<Option<DomeDeletion>> {
        let records = self
            .services
            .docs_sync
            .query_replica(replica, DocQuery::Exact(deletion_key(input)))
            .await?;
        let Some(record) = records.first() else {
            return Ok(None);
        };
        let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)?;
        envelope.verify()?;
        if envelope.kind != "dome-deletion" || envelope.pubkey != self.services.keys.public_key() {
            anyhow::bail!("invalid Dome deletion signature");
        }
        let operation: DomeDeletion = serde_json::from_str(&envelope.content)?;
        if operation.owner != envelope.pubkey
            || operation.request.spatial_context != input.spatial_context
            || operation.request.instance_id != input.instance_id
            || operation.request.expected_generation != input.expected_generation
        {
            anyhow::bail!("invalid Dome deletion identity");
        }
        Ok(Some(operation))
    }

    async fn save_dome_deletion(&self, replica: &ReplicaId, record: &DomeDeletion) -> Result<()> {
        let envelope = kukuri_core::sign_envelope_json(
            self.services.keys.as_ref(),
            "dome-deletion",
            vec![],
            record,
        )?;
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: deletion_key(&record.request),
                    value: serde_json::to_value(envelope)?,
                },
            )
            .await
    }
}

fn deletion_key(input: &DeleteDomeInput) -> String {
    // Hash the identity instead of interpolating untrusted path components.
    let hash = kukuri_core::blob_hash(format!(
        "{}:{}:{}",
        input.spatial_context.canonical_id(),
        input.instance_id,
        input.expected_generation
    ));
    format!("metaverse/dome-deletions/{}/state", hash.as_str())
}
fn deletion_view(record: &DomeDeletion) -> DeleteDomeView {
    DeleteDomeView {
        instance_id: record.request.instance_id.clone(),
        generation: record.request.expected_generation,
        deleted: record.completed,
        cleanup_pending: record.node_url.is_some(),
    }
}
