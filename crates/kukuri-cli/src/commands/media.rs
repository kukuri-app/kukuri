use std::{fs::File, io::Read, path::PathBuf};

use base64::Engine;
use kukuri_desktop_runtime::CreateAttachmentRequest;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::protocol::{ProtocolError, error_code};

/// 明示したファイルの内容をhashで固定し、再送時の別内容送信を防ぐ。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileAttachment {
    path: PathBuf,
    hash: String,
    byte_size: u64,
    mime: String,
    file_name: Option<String>,
    role: Option<String>,
}

pub(super) fn input_schema() -> Value {
    super::schema::object(
        json!({
            "path": {"type": "string", "description": "読み込む通常ファイルの絶対path"},
            "hash": {"type": "string", "minLength": 64, "maxLength": 64,
                "description": "ファイル全体のBLAKE3 hash（小文字hex）"},
            "byte_size": {"type": "integer", "minimum": 0},
            "mime": {"type": "string"}, "file_name": {"type": "string"},
            "role": {"type": "string"}
        }),
        &["path", "hash", "byte_size", "mime"],
    )
}

pub(super) async fn load_attachments(
    files: Vec<FileAttachment>,
) -> Result<Vec<CreateAttachmentRequest>, ProtocolError> {
    tokio::task::spawn_blocking(move || files.into_iter().map(load_attachment).collect())
        .await
        .map_err(|_| {
            ProtocolError::new(
                error_code::INTERNAL_ERROR,
                "添付ファイルの読込みに失敗しました",
            )
        })?
}

pub(super) async fn decode_with_files<T: serde::de::DeserializeOwned>(
    mut payload: Value,
    field: &str,
    multiple: bool,
) -> Result<T, ProtocolError> {
    if let Some(value) = payload.get(field).filter(|value| !value.is_null()).cloned() {
        let files = if multiple {
            super::decode(value)?
        } else {
            vec![super::decode(value)?]
        };
        let attachments = load_attachments(files).await?;
        payload[field] = if multiple {
            serde_json::to_value(attachments)
        } else {
            serde_json::to_value(attachments.into_iter().next().expect("single attachment"))
        }
        .map_err(|_| {
            ProtocolError::new(error_code::INTERNAL_ERROR, "添付入力の変換に失敗しました")
        })?;
    }
    super::decode(payload)
}

fn load_attachment(file: FileAttachment) -> Result<CreateAttachmentRequest, ProtocolError> {
    let invalid = || {
        ProtocolError::new(
            error_code::VALIDATION_FAILED,
            "添付ファイルのpath、byte_sizeまたはhashが一致しません",
        )
    };
    if !file.path.is_absolute()
        || file.hash.len() != 64
        || !file
            .hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    let source = File::open(&file.path).map_err(|_| invalid())?;
    let metadata = source.metadata().map_err(|_| invalid())?;
    if !metadata.is_file() || metadata.len() != file.byte_size {
        return Err(invalid());
    }
    // metadata確認後の追記でも無制限に読み込まない。内容変更はhashで検出する。
    let limit = file.byte_size.checked_add(1).ok_or_else(invalid)?;
    let mut bytes = Vec::new();
    source
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() as u64 != file.byte_size || blake3::hash(&bytes).to_hex().as_str() != file.hash {
        return Err(invalid());
    }
    Ok(CreateAttachmentRequest {
        bookmarked_media_hash: None,
        file_name: file.file_name,
        mime: file.mime,
        byte_size: file.byte_size,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        role: file.role,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_reference_pins_bytes_and_never_includes_content_in_errors() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("attachment");
        let content = b"private-attachment-sentinel";
        std::fs::write(&path, content).unwrap();
        let reference = || FileAttachment {
            path: path.clone(),
            hash: blake3::hash(content).to_hex().to_string(),
            byte_size: content.len() as u64,
            mime: "image/png".into(),
            file_name: None,
            role: None,
        };
        let loaded = load_attachment(reference()).unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(loaded.data_base64)
                .unwrap(),
            content
        );
        std::fs::write(&path, vec![b'x'; content.len()]).unwrap();
        let error = load_attachment(reference()).unwrap_err();
        assert_eq!(error.code, error_code::VALIDATION_FAILED);
        assert!(!error.message.contains("sentinel"));
        assert!(!error.message.contains(&path.to_string_lossy().to_string()));
        let mut relative = reference();
        relative.path = "attachment".into();
        assert!(load_attachment(relative).is_err());
        let mut directory = reference();
        directory.path = root.path().to_path_buf();
        assert!(load_attachment(directory).is_err());
    }
}
