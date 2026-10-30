//! ゲームの公式アセットの release(ADR 0070 §5.3、#1980)。発行者のアカウント鍵で署名した envelope を、発行者の制御領域の
//! application ID と asset ID から決まる key に置く。本体は blob で、hash と長さを release が持つ。

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::{BlobHash, KukuriEnvelope, Pubkey};

pub const GAME_ASSET_RELEASE_KIND: &str = "game-asset-release";
/// 1 件の上限(既存の blob の上限と同じ 3 GiB。2026-10-30 のユーザー判断)。
pub const GAME_ASSET_MAX_BYTES: u64 = 3 << 30;

/// 発行できる形式(2026-10-30 のユーザー判断)。AssetBundle は作った Unity の版と build の対象に縛られるので、それを記録する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GameAssetFormat {
    Glb,
    Png,
    Jpeg,
    AssetBundle {
        unity_version: String,
        build_target: String,
    },
}

impl GameAssetFormat {
    pub fn mime(&self) -> &'static str {
        match self {
            Self::Glb => "model/gltf-binary",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::AssetBundle { .. } => "application/vnd.unity.assetbundle",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Glb => "glb",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::AssetBundle { .. } => "bundle",
        }
    }

    /// file の先頭が形式の印と合うか(glb は `glTF` と版 2、PNG の signature、JPEG の SOI、AssetBundle の `UnityFS`)。
    pub fn matches_header(&self, head: &[u8]) -> bool {
        match self {
            Self::Glb => {
                head.len() >= 8 && &head[..4] == b"glTF" && head[4..8] == 2u32.to_le_bytes()
            }
            Self::Png => head.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Jpeg => head.starts_with(&[0xff, 0xd8, 0xff]),
            Self::AssetBundle { .. } => head.starts_with(b"UnityFS\0"),
        }
    }
}

/// 1 つのアセットの release。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameAssetReleaseV1 {
    pub application_id: String,
    pub asset_id: String,
    /// 同じアセットの release ごとに増える版。古い版を受けても戻さない。
    pub revision: u64,
    pub blob_hash: BlobHash,
    pub bytes: u64,
    pub format: GameAssetFormat,
    pub published_at: i64,
}

/// application ID は 1〜128 byte の、空白と制御文字を含まない ASCII(action session の room と共用。#1978)。
pub fn validate_application_id(application_id: &str) -> Result<()> {
    let id = application_id.as_bytes();
    if id.is_empty() || id.len() > 128 || !id.iter().all(u8::is_ascii_graphic) {
        bail!("application_id must be 1-128 printable ASCII characters without spaces");
    }
    Ok(())
}

/// asset ID は 1〜64 byte の英数字・`.`・`_`・`-`。
pub fn validate_asset_id(asset_id: &str) -> Result<()> {
    if asset_id.is_empty()
        || asset_id.len() > 64
        || !asset_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        bail!("asset_id must be 1-64 characters of letters, digits, '.', '_' or '-'");
    }
    Ok(())
}

fn validate_label(name: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 64 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        bail!("{name} must be 1-64 printable ASCII characters without spaces");
    }
    Ok(())
}

pub fn validate_game_asset_release(release: &GameAssetReleaseV1) -> Result<()> {
    validate_application_id(&release.application_id)?;
    validate_asset_id(&release.asset_id)?;
    if release.revision == 0 {
        bail!("game asset revision must be at least 1");
    }
    if release.bytes == 0 || release.bytes > GAME_ASSET_MAX_BYTES {
        bail!("game asset must be 1 byte to 3 GiB");
    }
    if let GameAssetFormat::AssetBundle {
        unity_version,
        build_target,
    } = &release.format
    {
        validate_label("unity_version", unity_version)?;
        validate_label("build_target", build_target)?;
    }
    Ok(())
}

pub fn build_game_asset_release_envelope(
    keys: &crate::KukuriKeys,
    release: &GameAssetReleaseV1,
) -> Result<KukuriEnvelope> {
    validate_game_asset_release(release)?;
    crate::sign_envelope_json(
        keys,
        GAME_ASSET_RELEASE_KIND,
        vec![
            vec!["application".into(), release.application_id.clone()],
            vec!["asset".into(), release.asset_id.clone()],
            vec!["revision".into(), release.revision.to_string()],
        ],
        release,
    )
}

/// 信頼する発行者が署名した、要求したアセットの release だけを返す(別の鍵・改変・別のアセットは error)。
pub fn verify_game_asset_release(
    envelope: &KukuriEnvelope,
    publisher: &Pubkey,
    application_id: &str,
    asset_id: &str,
) -> Result<GameAssetReleaseV1> {
    envelope.verify()?;
    if envelope.kind != GAME_ASSET_RELEASE_KIND || envelope.pubkey != *publisher {
        bail!("the game asset release is not signed by the trusted publisher");
    }
    let release: GameAssetReleaseV1 = serde_json::from_str(&envelope.content)?;
    validate_game_asset_release(&release)?;
    if release.application_id != application_id || release.asset_id != asset_id {
        bail!("the game asset release is for another asset");
    }
    Ok(release)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KukuriKeys;

    fn release(revision: u64) -> GameAssetReleaseV1 {
        GameAssetReleaseV1 {
            application_id: "com.kukuri.test".into(),
            asset_id: "skin.red".into(),
            revision,
            blob_hash: BlobHash::new("a".repeat(64)),
            bytes: 10,
            format: GameAssetFormat::Glb,
            published_at: 1,
        }
    }

    /// F-AST-2: 信頼する発行者の署名だけを受け、別の鍵・改変・別のアセットの release は拒む。
    #[test]
    fn only_the_trusted_publishers_unmodified_release_verifies() {
        let publisher = KukuriKeys::generate();
        let other = KukuriKeys::generate();
        let envelope = build_game_asset_release_envelope(&publisher, &release(2)).unwrap();
        let trusted = publisher.public_key();
        assert_eq!(
            verify_game_asset_release(&envelope, &trusted, "com.kukuri.test", "skin.red").unwrap(),
            release(2)
        );

        let by_other = build_game_asset_release_envelope(&other, &release(3)).unwrap();
        assert!(
            verify_game_asset_release(&by_other, &trusted, "com.kukuri.test", "skin.red").is_err()
        );

        let mut tampered = envelope.clone();
        tampered.content = serde_json::to_string(&release(9)).unwrap();
        assert!(
            verify_game_asset_release(&tampered, &trusted, "com.kukuri.test", "skin.red").is_err()
        );

        assert!(
            verify_game_asset_release(&envelope, &trusted, "com.kukuri.test", "skin.blue").is_err()
        );
        assert!(verify_game_asset_release(&envelope, &trusted, "com.other", "skin.red").is_err());
    }

    #[test]
    fn formats_check_their_file_header_and_bundles_record_the_unity_build() {
        assert!(GameAssetFormat::Glb.matches_header(b"glTF\x02\0\0\0rest"));
        assert!(!GameAssetFormat::Glb.matches_header(b"glTF\x01\0\0\0rest"));
        assert!(GameAssetFormat::Png.matches_header(b"\x89PNG\r\n\x1a\nrest"));
        assert!(GameAssetFormat::Jpeg.matches_header(&[0xff, 0xd8, 0xff, 0xe0]));
        assert!(!GameAssetFormat::Jpeg.matches_header(b"\x89PNG"));
        let bundle = GameAssetFormat::AssetBundle {
            unity_version: "6000.3.27f1".into(),
            build_target: "StandaloneWindows64".into(),
        };
        assert!(bundle.matches_header(b"UnityFS\0\0\0\0\x08"));
        let mut unlabeled = release(1);
        unlabeled.format = GameAssetFormat::AssetBundle {
            unity_version: String::new(),
            build_target: "StandaloneWindows64".into(),
        };
        assert!(validate_game_asset_release(&unlabeled).is_err());
        let mut huge = release(1);
        huge.bytes = GAME_ASSET_MAX_BYTES + 1;
        assert!(validate_game_asset_release(&huge).is_err());
        assert!(validate_asset_id("skins/red").is_err());
    }
}
