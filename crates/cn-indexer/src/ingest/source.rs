//! Read-only source resolution. Scan policy, metrics and index mutation stay in the pipeline.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use kukuri_blob_service::BlobService;
use kukuri_iroh_node::remote_fetch::BlobTooLarge;

use super::{RECORDS_PER_EXACT_KEY, failure::transient};
use kukuri_core::{
    AssetRef, KukuriEnvelope, KukuriMediaManifestV1, ObjectStatus, PayloadRef, ReplicaId, blob_hash,
};
use kukuri_docs_sync::{DocFetchPolicy, DocRecord, DocsSync, SharedReplicaKeyFamily};

/// app-api の投稿上限（10,000 Unicode scalar values）を UTF-8 bytes でも有界にする。
const MAX_INDEXABLE_POST_BODY_CHARS: usize = 10_000;
const MAX_INDEXABLE_POST_BODY_BYTES: u64 = (MAX_INDEXABLE_POST_BODY_CHARS as u64) * 4;

pub(super) struct SourceResolver<'a> {
    docs_sync: &'a dyn DocsSync,
    blob_service: Option<&'a dyn BlobService>,
}

impl<'a> SourceResolver<'a> {
    pub(super) fn new(
        docs_sync: &'a dyn DocsSync,
        blob_service: Option<&'a dyn BlobService>,
    ) -> Self {
        Self {
            docs_sync,
            blob_service,
        }
    }

    /// scan 対象の media 参照を blob 単位（hash + mime）に展開する（#609）。
    ///
    /// attachments は `AssetRef` から直接、`media_manifest_refs` は replica 上の署名済み
    /// manifest（`manifests/media/<id>/envelope`）を解決して items（+ thumbnail）に展開する。
    /// 同一 blob hash は先勝ちで dedup する。manifest の欠落・検証失敗は Err
    /// （呼び出し側が index しない = fail-closed）。
    pub(super) async fn media_scan_targets(
        &self,
        replica_id: &ReplicaId,
        object: &PostObjectView,
    ) -> Result<Vec<MediaScanTarget>> {
        let mut targets: Vec<MediaScanTarget> = Vec::new();
        for asset in &object.attachments {
            push_media_target(
                &mut targets,
                asset.hash.as_str().to_string(),
                non_empty_mime(asset.mime.as_str()),
            );
        }
        for reference in &object.media_manifest_refs {
            let manifest = self
                .resolve_media_manifest(replica_id, object, reference)
                .await?;
            for item in &manifest.items {
                push_media_target(
                    &mut targets,
                    item.blob_hash.as_str().to_string(),
                    non_empty_mime(item.mime.as_str()),
                );
                if let Some(thumbnail) = &item.thumbnail_blob_hash {
                    // thumbnail の mime は manifest に無い（fetcher の magic bytes 判定に委ねる）。
                    push_media_target(&mut targets, thumbnail.as_str().to_string(), None);
                }
            }
        }
        Ok(targets)
    }

    /// replica から署名済み media manifest を解決する。
    ///
    /// 共有 replica の entry が本物であること（署名検証）に加えて、post author 本人が署名した
    /// manifest であることを要求する（他者の manifest を参照して scan 対象を偽装する経路を
    /// 塞ぐ）。いずれの失敗も Err = fail-closed。
    async fn resolve_media_manifest(
        &self,
        replica_id: &ReplicaId,
        object: &PostObjectView,
        manifest_id: &str,
    ) -> Result<KukuriMediaManifestV1> {
        let key = format!(
            "{}{manifest_id}/envelope",
            SharedReplicaKeyFamily::MediaManifest.prefix()
        );
        let records = self
            .docs_sync
            .query_replica_exact_bounded(
                replica_id,
                &key,
                RECORDS_PER_EXACT_KEY,
                DocFetchPolicy::LocalThenRemote,
            )
            .await
            .map_err(|error| {
                transient(error.context(format!("failed to query media manifest `{manifest_id}`")))
            })?;
        let Some(record) = records.into_iter().next() else {
            bail!("media manifest `{manifest_id}` is not present in the replica");
        };
        let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)
            .with_context(|| format!("failed to decode media manifest envelope `{manifest_id}`"))?;
        envelope.verify().with_context(|| {
            format!("media manifest envelope `{manifest_id}` failed verification")
        })?;
        if envelope.kind != "media-manifest" {
            bail!(
                "entry for media manifest `{manifest_id}` has unexpected kind `{}`",
                envelope.kind
            );
        }
        if envelope.pubkey.as_str() != object.author.as_str() {
            bail!("media manifest `{manifest_id}` is not signed by the post author (fail-closed)");
        }
        let manifest: KukuriMediaManifestV1 = serde_json::from_str(envelope.content.as_str())
            .with_context(|| format!("failed to parse media manifest content `{manifest_id}`"))?;
        Ok(manifest)
    }

    /// post 本文 text を取り出す。
    ///
    /// inline text はそのまま返す。blob text は同一 scope scan で取得済みの署名済み envelope と object
    /// state の参照を突合し、`BlobService::fetch_blob_ephemeral` で本文 bytes を一時取得する。取得した
    /// bytes は宣言サイズ・上限・BLAKE3 hash・UTF-8 を検証し、raw blob は恒久保存しない。
    pub(super) async fn resolve_body_text(
        &self,
        _replica_id: &ReplicaId,
        object: &PostObjectView,
        envelopes: &HashMap<String, DocRecord>,
    ) -> Result<String> {
        match &object.payload_ref {
            PayloadRef::InlineText { text } => Ok(text.clone()),
            PayloadRef::BlobText { hash, bytes, .. } => {
                if *bytes > MAX_INDEXABLE_POST_BODY_BYTES {
                    bail!(
                        "blob text declared size exceeds the index body limit ({} > {} bytes)",
                        bytes,
                        MAX_INDEXABLE_POST_BODY_BYTES
                    );
                }
                let record = envelopes
                    .get(object.object_id.as_str())
                    .context("blob text envelope is missing")?;
                let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)
                    .context("failed to decode post envelope for blob text")?;
                envelope
                    .verify()
                    .context("post envelope failed verification")?;
                if envelope.id.as_str() != object.object_id {
                    bail!("blob text envelope id does not match the object state");
                }
                if envelope.pubkey.as_str() != object.author {
                    bail!("blob text envelope author does not match the object state");
                }
                let content = envelope
                    .post_content()
                    .context("failed to parse blob text post content")?
                    .context("blob text envelope is not a post")?;
                if content.payload_ref != object.payload_ref {
                    bail!("blob text payload metadata does not match the signed envelope");
                }

                let blob_service = self
                    .blob_service
                    .as_ref()
                    .context("blob service is not configured for blob text")?;
                // 取得できないこと（ピア不在・転送失敗）は一時的な失敗。実体が上限を超えることは
                // 宣言サイズとの不一致なので確定した理由として扱う（#1090）。
                let fetched = blob_service
                    .fetch_blob_ephemeral_bounded(hash, MAX_INDEXABLE_POST_BODY_BYTES)
                    .await
                    .map_err(|error| {
                        let oversized = error.is::<BlobTooLarge>();
                        let error = error.context("failed to fetch blob text body");
                        if oversized { error } else { transient(error) }
                    })?
                    .ok_or_else(|| transient(anyhow!("blob text body is not retrievable")))?;
                let actual_bytes = u64::try_from(fetched.len())
                    .context("blob text body size does not fit in u64")?;
                if actual_bytes != *bytes {
                    bail!(
                        "blob text body size does not match metadata ({} != {} bytes)",
                        actual_bytes,
                        bytes
                    );
                }
                if actual_bytes > MAX_INDEXABLE_POST_BODY_BYTES {
                    bail!(
                        "blob text body exceeds the index body limit ({} > {} bytes)",
                        actual_bytes,
                        MAX_INDEXABLE_POST_BODY_BYTES
                    );
                }
                if blob_hash(&fetched) != *hash {
                    bail!("blob text body hash does not match metadata");
                }
                let text = String::from_utf8(fetched).context("blob text body is not UTF-8")?;
                let char_count = text.chars().count();
                if char_count > MAX_INDEXABLE_POST_BODY_CHARS {
                    bail!(
                        "blob text body exceeds the index character limit ({} > {} characters)",
                        char_count,
                        MAX_INDEXABLE_POST_BODY_CHARS
                    );
                }
                Ok(text)
            }
        }
    }
}

/// docs replica に保存された post object state の最小 view。
///
/// `object_persistence_support` の `CanonicalPostHeader`（= `KukuriPostObjectV1`）と同じ JSON を
/// 部分的に読む。cn-indexer は index に必要な最小フィールドのみを取り出す。
#[derive(Debug, serde::Deserialize)]
pub(super) struct PostObjectView {
    pub(super) object_id: String,
    pub(super) author: String,
    pub(super) created_at: i64,
    pub(super) payload_ref: PayloadRef,
    #[serde(default)]
    pub(super) attachments: Vec<AssetRef>,
    #[serde(default)]
    pub(super) media_manifest_refs: Vec<String>,
    #[serde(default)]
    pub(super) status: ObjectStatus,
    /// 返信先の投稿（#1221 R5-E。関係の観測に使う）。
    #[serde(default)]
    pub(super) reply_to: Option<kukuri_core::EnvelopeId>,
    /// repost・引用の元投稿（#1221 R5-E）。
    #[serde(default)]
    pub(super) repost_of: Option<kukuri_core::RepostSourceSnapshotV1>,
}

/// scan 対象の media 参照 1 件（blob hash + 参照元 metadata 由来の mime）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MediaScanTarget {
    pub(super) hash: String,
    pub(super) mime: Option<String>,
}

/// 同一 blob hash を dedup しながら scan 対象へ追加する（mime は先勝ち）。
fn push_media_target(targets: &mut Vec<MediaScanTarget>, hash: String, mime: Option<String>) {
    if targets.iter().any(|target| target.hash == hash) {
        return;
    }
    targets.push(MediaScanTarget { hash, mime });
}

/// 空 / 空白のみの mime は「無し」として扱う。
fn non_empty_mime(mime: &str) -> Option<String> {
    let trimmed = mime.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
