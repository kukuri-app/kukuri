use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    DomeInstanceManifestV1, DomeInstanceStatusV1, DomeSpatialAccessProofV1,
    MetaverseRoomEventEnvelopeContentV1, MetaverseRoomEventV1, SpatialContextV1,
    validate_dome_instance_manifest,
};

pub const METAVERSE_MEDIA_TTL_MILLIS: i64 = 10_000;
/// 音声の broadcast の track 名(Opus)と、話者の位置の track 名。
pub const METAVERSE_AUDIO_TRACK: &str = "audio";
pub const METAVERSE_AUDIO_POSITION_TRACK: &str = "position";

/// 聞き手が話者の端末へ送る認可要求(ADR 0067 §3)。proof は話者の Dome の owner と Spatial Context に束縛する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaverseAudioAccessRequestV1 {
    pub proof: DomeSpatialAccessProofV1,
    pub instance_id: String,
    pub instance_generation: u64,
    pub session_id: String,
}

/// 話者の音声の broadcast の path。接続先の端末が 1 人の話者なので、Dome の session だけで決まる。
pub fn metaverse_audio_broadcast_path(instance_id: &str, session_id: &str) -> String {
    format!("metaverse-audio/{instance_id}/{session_id}")
}

pub fn validate_metaverse_room_event_content(
    content: &MetaverseRoomEventEnvelopeContentV1,
) -> Result<()> {
    if content.event_id.trim().is_empty()
        || content.room_id.trim().is_empty()
        || content.session_id.trim().is_empty()
        || content.peer_id.trim().is_empty()
        || content.instance_generation == 0
    {
        bail!("metaverse room event identity is incomplete");
    }
    if content.session_id != content.room_id {
        bail!("metaverse room event session does not match its Dome instance");
    }
    let expected_context = match &content.channel_id {
        Some(channel_id) => SpatialContextV1::Channel {
            topic_id: content.topic_id.clone(),
            channel_id: channel_id.clone(),
        },
        None => SpatialContextV1::Topic {
            topic_id: content.topic_id.clone(),
        },
    };
    if content.spatial_context != expected_context {
        bail!("metaverse room event Spatial Context does not match topic/channel identity");
    }
    match &content.event {
        MetaverseRoomEventV1::PresenceJoin { presence } => {
            if presence.room_id != content.room_id
                || presence.peer_id != content.peer_id
                || presence.last_seen_at < presence.joined_at
                || presence.last_seen_at
                    > content.sent_at.saturating_add(METAVERSE_MEDIA_TTL_MILLIS)
            {
                bail!("metaverse presence identity or timestamp is invalid");
            }
        }
        MetaverseRoomEventV1::PresenceLeave {
            room_id,
            peer_id,
            left_at,
        } => {
            if room_id != &content.room_id
                || peer_id != &content.peer_id
                || *left_at > content.sent_at.saturating_add(METAVERSE_MEDIA_TTL_MILLIS)
            {
                bail!("metaverse presence leave identity or timestamp is invalid");
            }
        }
        MetaverseRoomEventV1::ChatMessage { message } => {
            if message.room_id != content.room_id || message.author_peer_id != content.peer_id {
                bail!("metaverse chat identity is invalid");
            }
        }
    }
    Ok(())
}

pub fn metaverse_room_event_is_live(
    content: &MetaverseRoomEventEnvelopeContentV1,
    now_millis: i64,
) -> bool {
    match content.event {
        MetaverseRoomEventV1::PresenceJoin { .. } | MetaverseRoomEventV1::PresenceLeave { .. } => {
            content.sent_at <= now_millis.saturating_add(METAVERSE_MEDIA_TTL_MILLIS)
                && now_millis <= content.sent_at.saturating_add(METAVERSE_MEDIA_TTL_MILLIS)
        }
        MetaverseRoomEventV1::ChatMessage { .. } => true,
    }
}

pub fn validate_metaverse_room_event_for_instance(
    content: &MetaverseRoomEventEnvelopeContentV1,
    instance: &DomeInstanceManifestV1,
) -> Result<()> {
    validate_metaverse_room_event_content(content)?;
    validate_dome_instance_manifest(instance)?;
    if instance.status != DomeInstanceStatusV1::Active || instance.relationship_detach.is_some() {
        bail!("metaverse room events require an active attached Dome instance");
    }
    if content.room_id != instance.instance_id
        || content.session_id != instance.instance_id
        || content.spatial_context != instance.spatial_context
        || content.instance_generation != instance.generation
    {
        bail!("metaverse room event does not match the current Dome instance generation");
    }
    Ok(())
}

/// 距離減衰の gain(1 = 1000)。1 m 以内は 1、それ以遠は 1 m / 距離(ADR 0043)。
pub fn spatial_audio_gain_milli(distance_cm: u64) -> u16 {
    const REFERENCE_DISTANCE_CM: u64 = 100;
    if distance_cm <= REFERENCE_DISTANCE_CM {
        return 1_000;
    }
    ((REFERENCE_DISTANCE_CM.saturating_mul(1_000) / distance_cm).min(1_000)) as u16
}

/// 2 点の直線距離(cm)。
pub fn spatial_audio_distance_cm(left: [i64; 3], right: [i64; 3]) -> u64 {
    let squared = left
        .into_iter()
        .zip(right)
        .fold(0_f64, |sum, (left, right)| {
            let delta = (left - right) as f64;
            sum + delta * delta
        });
    squared.sqrt().round() as u64
}

/// 隣接 Dome の話者への合成距離: 話者 → 話者側の opening + 聞き手側の opening → 聞き手(ADR 0043)。
pub fn connection_opening_audio_distance_cm(
    speaker: [i64; 3],
    speaker_opening: [i64; 3],
    listener_opening: [i64; 3],
    listener: [i64; 3],
) -> u64 {
    spatial_audio_distance_cm(speaker, speaker_opening)
        .saturating_add(spatial_audio_distance_cm(listener_opening, listener))
}
