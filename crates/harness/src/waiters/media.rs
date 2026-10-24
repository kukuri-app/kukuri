use crate::*;

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
