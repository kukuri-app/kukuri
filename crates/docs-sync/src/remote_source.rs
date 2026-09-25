//! One provider and one object-sized read lease. No namespace sync or persistent blob import.

use std::collections::HashMap;

use anyhow::{Result, bail, ensure};
use async_trait::async_trait;
use iroh::EndpointAddr;
use iroh_docs::NamespaceSecret;
use kukuri_core::ReplicaId;
use kukuri_iroh_node::{DocReadQuery, DocReadRecord, DocReadResponse};
use tokio::sync::Mutex;

use crate::{
    DocEventStream, DocFetchPolicy, DocKeyEntry, DocKeyOrder, DocKeyPage, DocKeyQuery, DocOp,
    DocQuery, DocRecord, DocsSync, IrohDocsSync,
};

const MAX_LEASE_BYTES: usize = 1024 * 1024;
const MAX_LEASE_KEYS: usize = 32;

#[derive(Default)]
struct LeaseCache {
    records: HashMap<String, Vec<DocRecord>>,
    bytes: usize,
}

/// `LocalOnly` is the immutable snapshot already fetched in this lease. A new lease is used for
/// each object so a provider cannot make one scan consume memory proportional to a page/history.
pub struct RemoteDocsSource {
    inner: IrohDocsSync,
    peer: EndpointAddr,
    private: Option<(ReplicaId, NamespaceSecret)>,
    cache: Mutex<LeaseCache>,
}

impl RemoteDocsSource {
    pub(crate) fn new(inner: IrohDocsSync, peer: EndpointAddr) -> Self {
        Self {
            inner,
            peer,
            private: None,
            cache: Mutex::new(LeaseCache::default()),
        }
    }

    pub(crate) fn with_private_secret(
        inner: IrohDocsSync,
        peer: EndpointAddr,
        replica: ReplicaId,
        secret: [u8; 32],
    ) -> Self {
        let mut source = Self::new(inner, peer);
        source.private = Some((replica, NamespaceSecret::from_bytes(&secret)));
        source
    }

    fn private_secret(&self, replica: &ReplicaId) -> Result<Option<&NamespaceSecret>> {
        match &self.private {
            Some((expected, secret)) => {
                ensure!(expected == replica, "private reader changed replica");
                Ok(Some(secret))
            }
            None => Ok(None),
        }
    }

    async fn exact(
        &self,
        replica: &ReplicaId,
        key: &str,
        author: Option<&str>,
        limit: usize,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        ensure!((1..=8).contains(&limit), "remote exact read limit exceeded");
        if let Some(records) = self.cache.lock().await.records.get(key) {
            return Ok(filter_records(records, author, limit));
        }
        if policy == DocFetchPolicy::LocalOnly {
            return Ok(Vec::new());
        }
        if let (Some(cache), Some(author)) = (self.inner.remote_cache(), author) {
            let cached = cache
                .get_remote_records(replica.as_str(), key, Some(author), 1)
                .await?;
            if let Some(bytes) = cached.into_iter().next() {
                let entry: DocReadRecord = serde_json::from_slice(&bytes)?;
                return Ok(vec![checked_record(entry, key, Some(author))?]);
            }
        }
        let response = self
            .inner
            .query_remote_docs_with_secret(
                replica,
                self.peer.clone(),
                DocReadQuery::Exact {
                    key: key.to_owned(),
                    limit,
                    author: author.map(str::to_owned),
                },
                self.private_secret(replica)?,
            )
            .await?;
        let DocReadResponse::Records(entries) = response else {
            bail!("remote docs provider returned the wrong response type")
        };
        ensure!(
            entries.len() <= limit,
            "remote docs exact result exceeded limit"
        );
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            records.push(checked_record(entry, key, author)?);
        }
        let bytes = records
            .iter()
            .map(|record| record.value.len())
            .sum::<usize>();
        let mut cache = self.cache.lock().await;
        ensure!(
            cache.records.len() < MAX_LEASE_KEYS
                && cache.bytes.saturating_add(bytes) <= MAX_LEASE_BYTES,
            "remote docs lease budget exceeded"
        );
        cache.bytes += bytes;
        cache.records.insert(key.to_owned(), records.clone());
        Ok(records)
    }
}

impl RemoteDocsSource {
    async fn keys(
        &self,
        replica: &ReplicaId,
        query: DocKeyQuery,
        author: Option<&str>,
    ) -> Result<DocKeyPage> {
        if query.limit == 0 {
            return Ok(DocKeyPage::default());
        }
        let response = self
            .inner
            .query_remote_docs_with_secret(
                replica,
                self.peer.clone(),
                DocReadQuery::Keys {
                    prefix: query.prefix.clone(),
                    descending: query.order == DocKeyOrder::Descending,
                    limit: query.limit,
                    author: author.map(str::to_owned),
                },
                self.private_secret(replica)?,
            )
            .await?;
        let DocReadResponse::Keys {
            entries,
            reached_limit,
        } = response
        else {
            bail!("remote docs provider returned the wrong response type")
        };
        ensure!(
            entries.len() <= query.limit
                && entries.iter().all(|entry| {
                    entry.key.starts_with(&query.prefix)
                        && author.is_none_or(|author| entry.docs_author == author)
                }),
            "remote docs key page exceeded its query"
        );
        Ok(DocKeyPage {
            entries: entries
                .into_iter()
                .map(|entry| DocKeyEntry {
                    key: entry.key,
                    content_hash: entry.content_hash,
                    content_len: entry.content_len,
                    docs_author: Some(entry.docs_author),
                })
                .collect(),
            reached_limit,
        })
    }
}

pub(crate) fn checked_record(
    entry: DocReadRecord,
    key: &str,
    author: Option<&str>,
) -> Result<DocRecord> {
    ensure!(entry.key == key, "remote docs provider changed exact key");
    ensure!(
        entry.value.len() <= 64 * 1024,
        "remote docs record exceeded 64 KiB"
    );
    ensure!(
        author.is_none_or(|author| entry.docs_author == author),
        "remote docs provider changed docs author"
    );
    ensure!(
        entry.content_len == entry.value.len() as u64
            && iroh_blobs::Hash::new(&entry.value).to_string() == entry.content_hash,
        "remote docs record hash mismatch"
    );
    Ok(DocRecord {
        key: entry.key,
        value: entry.value,
        content_hash: entry.content_hash,
        content_len: entry.content_len,
        docs_author: Some(entry.docs_author),
    })
}

fn filter_records(records: &[DocRecord], author: Option<&str>, limit: usize) -> Vec<DocRecord> {
    records
        .iter()
        .filter(|record| author.is_none_or(|author| record.docs_author.as_deref() == Some(author)))
        .take(limit)
        .cloned()
        .collect()
}

#[async_trait]
impl DocsSync for RemoteDocsSource {
    fn remote_reader_id(&self) -> Option<String> {
        Some(self.peer.id.to_string())
    }
    async fn finish_remote_object(&self) {
        *self.cache.lock().await = LeaseCache::default();
    }
    async fn persist_verified_record(
        &self,
        replica: &ReplicaId,
        key: &str,
        author: Option<&str>,
    ) -> Result<()> {
        let Some(cache) = self.inner.remote_cache() else {
            return Ok(());
        };
        let record = self
            .cache
            .lock()
            .await
            .records
            .get(key)
            .and_then(|records| {
                records
                    .iter()
                    .find(|record| {
                        author.is_some_and(|author| record.docs_author.as_deref() == Some(author))
                    })
                    .cloned()
            });
        let Some(record) = record else {
            return Ok(());
        };
        let docs_author = record.docs_author.as_deref().expect("matched author");
        if self
            .inner
            .read_local_source_owned(replica, key, Some(docs_author), 1)
            .await
            .is_ok_and(|local| {
                local
                    .iter()
                    .any(|existing| existing.content_hash == record.content_hash)
            })
        {
            return Ok(());
        }
        let payload = serde_json::to_vec(&DocReadRecord {
            key: record.key.clone(),
            value: record.value.clone(),
            content_hash: record.content_hash.clone(),
            content_len: record.content_len,
            docs_author: docs_author.to_string(),
        })?;
        ensure!(
            cache
                .put_remote_record(replica.as_str(), key, docs_author, &payload)
                .await?,
            "remote record cache capacity exceeded"
        );
        Ok(())
    }
    async fn open_replica(&self, replica: &ReplicaId) -> Result<()> {
        if self.private_secret(replica)?.is_none() {
            let _ = self.inner.replica_secret(replica).await?;
        }
        Ok(())
    }

    async fn apply_doc_op(&self, _replica: &ReplicaId, _op: DocOp) -> Result<()> {
        bail!("remote docs source is read-only")
    }

    async fn query_replica_with_policy(
        &self,
        replica: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        match query {
            DocQuery::Exact(key) => self.exact(replica, &key, None, 8, policy).await,
            DocQuery::Prefix(_) | DocQuery::All => {
                bail!("remote docs source has no unbounded read")
            }
        }
    }

    async fn query_replica_exact_bounded(
        &self,
        replica: &ReplicaId,
        key: &str,
        limit: usize,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.exact(replica, key, None, limit, policy).await
    }

    async fn query_replica_by_author(
        &self,
        replica: &ReplicaId,
        author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<DocRecord>> {
        Ok(self
            .exact(replica, key, Some(author), 1, policy)
            .await?
            .into_iter()
            .next())
    }

    async fn query_replica_keys(
        &self,
        replica: &ReplicaId,
        query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        self.keys(replica, query, None).await
    }

    async fn query_replica_keys_by_author(
        &self,
        replica: &ReplicaId,
        docs_author: &str,
        query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        self.keys(replica, query, Some(docs_author)).await
    }

    async fn subscribe_replica(&self, _replica: &ReplicaId) -> Result<DocEventStream> {
        bail!("remote docs source cannot subscribe")
    }

    async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
        bail!("remote docs source has a fixed provider")
    }
}
