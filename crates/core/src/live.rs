use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{ChannelId, EnvelopeId, ManifestBlobRef, Pubkey, TopicId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveSignalKind {
    SessionStarted,
    SessionEnded,
    RoomActivity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum LiveSessionStatus {
    Scheduled,
    Live,
    Paused,
    Ended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSessionManifestBlobV1 {
    pub session_id: String,
    /// Owner-signed monotonic revision. The first persisted state is 1.
    pub revision: i64,
    pub topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub owner_pubkey: Pubkey,
    pub title: String,
    pub description: String,
    pub status: LiveSessionStatus,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// 映像・音声を送る配信者の端末の endpoint id(ADR 0067 §8)。無い session は media を持たない。
    /// 旧版は知らない項目として読み飛ばす。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_endpoint_id: Option<String>,
}

/// 配信の映像・音声の broadcast の path(ADR 0067 §9)。
pub fn live_media_broadcast_path(session_id: &str) -> String {
    format!("live/{session_id}")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSessionStateDocV1 {
    pub session_id: String,
    pub topic_id: TopicId,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    pub owner_pubkey: Pubkey,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: LiveSessionStatus,
    pub current_manifest: ManifestBlobRef,
    pub last_envelope_id: EnvelopeId,
}

pub fn build_live_session_envelope<T: Serialize>(
    keys: &crate::KukuriKeys,
    topic: &TopicId,
    session_id: &str,
    content: &T,
) -> Result<crate::KukuriEnvelope> {
    crate::sign_envelope_json(
        keys,
        "live-session",
        vec![
            vec!["topic".into(), topic.as_str().into()],
            vec!["object".into(), "live-session".into()],
            vec!["session_id".into(), session_id.to_string()],
        ],
        content,
    )
}
