use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{BlobHash, Pubkey};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRef {
    pub hash: BlobHash,
    pub mime: String,
    pub bytes: u64,
    pub role: AssetRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<MediaOrigin>,
}

/// 再利用する添付の参照元。最初の作者であることの証明ではない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MediaOrigin {
    pub topic_id: String,
    pub object_id: String,
    pub author_pubkey: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestBlobRef {
    pub hash: BlobHash,
    pub mime: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetRole {
    ImageOriginal,
    ImagePreview,
    VideoPoster,
    VideoManifest,
    ProfileAvatar,
    Attachment,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaManifestItem {
    pub blob_hash: BlobHash,
    pub mime: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub codec: Option<String>,
    pub thumbnail_blob_hash: Option<BlobHash>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriMediaManifestV1 {
    pub manifest_id: String,
    pub owner_pubkey: Pubkey,
    pub created_at: i64,
    pub items: Vec<MediaManifestItem>,
}

pub const LIVE_MANIFEST_MIME: &str = "application/vnd.kukuri.live-manifest+json";
pub const GAME_MANIFEST_MIME: &str = "application/vnd.kukuri.game-manifest+json";
pub const DOME_PRESET_MANIFEST_MIME: &str = "application/vnd.kukuri.dome-preset+json";
pub const DOME_INSTANCE_MANIFEST_MIME: &str = "application/vnd.kukuri.dome-instance+json";

pub fn blob_hash(data: impl AsRef<[u8]>) -> BlobHash {
    BlobHash::new(blake3::hash(data.as_ref()).to_hex().to_string())
}

pub fn build_media_manifest_envelope(
    keys: &crate::KukuriKeys,
    topic: &crate::TopicId,
    manifest: &KukuriMediaManifestV1,
) -> Result<crate::KukuriEnvelope> {
    crate::sign_envelope_json(
        keys,
        "media-manifest",
        vec![
            vec!["topic".into(), topic.as_str().into()],
            vec!["object".into(), "media-manifest".into()],
            vec!["manifest_id".into(), manifest.manifest_id.clone()],
        ],
        manifest,
    )
}

#[cfg(test)]
mod bookmark_contract {
    use super::*;

    #[test]
    fn media_origin_is_optional_and_survives_serialization() {
        let legacy = r#"{"hash":"abc","mime":"image/png","bytes":3,"role":"ImageOriginal"}"#;
        let mut asset: AssetRef = serde_json::from_str(legacy).unwrap();
        assert!(asset.origin.is_none());
        assert_eq!(serde_json::to_string(&asset).unwrap(), legacy);
        asset.origin = Some(MediaOrigin {
            topic_id: "topic".into(),
            object_id: "source".into(),
            author_pubkey: "a".repeat(64),
            created_at: 1,
        });
        let json = serde_json::to_string(&asset).unwrap();
        assert_eq!(serde_json::from_str::<AssetRef>(&json).unwrap(), asset);
    }
}
