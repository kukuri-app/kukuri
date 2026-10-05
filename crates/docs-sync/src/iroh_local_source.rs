//! 外部locatorによるLocalOnly参照。不存在namespaceをimportせず、常駐handle/task/secretを増やさない。
use super::*;
use tokio::sync::oneshot;

impl IrohDocsSync {
    /// 手元にある namespace だけを開く(import しない)。無ければ `None`。
    pub(super) async fn open_existing(
        &self,
        namespace: iroh_docs::NamespaceId,
    ) -> Result<Option<iroh_docs::api::Doc>> {
        match self.node.docs().open(namespace).await {
            Ok(doc) => Ok(doc),
            // Pinned iroh-docs serializes OpenError through RPC instead of returning None.
            // Only its explicit NotFound result is a miss; actor/I/O failures stay errors.
            Err(error)
                if error
                    .downcast_ref::<iroh_docs::api::RpcError>()
                    .is_some_and(|error| {
                        error.to_string() == iroh_docs::store::OpenError::NotFound.to_string()
                    }) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) async fn read_cached_local_source(
        &self,
        replica: &ReplicaId,
        key: &str,
        author: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DocRecord>> {
        let local = self
            .read_local_source_owned(replica, key, author, limit)
            .await;
        self.with_cached_records(replica, key, author, limit, false, local)
            .await
    }

    /// key 指定の読み出しに、保持分の record を足す。private は保護所有先へ移した record を含む保持分のすべて
    /// (#1221 R5-G: 旧領域が消えても読める)、公開の replica は自分の record だけ(namespace の無い Web の reload・
    /// native の restore の後も読める。他人の古い版で書き手本人への読み出しを止めない。ADR 0058 §7)。
    /// private の capability の確認は手元の読み出し(replica を開く)が済ませている。
    pub(super) async fn with_held_records(
        &self,
        replica: &ReplicaId,
        query: Option<DocQuery>,
        limit: usize,
        records: Vec<DocRecord>,
    ) -> Result<Vec<DocRecord>> {
        match query {
            Some(DocQuery::Exact(key)) if records.len() < limit => {
                let own_only = public_replica_secret(replica).is_some();
                self.with_cached_records(replica, &key, None, limit, own_only, Ok(records))
                    .await
            }
            _ => Ok(records),
        }
    }

    /// 手元の読み出しの結果に、record cache(保護所有先を含む)の同じ key の record を足す。
    pub(super) async fn with_cached_records(
        &self,
        replica: &ReplicaId,
        key: &str,
        author: Option<&str>,
        limit: usize,
        own_only: bool,
        local: Result<Vec<DocRecord>>,
    ) -> Result<Vec<DocRecord>> {
        let Some(cache) = self.remote_cache() else {
            return local;
        };
        let cached = cache
            .get_remote_records(replica.as_str(), key, author, limit.min(8), own_only)
            .await?;
        let mut records = match local {
            Ok(records) => records,
            Err(error) if cached.is_empty() => return Err(error),
            Err(_) => Vec::new(),
        };
        for bytes in cached {
            let entry = serde_json::from_slice(&bytes)?;
            let record = crate::remote_source::checked_record(entry, key, author)?;
            if !records
                .iter()
                .any(|existing| existing.docs_author == record.docs_author)
            {
                records.push(record);
            }
            if records.len() >= limit {
                break;
            }
        }
        Ok(records)
    }

    pub(crate) async fn read_local_source_owned(
        &self,
        replica: &ReplicaId,
        key: &str,
        author: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DocRecord>> {
        if replica.as_str().starts_with("bucket::") {
            crate::BucketReplica::parse(replica)?;
        }
        let secret = public_replica_secret(replica)
            .context("local source reader only accepts public replicas")?;
        self.read_local_records(replica, secret, key, author, limit)
            .await
    }

    /// 旧 store(`iroh-data`)に残る 1 key の record(#1221 R5-G・R5-I の移行)。旧 store に無ければ、新しい store の
    /// 手元の record を namespace を import せずに読む(移行の途中に書いた本人の record)。private は登録済みの
    /// capability で namespace を求める。
    #[cfg(not(target_family = "wasm"))]
    pub async fn read_legacy_records(
        &self,
        legacy: &kukuri_iroh_node::LegacyStore,
        replica: &ReplicaId,
        key: &str,
    ) -> Result<Vec<DocRecord>> {
        let secret = self.replica_secret(replica).await?;
        let records = legacy.records(secret.id(), key).await?;
        if records.is_empty() {
            return self.read_local_records(replica, secret, key, None, 8).await;
        }
        Ok(records
            .into_iter()
            .map(|record| DocRecord {
                key: record.key,
                value: record.value,
                content_hash: record.content_hash,
                content_len: record.content_len,
                docs_author: Some(record.docs_author),
            })
            .collect())
    }

    async fn read_local_records(
        &self,
        replica: &ReplicaId,
        secret: iroh_docs::NamespaceSecret,
        key: &str,
        author: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DocRecord>> {
        let query = match author {
            Some(author) => {
                let Ok(author) = AuthorId::from_str(author) else {
                    return Ok(Vec::new());
                };
                Query::author(author)
                    .key_exact(key)
                    .limit(limit.min(8) as u64)
                    .build()
            }
            None => bounded_exact_query(key, limit.min(8)),
        };
        if limit == 0 {
            return Ok(Vec::new());
        }
        // lifecycle ownerの既存32枠を共有。callerがcancelしてもopen後のcloseを完了させる。
        let mut tasks = self.close_tasks.lock().await;
        // tokio の try_join_next と同じく coop の budget に左右されずに回収する（wasm の JoinSet には無い）。
        while futures_util::FutureExt::now_or_never(tokio::task::unconstrained(tasks.join_next()))
            .flatten()
            .is_some()
        {}
        anyhow::ensure!(tasks.len() < 32, "local source reader is at capacity");
        let this = self.clone();
        let replica_key = replica.as_str().to_owned();
        let (send, receive) = oneshot::channel();
        tasks.spawn(async move {
            let result = async {
                // openは既存namespaceだけ。import_namespace、leave、subscribeは呼ばない。
                let registry = this.replicas.lock().await;
                anyhow::ensure!(
                    !registry
                        .get(replica_key.as_str())
                        .is_some_and(|handle| handle.closing),
                    "replica close is pending"
                );
                let Some(doc) = this.open_existing(secret.id()).await? else {
                    return Ok(Vec::new());
                };
                let read = async {
                    let stream = doc.get_many(query).await?;
                    tokio::pin!(stream);
                    let mut records = Vec::new();
                    while let Some(entry) = stream.next().await {
                        let entry = entry?;
                        let Some(key) = utf8_key(entry.key()) else {
                            continue;
                        };
                        let hash = entry.content_hash().to_string();
                        if let Some(value) = this
                            .fetch_entry_bytes(&hash, DocFetchPolicy::LocalOnly)
                            .await?
                        {
                            records.push(DocRecord {
                                key,
                                value,
                                content_hash: hash,
                                content_len: entry.content_len(),
                                docs_author: Some(entry.author().to_string()),
                            });
                        }
                    }
                    Ok::<_, anyhow::Error>(records)
                }
                .await;
                let close = doc.close().await;
                close?;
                read
            }
            .await;
            let _ = send.send(result);
        });
        drop(tasks);
        receive.await.context("local source read task stopped")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_source_misses_do_not_import_namespaces_or_leave_handles() -> Result<()> {
        let node = IrohDocsNode::memory().await?;
        let docs = IrohDocsSync::new(node.clone());
        for bucket in 0..20 {
            let replica = crate::BucketReplica::new(
                crate::BucketScope::Topic {
                    topic_id: "source".into(),
                },
                crate::TimeBucket::from_index(bucket)?,
            )?
            .replica_id();
            let result = docs
                .query_local_source(&replica, "objects/missing/envelope", None, 8)
                .await;
            assert!(result?.is_empty());
            assert!(
                node.docs()
                    .open(public_replica_secret(&replica).unwrap().id())
                    .await
                    .is_err()
            );
            assert!(docs.replicas.lock().await.is_empty());
            assert!(
                docs.private_replica_secrets
                    .registered
                    .lock()
                    .await
                    .is_empty()
            );
        }
        docs.shutdown().await;
        node.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn local_source_read_preserves_the_existing_sync_owner() -> Result<()> {
        let node = IrohDocsNode::memory().await?;
        let docs = IrohDocsSync::new(node.clone());
        let replica = crate::topic_replica_id("source-existing");
        docs.apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: "key".into(),
                value: b"value".to_vec(),
            },
        )
        .await?;
        let rows = docs.query_local_source(&replica, "key", None, 8).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value, b"value");
        let handles = docs.replicas.lock().await;
        assert_eq!(handles.len(), 1);
        let handle = handles.get(replica.as_str()).unwrap();
        // R5-H: 書いた namespace も同期を始めない。
        assert!(!handle.doc.status().await?.sync);
        drop(handles);
        docs.shutdown().await;
        node.shutdown().await?;
        Ok(())
    }
}
