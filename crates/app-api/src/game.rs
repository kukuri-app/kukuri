use crate::service::*;
use kukuri_core::{DomeInstanceStatusV1, MetaverseRoomEventV1, SpatialContextV1};

const METAVERSE_CHAT_HISTORY_LIMIT: usize = 100;

impl AppService {
    pub async fn list_game_rooms(&self, topic_id: &str) -> Result<Vec<GameRoomView>> {
        self.list_game_rooms_scoped(topic_id, TimelineScope::Public)
            .await
    }

    pub async fn list_game_rooms_scoped(
        &self,
        topic_id: &str,
        scope: TimelineScope,
    ) -> Result<Vec<GameRoomView>> {
        let hidden_author_pubkeys = self.current_hidden_author_pubkeys().await?;
        let channel_id = self.allowed_channel_id_for_scope(topic_id, &scope).await?;
        let allowed = BTreeSet::from([channel_id.clone()]);
        let projection_store = Arc::clone(&self.services.projection_store);
        let topic_key = topic_id.to_string();
        let channel_key = channel_id.clone();
        let hidden_author_pubkeys = Arc::new(hidden_author_pubkeys);
        let rows = load_projection_rows_with_one_refresh(
            move || {
                let projection_store = Arc::clone(&projection_store);
                let topic_key = topic_key.clone();
                let channel_key = channel_key.clone();
                let hidden_author_pubkeys = Arc::clone(&hidden_author_pubkeys);
                async move {
                    Ok(projection_store
                        .list_channel_game_rooms(
                            topic_key.as_str(),
                            channel_key.as_str(),
                            LIVE_GAME_LIST_LIMIT,
                        )
                        .await?
                        .into_iter()
                        .filter(|row| !hidden_author_pubkeys.contains(row.host_pubkey.as_str()))
                        .filter(|row| {
                            row.room_kind != GameRoomKind::MetaverseRoom
                                || row.metaverse.as_ref().is_some_and(|state| {
                                    state.instance_status == DomeInstanceStatusV1::Active
                                })
                        })
                        .collect())
                }
            },
            |rows| rows.is_empty(),
            || async {
                // #1239: replica を走査しない。session の固定件数だけを、key の一覧から反映する。
                self.catch_up_scope_sessions(topic_id, &scope).await?;
                Ok(())
            },
        )
        .await?;
        let mut items = Vec::with_capacity(rows.len());
        for mut row in rows {
            let dome_hosting = if let Some(metaverse) = row.metaverse.as_ref() {
                let replica = self
                    .hosting_context_replica(&metaverse.spatial_context)
                    .await?;
                let owner = Pubkey::from(row.host_pubkey.clone());
                let instance = match self
                    .hosting_instance_for_owner(
                        &replica,
                        &metaverse.spatial_context,
                        &metaverse.instance_id,
                        &owner,
                    )
                    .await
                {
                    Ok(Some(instance)) => instance,
                    Ok(None) => continue,
                    Err(error) if error.downcast_ref::<DomeReadUnavailable>().is_some() => continue,
                    Err(error) => return Err(error),
                };
                if instance.status != DomeInstanceStatusV1::Active
                    || instance.generation != metaverse.instance_generation
                {
                    continue;
                }
                // Resolve readiness from the canonical Instance, not the cached ref.
                // A pending Preset keeps its row for refresh; invalid data still fails.
                match self.hosting_view_for_instance(&replica, &instance).await {
                    Ok(hosting)
                        if hosting.preset_manifest_json.is_none()
                            && instance.owner_pubkey != self.services.keys.public_key() =>
                    {
                        continue;
                    }
                    Ok(hosting) => {
                        if hosting.preset_manifest_json.is_none() {
                            row.phase_label = Some("management_only".into());
                        }
                        Some(hosting.state)
                    }
                    Err(error)
                        if matches!(
                            error.downcast_ref::<DomeReadUnavailable>(),
                            Some(
                                DomeReadUnavailable::Preset
                                    | DomeReadUnavailable::Instance
                                    | DomeReadUnavailable::Envelope
                            )
                        ) =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            } else {
                None
            };
            items.push(GameRoomView {
                room_id: row.room_id,
                host_pubkey: row.host_pubkey,
                title: row.title,
                description: row.description,
                status: row.status,
                phase_label: row.phase_label,
                scores: row
                    .scores
                    .into_iter()
                    .map(|score| GameScoreView {
                        participant_id: score.participant_id,
                        label: score.label,
                        score: score.score,
                    })
                    .collect(),
                room_kind: row.room_kind,
                metaverse: row.metaverse,
                dome_hosting,
                manifest_blob_hash: row.manifest_blob_hash.as_str().to_string(),
                updated_at: row.updated_at,
                channel_id: channel_id_for_view(row.channel_id.as_str()),
                audience_label: self
                    .audience_label_for_storage(topic_id, row.channel_id.as_str())
                    .await,
            });
        }
        self.append_owned_dome_management(topic_id, &allowed, &mut items)
            .await?;
        Ok(items)
    }

    pub async fn create_game_room(
        &self,
        topic_id: &str,
        input: CreateGameRoomInput,
    ) -> Result<String> {
        self.create_game_room_in_channel(topic_id, ChannelRef::Public, input)
            .await
    }

    pub async fn create_game_room_in_channel(
        &self,
        topic_id: &str,
        channel_ref: ChannelRef,
        input: CreateGameRoomInput,
    ) -> Result<String> {
        let private_state = match channel_ref {
            ChannelRef::Public => None,
            ChannelRef::PrivateChannel { channel_id } => Some(
                self.private_channel_write_state(topic_id, &channel_id)
                    .await?,
            ),
        };
        let channel_id = private_state.as_ref().map(|state| state.channel_id.clone());
        let source_replica_id = private_state
            .as_ref()
            .map(current_private_channel_replica_id)
            .unwrap_or_else(|| topic_replica_id(topic_id));
        let participants = sanitize_game_participants(input.participants)?;
        let now = Utc::now().timestamp_millis();
        let title = input.title.trim();
        if title.is_empty() {
            anyhow::bail!("game room title is required");
        }
        let room_id = format!(
            "game-{}-{}",
            now,
            owner_bound_id_suffix(self.current_author_pubkey().as_str())
        );
        let manifest = GameRoomManifestBlobV1 {
            room_id: room_id.clone(),
            score_revision: Some(1),
            topic_id: TopicId::new(topic_id),
            channel_id: channel_id.clone(),
            owner_pubkey: Pubkey::from(self.current_author_pubkey()),
            title: title.to_string(),
            description: input.description.trim().to_string(),
            status: GameRoomStatus::Waiting,
            phase_label: None,
            participants: participants
                .iter()
                .enumerate()
                .map(|(index, label)| GameParticipant {
                    participant_id: format!("participant-{}", index + 1),
                    label: label.clone(),
                })
                .collect(),
            scores: participants
                .iter()
                .enumerate()
                .map(|(index, label)| GameScoreEntry {
                    participant_id: format!("participant-{}", index + 1),
                    label: label.clone(),
                    score: 0,
                })
                .collect(),
            room_kind: GameRoomKind::ScoreGame,
            metaverse: None,
            updated_at: now,
        };
        let projection_guard = self.services.game_room_projections.lock(&room_id).await;
        let state = self
            .persist_game_room_manifest(&source_replica_id, topic_id, manifest.clone(), now)
            .await?;
        self.services
            .projection_store
            .upsert_game_room_cache(game_projection_row(&state))
            .await?;
        drop(projection_guard);
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(topic_id, channel_id.as_ref()),
                GossipHint::SessionChanged {
                    topic_id: TopicId::new(topic_id),
                    session_id: room_id.clone(),
                    object_kind: "game-session".into(),
                },
            )
            .await?;
        *self.last_sync_ts.lock().await = Some(now);
        Ok(room_id)
    }

    pub async fn create_metaverse_room(
        &self,
        topic_id: &str,
        input: CreateMetaverseRoomInput,
    ) -> Result<String> {
        self.create_metaverse_room_in_channel(topic_id, ChannelRef::Public, input)
            .await
    }

    pub async fn create_metaverse_room_in_channel(
        &self,
        topic_id: &str,
        channel_ref: ChannelRef,
        input: CreateMetaverseRoomInput,
    ) -> Result<String> {
        let _guard = self.services.dome_mutations.lock().await;
        let private_state = match channel_ref {
            ChannelRef::Public => None,
            ChannelRef::PrivateChannel { channel_id } => Some(
                self.private_channel_write_state(topic_id, &channel_id)
                    .await?,
            ),
        };
        let channel_id = private_state.as_ref().map(|state| state.channel_id.clone());
        let spatial_context = match &channel_id {
            Some(channel_id) => SpatialContextV1::Channel {
                topic_id: TopicId::new(topic_id),
                channel_id: channel_id.clone(),
            },
            None => SpatialContextV1::Topic {
                topic_id: TopicId::new(topic_id),
            },
        };
        let source_replica_id = private_state
            .as_ref()
            .map(current_private_channel_replica_id)
            .unwrap_or_else(|| topic_replica_id(topic_id));
        let now = Utc::now().timestamp_millis();
        let title = input.title.trim();
        if title.is_empty() {
            anyhow::bail!("metaverse room title is required");
        }
        let owner_pubkey = Pubkey::from(self.current_author_pubkey());
        let instance_hash = kukuri_core::blob_hash(format!(
            "dome-instance:{}:{}",
            spatial_context.canonical_id(),
            owner_pubkey.as_str()
        ));
        let room_id = format!("dome-{}", &instance_hash.as_str()[..24]);
        let instance_generation = match self
            .fetch_dome_instance_manifest(&source_replica_id, &owner_pubkey)
            .await?
        {
            Some((_, existing)) if existing.status != DomeInstanceStatusV1::Tombstoned => {
                anyhow::bail!("the owner already has a Dome in this Spatial Context");
            }
            Some((_, existing)) => {
                self.ensure_dome_deletion_finished(&spatial_context, &room_id, existing.generation)
                    .await?;
                existing.generation.saturating_add(1)
            }
            None => 1,
        };
        let preset_id = format!(
            "dome-preset-{}-{now}",
            short_id_suffix(owner_pubkey.as_str())
        );
        let dome = MetaverseDomeV1::default();
        let preset_ref = self
            .persist_dome_preset_manifest(DomePresetManifestV1 {
                preset_id,
                owner_pubkey: owner_pubkey.clone(),
                revision: 1,
                dome: dome.clone(),
                asset_refs: Vec::new(),
                updated_at: now,
            })
            .await?;
        let metaverse = MetaverseRoomStateV1 {
            world_version: METAVERSE_WORLD_VERSION,
            instance_id: room_id.clone(),
            spatial_context,
            instance_generation,
            instance_status: DomeInstanceStatusV1::Active,
            relationship_detach: None,
            replacement_instance_id: None,
            preset_ref,
            session_id: room_id.clone(),
            max_peers: input.max_peers,
            dome,
            default_spawn: MetaverseRoomSpawnV1 {
                position: [0, 0, 260],
                rotation: [0, 180, 0],
            },
            asset_refs: Vec::new(),
            chat_history: Vec::new(),
        };
        validate_metaverse_room_state(&metaverse)?;
        let manifest = GameRoomManifestBlobV1 {
            room_id: room_id.clone(),
            score_revision: None,
            topic_id: TopicId::new(topic_id),
            channel_id: channel_id.clone(),
            owner_pubkey,
            title: title.to_string(),
            description: input.description.trim().to_string(),
            status: GameRoomStatus::Waiting,
            phase_label: Some("fixed-dome-v1".to_string()),
            participants: Vec::new(),
            scores: Vec::new(),
            room_kind: GameRoomKind::MetaverseRoom,
            metaverse: Some(metaverse),
            updated_at: now,
        };
        let instance_manifest = dome_instance_manifest_from_game_manifest(&manifest)?;
        self.persist_dome_instance_manifest(&source_replica_id, &instance_manifest, now)
            .await?;
        let state = self
            .persist_game_room_manifest(&source_replica_id, topic_id, manifest.clone(), now)
            .await?;
        self.services
            .projection_store
            .upsert_game_room_cache(game_projection_row(&state))
            .await?;
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(topic_id, channel_id.as_ref()),
                GossipHint::SessionChanged {
                    topic_id: TopicId::new(topic_id),
                    session_id: room_id.clone(),
                    object_kind: "game-session".into(),
                },
            )
            .await?;
        *self.last_sync_ts.lock().await = Some(now);
        Ok(room_id)
    }

    pub async fn update_game_room(
        &self,
        topic_id: &str,
        room_id: &str,
        input: UpdateGameRoomInput,
    ) -> Result<()> {
        let projection_guard = self.services.game_room_projections.lock(room_id).await;
        let (source_replica_id, state, mut manifest) = self
            .fetch_game_room_state_and_manifest(topic_id, room_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("game room not found"))?;
        let owner = self.current_author_pubkey();
        if state.owner_pubkey.as_str() != owner {
            anyhow::bail!("only the game room owner can update the room");
        }
        validate_game_room_transition(&manifest.status, &input.status)?;
        validate_game_room_scores(&manifest, &input.scores)?;
        manifest.status = input.status;
        manifest.phase_label = input
            .phase_label
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        manifest.scores = input
            .scores
            .into_iter()
            .map(|score| GameScoreEntry {
                participant_id: score.participant_id,
                label: score.label,
                score: score.score,
            })
            .collect();
        manifest.score_revision = Some(
            manifest
                .score_revision
                .context("ScoreGame revision is missing")?
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("ScoreGame revision overflow"))?,
        );
        manifest.updated_at = Utc::now().timestamp_millis();
        let state = self
            .persist_game_room_manifest(
                &source_replica_id,
                topic_id,
                manifest.clone(),
                state.created_at,
            )
            .await?;
        self.services
            .projection_store
            .upsert_game_room_cache(game_projection_row(&state))
            .await?;
        drop(projection_guard);
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(topic_id, state.state().channel_id.as_ref()),
                GossipHint::SessionChanged {
                    topic_id: TopicId::new(topic_id),
                    session_id: room_id.to_string(),
                    object_kind: "game-session".into(),
                },
            )
            .await?;
        *self.last_sync_ts.lock().await = Some(manifest.updated_at);
        Ok(())
    }

    pub async fn update_metaverse_room(
        &self,
        topic_id: &str,
        room_id: &str,
        input: UpdateMetaverseRoomInput,
    ) -> Result<()> {
        let _guard = self.services.dome_mutations.lock().await;
        self.update_metaverse_room_unlocked(topic_id, room_id, input)
            .await
    }

    pub(crate) async fn update_metaverse_room_unlocked(
        &self,
        topic_id: &str,
        room_id: &str,
        input: UpdateMetaverseRoomInput,
    ) -> Result<()> {
        let (source_replica_id, state, mut manifest) = self
            .fetch_game_room_state_and_manifest(topic_id, room_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("metaverse room not found"))?;
        if manifest.room_kind != GameRoomKind::MetaverseRoom {
            anyhow::bail!("game room is not a metaverse room");
        }
        let actor = self.current_author_pubkey();
        if state.owner_pubkey.as_str() != actor {
            anyhow::bail!("only the metaverse room owner can update Dome customization");
        }
        validate_game_room_transition(&manifest.status, &input.status)?;
        if let Some(m) = manifest.metaverse.as_ref() {
            self.ensure_dome_not_deleting(
                &m.spatial_context,
                &m.instance_id,
                m.instance_generation,
            )
            .await?;
        }
        validate_dome_customization(&input.customization)?;
        let now = Utc::now().timestamp_millis();
        let current_metaverse = manifest
            .metaverse
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("metaverse room state is missing"))?;
        if current_metaverse.instance_status != DomeInstanceStatusV1::Active
            || current_metaverse.relationship_detach.is_some()
        {
            anyhow::bail!("only an active attached Dome instance can be customized");
        }
        if current_metaverse.dome.customization == input.customization
            && manifest.status == input.status
        {
            return Ok(());
        }
        let preset_ref = self
            .persist_dome_preset_manifest(DomePresetManifestV1 {
                preset_id: current_metaverse.preset_ref.preset_id.clone(),
                owner_pubkey: Pubkey::from(actor),
                revision: current_metaverse.preset_ref.revision.saturating_add(1),
                dome: MetaverseDomeV1 {
                    spec_id: current_metaverse.dome.spec_id.clone(),
                    customization: input.customization.clone(),
                },
                asset_refs: current_metaverse.asset_refs.clone(),
                updated_at: now,
            })
            .await?;
        let Some(metaverse) = manifest.metaverse.as_mut() else {
            anyhow::bail!("metaverse room state is missing");
        };
        metaverse.dome.customization = input.customization;
        metaverse.preset_ref = preset_ref;
        validate_metaverse_room_state(metaverse)?;
        manifest.status = input.status;
        manifest.updated_at = now;
        let instance_manifest = dome_instance_manifest_from_game_manifest(&manifest)?;
        self.persist_dome_instance_manifest(
            &source_replica_id,
            &instance_manifest,
            state.created_at,
        )
        .await?;
        let state = self
            .persist_game_room_manifest(
                &source_replica_id,
                topic_id,
                manifest.clone(),
                state.created_at,
            )
            .await?;
        self.services
            .projection_store
            .upsert_game_room_cache(game_projection_row(&state))
            .await?;
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(topic_id, state.state().channel_id.as_ref()),
                GossipHint::SessionChanged {
                    topic_id: TopicId::new(topic_id),
                    session_id: room_id.to_string(),
                    object_kind: "game-session".into(),
                },
            )
            .await?;
        *self.last_sync_ts.lock().await = Some(manifest.updated_at);
        Ok(())
    }

    pub async fn publish_metaverse_room_event(
        &self,
        topic_id: &str,
        input: PublishMetaverseRoomEventInput,
    ) -> Result<MetaverseRoomEventView> {
        let (source_replica_id, state, mut manifest) = self
            .fetch_game_room_state_and_manifest(topic_id, input.room_id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("metaverse room not found"))?;
        if manifest.room_kind != GameRoomKind::MetaverseRoom {
            anyhow::bail!("game room is not a metaverse room");
        }
        let metaverse_state = manifest
            .metaverse
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("metaverse room state is missing"))?;
        if !matches!(
            self.evaluate_dome_room_access(
                &metaverse_state.spatial_context,
                &state.owner_pubkey,
                &self.services.keys.public_key(),
            )
            .await?,
            kukuri_core::DomeTransitionAccessDecisionV1::Allowed
        ) {
            anyhow::bail!(kukuri_core::DomeTransitionDenialReasonV1::AccessDenied.code());
        }
        if metaverse_state.instance_status != DomeInstanceStatusV1::Active {
            anyhow::bail!("cannot publish events to a non-active Dome instance");
        }
        if manifest.status == GameRoomStatus::Ended {
            anyhow::bail!("cannot publish events to an ended metaverse room");
        }
        validate_metaverse_room_event_identity(
            input.room_id.as_str(),
            input.peer_id.as_str(),
            &input.event,
        )?;
        let now = Utc::now().timestamp_millis();
        let event_id = format!(
            "mre-{}-{}-{}",
            now,
            input.seq,
            short_id_suffix(self.current_author_pubkey().as_str())
        );
        let content = MetaverseRoomEventEnvelopeContentV1 {
            event_id,
            topic_id: TopicId::new(topic_id),
            channel_id: state.channel_id.clone(),
            room_id: input.room_id,
            spatial_context: manifest
                .metaverse
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("metaverse room state is missing"))?
                .spatial_context
                .clone(),
            instance_generation: manifest
                .metaverse
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("metaverse room state is missing"))?
                .instance_generation,
            session_id: manifest
                .metaverse
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("metaverse room state is missing"))?
                .session_id
                .clone(),
            peer_id: input.peer_id,
            seq: input.seq,
            sent_at: now,
            event: input.event,
        };
        self.preflight_spatial_audio_event(&content, now).await?;
        let instance_manifest = dome_instance_manifest_from_game_manifest(&manifest)?;
        validate_metaverse_room_event_for_instance(&content, &instance_manifest)?;
        let envelope = build_metaverse_room_event_envelope(
            self.services.keys.as_ref(),
            &TopicId::new(topic_id),
            content.room_id.as_str(),
            &content,
        )?;
        let view = parse_metaverse_room_event_envelope(envelope.clone(), now, "local".to_string())?
            .ok_or_else(|| anyhow::anyhow!("failed to build metaverse room event"))?;
        push_metaverse_room_event_buffer(&self.metaverse_room_events, view.clone()).await;
        if let MetaverseRoomEventV1::ChatMessage { message } = &content.event {
            let Some(metaverse) = manifest.metaverse.as_mut() else {
                anyhow::bail!("metaverse room state is missing");
            };
            if !metaverse
                .chat_history
                .iter()
                .any(|existing| existing.message_id == message.message_id)
            {
                metaverse.chat_history.push(message.clone());
                if metaverse.chat_history.len() > METAVERSE_CHAT_HISTORY_LIMIT {
                    let overflow = metaverse
                        .chat_history
                        .len()
                        .saturating_sub(METAVERSE_CHAT_HISTORY_LIMIT);
                    metaverse.chat_history.drain(0..overflow);
                }
                manifest.updated_at = now;
                let persisted = self
                    .persist_game_room_manifest(
                        &source_replica_id,
                        topic_id,
                        manifest.clone(),
                        state.created_at,
                    )
                    .await?;
                self.services
                    .projection_store
                    .upsert_game_room_cache(game_projection_row(&persisted))
                    .await?;
            }
        }
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(topic_id, state.channel_id.as_ref()),
                GossipHint::MetaverseRoomEvent {
                    topic_id: TopicId::new(topic_id),
                    room_id: content.room_id,
                    event: Box::new(envelope),
                },
            )
            .await?;
        *self.last_sync_ts.lock().await = Some(now);
        Ok(view)
    }

    pub async fn import_metaverse_room_asset(
        &self,
        topic_id: &str,
        input: ImportMetaverseRoomAssetInput,
    ) -> Result<MetaverseAssetRefView> {
        let _guard = self.services.dome_mutations.lock().await;
        let (source_replica_id, state, mut manifest) = self
            .fetch_game_room_state_and_manifest(topic_id, input.room_id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("metaverse room not found"))?;
        if manifest.room_kind != GameRoomKind::MetaverseRoom {
            anyhow::bail!("game room is not a metaverse room");
        }
        let actor = self.current_author_pubkey();
        if state.owner_pubkey.as_str() != actor {
            anyhow::bail!("only the Dome owner can import assets into its Preset");
        }
        if input.bytes.is_empty() {
            anyhow::bail!("metaverse asset bytes are required");
        }
        let budget_metadata =
            kukuri_core::inspect_metaverse_asset(input.kind.clone(), &input.bytes)?;
        if budget_metadata.stored_bytes
            > self.metaverse_resource_budget.host.max_session_asset_bytes
        {
            return Err(kukuri_core::MetaverseResourceRejection::new(
                kukuri_core::MetaverseBudgetScope::Host,
                kukuri_core::MetaverseBudgetResource::SessionAssetBytes,
                kukuri_core::MetaverseResourceRejectionReason::LimitExceeded,
                budget_metadata.stored_bytes,
                self.metaverse_resource_budget.host.max_session_asset_bytes,
            )
            .into());
        }
        if input.kind == kukuri_core::MetaverseAssetKind::Vrm
            && budget_metadata.stored_bytes
                > self.metaverse_resource_budget.player.max_avatar_asset_bytes
        {
            return Err(kukuri_core::MetaverseResourceRejection::new(
                kukuri_core::MetaverseBudgetScope::Player,
                kukuri_core::MetaverseBudgetResource::AvatarAssetBytes,
                kukuri_core::MetaverseResourceRejectionReason::LimitExceeded,
                budget_metadata.stored_bytes,
                self.metaverse_resource_budget.player.max_avatar_asset_bytes,
            )
            .into());
        }
        let now = Utc::now().timestamp_millis();
        let current = manifest
            .metaverse
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("metaverse room state is missing"))?;
        if current.instance_status != DomeInstanceStatusV1::Active
            || current.relationship_detach.is_some()
        {
            anyhow::bail!("only an active attached Dome instance can import Preset assets");
        }
        self.ensure_dome_not_deleting(
            &current.spatial_context,
            &current.instance_id,
            current.instance_generation,
        )
        .await?;
        let prospective_hash = blake3::hash(&input.bytes).to_hex().to_string();
        if let Some(existing) = current
            .asset_refs
            .iter()
            .find(|existing| existing.blob_hash == prospective_hash)
        {
            return Ok(existing.clone());
        }
        let mut asset_refs = current.asset_refs.clone();
        asset_refs.push(MetaverseAssetRef {
            kind: input.kind.clone(),
            blob_hash: prospective_hash.clone(),
            mime_type: Some(input.mime_type.clone()),
            size_bytes: Some(budget_metadata.stored_bytes),
            name: input.name.clone(),
            budget_metadata: Some(budget_metadata.clone()),
        });
        kukuri_core::validate_dome_asset_budget(&asset_refs, &self.metaverse_resource_budget)?;
        let stored = self
            .services
            .blob_service
            .put_blob(input.bytes, input.mime_type.as_str())
            .await?;
        if stored.hash.as_str() != prospective_hash {
            anyhow::bail!("metaverse blob service returned an unexpected content hash");
        }
        let asset = MetaverseAssetRef {
            kind: input.kind,
            blob_hash: stored.hash.as_str().to_string(),
            mime_type: Some(stored.mime),
            size_bytes: Some(stored.bytes),
            name: input.name,
            budget_metadata: Some(budget_metadata),
        };
        *asset_refs
            .last_mut()
            .expect("prospective asset was appended") = asset.clone();
        let preset_ref = self
            .persist_dome_preset_manifest(DomePresetManifestV1 {
                preset_id: current.preset_ref.preset_id.clone(),
                owner_pubkey: Pubkey::from(actor),
                revision: current.preset_ref.revision.saturating_add(1),
                dome: current.dome.clone(),
                asset_refs: asset_refs.clone(),
                updated_at: now,
            })
            .await?;
        let metaverse = manifest
            .metaverse
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("metaverse room state is missing"))?;
        metaverse.preset_ref = preset_ref;
        metaverse.asset_refs = asset_refs;
        manifest.updated_at = now;
        let instance_manifest = dome_instance_manifest_from_game_manifest(&manifest)?;
        self.persist_dome_instance_manifest(
            &source_replica_id,
            &instance_manifest,
            state.created_at,
        )
        .await?;
        let persisted = self
            .persist_game_room_manifest(
                &source_replica_id,
                topic_id,
                manifest.clone(),
                state.created_at,
            )
            .await?;
        self.services
            .projection_store
            .upsert_game_room_cache(game_projection_row(&persisted))
            .await?;
        Ok(asset)
    }

    pub async fn list_metaverse_room_events(
        &self,
        topic_id: &str,
        room_id: &str,
        after_envelope_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<MetaverseRoomEventView>> {
        let room_state = self
            .fetch_game_room_state_and_manifest(topic_id, room_id)
            .await?;
        let instance_manifest = if let Some((_, state, manifest)) = room_state {
            let Some(instance) = manifest.metaverse.as_ref() else {
                return Ok(Vec::new());
            };
            if instance.instance_status != DomeInstanceStatusV1::Active
                || manifest.status == GameRoomStatus::Ended
            {
                return Ok(Vec::new());
            }
            if !matches!(
                self.evaluate_dome_room_access(
                    &instance.spatial_context,
                    &state.owner_pubkey,
                    &self.services.keys.public_key(),
                )
                .await?,
                kukuri_core::DomeTransitionAccessDecisionV1::Allowed
            ) {
                return Ok(Vec::new());
            }
            Some(dome_instance_manifest_from_game_manifest(&manifest)?)
        } else {
            None
        };
        let key = metaverse_room_event_buffer_key(topic_id, room_id);
        let guard = self.metaverse_room_events.lock().await;
        let Some(queue) = guard.get(key.as_str()) else {
            return Ok(Vec::new());
        };
        let mut include = after_envelope_id.is_none();
        let mut items = Vec::new();
        for event in queue {
            if !include {
                include = after_envelope_id == Some(event.envelope_id.as_str());
                continue;
            }
            if metaverse_room_event_is_live(&event.content, Utc::now().timestamp_millis())
                && instance_manifest.as_ref().map_or_else(
                    || validate_metaverse_room_event_content(&event.content).is_ok(),
                    |instance| {
                        validate_metaverse_room_event_for_instance(&event.content, instance).is_ok()
                    },
                )
            {
                items.push(event.clone());
            }
        }
        if let Some(limit) = limit
            && items.len() > limit
        {
            items = items.split_off(items.len() - limit);
        }
        Ok(items)
    }
}

pub(crate) fn dome_instance_manifest_from_game_manifest(
    manifest: &GameRoomManifestBlobV1,
) -> Result<DomeInstanceManifestV1> {
    let metaverse = manifest
        .metaverse
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("metaverse room state is missing"))?;
    let instance = DomeInstanceManifestV1 {
        instance_id: metaverse.instance_id.clone(),
        spatial_context: metaverse.spatial_context.clone(),
        owner_pubkey: manifest.owner_pubkey.clone(),
        preset_ref: metaverse.preset_ref.clone(),
        title: manifest.title.clone(),
        description: manifest.description.clone(),
        max_peers: metaverse.max_peers,
        default_spawn: metaverse.default_spawn.clone(),
        generation: metaverse.instance_generation,
        status: metaverse.instance_status,
        relationship_detach: metaverse.relationship_detach.clone(),
        replacement_instance_id: metaverse.replacement_instance_id.clone(),
        chat_history: metaverse.chat_history.clone(),
        updated_at: manifest.updated_at,
    };
    kukuri_core::validate_dome_instance_manifest(&instance)?;
    Ok(instance)
}

fn validate_metaverse_room_event_identity(
    room_id: &str,
    peer_id: &str,
    event: &MetaverseRoomEventV1,
) -> Result<()> {
    match event {
        MetaverseRoomEventV1::PresenceJoin { presence } => {
            if presence.room_id != room_id || presence.peer_id != peer_id {
                anyhow::bail!("metaverse presence event identity does not match request");
            }
        }
        MetaverseRoomEventV1::PresenceLeave {
            room_id: event_room_id,
            peer_id: event_peer_id,
            ..
        } => {
            if event_room_id != room_id || event_peer_id != peer_id {
                anyhow::bail!("metaverse presence event identity does not match request");
            }
        }
        MetaverseRoomEventV1::ChatMessage { message } => {
            if message.room_id != room_id || message.author_peer_id != peer_id {
                anyhow::bail!("metaverse chat event identity does not match request");
            }
        }
        MetaverseRoomEventV1::SpatialAudioFrame { frame } => {
            if frame.room_id != room_id || frame.peer_id != peer_id {
                anyhow::bail!("metaverse audio event identity does not match request");
            }
        }
    }
    Ok(())
}
