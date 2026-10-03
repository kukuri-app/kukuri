//! 公開投稿のリンクプレビューの record(ADR 0051 §7、#1220 W8 AC-2d)。
//!
//! 書くのは投稿者本人の native だけで、表示のときに取得した OGP を、投稿と同じ replica の
//! `link-previews/<object id>/state` へ投稿者の署名つきで置く。Web は投稿者の docs author と key の組で 1 件読み、
//! 投稿者の署名・URL の一致・上限を確かめてから表示する。

use super::remote_read_support::{read_local_then_remote, writer_readers};
use super::*;
use kukuri_core::{
    KukuriLinkPreviewContentV1, KukuriLinkPreviewImageV1, LINK_PREVIEW_MAX_IMAGE_BYTES,
    build_link_preview_envelope, link_preview_image_mime, verify_link_preview,
};

/// native が取得したリンクプレビュー(画像は取得した bytes)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkPreviewRecordInput {
    pub url: String,
    pub title: String,
    pub description: Option<String>,
    pub site_name: String,
    pub image: Option<Vec<u8>>,
}

/// 検証した record の表示の形(native の `fetch_link_preview` の preview と同じ field)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LinkPreviewRecordView {
    pub url: String,
    pub source_label: String,
    pub title: String,
    pub description: Option<String>,
    pub image_data_url: Option<String>,
}

fn link_preview_key(object_id: &EnvelopeId) -> String {
    stable_key("link-previews", &format!("{}/state", object_id.as_str()))
}

impl AppService {
    /// 自分の公開投稿で、本文に `url` があるときだけ record を書く。record は投稿ごとに 1 回だけ書き、書いた後に
    /// 取得し直しても書き換えない(取得のたびに画像の blob を増やさない)。
    pub async fn record_link_preview(
        &self,
        object_id: &str,
        input: LinkPreviewRecordInput,
    ) -> Result<()> {
        let object_id = EnvelopeId::from(object_id);
        let row = self
            .services
            .projection_store
            .get_object_projection(&object_id)
            .await?
            .context("link preview target was not found")?;
        let keys = self.services.keys.as_ref();
        anyhow::ensure!(
            row.author_pubkey == keys.public_key_hex() && row.channel_id == PUBLIC_CHANNEL_ID,
            "link preview target must be an own public post"
        );
        anyhow::ensure!(
            row.content
                .as_deref()
                .is_some_and(|content| content.contains(&input.url)),
            "link preview url is not in the post"
        );
        let docs_sync = self.services.docs_sync.as_ref();
        let replica = &row.source_replica_id;
        let key = link_preview_key(&object_id);
        let docs_author = docs_sync
            .local_docs_author()
            .await?
            .context("the docs author is unknown")?;
        if docs_sync
            .query_replica_by_author(replica, &docs_author, &key, DocFetchPolicy::LocalOnly)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let image = match input.image.and_then(|bytes| {
            let mime = link_preview_image_mime(&bytes)?;
            (bytes.len() as u64 <= LINK_PREVIEW_MAX_IMAGE_BYTES).then_some((bytes, mime))
        }) {
            Some((bytes, mime)) => {
                let stored = self.services.blob_service.put_blob(bytes, mime).await?;
                Some(KukuriLinkPreviewImageV1 {
                    hash: stored.hash,
                    mime: mime.to_string(),
                    bytes: stored.bytes,
                })
            }
            None => None,
        };
        let envelope = build_link_preview_envelope(
            keys,
            &KukuriLinkPreviewContentV1 {
                target_object_id: object_id,
                url: input.url,
                title: input.title,
                description: input.description,
                site_name: input.site_name,
                image,
            },
        )?;
        docs_sync.open_replica(replica).await?;
        docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key,
                    value: serde_json::to_value(&envelope)?,
                },
            )
            .await
    }

    /// 表示中の公開投稿の record を、投稿者の docs author と key の組で 1 件読む。手元に無ければ投稿者と topic の参加者から
    /// 読む。record が無い・検証に通らないときは `None`(画面は URL だけを示す)。画像だけが取れないときは文字だけを返す。
    pub async fn link_preview_record(
        &self,
        object_id: &str,
        url: &str,
    ) -> Result<Option<LinkPreviewRecordView>> {
        let object_id = EnvelopeId::from(object_id);
        let Some(row) = self
            .services
            .projection_store
            .get_object_projection(&object_id)
            .await?
        else {
            return Ok(None);
        };
        let Some(docs_author) = row.source_docs_author.clone() else {
            return Ok(None);
        };
        if row.channel_id != PUBLIC_CHANNEL_ID {
            return Ok(None);
        }
        let replica = row.source_replica_id.clone();
        let key = link_preview_key(&object_id);
        let author = Pubkey::from(row.author_pubkey.as_str());
        let peers = self
            .hint_transport()
            .topic_read_candidates(&TopicId::new(row.topic_id.as_str()))
            .await
            .unwrap_or_default();
        let found = read_local_then_remote(
            self.services.docs_sync.clone(),
            writer_readers(
                &self.services,
                &replica,
                &[row.author_pubkey.as_str()],
                None,
                peers,
            ),
            |source: Arc<dyn DocsSync>, policy| {
                let (replica, docs_author, key) =
                    (replica.clone(), docs_author.clone(), key.clone());
                let (object_id, author, url) = (object_id.clone(), author.clone(), url.to_string());
                async move {
                    let Some(record) = source
                        .query_replica_by_author(&replica, &docs_author, &key, policy)
                        .await?
                    else {
                        return Ok(None);
                    };
                    match serde_json::from_slice::<KukuriEnvelope>(&record.value)
                        .map_err(anyhow::Error::from)
                        .and_then(|envelope| {
                            verify_link_preview(&envelope, &object_id, &author, &url)
                        }) {
                        Ok(content) => Ok(Some((content, source.remote_reader_id()))),
                        Err(error) => {
                            warn!(%error, "ignored an invalid link preview record");
                            Ok(None)
                        }
                    }
                }
            },
        )
        .await?;
        let Some((content, provider)) = found else {
            return Ok(None);
        };
        let image_data_url = match &content.image {
            Some(image) => self.link_preview_image(image, provider.as_deref()).await,
            None => None,
        };
        Ok(Some(LinkPreviewRecordView {
            url: content.url,
            source_label: content.site_name,
            title: content.title,
            description: content.description,
            image_data_url,
        }))
    }

    /// record が指す画像を、record の bytes 数までだけ取得し、bytes 数と形式が一致するときだけ data URL にする。
    async fn link_preview_image(
        &self,
        image: &KukuriLinkPreviewImageV1,
        provider: Option<&str>,
    ) -> Option<String> {
        let blobs = self.services.blob_service.as_ref();
        if let Some(provider) = provider {
            let _ = blobs.learn_content_source(&image.hash, provider).await;
        }
        let bytes = blobs
            .fetch_blob_ephemeral_bounded(&image.hash, image.bytes)
            .await
            .ok()
            .flatten()?;
        (bytes.len() as u64 == image.bytes
            && link_preview_image_mime(&bytes) == Some(image.mime.as_str()))
        .then(|| {
            format!(
                "data:{};base64,{}",
                image.mime,
                BASE64_STANDARD.encode(&bytes)
            )
        })
    }
}
