use std::collections::HashMap;

use anyhow::Result;
use kukuri_cn_core::IndexScopeKind;

use super::{IngestPipeline, failure::transient};

impl IngestPipeline {
    pub(super) async fn apply_verified_withdrawals(
        &self,
        kind: IndexScopeKind,
        scope: &str,
        object_ids: &HashMap<String, i64>,
    ) -> Result<()> {
        for (id, created_at) in object_ids {
            self.entries
                .record_verified_withdrawal(kind, scope, id, *created_at)
                .await?;
            self.projection.remove_object(kind, scope, id).await?;
        }
        Ok(())
    }

    pub(super) async fn suppress_known_withdrawal(
        &self,
        kind: IndexScopeKind,
        scope: &str,
        object_id: &str,
        created_at: i64,
    ) -> Result<bool> {
        if !self
            .entries
            .is_known_withdrawn(kind, scope, object_id, created_at)
            .await
            .map_err(transient)?
        {
            return Ok(false);
        }
        self.deindex_object(kind, scope, object_id).await?;
        Ok(true)
    }
}
