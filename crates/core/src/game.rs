use std::collections::HashSet;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::{ChannelId, EnvelopeId, ManifestBlobRef, Pubkey, TopicId};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum GameRoomKind {
    #[default]
    ScoreGame,
    MetaverseRoom,
    /// SDK のゲームの action session(ADR 0070 §5.2)。参加者は manifest に書かず、host の端末の session が受け付ける。
    ActionSession,
}

/// action session の room の、owner が署名する metadata(ADR 0070 §5.2)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ActionSessionV1 {
    /// ゲームを識別する ID。一覧はこの値ごとに引く。
    pub application_id: String,
    /// 参加者が接続する host の端末の endpoint id。
    pub host_endpoint_id: String,
    pub max_players: u32,
}

/// application ID は 1〜128 byte の、空白と制御文字を含まない ASCII。
pub fn validate_action_session(action: &ActionSessionV1) -> Result<()> {
    let id = action.application_id.as_bytes();
    if id.is_empty() || id.len() > 128 || !id.iter().all(u8::is_ascii_graphic) {
        bail!("application_id must be 1-128 printable ASCII characters without spaces");
    }
    if action.host_endpoint_id.is_empty() {
        bail!("action session host endpoint id is required");
    }
    if action.max_players == 0 {
        bail!("max_players must be at least 1");
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum GameRoomStatus {
    Waiting,
    Running,
    Paused,
    Ended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameParticipant {
    pub participant_id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameScoreEntry {
    pub participant_id: String,
    pub label: String,
    pub score: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MetaverseAssetKind {
    Vrm,
    Glb,
    Texture,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct MetaverseAssetRef {
    pub kind: MetaverseAssetKind,
    pub blob_hash: String,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub name: Option<String>,
    #[serde(default)]
    pub budget_metadata: Option<crate::MetaverseAssetBudgetMetadataV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct MetaverseRoomPresenceV1 {
    pub room_id: String,
    pub peer_id: String,
    pub display_name: Option<String>,
    pub avatar_asset_ref: Option<MetaverseAssetRef>,
    pub joined_at: i64,
    pub last_seen_at: i64,
    /// 音声を送っている(聞き手はこの話者だけを購読の候補にする。ADR 0067 §2)。
    #[serde(default)]
    pub microphone_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct MetaverseRoomChatMessageV1 {
    pub room_id: String,
    pub message_id: String,
    pub author_peer_id: String,
    pub display_name: Option<String>,
    pub body: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MetaversePrimitive {
    Cube,
    Sphere,
}

pub const METAVERSE_WORLD_VERSION: u64 = 7;
pub const FIXED_DOME_SPEC_ID: &str = "fixed_dome_v1";

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpatialContextV1 {
    Topic {
        #[cfg_attr(feature = "ts", ts(type = "string"))]
        topic_id: TopicId,
    },
    Channel {
        #[cfg_attr(feature = "ts", ts(type = "string"))]
        topic_id: TopicId,
        #[cfg_attr(feature = "ts", ts(type = "string"))]
        channel_id: ChannelId,
    },
}

impl SpatialContextV1 {
    pub fn topic_id(&self) -> &TopicId {
        match self {
            Self::Topic { topic_id } | Self::Channel { topic_id, .. } => topic_id,
        }
    }

    pub fn channel_id(&self) -> Option<&ChannelId> {
        match self {
            Self::Topic { .. } => None,
            Self::Channel { channel_id, .. } => Some(channel_id),
        }
    }

    pub fn canonical_id(&self) -> String {
        match self {
            Self::Topic { topic_id } => format!("topic:{}", topic_id.as_str()),
            Self::Channel {
                topic_id,
                channel_id,
            } => format!("channel:{}:{}", topic_id.as_str(), channel_id.as_str()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeDirection {
    North,
    East,
    South,
    West,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedDomeEndpointV1 {
    pub direction: DomeDirection,
    pub wall_midpoint_cm: [i64; 3],
    pub adjacent_dome_offset_cm: [i64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedDomeSpecV1 {
    pub spec_id: &'static str,
    pub inner_radius_cm: i64,
    pub outer_radius_cm: i64,
    pub apex_height_cm: i64,
    pub wall_thickness_cm: i64,
    pub opening_width_cm: i64,
    pub opening_height_cm: i64,
    pub opening_arch_radius_cm: i64,
    pub opening_rect_height_cm: i64,
    pub connection_zone_depth_cm: i64,
    pub connection_boundary_offset_cm: i64,
    pub adjacent_dome_center_distance_cm: i64,
    pub endpoints: [FixedDomeEndpointV1; 4],
    pub gravity_direction: [i64; 3],
    pub physics_enabled: bool,
}

pub const fn fixed_dome_v1() -> FixedDomeSpecV1 {
    const WALL_MIDPOINT_CM: i64 = 2_100;
    const ADJACENT_OFFSET_CM: i64 = 5_700;
    FixedDomeSpecV1 {
        spec_id: FIXED_DOME_SPEC_ID,
        inner_radius_cm: 2_000,
        outer_radius_cm: 2_200,
        apex_height_cm: 2_000,
        wall_thickness_cm: 200,
        opening_width_cm: 500,
        opening_height_cm: 1_000,
        opening_arch_radius_cm: 250,
        opening_rect_height_cm: 750,
        connection_zone_depth_cm: 1_500,
        connection_boundary_offset_cm: 2_850,
        adjacent_dome_center_distance_cm: ADJACENT_OFFSET_CM,
        endpoints: [
            FixedDomeEndpointV1 {
                direction: DomeDirection::North,
                wall_midpoint_cm: [0, 0, -WALL_MIDPOINT_CM],
                adjacent_dome_offset_cm: [0, 0, -ADJACENT_OFFSET_CM],
            },
            FixedDomeEndpointV1 {
                direction: DomeDirection::East,
                wall_midpoint_cm: [WALL_MIDPOINT_CM, 0, 0],
                adjacent_dome_offset_cm: [ADJACENT_OFFSET_CM, 0, 0],
            },
            FixedDomeEndpointV1 {
                direction: DomeDirection::South,
                wall_midpoint_cm: [0, 0, WALL_MIDPOINT_CM],
                adjacent_dome_offset_cm: [0, 0, ADJACENT_OFFSET_CM],
            },
            FixedDomeEndpointV1 {
                direction: DomeDirection::West,
                wall_midpoint_cm: [-WALL_MIDPOINT_CM, 0, 0],
                adjacent_dome_offset_cm: [-ADJACENT_OFFSET_CM, 0, 0],
            },
        ],
        gravity_direction: [0, -1, 0],
        physics_enabled: true,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeMaterialPreset {
    Concrete,
    Stone,
    Metal,
    Wood,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct DomeSurfaceCustomizationV1 {
    pub wall_material: DomeMaterialPreset,
    pub floor_material: DomeMaterialPreset,
    pub wall_texture: Option<MetaverseAssetRef>,
    pub floor_texture: Option<MetaverseAssetRef>,
}

impl Default for DomeSurfaceCustomizationV1 {
    fn default() -> Self {
        Self {
            wall_material: DomeMaterialPreset::Concrete,
            floor_material: DomeMaterialPreset::Stone,
            wall_texture: None,
            floor_texture: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomeEnvironmentV1 {
    pub key_light_milli: u32,
    pub ambient_light_milli: u32,
    pub fog_density_micros: u32,
    pub gravity_milli: u32,
}

impl Default for DomeEnvironmentV1 {
    fn default() -> Self {
        Self {
            key_light_milli: 2_400,
            ambient_light_milli: 400,
            fog_density_micros: 8_000,
            gravity_milli: 9_800,
        }
    }
}

pub fn interpolate_dome_environment(
    from: &DomeEnvironmentV1,
    to: &DomeEnvironmentV1,
    progress_milli: u16,
) -> DomeEnvironmentV1 {
    let progress = u32::from(progress_milli.min(1_000));
    let interpolate = |left: u32, right: u32| -> u32 {
        let left = i64::from(left);
        let delta = i64::from(right) - left;
        (left + delta * i64::from(progress) / 1_000) as u32
    };
    DomeEnvironmentV1 {
        key_light_milli: interpolate(from.key_light_milli, to.key_light_milli),
        ambient_light_milli: interpolate(from.ambient_light_milli, to.ambient_light_milli),
        fog_density_micros: interpolate(from.fog_density_micros, to.fog_density_micros),
        gravity_milli: interpolate(from.gravity_milli, to.gravity_milli),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MetaverseInteractionKind {
    Grab,
    Throw,
    Push,
    Sit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetaverseColliderV1 {
    Capsule {
        center: [i64; 3],
        radius: i64,
        half_height: i64,
    },
    Cuboid {
        center: [i64; 3],
        half_extents: [i64; 3],
    },
}

pub fn validate_metaverse_collider(collider: &MetaverseColliderV1) -> Result<()> {
    let center = match collider {
        MetaverseColliderV1::Capsule {
            center,
            radius,
            half_height,
        } => {
            if *radius <= 0 || *half_height < 0 || *radius > 2_000 || *half_height > 2_000 {
                bail!("capsule collider dimensions are outside the supported range");
            }
            center
        }
        MetaverseColliderV1::Cuboid {
            center,
            half_extents,
        } => {
            if half_extents
                .iter()
                .any(|extent| !(1..=2_000).contains(extent))
            {
                bail!("cuboid collider dimensions are outside the supported range");
            }
            center
        }
    };
    if center
        .iter()
        .any(|component| component.unsigned_abs() > 4_000)
    {
        bail!("collider center is outside the supported range");
    }
    Ok(())
}

pub fn fallback_capsule_collider(
    bounds_min: [i64; 3],
    bounds_max: [i64; 3],
) -> Result<MetaverseColliderV1> {
    if bounds_min
        .iter()
        .zip(bounds_max)
        .any(|(minimum, maximum)| *minimum >= maximum)
    {
        bail!("metaverse collider bounds must have positive volume");
    }
    let width = bounds_max[0] - bounds_min[0];
    let height = bounds_max[1] - bounds_min[1];
    let depth = bounds_max[2] - bounds_min[2];
    let radius = (width.max(depth) + 1) / 2;
    let half_height = ((height + 1) / 2 - radius).max(0);
    Ok(MetaverseColliderV1::Capsule {
        center: [
            (bounds_min[0] + bounds_max[0]) / 2,
            (bounds_min[1] + bounds_max[1]) / 2,
            (bounds_min[2] + bounds_max[2]) / 2,
        ],
        radius,
        half_height,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct MetaversePersistentPropV1 {
    pub prop_id: String,
    pub asset_ref: Option<MetaverseAssetRef>,
    pub primitive_fallback: MetaversePrimitive,
    pub position: [i64; 3],
    pub rotation: [i64; 3],
    pub scale: [i64; 3],
    pub visual_only: bool,
    pub interactions: Vec<MetaverseInteractionKind>,
    pub collider: Option<MetaverseColliderV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomeCustomizationV1 {
    pub surface: DomeSurfaceCustomizationV1,
    pub environment: DomeEnvironmentV1,
    pub persistent_props: Vec<MetaversePersistentPropV1>,
}

impl Default for DomeCustomizationV1 {
    fn default() -> Self {
        Self {
            surface: DomeSurfaceCustomizationV1::default(),
            environment: DomeEnvironmentV1::default(),
            persistent_props: vec![MetaversePersistentPropV1 {
                prop_id: "dome-prop-1".to_string(),
                asset_ref: None,
                primitive_fallback: MetaversePrimitive::Cube,
                position: [0, 50, -240],
                rotation: [0, 0, 0],
                scale: [100, 100, 100],
                visual_only: false,
                interactions: vec![
                    MetaverseInteractionKind::Grab,
                    MetaverseInteractionKind::Throw,
                    MetaverseInteractionKind::Push,
                    MetaverseInteractionKind::Sit,
                ],
                collider: None,
            }],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MetaverseDomeV1 {
    pub spec_id: String,
    pub customization: DomeCustomizationV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DomePresetManifestV1 {
    pub preset_id: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub owner_pubkey: Pubkey,
    /// Owner-managed, monotonically increasing durable layout revision.
    pub revision: u64,
    pub dome: MetaverseDomeV1,
    pub asset_refs: Vec<MetaverseAssetRef>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DomePresetRefV1 {
    pub preset_id: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub owner_pubkey: Pubkey,
    pub revision: u64,
    pub manifest_blob_hash: String,
    pub manifest_mime: String,
    pub manifest_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomePresetStateDocV1 {
    pub preset_id: String,
    pub owner_pubkey: Pubkey,
    pub revision: u64,
    pub current_manifest: ManifestBlobRef,
    pub updated_at: i64,
    pub last_envelope_id: EnvelopeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeInstanceStatusV1 {
    Staging,
    Active,
    Tombstoned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct DomeRelationshipDetachV1 {
    pub move_id: String,
    pub instance_generation: u64,
    pub detached_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct DomeInstanceManifestV1 {
    pub instance_id: String,
    pub spatial_context: SpatialContextV1,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub owner_pubkey: Pubkey,
    pub preset_ref: DomePresetRefV1,
    pub title: String,
    pub description: String,
    pub max_peers: Option<u32>,
    pub default_spawn: MetaverseRoomSpawnV1,
    pub generation: u64,
    pub status: DomeInstanceStatusV1,
    pub relationship_detach: Option<DomeRelationshipDetachV1>,
    pub replacement_instance_id: Option<String>,
    #[serde(default)]
    pub chat_history: Vec<MetaverseRoomChatMessageV1>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomeInstanceStateDocV1 {
    pub instance_id: String,
    pub spatial_context: SpatialContextV1,
    pub owner_pubkey: Pubkey,
    pub generation: u64,
    pub status: DomeInstanceStatusV1,
    pub created_at: i64,
    pub updated_at: i64,
    pub current_manifest: ManifestBlobRef,
    pub last_envelope_id: EnvelopeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeMovePhaseV1 {
    Preparing,
    TargetStaged,
    SourceDetached,
    TargetActive,
    SourceTombstoned,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct DomeMoveRecordV1 {
    pub move_id: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub owner_pubkey: Pubkey,
    pub source_instance_id: String,
    pub source_context: SpatialContextV1,
    pub source_generation: u64,
    pub target_instance_id: String,
    pub target_context: SpatialContextV1,
    pub target_generation: u64,
    pub preset_ref: DomePresetRefV1,
    pub phase: DomeMovePhaseV1,
    pub failure_reason: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomeMoveStateDocV1 {
    pub record: DomeMoveRecordV1,
    pub last_envelope_id: EnvelopeId,
}

impl Default for MetaverseDomeV1 {
    fn default() -> Self {
        Self {
            spec_id: FIXED_DOME_SPEC_ID.to_string(),
            customization: DomeCustomizationV1::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SharedRoomObjectV1 {
    pub object_id: String,
    pub asset_ref: Option<MetaverseAssetRef>,
    pub primitive_fallback: MetaversePrimitive,
    pub position: [i64; 3],
    pub rotation: [i64; 3],
    pub scale: [i64; 3],
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub updated_by: Pubkey,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MetaverseRoomEventV1 {
    PresenceJoin {
        presence: MetaverseRoomPresenceV1,
    },
    PresenceLeave {
        room_id: String,
        peer_id: String,
        left_at: i64,
    },
    ChatMessage {
        message: MetaverseRoomChatMessageV1,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct MetaverseRoomEventEnvelopeContentV1 {
    pub event_id: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub topic_id: TopicId,
    #[cfg_attr(feature = "ts", ts(as = "Option<String>"))]
    pub channel_id: Option<ChannelId>,
    pub room_id: String,
    pub spatial_context: SpatialContextV1,
    pub instance_generation: u64,
    pub session_id: String,
    pub peer_id: String,
    pub seq: u64,
    pub sent_at: i64,
    pub event: MetaverseRoomEventV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct MetaverseRoomSpawnV1 {
    pub position: [i64; 3],
    pub rotation: [i64; 3],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct MetaverseRoomStateV1 {
    pub world_version: u64,
    pub instance_id: String,
    pub spatial_context: SpatialContextV1,
    pub instance_generation: u64,
    pub instance_status: DomeInstanceStatusV1,
    pub relationship_detach: Option<DomeRelationshipDetachV1>,
    pub replacement_instance_id: Option<String>,
    pub preset_ref: DomePresetRefV1,
    pub session_id: String,
    pub max_peers: Option<u32>,
    pub dome: MetaverseDomeV1,
    pub default_spawn: MetaverseRoomSpawnV1,
    pub asset_refs: Vec<MetaverseAssetRef>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<MetaverseRoomChatMessageV1>>"))]
    pub chat_history: Vec<MetaverseRoomChatMessageV1>,
}

pub fn validate_dome_customization(customization: &DomeCustomizationV1) -> Result<()> {
    fn validate_texture(asset: &Option<MetaverseAssetRef>, field: &str) -> Result<()> {
        if let Some(asset) = asset
            && asset.kind != MetaverseAssetKind::Texture
        {
            bail!("{field} must reference a texture asset");
        }
        Ok(())
    }

    validate_texture(&customization.surface.wall_texture, "wall texture")?;
    validate_texture(&customization.surface.floor_texture, "floor texture")?;
    let environment = &customization.environment;
    if environment.key_light_milli > 4_000 {
        bail!("key light intensity is outside the supported range");
    }
    if environment.ambient_light_milli > 2_000 {
        bail!("ambient light intensity is outside the supported range");
    }
    if environment.fog_density_micros > 200_000 {
        bail!("fog density is outside the supported range");
    }
    if !(1_000..=30_000).contains(&environment.gravity_milli) {
        bail!("gravity strength is outside the supported range");
    }
    if customization.persistent_props.len() > 1_024 {
        bail!("a Dome supports at most 1024 persistent props");
    }
    let mut prop_ids = HashSet::new();
    for prop in &customization.persistent_props {
        if prop.prop_id.trim().is_empty() || !prop_ids.insert(prop.prop_id.as_str()) {
            bail!("persistent prop ids must be non-empty and unique");
        }
        if prop.scale.iter().any(|scale| !(1..=1_000).contains(scale)) {
            bail!("persistent prop scale is outside the supported range");
        }
        let distance_squared = prop.position[0] * prop.position[0]
            + prop.position[1] * prop.position[1]
            + prop.position[2] * prop.position[2];
        if prop.position[1] < 0
            || prop.position[1] > fixed_dome_v1().apex_height_cm
            || distance_squared > fixed_dome_v1().inner_radius_cm * fixed_dome_v1().inner_radius_cm
        {
            bail!("persistent prop position is outside the fixed Dome");
        }
        let mut interactions = HashSet::new();
        if prop
            .interactions
            .iter()
            .any(|interaction| !interactions.insert(*interaction))
        {
            bail!("persistent prop interactions must be unique");
        }
        if prop.visual_only && !prop.interactions.is_empty() {
            bail!("visual-only props cannot expose interactions");
        }
        if let Some(collider) = &prop.collider {
            validate_metaverse_collider(collider)?;
        }
    }
    Ok(())
}

pub fn validate_metaverse_room_state(state: &MetaverseRoomStateV1) -> Result<()> {
    if state.world_version != METAVERSE_WORLD_VERSION {
        bail!("unsupported metaverse world version");
    }
    if state.dome.spec_id != FIXED_DOME_SPEC_ID {
        bail!("unsupported Dome spec id");
    }
    if state.instance_id.trim().is_empty()
        || state.preset_ref.preset_id.trim().is_empty()
        || state.preset_ref.manifest_blob_hash.trim().is_empty()
        || state.session_id.trim().is_empty()
        || state.instance_generation == 0
    {
        bail!("metaverse instance identity is incomplete");
    }
    if state.instance_status == DomeInstanceStatusV1::Tombstoned {
        bail!("tombstoned Dome instance cannot be resolved as an active room");
    }
    if state
        .max_peers
        .is_some_and(|max_peers| !(1..=512).contains(&max_peers))
    {
        bail!("max peers is outside the supported range");
    }
    validate_dome_customization(&state.dome.customization)
}

pub fn validate_dome_preset_manifest(manifest: &DomePresetManifestV1) -> Result<()> {
    if manifest.preset_id.trim().is_empty() {
        bail!("Dome preset id is required");
    }
    if manifest.owner_pubkey.as_str().trim().is_empty() {
        bail!("Dome preset owner is required");
    }
    if manifest.revision == 0 {
        bail!("Dome preset revision is required");
    }
    if manifest.dome.spec_id != FIXED_DOME_SPEC_ID {
        bail!("unsupported Dome spec id");
    }
    validate_dome_customization(&manifest.dome.customization)?;
    let mut hashes = HashSet::new();
    for asset in &manifest.asset_refs {
        if asset.blob_hash.trim().is_empty() || !hashes.insert(asset.blob_hash.as_str()) {
            bail!("Dome preset asset hashes must be non-empty and unique");
        }
    }
    let referenced_hashes = manifest
        .dome
        .customization
        .persistent_props
        .iter()
        .filter_map(|prop| prop.asset_ref.as_ref())
        .chain(manifest.dome.customization.surface.wall_texture.as_ref())
        .chain(manifest.dome.customization.surface.floor_texture.as_ref())
        .map(|asset| asset.blob_hash.as_str())
        .collect::<HashSet<_>>();
    if !referenced_hashes.is_subset(&hashes) {
        bail!("Dome customization references an asset outside the Preset asset list");
    }
    crate::metaverse_dome_resource_usage(&manifest.asset_refs)?;
    Ok(())
}

pub fn validate_dome_instance_manifest(manifest: &DomeInstanceManifestV1) -> Result<()> {
    if manifest.instance_id.trim().is_empty()
        || manifest.preset_ref.preset_id.trim().is_empty()
        || manifest.preset_ref.revision == 0
        || manifest.preset_ref.manifest_blob_hash.trim().is_empty()
        || manifest.preset_ref.manifest_mime.trim().is_empty()
        || manifest.generation == 0
    {
        bail!("Dome instance identity is incomplete");
    }
    if manifest.owner_pubkey != manifest.preset_ref.owner_pubkey {
        bail!("Dome instance owner must match Dome preset owner");
    }
    if manifest.title.trim().is_empty() {
        bail!("Dome instance title is required");
    }
    if manifest
        .max_peers
        .is_some_and(|max_peers| !(1..=512).contains(&max_peers))
    {
        bail!("max peers is outside the supported range");
    }
    if manifest.status == DomeInstanceStatusV1::Staging && manifest.relationship_detach.is_some() {
        bail!("staging Dome instance cannot detach relationships");
    }
    if let Some(detach) = &manifest.relationship_detach
        && (detach.move_id.trim().is_empty() || detach.instance_generation != manifest.generation)
    {
        bail!("Dome relationship detach does not match instance generation");
    }
    Ok(())
}

pub fn validate_dome_relationship_scope(
    left: &DomeInstanceManifestV1,
    right: &DomeInstanceManifestV1,
) -> Result<()> {
    if left.spatial_context != right.spatial_context {
        bail!("Dome Connection endpoints must share one Spatial Context");
    }
    if left.status != DomeInstanceStatusV1::Active
        || right.status != DomeInstanceStatusV1::Active
        || left.relationship_detach.is_some()
        || right.relationship_detach.is_some()
    {
        bail!("Dome Connection endpoints must be active and attached");
    }
    Ok(())
}

pub fn validate_dome_move_record(record: &DomeMoveRecordV1) -> Result<()> {
    if record.move_id.trim().is_empty()
        || record.source_instance_id.trim().is_empty()
        || record.target_instance_id.trim().is_empty()
        || record.source_generation == 0
        || record.target_generation == 0
    {
        bail!("Dome move identity is incomplete");
    }
    if record.source_context == record.target_context {
        bail!("Dome move target must be a different Spatial Context");
    }
    if record.owner_pubkey != record.preset_ref.owner_pubkey {
        bail!("Dome move owner must match Dome preset owner");
    }
    if record.phase != DomeMovePhaseV1::Failed && record.failure_reason.is_some() {
        bail!("only a failed Dome move can contain a failure reason");
    }
    Ok(())
}

pub fn resolve_metaverse_room_state(
    instance: &DomeInstanceManifestV1,
    preset: &DomePresetManifestV1,
) -> Result<MetaverseRoomStateV1> {
    validate_dome_instance_manifest(instance)?;
    validate_dome_preset_manifest(preset)?;
    if instance.preset_ref.preset_id != preset.preset_id
        || instance.preset_ref.owner_pubkey != preset.owner_pubkey
        || instance.preset_ref.revision != preset.revision
    {
        bail!("Dome instance preset reference does not match preset manifest");
    }
    if instance.status == DomeInstanceStatusV1::Tombstoned {
        bail!("cannot resolve a tombstoned Dome instance");
    }
    let state = MetaverseRoomStateV1 {
        world_version: METAVERSE_WORLD_VERSION,
        instance_id: instance.instance_id.clone(),
        spatial_context: instance.spatial_context.clone(),
        instance_generation: instance.generation,
        instance_status: instance.status,
        preset_ref: instance.preset_ref.clone(),
        relationship_detach: instance.relationship_detach.clone(),
        replacement_instance_id: instance.replacement_instance_id.clone(),
        session_id: instance.instance_id.clone(),
        max_peers: instance.max_peers,
        dome: preset.dome.clone(),
        default_spawn: instance.default_spawn.clone(),
        asset_refs: preset.asset_refs.clone(),
        chat_history: instance.chat_history.clone(),
    };
    validate_metaverse_room_state(&state)?;
    Ok(state)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameRoomManifestBlobV1 {
    pub room_id: String,
    /// Owner-signed monotonic revision for ScoreGame. Metaverse rooms leave this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_revision: Option<i64>,
    pub topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub owner_pubkey: Pubkey,
    pub title: String,
    pub description: String,
    pub status: GameRoomStatus,
    pub phase_label: Option<String>,
    pub participants: Vec<GameParticipant>,
    pub scores: Vec<GameScoreEntry>,
    #[serde(default)]
    pub room_kind: GameRoomKind,
    #[serde(default)]
    pub metaverse: Option<MetaverseRoomStateV1>,
    /// action session の room だけが持つ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<ActionSessionV1>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameRoomStateDocV1 {
    pub room_id: String,
    pub topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub owner_pubkey: Pubkey,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: GameRoomStatus,
    pub current_manifest: ManifestBlobRef,
    pub last_envelope_id: EnvelopeId,
}

pub fn build_game_session_envelope<T: Serialize>(
    keys: &crate::KukuriKeys,
    topic: &TopicId,
    room_id: &str,
    content: &T,
) -> Result<crate::KukuriEnvelope> {
    crate::sign_envelope_json(
        keys,
        "game-session",
        vec![
            vec!["topic".into(), topic.as_str().into()],
            vec!["object".into(), "game-session".into()],
            vec!["room_id".into(), room_id.to_string()],
        ],
        content,
    )
}

pub fn build_metaverse_room_event_envelope(
    keys: &crate::KukuriKeys,
    topic: &TopicId,
    room_id: &str,
    content: &MetaverseRoomEventEnvelopeContentV1,
) -> Result<crate::KukuriEnvelope> {
    crate::metaverse_audio::validate_metaverse_room_event_content(content)?;
    if &content.topic_id != topic || content.room_id != room_id {
        bail!("metaverse room event envelope identity does not match content");
    }
    crate::sign_envelope_json(
        keys,
        "metaverse-room-event",
        vec![
            vec!["topic".into(), topic.as_str().into()],
            vec!["object".into(), "metaverse-room-event".into()],
            vec!["room_id".into(), room_id.to_string()],
            vec!["event_id".into(), content.event_id.clone()],
        ],
        content,
    )
}
