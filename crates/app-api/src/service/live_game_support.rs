use super::remote_read_support::{read_local_then_remote, writer_readers};
use super::*;
use kukuri_core::SpatialContextV1;

#[derive(Debug)]
pub(crate) enum DomeReadUnavailable {
    Instance,
    Preset,
    Envelope,
}

impl std::fmt::Display for DomeReadUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Instance => "Dome instance manifest is unavailable",
            Self::Preset => "Dome preset manifest is unavailable",
            Self::Envelope => "signed Dome envelope is unavailable",
        })
    }
}

impl std::error::Error for DomeReadUnavailable {}

async fn load_signed_dome_instance_manifest(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope_id: &EnvelopeId,
    owner_pubkey: &Pubkey,
) -> Result<Option<DomeInstanceManifestV1>> {
    let key = stable_key("envelopes", envelope_id.as_str());
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            key.as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?;
    for record in records {
        let envelope = match serde_json::from_slice::<KukuriEnvelope>(&record.value) {
            Ok(envelope) => envelope,
            Err(error) => {
                warn!(replica = %replica.as_str(), key, %error, "ignored an unreadable Dome Instance envelope");
                continue;
            }
        };
        if envelope.verify().is_err()
            || envelope.id != *envelope_id
            || envelope.kind != "dome-instance"
            || envelope.pubkey != *owner_pubkey
        {
            warn!(replica = %replica.as_str(), key, "ignored an invalid Dome Instance envelope");
            continue;
        }
        match serde_json::from_str::<DomeInstanceManifestV1>(&envelope.content) {
            Ok(manifest) => return Ok(Some(manifest)),
            Err(error) => {
                warn!(replica = %replica.as_str(), key, %error, "ignored an unreadable signed Dome Instance manifest");
            }
        }
    }
    Ok(None)
}

impl AppService {
    pub async fn fetch_metaverse_blob_bytes(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        self.services
            .blob_service
            .fetch_blob(&kukuri_core::BlobHash::new(hash))
            .await
    }

    pub async fn collect_metaverse_blob_garbage(&self, now_millis: i64) -> Result<Vec<String>> {
        let hashes = self
            .metaverse_blob_cache
            .lock()
            .await
            .collect_garbage(now_millis);
        for hash in &hashes {
            self.services.blob_service.unpin_blob(hash).await?;
        }
        Ok(hashes
            .into_iter()
            .map(|hash| hash.as_str().to_string())
            .collect())
    }

    pub(crate) async fn persist_dome_preset_manifest(
        &self,
        manifest: DomePresetManifestV1,
    ) -> Result<DomePresetRefV1> {
        let envelope = build_dome_preset_envelope(self.services.keys.as_ref(), &manifest)?;
        let stored = store_manifest_blob(
            self.services.blob_service.as_ref(),
            &manifest,
            DOME_PRESET_MANIFEST_MIME,
        )
        .await?;
        let current_reference = MetaverseBlobPin {
            reason: MetaverseBlobPinReason::Current,
            reference_id: format!("{}:{}", manifest.preset_id, manifest.revision),
        };
        let mut required = vec![(stored.hash.clone(), stored.bytes)];
        required.extend(manifest.asset_refs.iter().map(|asset| {
            (
                kukuri_core::BlobHash::new(asset.blob_hash.clone()),
                asset.size_bytes.unwrap_or(0),
            )
        }));
        {
            let mut cache = self.metaverse_blob_cache.lock().await;
            cache.ensure_staging_capacity(&required)?;
            if manifest.revision > 1 {
                let previous = MetaverseBlobPin {
                    reason: MetaverseBlobPinReason::Current,
                    reference_id: format!("{}:{}", manifest.preset_id, manifest.revision - 1),
                };
                cache.replace_reference(
                    &previous,
                    MetaverseBlobPin {
                        reason: MetaverseBlobPinReason::Rollback,
                        reference_id: previous.reference_id.clone(),
                    },
                    manifest.updated_at,
                );
            }
            if manifest.revision > METAVERSE_ROLLBACK_REVISION_LIMIT as u64 + 1 {
                cache.unpin_reference(
                    &MetaverseBlobPin {
                        reason: MetaverseBlobPinReason::Rollback,
                        reference_id: format!(
                            "{}:{}",
                            manifest.preset_id,
                            manifest.revision - METAVERSE_ROLLBACK_REVISION_LIMIT as u64 - 1
                        ),
                    },
                    manifest.updated_at,
                );
            }
            for (hash, bytes) in &required {
                cache.pin(hash, *bytes, current_reference.clone(), manifest.updated_at);
            }
        }
        for (hash, _) in &required {
            self.services.blob_service.pin_blob(hash).await?;
        }
        let state = DomePresetStateDocV1 {
            preset_id: manifest.preset_id.clone(),
            owner_pubkey: manifest.owner_pubkey.clone(),
            revision: manifest.revision,
            current_manifest: ManifestBlobRef {
                hash: stored.hash.clone(),
                mime: stored.mime.clone(),
                bytes: stored.bytes,
            },
            updated_at: manifest.updated_at,
            last_envelope_id: envelope.id.clone(),
        };
        let replica = author_replica_id(manifest.owner_pubkey.as_str());
        self.services.docs_sync.open_replica(&replica).await?;
        self.services
            .docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key("envelopes", envelope.id.as_str()),
                    value: serde_json::to_value(&envelope)?,
                },
            )
            .await?;
        self.services
            .docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key(
                        "metaverse/dome-presets",
                        &format!("{}/revisions/{:020}", manifest.preset_id, manifest.revision),
                    ),
                    value: serde_json::to_value(&state)?,
                },
            )
            .await?;
        self.services
            .docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key(
                        "metaverse/dome-presets",
                        &format!("{}/state", manifest.preset_id),
                    ),
                    value: serde_json::to_value(&state)?,
                },
            )
            .await?;
        self.collect_metaverse_blob_garbage(manifest.updated_at)
            .await?;
        Ok(DomePresetRefV1 {
            preset_id: manifest.preset_id,
            owner_pubkey: manifest.owner_pubkey,
            revision: manifest.revision,
            manifest_blob_hash: stored.hash.as_str().to_string(),
            manifest_mime: stored.mime,
            manifest_bytes: stored.bytes,
        })
    }

    /// author replica の対象を、手元の次に author 本人を含む有界な provider から読む(#1221 R5-C)。
    /// remote の読取りは namespace を import・sync しない。自分の replica は手元だけを読む。
    pub(crate) async fn read_author_object<T, Fut>(
        &self,
        author_pubkey: &str,
        read: impl Fn(Arc<dyn DocsSync>, DocFetchPolicy) -> Fut,
    ) -> Result<Option<T>>
    where
        Fut: Future<Output = Result<Option<T>>>,
    {
        let replica = author_replica_id(author_pubkey);
        let own = author_pubkey == self.current_author_pubkey();
        let readers = async {
            if own {
                return Vec::new();
            }
            writer_readers(&self.services, &replica, &[author_pubkey], None).await
        };
        read_local_then_remote(self.services.docs_sync.clone(), readers, read).await
    }

    pub(crate) async fn fetch_dome_preset_manifest(
        &self,
        preset_ref: &DomePresetRefV1,
    ) -> Result<Option<DomePresetManifestV1>> {
        self.read_author_object(
            preset_ref.owner_pubkey.as_str(),
            |docs, policy| async move {
                self.read_dome_preset_manifest(docs.as_ref(), policy, preset_ref)
                    .await
            },
        )
        .await
    }

    async fn read_dome_preset_manifest(
        &self,
        docs: &dyn DocsSync,
        policy: DocFetchPolicy,
        preset_ref: &DomePresetRefV1,
    ) -> Result<Option<DomePresetManifestV1>> {
        let replica = author_replica_id(preset_ref.owner_pubkey.as_str());
        let state_records = docs
            .query_replica_with_policy(
                &replica,
                DocQuery::Exact(stable_key(
                    "metaverse/dome-presets",
                    &format!(
                        "{}/revisions/{:020}",
                        preset_ref.preset_id, preset_ref.revision
                    ),
                )),
                policy,
            )
            .await?;
        let Some(state_record) = state_records.into_iter().next() else {
            return Ok(None);
        };
        let state: DomePresetStateDocV1 = serde_json::from_slice(&state_record.value)?;
        if state.preset_id != preset_ref.preset_id
            || state.owner_pubkey != preset_ref.owner_pubkey
            || state.revision != preset_ref.revision
            || state.current_manifest.hash.as_str() != preset_ref.manifest_blob_hash
            || state.current_manifest.mime != preset_ref.manifest_mime
            || state.current_manifest.bytes != preset_ref.manifest_bytes
        {
            anyhow::bail!("Dome Preset state does not match its reference");
        }
        let manifest = fetch_manifest_blob::<DomePresetManifestV1>(
            self.services.blob_service.as_ref(),
            &ManifestBlobRef {
                hash: kukuri_core::BlobHash::new(preset_ref.manifest_blob_hash.clone()),
                mime: preset_ref.manifest_mime.clone(),
                bytes: preset_ref.manifest_bytes,
            },
        )
        .await?;
        let Some(manifest) = manifest else {
            return Ok(None);
        };
        validate_dome_preset_manifest(&manifest)?;
        if manifest.preset_id != preset_ref.preset_id
            || manifest.owner_pubkey != preset_ref.owner_pubkey
            || manifest.revision != preset_ref.revision
        {
            anyhow::bail!("Dome Preset reference does not match its manifest");
        }
        let signed: DomePresetManifestV1 = match fetch_verified_dome_envelope(
            docs,
            &replica,
            &state.last_envelope_id,
            "dome-preset",
            &manifest.owner_pubkey,
            policy,
        )
        .await
        {
            Ok(signed) => signed,
            Err(error)
                if matches!(
                    error.downcast_ref::<DomeReadUnavailable>(),
                    Some(DomeReadUnavailable::Envelope)
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if signed != manifest {
            anyhow::bail!("signed Dome Preset content does not match its manifest blob");
        }
        Ok(Some(manifest))
    }

    pub(crate) async fn persist_dome_instance_manifest(
        &self,
        replica: &ReplicaId,
        manifest: &DomeInstanceManifestV1,
        created_at: i64,
    ) -> Result<DomeInstanceStateDocV1> {
        let envelope = build_dome_instance_envelope(self.services.keys.as_ref(), manifest)?;
        let stored = store_manifest_blob(
            self.services.blob_service.as_ref(),
            manifest,
            DOME_INSTANCE_MANIFEST_MIME,
        )
        .await?;
        let state = DomeInstanceStateDocV1 {
            instance_id: manifest.instance_id.clone(),
            spatial_context: manifest.spatial_context.clone(),
            owner_pubkey: manifest.owner_pubkey.clone(),
            generation: manifest.generation,
            status: manifest.status,
            created_at,
            updated_at: manifest.updated_at,
            current_manifest: ManifestBlobRef {
                hash: stored.hash.clone(),
                mime: stored.mime.clone(),
                bytes: stored.bytes,
            },
            last_envelope_id: envelope.id.clone(),
        };
        self.services.docs_sync.open_replica(replica).await?;
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key("envelopes", envelope.id.as_str()),
                    value: serde_json::to_value(&envelope)?,
                },
            )
            .await?;
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key(
                        "metaverse/dome-instances",
                        &format!("{}/state", manifest.owner_pubkey.as_str()),
                    ),
                    value: serde_json::to_value(&state)?,
                },
            )
            .await?;
        Ok(state)
    }

    pub(crate) async fn fetch_dome_instance_manifest(
        &self,
        replica: &ReplicaId,
        owner_pubkey: &Pubkey,
    ) -> Result<Option<(DomeInstanceStateDocV1, DomeInstanceManifestV1)>> {
        let key = stable_key(
            "metaverse/dome-instances",
            &format!("{}/state", owner_pubkey.as_str()),
        );
        let records = self
            .services
            .docs_sync
            .query_replica_exact_bounded(
                replica,
                key.as_str(),
                MAX_ENVELOPE_RECORDS_PER_OBJECT,
                DocFetchPolicy::LocalThenRemote,
            )
            .await?;
        let mut newest: Option<(DomeInstanceStateDocV1, DomeInstanceManifestV1)> = None;
        let mut unavailable = None;
        for record in records {
            let state = match serde_json::from_slice::<DomeInstanceStateDocV1>(&record.value) {
                Ok(state) => state,
                Err(error) => {
                    warn!(replica = %replica.as_str(), key, %error, "ignored an unreadable Dome Instance state");
                    continue;
                }
            };
            if state.owner_pubkey != *owner_pubkey {
                warn!(replica = %replica.as_str(), key, "ignored a Dome Instance state with a mismatched owner");
                continue;
            }
            let bytes = match tokio::time::timeout(
                projection_blob_fetch_timeout(),
                self.services
                    .blob_service
                    .fetch_blob(&state.current_manifest.hash),
            )
            .await
            {
                Ok(result) => result?,
                Err(error) => return Err(error.into()),
            };
            let Some(bytes) = bytes else {
                unavailable = Some(DomeReadUnavailable::Instance);
                continue;
            };
            let manifest = match serde_json::from_slice::<DomeInstanceManifestV1>(&bytes) {
                Ok(manifest) => manifest,
                Err(error) => {
                    warn!(replica = %replica.as_str(), key, %error, "ignored an unreadable Dome Instance manifest");
                    continue;
                }
            };
            if let Err(error) = kukuri_core::validate_dome_instance_manifest(&manifest) {
                warn!(replica = %replica.as_str(), key, %error, "ignored an invalid Dome Instance manifest");
                continue;
            }
            if state.instance_id != manifest.instance_id
                || state.owner_pubkey != manifest.owner_pubkey
                || state.spatial_context != manifest.spatial_context
                || state.generation != manifest.generation
                || state.status != manifest.status
            {
                warn!(replica = %replica.as_str(), key, "ignored a Dome Instance state that does not match its manifest");
                continue;
            }
            let Some(signed) = load_signed_dome_instance_manifest(
                self.services.docs_sync.as_ref(),
                replica,
                &state.last_envelope_id,
                &manifest.owner_pubkey,
            )
            .await?
            else {
                unavailable = Some(DomeReadUnavailable::Envelope);
                continue;
            };
            if signed != manifest {
                warn!(replica = %replica.as_str(), key, "ignored a Dome Instance without a matching owner signature");
                continue;
            }
            if newest.as_ref().is_none_or(|(_, current)| {
                (manifest.generation, manifest.updated_at)
                    > (current.generation, current.updated_at)
            }) {
                newest = Some((state, manifest));
            }
        }
        match newest {
            Some(value) => Ok(Some(value)),
            None => match unavailable {
                Some(reason) => Err(reason.into()),
                None => Ok(None),
            },
        }
    }

    pub(crate) async fn hosting_instance(
        &self,
        replica: &ReplicaId,
        spatial_context: &SpatialContextV1,
        instance_id: &str,
    ) -> Result<Option<DomeInstanceManifestV1>> {
        let local_owner = self.services.keys.public_key();
        if dome_instance_id(spatial_context, &local_owner) == instance_id {
            return self
                .hosting_instance_for_owner(replica, spatial_context, instance_id, &local_owner)
                .await;
        }
        let Some(room) = load_verified_game_room(
            self.services.docs_sync.as_ref(),
            self.services.blob_service.as_ref(),
            replica,
            spatial_context.topic_id().as_str(),
            instance_id,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?
        else {
            return Ok(None);
        };
        let Some(metaverse) = room.manifest().metaverse.as_ref() else {
            return Ok(None);
        };
        if metaverse.spatial_context != *spatial_context || metaverse.instance_id != instance_id {
            return Ok(None);
        }
        self.hosting_instance_for_owner(
            replica,
            spatial_context,
            instance_id,
            &room.manifest().owner_pubkey,
        )
        .await
    }

    pub(crate) async fn hosting_instance_for_owner(
        &self,
        replica: &ReplicaId,
        spatial_context: &SpatialContextV1,
        instance_id: &str,
        owner_pubkey: &Pubkey,
    ) -> Result<Option<DomeInstanceManifestV1>> {
        if dome_instance_id(spatial_context, owner_pubkey) != instance_id {
            return Ok(None);
        }
        let resolved = self
            .fetch_dome_instance_manifest(replica, owner_pubkey)
            .await?;
        let Some((_, manifest)) = resolved else {
            return Ok(None);
        };
        if manifest.instance_id != instance_id
            || manifest.spatial_context != *spatial_context
            || manifest.owner_pubkey != *owner_pubkey
        {
            warn!(
                replica = %replica.as_str(),
                instance_id,
                owner_pubkey = %owner_pubkey.as_str(),
                "ignored a Dome Instance that does not match its lookup identity"
            );
            return Ok(None);
        }
        Ok(Some(manifest))
    }

    pub(crate) async fn persist_dome_move_record(&self, record: &DomeMoveRecordV1) -> Result<()> {
        let envelope = build_dome_move_envelope(self.services.keys.as_ref(), record)?;
        let replica = author_replica_id(record.owner_pubkey.as_str());
        self.services.docs_sync.open_replica(&replica).await?;
        self.services
            .docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key("envelopes", envelope.id.as_str()),
                    value: serde_json::to_value(&envelope)?,
                },
            )
            .await?;
        self.services
            .docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key("metaverse/dome-moves", &format!("{}/state", record.move_id)),
                    value: serde_json::to_value(DomeMoveStateDocV1 {
                        record: record.clone(),
                        last_envelope_id: envelope.id,
                    })?,
                },
            )
            .await
    }

    pub(crate) async fn fetch_dome_move_record(
        &self,
        owner_pubkey: &str,
        move_id: &str,
    ) -> Result<Option<DomeMoveRecordV1>> {
        let replica = &author_replica_id(owner_pubkey);
        self.read_author_object(owner_pubkey, |docs, policy| async move {
            let docs = docs.as_ref();
            let records = docs
                .query_replica_with_policy(
                    replica,
                    DocQuery::Exact(stable_key(
                        "metaverse/dome-moves",
                        &format!("{move_id}/state"),
                    )),
                    policy,
                )
                .await?;
            let Some(state_record) = records.into_iter().next() else {
                return Ok(None);
            };
            let state: DomeMoveStateDocV1 = serde_json::from_slice(&state_record.value)?;
            let signed: DomeMoveRecordV1 = match fetch_verified_dome_envelope(
                docs,
                replica,
                &state.last_envelope_id,
                "dome-move",
                &state.record.owner_pubkey,
                policy,
            )
            .await
            {
                Ok(signed) => signed,
                Err(error)
                    if matches!(
                        error.downcast_ref::<DomeReadUnavailable>(),
                        Some(DomeReadUnavailable::Envelope)
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            if signed != state.record {
                anyhow::bail!("signed Dome move content does not match its state record");
            }
            Ok(Some(state.record))
        })
        .await
    }

    pub(crate) async fn stop_live_presence_task(
        &self,
        topic_id: &str,
        channel_id: &str,
        session_id: &str,
    ) {
        let key = live_presence_task_key(topic_id, channel_id, session_id);
        let handle = self
            .subscription_registry
            .live_presence_tasks
            .lock()
            .await
            .remove(key.as_str());
        if let Some(handle) = handle {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
    }

    pub(crate) async fn cleanup_ended_live_presence_tasks(
        &self,
        rows: &[LiveSessionProjectionRow],
    ) {
        for row in rows {
            if row.status == LiveSessionStatus::Ended {
                self.stop_live_presence_task(
                    row.topic_id.as_str(),
                    row.channel_id.as_str(),
                    row.session_id.as_str(),
                )
                .await;
            }
        }
    }

    pub(crate) async fn apply_live_presence(
        &self,
        topic_id: &str,
        channel_id: Option<&ChannelId>,
        session_id: &str,
        ttl_ms: u32,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        let author = self.current_author_pubkey();
        self.services
            .projection_store
            .upsert_live_presence(
                topic_id,
                channel_storage_id(channel_id).as_str(),
                session_id,
                author.as_str(),
                now + i64::from(ttl_ms),
                now,
            )
            .await?;
        self.services
            .projection_store
            .clear_expired_live_presence(now)
            .await?;
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(topic_id, channel_id),
                GossipHint::LivePresence {
                    topic_id: TopicId::new(topic_id),
                    session_id: session_id.to_string(),
                    author: Pubkey::from(author),
                    ttl_ms,
                },
            )
            .await?;
        Ok(())
    }

    /// manifest を blob と署名つき envelope(`envelopes/<envelope id>`)へ置き、state を書く(#1252)。
    ///
    /// envelope を state より先に書く。読む側は `state.last_envelope_id` から envelope を引いて、署名された manifest を確かめる。
    /// 戻り値は、docs から反映するときと同じ検証を通した session(docs は読まない)。
    pub(crate) async fn persist_live_session_manifest(
        &self,
        replica: &ReplicaId,
        topic_id: &str,
        manifest: LiveSessionManifestBlobV1,
        created_at: i64,
    ) -> Result<VerifiedLiveSession> {
        let now = Utc::now().timestamp_millis();
        let envelope = build_live_session_envelope(
            self.services.keys.as_ref(),
            &manifest.topic_id,
            manifest.session_id.as_str(),
            &manifest,
        )?;
        let stored = store_manifest_blob(
            self.services.blob_service.as_ref(),
            &manifest,
            LIVE_MANIFEST_MIME,
        )
        .await?;
        let state = LiveSessionStateDocV1 {
            session_id: manifest.session_id.clone(),
            topic_id: TopicId::new(topic_id),
            channel_id: manifest.channel_id.clone(),
            owner_pubkey: manifest.owner_pubkey.clone(),
            created_at,
            updated_at: now,
            status: manifest.status.clone(),
            current_manifest: ManifestBlobRef {
                hash: stored.hash.clone(),
                mime: stored.mime.clone(),
                bytes: stored.bytes,
            },
            last_envelope_id: envelope.id.clone(),
        };
        let verified =
            VerifiedLiveSession::verify(state, &envelope.pubkey, manifest, replica, topic_id)
                .map_err(|reason| {
                    anyhow::anyhow!("live session was rejected: {}", reason.as_str())
                })?;
        persist_session_envelope(self.services.docs_sync.as_ref(), replica, &envelope).await?;
        persist_live_session_state(self.services.docs_sync.as_ref(), replica, verified.state())
            .await?;
        Ok(verified)
    }

    /// `persist_live_session_manifest` と同じ形。metaverse room は訪問者も書くので、署名者が owner とは限らない。
    pub(crate) async fn persist_game_room_manifest(
        &self,
        replica: &ReplicaId,
        topic_id: &str,
        manifest: GameRoomManifestBlobV1,
        created_at: i64,
    ) -> Result<VerifiedGameRoom> {
        let now = Utc::now().timestamp_millis();
        let envelope = build_game_session_envelope(
            self.services.keys.as_ref(),
            &manifest.topic_id,
            manifest.room_id.as_str(),
            &manifest,
        )?;
        let stored = store_manifest_blob(
            self.services.blob_service.as_ref(),
            &manifest,
            GAME_MANIFEST_MIME,
        )
        .await?;
        let state = GameRoomStateDocV1 {
            room_id: manifest.room_id.clone(),
            topic_id: TopicId::new(topic_id),
            channel_id: manifest.channel_id.clone(),
            owner_pubkey: manifest.owner_pubkey.clone(),
            created_at,
            updated_at: now,
            status: manifest.status.clone(),
            current_manifest: ManifestBlobRef {
                hash: stored.hash.clone(),
                mime: stored.mime.clone(),
                bytes: stored.bytes,
            },
            last_envelope_id: envelope.id.clone(),
        };
        let verified =
            VerifiedGameRoom::verify(state, Some(&envelope.pubkey), manifest, replica, topic_id)
                .map_err(|reason| anyhow::anyhow!("game room was rejected: {}", reason.as_str()))?;
        persist_session_envelope(self.services.docs_sync.as_ref(), replica, &envelope).await?;
        persist_game_room_state(self.services.docs_sync.as_ref(), replica, verified.state())
            .await?;
        Ok(verified)
    }

    /// 操作(終了・参加・更新)が使う state と manifest。docs から反映するときと同じ検証を通す(#1252)。
    pub(crate) async fn fetch_live_session_state_and_manifest(
        &self,
        topic_id: &str,
        session_id: &str,
    ) -> Result<Option<(ReplicaId, LiveSessionStateDocV1, LiveSessionManifestBlobV1)>> {
        let projected = self
            .services
            .projection_store
            .get_live_session(topic_id, session_id)
            .await?;
        let source = projected.as_ref().map(|row| {
            (
                row.channel_id.as_str(),
                &row.source_replica_id,
                row.updated_at,
            )
        });
        let channel = source.map_or(PUBLIC_CHANNEL_ID, |(channel, _, _)| channel);
        let generation = self
            .services
            .active_content_scope_generation(topic_id, channel)
            .await;
        let readers = self
            .session_target_readers(topic_id, source, session_id)
            .await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        for (replica, docs, policy) in readers {
            let read = tokio::time::timeout_at(
                deadline,
                load_verified_live_session(
                    docs.as_ref(),
                    self.services.blob_service.as_ref(),
                    &replica,
                    topic_id,
                    session_id,
                    policy,
                ),
            );
            let result = if policy == DocFetchPolicy::LocalOnly {
                Some(read.await)
            } else if let Some(generation) = generation {
                self.services
                    .until_content_invalid(topic_id, channel, generation, read)
                    .await
            } else {
                None
            };
            let Some(result) = result else {
                return Ok(None);
            };
            if let Ok(Ok(Some(verified))) = result {
                if self
                    .services
                    .projection_store
                    .get_live_session(topic_id, session_id)
                    .await?
                    .is_some_and(|row| row.revision > verified.revision())
                {
                    continue;
                }
                return Ok(Some(verified.into_parts()));
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
        }
        Ok(None)
    }

    /// `fetch_live_session_state_and_manifest` と同じ形。
    pub(crate) async fn fetch_game_room_state_and_manifest(
        &self,
        topic_id: &str,
        room_id: &str,
    ) -> Result<Option<(ReplicaId, GameRoomStateDocV1, GameRoomManifestBlobV1)>> {
        let projected = self
            .services
            .projection_store
            .get_game_room(topic_id, room_id)
            .await?;
        let source = projected.as_ref().map(|row| {
            (
                row.channel_id.as_str(),
                &row.source_replica_id,
                row.updated_at,
            )
        });
        let channel = source.map_or(PUBLIC_CHANNEL_ID, |(channel, _, _)| channel);
        let generation = self
            .services
            .active_content_scope_generation(topic_id, channel)
            .await;
        let readers = self
            .session_target_readers(topic_id, source, room_id)
            .await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        for (replica, docs, policy) in readers {
            let read = tokio::time::timeout_at(
                deadline,
                load_verified_game_room(
                    docs.as_ref(),
                    self.services.blob_service.as_ref(),
                    &replica,
                    topic_id,
                    room_id,
                    policy,
                ),
            );
            let result = if policy == DocFetchPolicy::LocalOnly {
                Some(read.await)
            } else if let Some(generation) = generation {
                self.services
                    .until_content_invalid(topic_id, channel, generation, read)
                    .await
            } else {
                None
            };
            let Some(result) = result else {
                return Ok(None);
            };
            if let Ok(Ok(Some(verified))) = result {
                if let Some(revision) = verified.score_revision()
                    && self
                        .services
                        .projection_store
                        .get_game_room(topic_id, room_id)
                        .await?
                        .and_then(|row| row.score_revision)
                        .is_some_and(|projected| projected > revision)
                {
                    continue;
                }
                return Ok(Some(verified.into_parts()));
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
        }
        Ok(None)
    }
}

pub(crate) async fn fetch_verified_dome_envelope<T: DeserializeOwned>(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope_id: &EnvelopeId,
    expected_kind: &str,
    expected_owner: &Pubkey,
    policy: DocFetchPolicy,
) -> Result<T> {
    let records = docs_sync
        .query_replica_with_policy(
            replica,
            DocQuery::Exact(stable_key("envelopes", envelope_id.as_str())),
            policy,
        )
        .await?;
    let envelope: KukuriEnvelope = records
        .into_iter()
        .next()
        .map(|record| serde_json::from_slice(&record.value))
        .transpose()?
        .ok_or(DomeReadUnavailable::Envelope)?;
    envelope.verify()?;
    if envelope.id != *envelope_id
        || envelope.kind != expected_kind
        || envelope.pubkey != *expected_owner
    {
        anyhow::bail!("signed Dome envelope identity does not match state");
    }
    serde_json::from_str(envelope.content.as_str()).map_err(Into::into)
}
