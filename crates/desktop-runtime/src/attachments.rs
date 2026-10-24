use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image::imageops::FilterType;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat};
use kukuri_app_api::PendingAttachment;
use kukuri_core::{AssetRole, BlobHash, CustomReactionAssetSnapshotV1, ReactionKeyV1};

use crate::requests::{CreateAttachmentRequest, CustomReactionCropRect, ReactionKeyRequest};

#[cfg(target_family = "wasm")]
#[async_trait::async_trait]
pub trait WebAttachmentInput: Send + Sync {
    async fn read(&self, source: &crate::requests::WebAttachmentRef) -> Result<Vec<u8>>;
}

#[cfg(target_family = "wasm")]
static WEB_INPUT: std::sync::OnceLock<&'static dyn WebAttachmentInput> = std::sync::OnceLock::new();

#[cfg(target_family = "wasm")]
pub fn install_web_attachment_input(input: &'static dyn WebAttachmentInput) {
    let _ = WEB_INPUT.set(input);
}

pub(crate) async fn attachment_bytes(request: &CreateAttachmentRequest) -> Result<Vec<u8>> {
    #[cfg(target_family = "wasm")]
    if let Some(source) = &request.source_ref {
        return WEB_INPUT
            .get()
            .context("Web attachment input is unavailable")?
            .read(source)
            .await;
    }
    BASE64_STANDARD
        .decode(request.data_base64.as_bytes())
        .map_err(Into::into)
}

pub(crate) async fn pending_attachment_from_request(
    request: CreateAttachmentRequest,
) -> Result<PendingAttachment> {
    anyhow::ensure!(
        request.bookmarked_media_hash.is_none(),
        "bookmarked media is supported only for posts"
    );
    let bytes = attachment_bytes(&request)
        .await
        .context("failed to decode attachment data")?;
    let role = match request.role.as_deref() {
        Some("image_preview") => AssetRole::ImagePreview,
        Some("video_poster") => AssetRole::VideoPoster,
        Some("video_manifest") => AssetRole::VideoManifest,
        Some("profile_avatar") => AssetRole::ProfileAvatar,
        Some("attachment") => AssetRole::Attachment,
        _ => AssetRole::ImageOriginal,
    };
    Ok(PendingAttachment {
        bookmarked_media_hash: None,
        mime: request.mime,
        bytes,
        role,
    })
}

pub(crate) struct NormalizedCustomReactionUpload {
    pub(crate) mime: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) async fn pending_attachments_from_requests(
    requests: Vec<CreateAttachmentRequest>,
) -> Result<Vec<PendingAttachment>> {
    let mut attachments = Vec::with_capacity(requests.len());
    for request in requests {
        attachments.push(pending_attachment_from_request(request).await?);
    }
    Ok(attachments)
}

pub(crate) fn reaction_key_from_request(request: ReactionKeyRequest) -> Result<ReactionKeyV1> {
    Ok(match request {
        ReactionKeyRequest::Emoji { emoji } => ReactionKeyV1::Emoji { emoji },
        ReactionKeyRequest::CustomAsset {
            asset_id,
            owner_pubkey,
            blob_hash,
            search_key,
            mime,
            bytes,
            width,
            height,
        } => ReactionKeyV1::CustomAsset {
            asset_id: asset_id.clone(),
            snapshot: CustomReactionAssetSnapshotV1 {
                asset_id,
                owner_pubkey: owner_pubkey.into(),
                blob_hash: BlobHash::new(blob_hash),
                search_key,
                mime,
                bytes,
                width,
                height,
            },
        },
    })
}

pub(crate) fn normalize_custom_reaction_upload(
    bytes: Vec<u8>,
    mime: &str,
    crop_rect: &CustomReactionCropRect,
) -> Result<NormalizedCustomReactionUpload> {
    if crop_rect.size == 0 {
        bail!("custom reaction crop size must be greater than zero");
    }
    if mime.trim() == "image/gif" {
        return normalize_custom_reaction_gif(bytes, crop_rect);
    }
    normalize_custom_reaction_static(bytes, crop_rect)
}

pub(crate) fn normalize_custom_reaction_static(
    bytes: Vec<u8>,
    crop_rect: &CustomReactionCropRect,
) -> Result<NormalizedCustomReactionUpload> {
    let image = image::load_from_memory(bytes.as_slice()).context("failed to decode image")?;
    validate_crop_rect(image.width(), image.height(), crop_rect)?;
    let cropped = crop_static_image(image, crop_rect);
    let mut out = std::io::Cursor::new(Vec::new());
    cropped
        .write_to(&mut out, ImageFormat::Png)
        .context("failed to encode normalized PNG")?;
    Ok(NormalizedCustomReactionUpload {
        mime: "image/png".into(),
        bytes: out.into_inner(),
    })
}

pub(crate) fn normalize_custom_reaction_gif(
    bytes: Vec<u8>,
    crop_rect: &CustomReactionCropRect,
) -> Result<NormalizedCustomReactionUpload> {
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
        .context("failed to decode GIF")?;
    let (width, height) = decoder.dimensions();
    validate_crop_rect(width, height, crop_rect)?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .context("failed to collect GIF frames")?;
    let normalized_frames = frames.into_iter().map(|frame| {
        let delay = frame.delay();
        let buffer = frame.into_buffer();
        let image = DynamicImage::ImageRgba8(buffer);
        let resized = crop_static_image(image, crop_rect).into_rgba8();
        image::Frame::from_parts(resized, 0, 0, delay)
    });
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
        encoder
            .encode_frames(normalized_frames)
            .context("failed to encode normalized GIF")?;
    }
    Ok(NormalizedCustomReactionUpload {
        mime: "image/gif".into(),
        bytes: out.into_inner(),
    })
}

pub(crate) fn crop_static_image(
    image: DynamicImage,
    crop_rect: &CustomReactionCropRect,
) -> DynamicImage {
    image
        .crop_imm(crop_rect.x, crop_rect.y, crop_rect.size, crop_rect.size)
        .resize_exact(128, 128, FilterType::Lanczos3)
}

pub(crate) fn validate_crop_rect(
    width: u32,
    height: u32,
    crop_rect: &CustomReactionCropRect,
) -> Result<()> {
    if crop_rect.x.saturating_add(crop_rect.size) > width
        || crop_rect.y.saturating_add(crop_rect.size) > height
    {
        bail!("custom reaction crop rectangle exceeds the source image bounds");
    }
    Ok(())
}

pub(crate) async fn pending_post_attachments_from_requests(
    requests: Vec<CreateAttachmentRequest>,
) -> Result<Vec<PendingAttachment>> {
    let mut result = Vec::with_capacity(requests.len());
    for request in requests {
        if let Some(hash) = request.bookmarked_media_hash {
            anyhow::ensure!(
                request.data_base64.is_empty(),
                "reused media cannot include upload bytes"
            );
            result.push(PendingAttachment {
                mime: request.mime,
                bytes: Vec::new(),
                role: AssetRole::Attachment,
                bookmarked_media_hash: Some(hash),
            });
        } else {
            result.push(pending_attachment_from_request(request).await?);
        }
    }
    Ok(result)
}
