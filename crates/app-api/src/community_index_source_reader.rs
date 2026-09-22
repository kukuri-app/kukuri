//! 既存の署名・取り下げ検証へ、namespaceを作成しない読取りだけを渡す。
use anyhow::{Result, bail, ensure};
use async_trait::async_trait;
use kukuri_core::ReplicaId;
use kukuri_docs_sync::{DocEventStream, DocFetchPolicy, DocOp, DocQuery, DocRecord, DocsSync};

pub(super) struct LocalSourceReader {
    pub docs: std::sync::Arc<dyn DocsSync>,
    pub replica: ReplicaId,
}

impl LocalSourceReader {
    async fn read(
        &self,
        replica: &ReplicaId,
        key: &str,
        author: Option<&str>,
        limit: usize,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        ensure!(
            replica == &self.replica && policy == DocFetchPolicy::LocalOnly,
            "source reader cannot expand its scope or fetch policy"
        );
        self.docs
            .query_local_source(replica, key, author, limit.min(8))
            .await
    }
}

#[async_trait]
impl DocsSync for LocalSourceReader {
    async fn open_replica(&self, _: &ReplicaId) -> Result<()> {
        bail!("source reader cannot start sync")
    }
    async fn apply_doc_op(&self, _: &ReplicaId, _: DocOp) -> Result<()> {
        bail!("source reader is read-only")
    }
    async fn query_replica_with_policy(
        &self,
        replica: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        let DocQuery::Exact(key) = query else {
            bail!("source reader requires an exact key");
        };
        self.read(replica, &key, None, 8, policy).await
    }
    async fn query_replica_exact_bounded(
        &self,
        replica: &ReplicaId,
        key: &str,
        limit: usize,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.read(replica, key, None, limit, policy).await
    }
    async fn query_replica_by_author(
        &self,
        replica: &ReplicaId,
        author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<DocRecord>> {
        Ok(self
            .read(replica, key, Some(author), 1, policy)
            .await?
            .into_iter()
            .next())
    }
    async fn subscribe_replica(&self, _: &ReplicaId) -> Result<DocEventStream> {
        bail!("source reader cannot subscribe")
    }
    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        bail!("source reader cannot connect peers")
    }
}
