use anyhow::{Context, Result};
use kukuri_cn_core::IndexScopeKind;
use kukuri_core::ReplicaId;
use kukuri_docs_sync::{
    DocFetchPolicy, DocRecord, DocsSync, SharedReplicaKeyFamily, query_time_index_window,
};
use std::collections::HashSet;

use super::{IngestPipeline, IngestSummary, RECORDS_PER_EXACT_KEY};

const RECENT_INDEX_PAGE: usize = 100;

pub(crate) async fn recent_object_keys(
    docs: &dyn DocsSync,
    replica_id: &ReplicaId,
    limit: usize,
) -> Result<Vec<String>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let page = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        query_time_index_window(
            docs,
            replica_id,
            SharedReplicaKeyFamily::TimelineIndex.prefix(),
            chrono::Utc::now().timestamp().saturating_add(600),
            limit.min(RECENT_INDEX_PAGE),
        ),
    )
    .await??;
    Ok(page
        .entries
        .into_iter()
        .filter(|entry| !entry.object_id.contains('/') && !entry.object_id.is_empty())
        .map(|entry| format!("objects/{}/state", entry.object_id))
        .collect())
}

impl IngestPipeline {
    /// Read only the named object and its withdrawal, never a replica-wide prefix.
    pub(super) async fn ingest_object_ids(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        replica_id: &ReplicaId,
        object_ids: &[String],
    ) -> Result<IngestSummary> {
        if !self.retain_supported_scope(scope_kind, scope_id).await? {
            return Ok(IngestSummary::default());
        }
        self.docs_sync.open_replica(replica_id).await?;

        let mut records: Vec<DocRecord> = Vec::new();
        for object_id in object_ids {
            for suffix in ["state", "envelope"] {
                let key = format!(
                    "{}{object_id}/{suffix}",
                    SharedReplicaKeyFamily::PostObject.prefix()
                );
                records.extend(
                    self.docs_sync
                        .query_replica_exact_bounded(
                            replica_id,
                            &key,
                            RECORDS_PER_EXACT_KEY,
                            DocFetchPolicy::LocalThenRemote,
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "failed to query object `{object_id}` in replica {}",
                                replica_id.as_str()
                            )
                        })?,
                );
            }
        }
        let (state_records, context) = self.scope_context(replica_id, records, object_ids).await?;
        let summary = self
            .ingest_records(scope_kind, scope_id, replica_id, &state_records, &context)
            .await?;
        self.observe_reactions(scope_kind, scope_id, replica_id, object_ids, None)
            .await;
        Ok(summary)
    }

    /// Poll only the current index window. A missing page never de-indexes older entries.
    /// Remote readers can reuse this object-scoped path without importing a namespace.
    pub async fn ingest_recent_scope(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        replica_id: &ReplicaId,
    ) -> Result<IngestSummary> {
        self.ingest_recent_scope_excluding(scope_kind, scope_id, replica_id, &[])
            .await
    }

    pub(crate) async fn ingest_recent_scope_excluding(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        replica_id: &ReplicaId,
        known_objects: &[String],
    ) -> Result<IngestSummary> {
        crate::replica_plan::validate_scope_replica(scope_kind, scope_id, replica_id)?;
        if !self.retain_supported_scope(scope_kind, scope_id).await? {
            return Ok(IngestSummary::default());
        }
        let mut keys =
            recent_object_keys(self.docs_sync.as_ref(), replica_id, RECENT_INDEX_PAGE).await?;
        if !known_objects.is_empty() {
            let known = known_objects
                .iter()
                .map(|id| format!("objects/{id}/state"))
                .collect::<HashSet<_>>();
            keys.retain(|key| !known.contains(key));
        }
        if keys.is_empty() {
            return Ok(IngestSummary::default());
        }
        self.ingest_changed_keys(scope_kind, scope_id, replica_id, &keys)
            .await
    }
}
