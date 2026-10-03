//! 公開投稿のリンクプレビューの record(ADR 0051 §7、#1220 W8 AC-2d)。
//!
//! 投稿者本人の native が取得した OGP を、投稿者の署名つき envelope にして投稿と同じ replica に置く。読む側は投稿者の署名、
//! 対象の投稿、URL の一致、上限を確かめてから表示する。上限は native の取得(ADR 0051 §4・§5)と同じ値。

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{BlobHash, EnvelopeId, KukuriEnvelope, KukuriKeys, Pubkey};

pub const LINK_PREVIEW_KIND: &str = "link_preview";
pub const LINK_PREVIEW_MAX_URL_BYTES: usize = 4_096;
pub const LINK_PREVIEW_MAX_TITLE_CHARS: usize = 200;
pub const LINK_PREVIEW_MAX_DESCRIPTION_CHARS: usize = 500;
pub const LINK_PREVIEW_MAX_SITE_NAME_CHARS: usize = 100;
pub const LINK_PREVIEW_MAX_IMAGE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriLinkPreviewImageV1 {
    pub hash: BlobHash,
    pub mime: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriLinkPreviewContentV1 {
    pub target_object_id: EnvelopeId,
    /// 投稿の本文の先頭の URL(本文に書かれた文字列のまま)。
    pub url: String,
    pub title: String,
    pub description: Option<String>,
    pub site_name: String,
    pub image: Option<KukuriLinkPreviewImageV1>,
}

/// 先頭の bytes から、表示してよい raster 画像の MIME を返す(PNG・JPEG・GIF・WebP。ADR 0051 §5)。
pub fn link_preview_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn validate(content: &KukuriLinkPreviewContentV1) -> Result<()> {
    let url = content.url.as_str();
    ensure!(
        (url.starts_with("https://") || url.starts_with("http://"))
            && url.len() <= LINK_PREVIEW_MAX_URL_BYTES,
        "link preview url is invalid"
    );
    let within = |value: &str, max: usize| !value.trim().is_empty() && value.chars().count() <= max;
    ensure!(
        within(&content.title, LINK_PREVIEW_MAX_TITLE_CHARS)
            && within(&content.site_name, LINK_PREVIEW_MAX_SITE_NAME_CHARS)
            && content
                .description
                .as_deref()
                .is_none_or(|value| within(value, LINK_PREVIEW_MAX_DESCRIPTION_CHARS)),
        "link preview text exceeds the limit"
    );
    if let Some(image) = &content.image {
        ensure!(
            ["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&image.mime.as_str())
                && (1..=LINK_PREVIEW_MAX_IMAGE_BYTES).contains(&image.bytes),
            "link preview image exceeds the limit"
        );
    }
    Ok(())
}

/// 投稿者の鍵で record に署名する。対象の投稿の著者が署名者であることは呼び出し側が確かめる。
pub fn build_link_preview_envelope(
    keys: &KukuriKeys,
    content: &KukuriLinkPreviewContentV1,
) -> Result<KukuriEnvelope> {
    validate(content)?;
    let tags = vec![
        vec!["object".into(), LINK_PREVIEW_KIND.into()],
        vec![
            "target".into(),
            content.target_object_id.as_str().to_string(),
        ],
    ];
    crate::sign_envelope_json(keys, LINK_PREVIEW_KIND, tags, content)
}

/// 投稿者の署名、対象の投稿、URL の一致、上限を確かめる。
pub fn verify_link_preview(
    envelope: &KukuriEnvelope,
    target_object_id: &EnvelopeId,
    target_author: &Pubkey,
    url: &str,
) -> Result<KukuriLinkPreviewContentV1> {
    envelope.verify()?;
    if envelope.kind != LINK_PREVIEW_KIND || envelope.pubkey != *target_author {
        bail!("link preview is not signed by the post author");
    }
    let content: KukuriLinkPreviewContentV1 = serde_json::from_str(&envelope.content)?;
    if content.target_object_id != *target_object_id || content.url != url {
        bail!("link preview does not match the post");
    }
    validate(&content)?;
    Ok(content)
}
