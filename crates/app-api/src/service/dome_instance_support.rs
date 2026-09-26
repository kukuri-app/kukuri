//! Dome Instance の置き場所と読取り(#1221 R5-H、2026-09-27 ユーザー決定)。
//!
//! Instance は owner の制御領域(`author::<owner>`)の、context と owner から決まる instance の id の key に置く。
//! 読取りは手元→owner を含む有界な provider の順に exact に読み、無ければ更新前に旧 context replica へ置いた分を
//! 手元から読む。context の Instance の一覧は replica を走査せず、知っている owner について exact に読む。

use super::*;
use kukuri_core::SpatialContextV1;

/// owner の制御領域の、Dome Instance の現在値の key。instance の id は context と owner から決まる(#1221 R5-H)。
fn dome_instance_state_key(instance_id: &str) -> String {
    stable_key("metaverse/dome-instances", &format!("{instance_id}/state"))
}

async fn load_signed_dome_instance_manifest(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope_id: &EnvelopeId,
    owner_pubkey: &Pubkey,
    policy: DocFetchPolicy,
) -> Result<Option<DomeInstanceManifestV1>> {
    let key = stable_key("envelopes", envelope_id.as_str());
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            key.as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
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
    /// Dome Instance を owner の制御領域(`author::<owner>`)の、context から決まる instance の key へ置く
    /// (#1221 R5-H、2026-09-27 ユーザー決定)。旧 topic/channel replica へは書かない。
    pub(crate) async fn persist_dome_instance_manifest(
        &self,
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
        let owner = manifest.owner_pubkey.as_str();
        let replica = author_replica_id(owner);
        self.services.persist_author_event(owner, &envelope).await?;
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
                    key: dome_instance_state_key(&manifest.instance_id),
                    value: serde_json::to_value(&state)?,
                },
            )
            .await?;
        Ok(state)
    }

    /// owner の Dome Instance。owner の制御領域を手元の次に owner を含む有界な provider から exact に読み
    /// (#1221 R5-H)、無ければ更新前に旧 context replica へ置いた分を手元から読む。
    pub(crate) async fn fetch_dome_instance_manifest(
        &self,
        spatial_context: &SpatialContextV1,
        owner_pubkey: &Pubkey,
    ) -> Result<Option<(DomeInstanceStateDocV1, DomeInstanceManifestV1)>> {
        let author = author_replica_id(owner_pubkey.as_str());
        let key = dome_instance_state_key(&dome_instance_id(spatial_context, owner_pubkey));
        let current = self
            .read_author_object(owner_pubkey.as_str(), |docs, policy| {
                let (author, key) = (&author, &key);
                async move {
                    self.read_dome_instance(docs.as_ref(), author, key, owner_pubkey, policy)
                        .await
                }
            })
            .await?;
        if current.is_some() {
            return Ok(current);
        }
        let Some(legacy) = self.legacy_dome_replica(spatial_context).await else {
            return Ok(None);
        };
        let key = stable_key(
            "metaverse/dome-instances",
            &format!("{}/state", owner_pubkey.as_str()),
        );
        self.read_dome_instance(
            self.services.docs_sync.as_ref(),
            &legacy,
            &key,
            owner_pubkey,
            DocFetchPolicy::LocalOnly,
        )
        .await
    }

    /// 更新前に Dome の記録を置いていた旧 context replica。読むだけなので、private は参加中の現 epoch を回転なしで使う。
    pub(crate) async fn legacy_dome_replica(
        &self,
        context: &SpatialContextV1,
    ) -> Option<ReplicaId> {
        match context {
            SpatialContextV1::Topic { topic_id } => Some(topic_replica_id(topic_id.as_str())),
            SpatialContextV1::Channel {
                topic_id,
                channel_id,
            } => self
                .joined_private_channel_state(topic_id.as_str(), channel_id.as_str())
                .await
                .map(|state| current_private_channel_replica_id(&state)),
        }
    }

    async fn read_dome_instance(
        &self,
        docs: &dyn DocsSync,
        replica: &ReplicaId,
        key: &str,
        owner_pubkey: &Pubkey,
        policy: DocFetchPolicy,
    ) -> Result<Option<(DomeInstanceStateDocV1, DomeInstanceManifestV1)>> {
        let records = docs
            .query_replica_exact_bounded(replica, key, MAX_ENVELOPE_RECORDS_PER_OBJECT, policy)
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
                docs,
                replica,
                &state.last_envelope_id,
                &manifest.owner_pubkey,
                policy,
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
                .hosting_instance_for_owner(spatial_context, instance_id, &local_owner)
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
        self.hosting_instance_for_owner(spatial_context, instance_id, &room.manifest().owner_pubkey)
            .await
    }

    pub(crate) async fn hosting_instance_for_owner(
        &self,
        spatial_context: &SpatialContextV1,
        instance_id: &str,
        owner_pubkey: &Pubkey,
    ) -> Result<Option<DomeInstanceManifestV1>> {
        if dome_instance_id(spatial_context, owner_pubkey) != instance_id {
            return Ok(None);
        }
        let resolved = self
            .fetch_dome_instance_manifest(spatial_context, owner_pubkey)
            .await?;
        let Some((_, manifest)) = resolved else {
            return Ok(None);
        };
        if manifest.instance_id != instance_id
            || manifest.spatial_context != *spatial_context
            || manifest.owner_pubkey != *owner_pubkey
        {
            warn!(
                instance_id,
                owner_pubkey = %owner_pubkey.as_str(),
                "ignored a Dome Instance that does not match its lookup identity"
            );
            return Ok(None);
        }
        Ok(Some(manifest))
    }

    /// context の Dome Instance(#1221 R5-H)。replica は走査しない。手元の一覧の metaverse の行の owner、hosting の
    /// heartbeat で知った owner、自分、`owners`(提案・接続の端点)について、owner の制御領域を exact に読む。
    pub(crate) async fn list_context_dome_instances(
        &self,
        context: &SpatialContextV1,
        owners: impl IntoIterator<Item = Pubkey>,
    ) -> Result<Vec<DomeInstanceManifestV1>> {
        let rows = self
            .services
            .projection_store
            .list_channel_game_rooms(
                context.topic_id().as_str(),
                channel_storage_id(context.channel_id()).as_str(),
                LIVE_GAME_LIST_LIMIT,
            )
            .await?;
        let owners = rows
            .into_iter()
            .filter(|row| row.room_kind == GameRoomKind::MetaverseRoom)
            .map(|row| row.host_pubkey)
            .chain(
                owners
                    .into_iter()
                    .chain(self.heartbeat_dome_owners(context).await)
                    .map(|owner| owner.as_str().to_string()),
            )
            .chain([self.current_author_pubkey()])
            .collect::<BTreeSet<_>>();
        let mut instances = Vec::new();
        for owner in owners {
            let resolved = match self
                .fetch_dome_instance_manifest(context, &Pubkey::from(owner))
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
}
