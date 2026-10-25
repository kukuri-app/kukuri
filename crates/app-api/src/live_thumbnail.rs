use crate::service::*;
use kukuri_core::{KukuriEnvelope, sign_envelope_json};
use serde::Deserialize;

const KIND: &str = "live-thumbnail";
const MAX_IMAGE_CHARS: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SetLiveThumbnailInput {
    pub topic: String,
    pub session_id: String,
    pub data_url: Option<String>,
    // 手動設定・解除。自動更新は手動画像を上書きしない。
    pub custom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LiveThumbnailView {
    pub data_url: Option<String>,
    pub custom: bool,
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize)]
struct ThumbnailRecord {
    input: SetLiveThumbnailInput,
    updated_at: i64,
}

fn validate_image(data_url: Option<&str>) -> Result<()> {
    if let Some(data_url) = data_url {
        anyhow::ensure!(
            data_url.len() <= MAX_IMAGE_CHARS,
            "live thumbnail is too large"
        );
        let encoded = data_url
            .strip_prefix("data:image/jpeg;base64,")
            .ok_or_else(|| anyhow::anyhow!("live thumbnail must be JPEG"))?;
        let image = BASE64_STANDARD.decode(encoded)?;
        anyhow::ensure!(
            image.starts_with(&[0xff, 0xd8]) && image.ends_with(&[0xff, 0xd9]),
            "invalid live thumbnail"
        );
        let (width, height) =
            image::ImageReader::with_format(std::io::Cursor::new(image), image::ImageFormat::Jpeg)
                .into_dimensions()?;
        anyhow::ensure!(
            width > 0 && height > 0 && width <= 384 && height <= 216,
            "invalid live thumbnail dimensions"
        );
    }
    Ok(())
}

impl AppService {
    /// 同じ公開範囲の、配信者が署名した最新の画像だけを読む。視聴への参加は行わない。
    pub async fn get_live_thumbnail(
        &self,
        topic: &str,
        session_id: &str,
    ) -> Result<Option<LiveThumbnailView>> {
        if self.get_live_session(topic, session_id).await?.is_none() {
            return Ok(None);
        }
        let Some((replica, state, _)) = self
            .fetch_live_session_state_and_manifest(topic, session_id)
            .await?
        else {
            return Ok(None);
        };
        let owner = state.owner_pubkey.as_str();
        let key = format!("media/live/{session_id}/thumbnail");
        read_local_then_remote(
            self.services.docs_sync.clone(),
            async {
                self.remote_post_readers(
                    topic,
                    state.channel_id.as_ref().map(ChannelId::as_str),
                    &replica,
                    None,
                )
                .await
                .unwrap_or_default()
            },
            |docs: Arc<dyn DocsSync>, policy| {
                let (replica, key) = (replica.clone(), key.clone());
                async move {
                    let records = docs
                        .query_replica_exact_bounded(
                            &replica,
                            &key,
                            MAX_ENVELOPE_RECORDS_PER_OBJECT,
                            policy,
                        )
                        .await?;
                    let mut latest: Option<LiveThumbnailView> = None;
                    for record in records {
                        if record.value.len() > MAX_IMAGE_CHARS + 4_096 {
                            continue;
                        }
                        let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(&record.value)
                        else {
                            continue;
                        };
                        if envelope.kind != KIND
                            || envelope.pubkey.as_str() != owner
                            || envelope.verify().is_err()
                        {
                            continue;
                        }
                        let Ok(record) = serde_json::from_str::<ThumbnailRecord>(&envelope.content)
                        else {
                            continue;
                        };
                        if record.input.topic != topic
                            || record.input.session_id != session_id
                            || validate_image(record.input.data_url.as_deref()).is_err()
                        {
                            continue;
                        }
                        if latest
                            .as_ref()
                            .is_none_or(|previous| record.updated_at > previous.updated_at)
                        {
                            latest = Some(LiveThumbnailView {
                                data_url: record.input.data_url,
                                custom: record.input.custom,
                                updated_at: record.updated_at,
                            });
                        }
                    }
                    Ok(latest)
                }
            },
        )
        .await
    }

    /// 画像は固定keyの1件を置き換える。更新ごとの投稿・manifest・blobは作らない。
    pub async fn set_live_thumbnail(&self, mut input: SetLiveThumbnailInput) -> Result<()> {
        validate_image(input.data_url.as_deref())?;
        let _guard = self
            .services
            .live_session_projections
            .lock(&input.session_id)
            .await;
        let (replica, state, manifest) = self
            .fetch_live_session_state_and_manifest(&input.topic, &input.session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("live session not found"))?;
        anyhow::ensure!(
            state.owner_pubkey.as_str() == self.current_author_pubkey(),
            "only the broadcaster can set the thumbnail"
        );
        anyhow::ensure!(
            manifest.status == LiveSessionStatus::Live,
            "live session has ended"
        );
        if !input.custom
            && self
                .get_live_thumbnail(&input.topic, &input.session_id)
                .await?
                .is_some_and(|image| image.custom)
        {
            return Ok(());
        }
        input.custom = input.custom && input.data_url.is_some();
        let key = format!("media/live/{}/thumbnail", input.session_id);
        let record = ThumbnailRecord {
            input,
            updated_at: Utc::now().timestamp_millis(),
        };
        let envelope = sign_envelope_json(self.keys(), KIND, Vec::new(), &record)?;
        self.docs_sync()
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key,
                    value: serde_json::to_value(envelope)?,
                },
            )
            .await
    }
}
