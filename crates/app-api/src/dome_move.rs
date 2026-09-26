use crate::game::dome_instance_manifest_from_game_manifest;
use crate::service::*;
use kukuri_core::{
    DomeInstanceStatusV1, DomeMovePhaseV1, DomeMoveRecordV1, DomeRelationshipDetachV1,
    SpatialContextV1,
};

impl AppService {
    pub async fn move_dome(
        &self,
        source_topic_id: &str,
        input: MoveDomeInput,
    ) -> Result<DomeMoveView> {
        let _guard = self.services.dome_mutations.lock().await;
        let actor = Pubkey::from(self.current_author_pubkey());
        if input.move_id.trim().is_empty() {
            anyhow::bail!("Dome move id is required");
        }

        let mut record = if let Some(existing) = self
            .fetch_dome_move_record(actor.as_str(), input.move_id.as_str())
            .await?
        {
            if existing.owner_pubkey != actor
                || existing.source_instance_id != input.source_instance_id
                || existing.target_context != input.target_context
            {
                anyhow::bail!("Dome move id is already bound to a different operation");
            }
            existing
        } else {
            let (source_replica_id, source_state, source_manifest) = self
                .fetch_game_room_state_and_manifest(
                    source_topic_id,
                    input.source_instance_id.as_str(),
                )
                .await?
                .ok_or_else(|| anyhow::anyhow!("source Dome instance not found"))?;
            let source = source_manifest
                .metaverse
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("source Dome state is missing"))?;
            if source_state.owner_pubkey != actor
                || source.instance_status != DomeInstanceStatusV1::Active
            {
                anyhow::bail!("only the owner can move an active Dome instance");
            }
            let (_, canonical_source) = self
                .fetch_dome_instance_manifest(&source_replica_id, &actor)
                .await?
                .ok_or_else(|| anyhow::anyhow!("canonical source Dome instance not found"))?;
            if canonical_source.instance_id != source.instance_id
                || canonical_source.generation != source.instance_generation
                || canonical_source.status != DomeInstanceStatusV1::Active
            {
                anyhow::bail!("source Dome projection does not match its canonical instance");
            }
            self.ensure_dome_not_deleting(
                &source.spatial_context,
                &source.instance_id,
                source.instance_generation,
            )
            .await?;
            if source.spatial_context == input.target_context {
                anyhow::bail!("Dome move target must be a different Spatial Context");
            }
            let target_hash = kukuri_core::blob_hash(format!(
                "dome-instance:{}:{}",
                input.target_context.canonical_id(),
                actor.as_str()
            ));
            let target_instance_id = format!("dome-{}", &target_hash.as_str()[..24]);
            let target_generation = match self
                .fetch_game_room_state_and_manifest(
                    input.target_context.topic_id().as_str(),
                    target_instance_id.as_str(),
                )
                .await?
            {
                Some((_, _, existing)) => {
                    let existing = existing
                        .metaverse
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("target Dome state is missing"))?;
                    if existing.instance_status != DomeInstanceStatusV1::Tombstoned {
                        anyhow::bail!("the owner already has a Dome in the target Spatial Context");
                    }
                    existing.instance_generation.saturating_add(1)
                }
                None => 1,
            };
            let record = DomeMoveRecordV1 {
                move_id: input.move_id.clone(),
                owner_pubkey: actor.clone(),
                source_instance_id: input.source_instance_id.clone(),
                source_context: source.spatial_context.clone(),
                source_generation: source.instance_generation,
                target_instance_id,
                target_context: input.target_context.clone(),
                target_generation,
                preset_ref: source.preset_ref.clone(),
                phase: DomeMovePhaseV1::Preparing,
                failure_reason: None,
                updated_at: Utc::now().timestamp_millis(),
            };
            self.persist_dome_move_record(&record).await?;
            record
        };

        if record.source_context.topic_id().as_str() != source_topic_id {
            anyhow::bail!("Dome move source topic does not match the recorded Spatial Context");
        }

        let target_topic_id = record.target_context.topic_id().as_str().to_string();
        let (target_channel_id, target_replica_id) = match &record.target_context {
            SpatialContextV1::Topic { .. } => (None, topic_replica_id(target_topic_id.as_str())),
            SpatialContextV1::Channel { channel_id, .. } => {
                let state = self
                    .private_channel_write_state(target_topic_id.as_str(), channel_id)
                    .await?;
                (
                    Some(state.channel_id.clone()),
                    current_private_channel_replica_id(&state),
                )
            }
        };

        if record.phase == DomeMovePhaseV1::Preparing {
            if let Some((_, existing)) = self
                .fetch_dome_instance_manifest(&target_replica_id, &actor)
                .await?
            {
                let is_same_staging_attempt = existing.instance_id == record.target_instance_id
                    && existing.generation == record.target_generation
                    && existing.status == DomeInstanceStatusV1::Staging
                    && existing.preset_ref == record.preset_ref;
                if existing.status != DomeInstanceStatusV1::Tombstoned && !is_same_staging_attempt {
                    anyhow::bail!("the owner already has a Dome in the target Spatial Context");
                }
            }
            let (_, _, source_manifest) = self
                .fetch_game_room_state_and_manifest(
                    source_topic_id,
                    record.source_instance_id.as_str(),
                )
                .await?
                .ok_or_else(|| anyhow::anyhow!("source Dome instance not found"))?;
            let preset = self
                .fetch_dome_preset_manifest(&record.preset_ref)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Dome Preset manifest is unavailable"))?;
            validate_dome_preset_manifest(&preset)?;
            for asset in &preset.asset_refs {
                let status = self
                    .services
                    .blob_service
                    .blob_status(&kukuri_core::BlobHash::new(asset.blob_hash.clone()))
                    .await?;
                if status == BlobStatus::Missing {
                    anyhow::bail!("Dome Preset asset is unavailable: {}", asset.blob_hash);
                }
            }
            let mut staged = source_manifest;
            staged.room_id = record.target_instance_id.clone();
            staged.topic_id = TopicId::new(target_topic_id.as_str());
            staged.channel_id = target_channel_id.clone();
            staged.updated_at = Utc::now().timestamp_millis();
            let target = staged
                .metaverse
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("source Dome state is missing"))?;
            target.instance_id = record.target_instance_id.clone();
            target.spatial_context = record.target_context.clone();
            target.instance_generation = record.target_generation;
            target.instance_status = DomeInstanceStatusV1::Staging;
            target.relationship_detach = None;
            target.replacement_instance_id = None;
            target.session_id = record.target_instance_id.clone();
            target.chat_history.clear();
            let staged_instance = dome_instance_manifest_from_game_manifest(&staged)?;
            self.persist_dome_instance_manifest(
                &target_replica_id,
                &staged_instance,
                Utc::now().timestamp_millis(),
            )
            .await?;
            let staged = self
                .persist_game_room_manifest(
                    &target_replica_id,
                    target_topic_id.as_str(),
                    staged,
                    Utc::now().timestamp_millis(),
                )
                .await?;
            // 次の段は projection から staging を読む。購読 task の反映を待たずに自分で置く(#1221 R2-C)。
            self.services
                .projection_store
                .upsert_game_room_cache(game_projection_row(&staged))
                .await?;
            record.phase = DomeMovePhaseV1::TargetStaged;
            record.updated_at = Utc::now().timestamp_millis();
            self.persist_dome_move_record(&record).await?;
        }

        if record.phase == DomeMovePhaseV1::TargetStaged {
            let (source_replica_id, source_state, mut source_manifest) = self
                .fetch_game_room_state_and_manifest(
                    source_topic_id,
                    record.source_instance_id.as_str(),
                )
                .await?
                .ok_or_else(|| anyhow::anyhow!("source Dome instance not found"))?;
            let source = source_manifest
                .metaverse
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("source Dome state is missing"))?;
            if source.instance_generation != record.source_generation {
                anyhow::bail!("source Dome generation changed during move");
            }
            source.relationship_detach = Some(DomeRelationshipDetachV1 {
                move_id: record.move_id.clone(),
                instance_generation: record.source_generation,
                detached_at: Utc::now().timestamp_millis(),
            });
            source_manifest.updated_at = Utc::now().timestamp_millis();
            let source_instance = dome_instance_manifest_from_game_manifest(&source_manifest)?;
            self.persist_dome_instance_manifest(
                &source_replica_id,
                &source_instance,
                source_state.created_at,
            )
            .await?;
            let persisted = self
                .persist_game_room_manifest(
                    &source_replica_id,
                    source_topic_id,
                    source_manifest.clone(),
                    source_state.created_at,
                )
                .await?;
            self.services
                .projection_store
                .upsert_game_room_cache(game_projection_row(&persisted))
                .await?;
            record.phase = DomeMovePhaseV1::SourceDetached;
            record.updated_at = Utc::now().timestamp_millis();
            self.persist_dome_move_record(&record).await?;
        }

        if record.phase == DomeMovePhaseV1::SourceDetached {
            let (target_source_replica, target_state, mut target_manifest) = self
                .fetch_game_room_state_and_manifest(
                    target_topic_id.as_str(),
                    record.target_instance_id.as_str(),
                )
                .await?
                .ok_or_else(|| anyhow::anyhow!("staging Dome instance not found"))?;
            let target = target_manifest
                .metaverse
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("target Dome state is missing"))?;
            target.instance_status = DomeInstanceStatusV1::Active;
            target_manifest.updated_at = Utc::now().timestamp_millis();
            let target_instance = dome_instance_manifest_from_game_manifest(&target_manifest)?;
            self.persist_dome_instance_manifest(
                &target_source_replica,
                &target_instance,
                target_state.created_at,
            )
            .await?;
            let persisted = self
                .persist_game_room_manifest(
                    &target_source_replica,
                    target_topic_id.as_str(),
                    target_manifest.clone(),
                    target_state.created_at,
                )
                .await?;
            self.services
                .projection_store
                .upsert_game_room_cache(game_projection_row(&persisted))
                .await?;
            record.phase = DomeMovePhaseV1::TargetActive;
            record.updated_at = Utc::now().timestamp_millis();
            self.persist_dome_move_record(&record).await?;
        }

        if record.phase == DomeMovePhaseV1::TargetActive {
            let (source_replica_id, source_state, mut source_manifest) = self
                .fetch_game_room_state_and_manifest(
                    source_topic_id,
                    record.source_instance_id.as_str(),
                )
                .await?
                .ok_or_else(|| anyhow::anyhow!("source Dome instance not found"))?;
            let source = source_manifest
                .metaverse
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("source Dome state is missing"))?;
            source.instance_status = DomeInstanceStatusV1::Tombstoned;
            source.replacement_instance_id = Some(record.target_instance_id.clone());
            source_manifest.status = GameRoomStatus::Ended;
            source_manifest.updated_at = Utc::now().timestamp_millis();
            let source_instance = dome_instance_manifest_from_game_manifest(&source_manifest)?;
            self.persist_dome_instance_manifest(
                &source_replica_id,
                &source_instance,
                source_state.created_at,
            )
            .await?;
            let persisted = self
                .persist_game_room_manifest(
                    &source_replica_id,
                    source_topic_id,
                    source_manifest.clone(),
                    source_state.created_at,
                )
                .await?;
            self.services
                .projection_store
                .upsert_game_room_cache(game_projection_row(&persisted))
                .await?;
            record.phase = DomeMovePhaseV1::SourceTombstoned;
            record.updated_at = Utc::now().timestamp_millis();
            self.persist_dome_move_record(&record).await?;
        }

        if record.phase == DomeMovePhaseV1::SourceTombstoned {
            record.phase = DomeMovePhaseV1::Completed;
            record.updated_at = Utc::now().timestamp_millis();
            self.persist_dome_move_record(&record).await?;
        }

        Ok(record)
    }
}
