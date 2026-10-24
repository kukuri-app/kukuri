use super::super::*;

pub(crate) fn png_source_bytes() -> Vec<u8> {
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(320, 180, Rgba([0, 179, 164, 255])));
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

pub(crate) fn animated_gif_source_bytes() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
        let frames = vec![
            Frame::from_parts(
                RgbaImage::from_pixel(4, 2, Rgba([255, 0, 0, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(40, 1),
            ),
            Frame::from_parts(
                RgbaImage::from_pixel(4, 2, Rgba([0, 0, 255, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(40, 1),
            ),
        ];
        encoder.encode_frames(frames).expect("encode gif");
    }
    out.into_inner()
}
pub(crate) fn image_attachment_request(
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> CreateAttachmentRequest {
    CreateAttachmentRequest {
        bookmarked_media_hash: None,
        file_name: Some(name.to_string()),
        mime: mime.to_string(),
        byte_size: bytes.len() as u64,
        data_base64: BASE64_STANDARD.encode(bytes),
        role: Some("image_original".to_string()),
    }
}

pub(crate) fn profile_avatar_attachment_request(
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> CreateAttachmentRequest {
    CreateAttachmentRequest {
        bookmarked_media_hash: None,
        file_name: Some(name.to_string()),
        mime: mime.to_string(),
        byte_size: bytes.len() as u64,
        data_base64: BASE64_STANDARD.encode(bytes),
        role: Some("profile_avatar".to_string()),
    }
}

pub(crate) fn video_attachment_request(
    name: &str,
    mime: &str,
    bytes: &[u8],
    role: &str,
) -> CreateAttachmentRequest {
    CreateAttachmentRequest {
        bookmarked_media_hash: None,
        file_name: Some(name.to_string()),
        mime: mime.to_string(),
        byte_size: bytes.len() as u64,
        data_base64: BASE64_STANDARD.encode(bytes),
        role: Some(role.to_string()),
    }
}
