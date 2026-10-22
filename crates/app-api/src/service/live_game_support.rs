use super::remote_read_support::{read_local_then_remote, writer_readers};
use super::*;

/// 読んで検証した他の author の Dome の preset を覚える数(#1818)。超えたら古いものから捨てる。
const DOME_PRESET_REUSE_LIMIT: usize = 32;

#[derive(Debug)]
pub(crate) enum DomeReadUnavailable {
    Preset,
    Envelope,
}

impl std::fmt::Display for DomeReadUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Preset => "Dome preset manifest is unavailable",
            Self::Envelope => "signed Dome envelope is unavailable",
        })
    }
}

impl std::error::Error for DomeReadUnavailable {}

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
        self.services
            .persist_author_event(manifest.owner_pubkey.as_str(), &envelope)
            .await?;
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
    /// `topic` があれば、author 本人の宛先の次に、その公開 topic の参加者(最大 4)を読み先に入れる。公開 topic の Dome の
    /// 記録を、owner がオフラインでも、検証して保持した参加者から読む(#1909。#1419 の profile と同じ形)。
    pub(crate) async fn read_author_object<T, Fut>(
        &self,
        author_pubkey: &str,
        topic: Option<&str>,
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
            let participants = match topic {
                Some(topic) => self
                    .hint_transport()
                    .topic_read_candidates(&TopicId::new(topic))
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            writer_readers(
                &self.services,
                &replica,
                &[author_pubkey],
                None,
                participants,
            )
            .await
        };
        read_local_then_remote(self.services.docs_sync.clone(), readers, read).await
    }

    /// 他の author の preset は、参照(版と manifest の hash を含む)の中身が変わらないので、一度読んで検証したものを読み
    /// 直さない(画面は Dome の一覧の読み直しのたびに hosting を読み、訪問者は owner の制御領域から読み直していた。
    /// #1818)。自分の preset は手元から読み、そのたびに検証する。`context` は Dome の Spatial Context で、公開 topic なら読み先にその
    /// 参加者を入れる。
    /// `keep` なら、remote から読んで検証した record と manifest の blob を保持し、他の参加者へ提供する(公開 topic の保持。
    /// #1909)。覚えた preset で済ませず、読み先から読む。
    pub(crate) async fn fetch_dome_preset_manifest(
        &self,
        preset_ref: &DomePresetRefV1,
        context: &kukuri_core::SpatialContextV1,
        keep: bool,
    ) -> Result<Option<DomePresetManifestV1>> {
        let reuse = preset_ref.owner_pubkey.as_str() != self.current_author_pubkey();
        if reuse
            && !keep
            && let Some((_, manifest)) = self
                .dome_presets
                .lock()
                .await
                .iter()
                .find(|(known, _)| known == preset_ref)
        {
            return Ok(Some(manifest.clone()));
        }
        let manifest = self
            .read_author_object(
                preset_ref.owner_pubkey.as_str(),
                public_dome_topic(context),
                |docs, policy| async move {
                    self.read_dome_preset_manifest(docs.as_ref(), policy, preset_ref, keep)
                        .await
                },
            )
            .await?;
        if reuse && let Some(manifest) = &manifest {
            let mut presets = self.dome_presets.lock().await;
            if !presets.iter().any(|(known, _)| known == preset_ref) {
                presets.push_back((preset_ref.clone(), manifest.clone()));
                if presets.len() > DOME_PRESET_REUSE_LIMIT {
                    presets.pop_front();
                }
            }
        }
        Ok(manifest)
    }

    async fn read_dome_preset_manifest(
        &self,
        docs: &dyn DocsSync,
        policy: DocFetchPolicy,
        preset_ref: &DomePresetRefV1,
        keep: bool,
    ) -> Result<Option<DomePresetManifestV1>> {
        let replica = author_replica_id(preset_ref.owner_pubkey.as_str());
        // 保持の読み(#1909)の手元の読みは、保持した record(他の名義)も読む。版ごとの key なので中身は変わらない。
        let held;
        let docs = if keep && policy == DocFetchPolicy::LocalOnly {
            held = LocalSourceReader {
                docs: self.services.docs_sync.clone(),
                replica: replica.clone(),
            };
            &held as &dyn DocsSync
        } else {
            docs
        };
        let state_key = stable_key(
            "metaverse/dome-presets",
            &format!(
                "{}/revisions/{:020}",
                preset_ref.preset_id, preset_ref.revision
            ),
        );
        let state_records = docs
            .query_replica_with_policy(&replica, DocQuery::Exact(state_key.clone()), policy)
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
        // preset を返した peer は、その manifest と資材を持つと分かっている(候補の順序の手がかり。#1909)。
        let blobs = self.services.blob_service.as_ref();
        let manifest_hash = kukuri_core::BlobHash::new(preset_ref.manifest_blob_hash.clone());
        if let Some(provider) = docs.remote_reader_id() {
            let _ = blobs.learn_content_source(&manifest_hash, &provider).await;
        }
        let Some(bytes) = fetch_manifest_blob(blobs, &manifest_hash).await else {
            return Ok(None);
        };
        let manifest: DomePresetManifestV1 = serde_json::from_slice(&bytes)?;
        validate_dome_preset_manifest(&manifest)?;
        if manifest.preset_id != preset_ref.preset_id
            || manifest.owner_pubkey != preset_ref.owner_pubkey
            || manifest.revision != preset_ref.revision
        {
            anyhow::bail!("Dome Preset reference does not match its manifest");
        }
        let (signed, envelope_author): (DomePresetManifestV1, _) =
            match fetch_verified_dome_envelope(
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
        learn_asset_sources(docs, blobs, &manifest.asset_refs).await;
        if keep {
            docs.persist_verified_record(
                &replica,
                &state_key,
                state_record.docs_author.as_deref(),
                &[],
            )
            .await?;
            docs.persist_verified_record(
                &replica,
                &stable_key("envelopes", state.last_envelope_id.as_str()),
                envelope_author.as_deref(),
                &[],
            )
            .await?;
            if blobs.local_blob_status(&manifest_hash).await? == BlobStatus::Missing {
                let stored = blobs
                    .put_remote_blob(bytes, &preset_ref.manifest_mime)
                    .await?;
                anyhow::ensure!(
                    stored.hash == manifest_hash,
                    "Dome Preset manifest hash changed"
                );
            }
        }
        Ok(Some(manifest))
    }

    pub(crate) async fn persist_dome_move_record(&self, record: &DomeMoveRecordV1) -> Result<()> {
        let envelope = build_dome_move_envelope(self.services.keys.as_ref(), record)?;
        let replica = author_replica_id(record.owner_pubkey.as_str());
        self.services
            .persist_author_event(record.owner_pubkey.as_str(), &envelope)
            .await?;
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
        self.read_author_object(owner_pubkey, None, |docs, policy| async move {
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
            let (signed, _): (DomeMoveRecordV1, _) = match fetch_verified_dome_envelope(
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
            let _ = n0_future::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
        self.release_scope_holder(&live_holder(&key)).await;
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
                &session_hint_topic_for(topic_id, channel_id),
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
        let envelope = build_live_session_envelope(
            self.services.keys.as_ref(),
            &manifest.topic_id,
            manifest.session_id.as_str(),
            &manifest,
        )?;
        let target = self
            .session_write_replica(
                topic_id,
                manifest.channel_id.as_ref(),
                replica,
                envelope.created_at,
            )
            .await?;
        let moved = target != *replica;
        let replica = &target;
        let now = Utc::now().timestamp_millis();
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
        let verified = VerifiedLiveSession::verify(
            state,
            &envelope.pubkey,
            envelope.created_at,
            manifest,
            replica,
            topic_id,
        )
        .map_err(|reason| anyhow::anyhow!("live session was rejected: {}", reason.as_str()))?;
        super::bucket_writer::persist_session_envelope_and_locator(
            &self.services,
            replica,
            &envelope,
            &stable_key(
                "sessions/live",
                &format!("{}/locator", verified.state().session_id),
            ),
            moved,
        )
        .await?;
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
        let envelope = build_game_session_envelope(
            self.services.keys.as_ref(),
            &manifest.topic_id,
            manifest.room_id.as_str(),
            &manifest,
        )?;
        let target = self
            .session_write_replica(
                topic_id,
                manifest.channel_id.as_ref(),
                replica,
                envelope.created_at,
            )
            .await?;
        let moved = target != *replica;
        let replica = &target;
        let now = Utc::now().timestamp_millis();
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
        let verified = VerifiedGameRoom::verify(
            state,
            Some((&envelope.pubkey, envelope.created_at)),
            manifest,
            replica,
            topic_id,
        )
        .map_err(|reason| anyhow::anyhow!("game room was rejected: {}", reason.as_str()))?;
        super::bucket_writer::persist_session_envelope_and_locator(
            &self.services,
            replica,
            &envelope,
            &stable_key(
                "sessions/game",
                &format!("{}/locator", verified.state().room_id),
            ),
            moved,
        )
        .await?;
        persist_game_room_state(self.services.docs_sync.as_ref(), replica, verified.state())
            .await?;
        Ok(verified)
    }

    /// 手元 → 有界な provider の順に読み、検証した版。projection より古い版は返さない。
    ///
    /// `keep` は、呼出元がこの版を projection へ反映する経路(`read_session`)か。そのときだけ、remote から読んだ
    /// projection より新しい版を保持し、他の参加者へ提供する(#1395、INVAR-6)。操作が読んだ版は保持しない。
    pub(crate) async fn fetch_verified_live_session(
        &self,
        topic_id: &str,
        channel: &str,
        session_id: &str,
        keep: bool,
    ) -> Result<Option<VerifiedLiveSession>> {
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
        let channel = source.map_or(channel, |(channel, _, _)| channel);
        let generation = self
            .services
            .active_content_scope_generation(topic_id, channel)
            .await;
        let readers = self
            .session_target_readers(topic_id, channel, source, session_id, "live")
            .await?;
        let deadline = n0_future::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut settled = None;
        for (replica, docs, policy) in readers {
            let read = crate::timeout_at(
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
                let projected = self
                    .services
                    .projection_store
                    .get_live_session(topic_id, session_id)
                    .await?
                    .map(|row| row.revision);
                if projected.is_some_and(|revision| revision > verified.revision()) {
                    continue;
                }
                let newer = projected.is_none_or(|revision| revision < verified.revision());
                if !newer
                    && self
                        .searches_past_projected(policy, verified.relay(), keep)
                        .await
                {
                    settled.get_or_insert(verified);
                    continue;
                }
                if keep && newer {
                    self.keep_relayed_session(verified.relay(), docs.as_ref(), &replica)
                        .await;
                }
                return Ok(Some(verified));
            }
            if n0_future::time::Instant::now() >= deadline {
                break;
            }
        }
        Ok(settled)
    }

    /// `keep` は `fetch_verified_live_session` と同じ。
    pub(crate) async fn fetch_verified_game_room(
        &self,
        topic_id: &str,
        channel: &str,
        room_id: &str,
        keep: bool,
    ) -> Result<Option<VerifiedGameRoom>> {
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
        let channel = source.map_or(channel, |(channel, _, _)| channel);
        let generation = self
            .services
            .active_content_scope_generation(topic_id, channel)
            .await;
        let readers = self
            .session_target_readers(topic_id, channel, source, room_id, "game")
            .await?;
        let deadline = n0_future::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut settled = None;
        for (replica, docs, policy) in readers {
            let read = crate::timeout_at(
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
                let projected = self
                    .services
                    .projection_store
                    .get_game_room(topic_id, room_id)
                    .await?;
                if let Some(revision) = verified.score_revision()
                    && projected
                        .as_ref()
                        .and_then(|row| row.score_revision)
                        .is_some_and(|projected| projected > revision)
                {
                    continue;
                }
                // 新旧の比べ方は、1 つの provider の中で版を選ぶ規則(`load_verified_game_room`)と同じ。
                let newer = match (&projected, verified.score_revision()) {
                    (None, _) => true,
                    (Some(row), Some(revision)) => row.score_revision.is_none_or(|p| p < revision),
                    (Some(row), None) => row.updated_at < verified.state().updated_at,
                };
                if !newer
                    && self
                        .searches_past_projected(policy, verified.relay(), keep)
                        .await
                {
                    settled.get_or_insert(verified);
                    continue;
                }
                if keep && newer {
                    self.keep_relayed_session(verified.relay(), docs.as_ref(), &replica)
                        .await;
                }
                return Ok(Some(verified));
            }
            if n0_future::time::Instant::now() >= deadline {
                break;
            }
        }
        Ok(settled)
    }
}

impl AppService {
    /// 反映済みと同じ版を読んだとき、それで打ち切らずに残りの読み先から新しい版を先に探すか(見つからなければその版を
    /// 使う)。手元の読み取りが返した、他の参加者へ提供するために保持した版(#1395)は探す。自分の docs author が書いた
    /// state(自分の session、訪問者として書いた Dome の state)は手元が正本なので、手元で打ち切る。遠隔の読み先の版は、
    /// 反映の経路(`keep`。hint・読み直し・表示の読み)でだけ探す(先に読んだ参加者がまだ前の版を持つことがある。
    /// #1816)。操作や映像の認可の読みは、読み先への問い合わせを増やさないよう、その版で打ち切る。
    async fn searches_past_projected(
        &self,
        policy: DocFetchPolicy,
        relay: Option<&SessionRelay>,
        keep: bool,
    ) -> bool {
        if policy != DocFetchPolicy::LocalOnly {
            return keep;
        }
        let Some(relay) = relay else {
            return false;
        };
        let own = self
            .services
            .docs_sync
            .local_docs_author()
            .await
            .ok()
            .flatten();
        relay.state_author() != own.as_deref()
    }

    /// 反映する session の版を保持する。保持は他の参加者への提供のためで、失敗しても反映は続ける。
    async fn keep_relayed_session(
        &self,
        relay: Option<&SessionRelay>,
        docs: &dyn DocsSync,
        replica: &ReplicaId,
    ) {
        let Some(relay) = relay else {
            return;
        };
        if let Err(error) = relay
            .keep(docs, self.services.blob_service.as_ref(), replica)
            .await
        {
            warn!(%error, "failed to keep a relayed session");
        }
    }
}

/// 署名済みの Dome の envelope の content と、その record を書いた docs author。
pub(crate) async fn fetch_verified_dome_envelope<T: DeserializeOwned>(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope_id: &EnvelopeId,
    expected_kind: &str,
    expected_owner: &Pubkey,
    policy: DocFetchPolicy,
) -> Result<(T, Option<String>)> {
    let records = docs_sync
        .query_replica_with_policy(
            replica,
            DocQuery::Exact(stable_key("envelopes", envelope_id.as_str())),
            policy,
        )
        .await?;
    let record = records
        .into_iter()
        .next()
        .ok_or(DomeReadUnavailable::Envelope)?;
    let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)?;
    envelope.verify()?;
    if envelope.id != *envelope_id
        || envelope.kind != expected_kind
        || envelope.pubkey != *expected_owner
    {
        anyhow::bail!("signed Dome envelope identity does not match state");
    }
    Ok((
        serde_json::from_str(envelope.content.as_str())?,
        record.docs_author,
    ))
}
