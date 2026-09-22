use super::{
    DocFetchPolicy, DocQuery, DocRecord, IndexScopeKind, IngestPipeline, KukuriEnvelope, ReplicaId,
    failure::{is_transient, transient},
    source::{MediaScanTarget, PostObjectView, SourceResolver},
    verify_post_withdrawal,
};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use kukuri_cn_safety::{ScanError, provider::ScanReferenceGuard};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct ReferenceGuard<'a> {
    pub pipeline: &'a IngestPipeline,
    pub scope_kind: IndexScopeKind,
    pub scope_id: &'a str,
    pub replica: &'a ReplicaId,
    pub object: &'a PostObjectView,
    pub record: &'a DocRecord,
    pub envelope: Option<&'a DocRecord>,
    pub media_targets: Mutex<Option<Vec<MediaScanTarget>>>,
    /// 確定した理由で再確認に失敗したことがあるか（#1090）。scan service 越しに返る失敗は
    /// 分類の印を失うため、ここに残して呼び出し側の分類に使う。
    pub definitive_failure: AtomicBool,
    pub scheduler: &'a crate::scheduler::PostFetchScheduler,
    pub job_lease: &'a crate::scheduler::PostFetchJobLease,
}

impl ReferenceGuard<'_> {
    /// 参照を再確認する。確定した理由の失敗は記録し、一時的な失敗には印を付けて返す。
    pub(super) async fn verify(&self) -> Result<()> {
        let result = self.check_current().await;
        if let Err(error) = &result
            && !is_transient(error)
        {
            self.definitive_failure.store(true, Ordering::SeqCst);
        }
        result
    }

    /// scan service から返った失敗を分類する。再確認が確定した理由で失敗していなければ、
    /// 判定記録の読み書きや再確認の照会の障害として一時的な失敗に倒す。
    pub(super) fn classify_scan_error(&self, error: anyhow::Error) -> anyhow::Error {
        if self.definitive_failure.load(Ordering::SeqCst) {
            error
        } else {
            transient(error)
        }
    }

    async fn check_current(&self) -> Result<()> {
        if !self.scheduler.is_current(self.job_lease) {
            return Err(transient(anyhow::anyhow!(
                "post source revision was superseded while ingesting"
            )));
        }
        ensure!(
            !self.object.object_id.trim().is_empty()
                && !self.object.author.trim().is_empty()
                && self.record.key == format!("objects/{}/state", self.object.object_id),
            "invalid post reference"
        );
        ensure!(
            self.pipeline
                .entries
                .is_scope_supported(self.scope_kind, self.scope_id)
                .await
                .map_err(transient)?,
            "scope is no longer supported"
        );
        ensure!(
            !self
                .pipeline
                .entries
                .is_transmission_prevented(&self.object.object_id)
                .await
                .map_err(transient)?,
            "post is transmission prevented"
        );
        // 新bucketのmarkerは発見用。書換えで署名済み投稿の検査を失効させない。
        if !self.replica.as_str().starts_with("bucket::") {
            let state = self
                .pipeline
                .docs_sync
                .query_replica_with_policy(
                    self.replica,
                    DocQuery::Exact(self.record.key.clone()),
                    DocFetchPolicy::LocalOnly,
                )
                .await
                .map_err(transient)?;
            let state_current = state
                .iter()
                .any(|current| current.content_hash == self.record.content_hash);
            ensure!(state_current, "post state changed during scan");
        }
        {
            let original = self.envelope.context("signed post envelope missing")?;
            let envelope: KukuriEnvelope = serde_json::from_slice(&original.value)?;
            envelope.verify()?;
            if self.replica.as_str().starts_with("bucket::") {
                let bucket = kukuri_docs_sync::BucketReplica::parse(self.replica)?;
                ensure!(
                    bucket.bucket().contains(envelope.created_at)
                        && self.object.created_at == envelope.created_at
                        && envelope.created_at
                            <= chrono::Utc::now().timestamp().saturating_add(600),
                    "post creation time does not match its bucket or exceeds clock skew allowance"
                );
            }
            let content = envelope
                .post_content()?
                .context("source is not a signed post")?;
            ensure!(
                content.payload_ref == self.object.payload_ref
                    && content.attachments == self.object.attachments
                    && content.media_manifest_refs == self.object.media_manifest_refs,
                "materialized post differs from signed content"
            );
            match self.scope_kind {
                IndexScopeKind::PublicTopic => ensure!(
                    content.topic_id.as_str() == self.scope_id
                        && content.channel_id.is_none()
                        && content.visibility == kukuri_core::ObjectVisibility::Public,
                    "post does not belong to public scope"
                ),
                IndexScopeKind::PrivateChannel => ensure!(
                    content
                        .channel_id
                        .as_ref()
                        .is_some_and(|id| id.as_str() == self.scope_id),
                    "post does not belong to private scope"
                ),
            }

            ensure!(
                envelope.id.as_str() == self.object.object_id
                    && envelope.pubkey.as_str() == self.object.author,
                "post envelope identity changed"
            );
            let current = if self.replica.as_str().starts_with("bucket::")
                && let Some(author) = original.docs_author.as_deref()
            {
                self.pipeline
                    .docs_sync
                    .query_replica_by_author(
                        self.replica,
                        author,
                        &original.key,
                        DocFetchPolicy::LocalOnly,
                    )
                    .await
                    .map_err(transient)?
                    .into_iter()
                    .collect()
            } else {
                self.pipeline
                    .docs_sync
                    .query_replica_with_policy(
                        self.replica,
                        DocQuery::Exact(original.key.clone()),
                        DocFetchPolicy::LocalOnly,
                    )
                    .await
                    .map_err(transient)?
            };
            let envelope_current = current.iter().any(|record| {
                if self.replica.as_str().starts_with("bucket::") {
                    super::bucket_post::canonical_post(self.replica, &self.object.object_id, record)
                        .is_some()
                } else {
                    record.content_hash == original.content_hash
                }
            });
            if !envelope_current && self.replica.as_str().starts_with("bucket::") {
                return Err(transient(anyhow::anyhow!(
                    "post envelope changed during bucket scan"
                )));
            }
            ensure!(envelope_current, "post envelope changed during scan");
            let withdrawals = self
                .pipeline
                .docs_sync
                .query_replica_with_policy(
                    self.replica,
                    DocQuery::Exact(format!("withdrawals/{}/state", self.object.object_id)),
                    DocFetchPolicy::LocalOnly,
                )
                .await
                .map_err(transient)?;
            for record in withdrawals {
                if let Ok(withdrawal) = serde_json::from_slice::<KukuriEnvelope>(&record.value) {
                    ensure!(
                        verify_post_withdrawal(&withdrawal, &envelope).is_err(),
                        "post was withdrawn during scan"
                    );
                }
            }
        }
        let expected = {
            self.media_targets
                .lock()
                .expect("media reference guard mutex")
                .clone()
        };
        if let Some(expected) = expected {
            let source = SourceResolver::new(
                self.pipeline.docs_sync.as_ref(),
                self.pipeline.blob_service.as_deref(),
            );
            ensure!(
                source.media_scan_targets(self.replica, self.object).await? == expected,
                "media manifest changed during scan"
            );
        }
        Ok(())
    }
}

#[async_trait]
impl ScanReferenceGuard for ReferenceGuard<'_> {
    async fn check(&self) -> Result<(), ScanError> {
        self.verify()
            .await
            .map_err(|_| ScanError::Unavailable("scan reference is no longer valid".into()))
    }
}
