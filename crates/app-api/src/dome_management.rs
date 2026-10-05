//! Management is based on the signed Instance, independently of scene assets.
use crate::DomeHostingView;
use crate::service::*;
use kukuri_core::{DomeInstanceStatusV1, SpatialContextV1};

impl AppService {
    pub(crate) async fn get_dome_hosting_authority(
        &self,
        context: SpatialContextV1,
        id: &str,
    ) -> Result<DomeHostingView> {
        self.hosting_context_replica(&context).await?;
        let instance = self
            .hosting_instance(&context, id)
            .await?
            .context("Dome instance not found")?;
        let records = self.list_dome_hosting_records(&instance).await?;
        self.hosting_authority_view(&instance, &records, Utc::now().timestamp_millis())
            .await
    }

    /// 手元の一覧に無い Dome を、owner の制御領域の Instance と Preset から足す。自分の Dome と、hosting の heartbeat で
    /// 知った Dome(#1221 R5-H、2026-09-27 ユーザー決定)が対象。heartbeat の host は、context と合わせて instance の
    /// id を導けるときだけ owner とみなす(owner の端末の hosting)。
    pub(crate) async fn append_context_domes(
        &self,
        topic: &str,
        channels: &BTreeSet<String>,
        items: &mut Vec<GameRoomView>,
    ) -> Result<()> {
        let local = self.services.keys.public_key();
        for channel in channels {
            let context = if channel == PUBLIC_CHANNEL_ID {
                SpatialContextV1::Topic {
                    topic_id: TopicId::new(topic),
                }
            } else {
                SpatialContextV1::Channel {
                    topic_id: TopicId::new(topic),
                    channel_id: kukuri_core::ChannelId::new(channel),
                }
            };
            let hosted = self.heartbeat_dome_owners(&context).await;
            for owner in std::iter::once(local.clone()).chain(hosted) {
                if let Some(item) = self
                    .context_dome_view(topic, channel, &context, &owner, items)
                    .await?
                {
                    items.push(item);
                }
            }
        }
        Ok(())
    }

    /// hosting の heartbeat の host のうち、context と合わせて instance の id を導ける owner(owner の端末の hosting)。
    pub(crate) async fn heartbeat_dome_owners(&self, context: &SpatialContextV1) -> Vec<Pubkey> {
        self.dome_host_heartbeats
            .lock()
            .await
            .hosts(context, Utc::now().timestamp_millis())
    }

    async fn context_dome_view(
        &self,
        topic: &str,
        channel: &str,
        context: &SpatialContextV1,
        owner: &Pubkey,
        items: &[GameRoomView],
    ) -> Result<Option<GameRoomView>> {
        if items.iter().any(|room| {
            room.room_kind == GameRoomKind::MetaverseRoom
                && room.host_pubkey == owner.as_str()
                && room.channel_id.as_deref().unwrap_or(PUBLIC_CHANNEL_ID) == channel
        }) {
            return Ok(None);
        }
        let resolved = match self.fetch_dome_instance_manifest(context, owner).await {
            Ok(value) => value,
            Err(error) if error.downcast_ref::<DomeReadUnavailable>().is_some() => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some((state, instance)) = resolved else {
            return Ok(None);
        };
        if instance.status != DomeInstanceStatusV1::Active || instance.relationship_detach.is_some()
        {
            return Ok(None);
        }
        let preset = self
            .fetch_dome_preset_manifest(&instance.preset_ref)
            .await?;
        // 自分の Dome は Preset が読めなくても管理のために出す。他人の Dome は Preset が読めるときだけ出す。
        if preset.is_none() && *owner != self.services.keys.public_key() {
            return Ok(None);
        }
        let mut manifest = instance_management_manifest(&instance);
        let records = self.list_dome_hosting_records(&instance).await?;
        let hosting = self
            .hosting_authority_view(&instance, &records, Utc::now().timestamp_millis())
            .await?
            .state;
        if let Some(preset) = preset {
            manifest.metaverse = Some(kukuri_core::resolve_metaverse_room_state(
                &instance, &preset,
            )?);
            manifest.phase_label = None;
        }
        Ok(Some(GameRoomView {
            room_id: instance.instance_id,
            host_pubkey: owner.as_str().into(),
            title: instance.title,
            description: instance.description,
            status: GameRoomStatus::Waiting,
            phase_label: manifest.phase_label,
            scores: vec![],
            room_kind: GameRoomKind::MetaverseRoom,
            metaverse: manifest.metaverse,
            dome_hosting: Some(hosting),
            manifest_blob_hash: state.current_manifest.hash.as_str().into(),
            updated_at: instance.updated_at,
            channel_id: channel_id_for_view(channel),
            audience_label: self.audience_label_for_storage(topic, channel).await,
        }))
    }
}

/// Metadata-only projection. It is never a scene or a new active Preset. Delete
/// uses it only to publish an inactive tombstone; discovery marks it unhosted.
pub(crate) fn instance_management_manifest(
    instance: &DomeInstanceManifestV1,
) -> GameRoomManifestBlobV1 {
    GameRoomManifestBlobV1 {
        room_id: instance.instance_id.clone(),
        score_revision: None,
        topic_id: instance.spatial_context.topic_id().clone(),
        channel_id: instance.spatial_context.channel_id().cloned(),
        owner_pubkey: instance.owner_pubkey.clone(),
        title: instance.title.clone(),
        description: instance.description.clone(),
        status: GameRoomStatus::Waiting,
        phase_label: Some("management_only".into()),
        participants: vec![],
        scores: vec![],
        room_kind: GameRoomKind::MetaverseRoom,
        updated_at: instance.updated_at,
        metaverse: Some(kukuri_core::MetaverseRoomStateV1 {
            world_version: kukuri_core::METAVERSE_WORLD_VERSION,
            instance_id: instance.instance_id.clone(),
            spatial_context: instance.spatial_context.clone(),
            instance_generation: instance.generation,
            instance_status: instance.status,
            relationship_detach: instance.relationship_detach.clone(),
            replacement_instance_id: instance.replacement_instance_id.clone(),
            preset_ref: instance.preset_ref.clone(),
            session_id: instance.instance_id.clone(),
            max_peers: instance.max_peers,
            dome: kukuri_core::MetaverseDomeV1::default(),
            default_spawn: instance.default_spawn.clone(),
            asset_refs: vec![],
            chat_history: instance.chat_history.clone(),
        }),
    }
}
