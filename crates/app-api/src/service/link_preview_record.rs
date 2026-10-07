//! 公開投稿のリンクプレビューの record(ADR 0051 §7、#1220 W8 AC-2d)。
//!
//! 書くのは投稿者本人の native だけで、表示のときに取得した OGP を、投稿と同じ replica の
//! `link-previews/<object id>/state` へ投稿者の署名つきで置く。読む側(Web の表示と、native・Web の中継。#1220 AC-2f)は
//! 投稿者の docs author と key の組で 1 件読み、投稿者の署名・URL の一致・上限を確かめる。検証した record と画像は
//! 手元の cache に保持し、他の参加者へ提供する(#1395 と同じ)。

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

/// 同時に読む record の数と、待つものを含めた読取りの数の上限(native の取得と同じ。ADR 0051 §4)。
const MAX_CONCURRENT_READS: usize = 4;
const MAX_PENDING_READS: usize = 32;

/// record の読取りの上限。待つものを含めて上限を超えた読取りは、待たずに断る(画面は URL だけを示す)。
pub(crate) struct LinkPreviewReads {
    pub(super) permits: tokio::sync::Semaphore,
    pending: std::sync::atomic::AtomicUsize,
}

impl Default for LinkPreviewReads {
    fn default() -> Self {
        Self {
            permits: tokio::sync::Semaphore::new(MAX_CONCURRENT_READS),
            pending: Default::default(),
        }
    }
}

/// 受け付けた読取り。終わったら(drop で)数を戻す。
pub(super) struct PendingRead<'a>(&'a std::sync::atomic::AtomicUsize);

impl Drop for PendingRead<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

impl LinkPreviewReads {
    pub(super) fn admit(&self) -> Option<PendingRead<'_>> {
        use std::sync::atomic::Ordering;
        self.pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                (pending < MAX_PENDING_READS).then_some(pending + 1)
            })
            .ok()
            .map(|_| PendingRead(&self.pending))
    }
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
                // 公開投稿のリンクプレビュー画像は公開参照にし、DHT で告知・検索する(#1632 D3)。
                self.services
                    .projection_store
                    .note_link_preview_image(object_id.as_str(), stored.hash.as_str())
                    .await?;
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
    /// 読取りの数が上限を超えたときは、待たずにエラーを返す(画面は結果を持たず、次の表示で読み直す)。
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
        let reads = self.services.link_preview_reads.as_ref();
        let _pending = reads.admit().context("link preview reads are busy")?;
        let _permit = reads.permits.acquire().await?;
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
                        Ok(content) => {
                            // 検証した record を保持し、他の参加者へ提供する(#1395)。
                            if let Err(error) = source
                                .persist_verified_record(&replica, &key, Some(&docs_author), &[])
                                .await
                            {
                                warn!(%error, "failed to keep a link preview record");
                            }
                            Ok(Some((content, source.remote_reader_id())))
                        }
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
            Some(image) => {
                // 検証した record の画像は、取得の前に公開参照にする(取得で DHT の保持端末を探せる。#1632 D3)。
                if let Err(error) = self
                    .services
                    .projection_store
                    .note_link_preview_image(object_id.as_str(), image.hash.as_str())
                    .await
                {
                    warn!(%error, "failed to note a link preview image as public");
                }
                self.link_preview_image(image, provider.as_deref()).await
            }
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

    /// record が指す画像を、手元に保持していればそれを使い、無ければ record の bytes 数までだけ取得する。bytes 数と形式が
    /// 一致するときだけ data URL にする。取得した画像は保持し、他の参加者へ提供する(#1395)。
    async fn link_preview_image(
        &self,
        image: &KukuriLinkPreviewImageV1,
        provider: Option<&str>,
    ) -> Option<String> {
        let blobs = self.services.blob_service.as_ref();
        let (bytes, fetched) = match blobs.fetch_local_blob(&image.hash).await.ok().flatten() {
            Some(bytes) => (bytes, false),
            None => {
                if let Some(provider) = provider {
                    let _ = blobs.learn_content_source(&image.hash, provider).await;
                }
                let bytes = blobs
                    .fetch_blob_ephemeral_bounded(&image.hash, image.bytes)
                    .await
                    .ok()
                    .flatten()?;
                (bytes, true)
            }
        };
        if bytes.len() as u64 != image.bytes
            || link_preview_image_mime(&bytes) != Some(image.mime.as_str())
        {
            return None;
        }
        let data_url = format!(
            "data:{};base64,{}",
            image.mime,
            BASE64_STANDARD.encode(&bytes)
        );
        if fetched && let Err(error) = blobs.put_remote_blob(bytes, &image.mime).await {
            warn!(%error, "failed to keep a link preview image");
        }
        Some(data_url)
    }
}
