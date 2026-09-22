//! 外部locatorによるLocalOnly参照。不存在namespaceをimportせず、常駐handle/task/secretを増やさない。
use super::*;
use tokio::sync::oneshot;

impl IrohDocsSync {
    pub(super) async fn read_local_source_owned(
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
        while tasks.try_join_next().is_some() {}
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
                let doc = match this.node.docs().open(secret.id()).await {
                    Ok(Some(doc)) => doc,
                    Ok(None) => return Ok(Vec::new()),
                    // Pinned iroh-docs serializes OpenError through RPC instead of returning None.
                    // Only its explicit NotFound result is a miss; actor/I/O failures stay errors.
                    Err(error)
                        if error
                            .downcast_ref::<iroh_docs::api::RpcError>()
                            .is_some_and(|error| {
                                error.to_string()
                                    == iroh_docs::store::OpenError::NotFound.to_string()
                            }) =>
                    {
                        return Ok(Vec::new());
                    }
                    Err(error) => return Err(error),
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
            assert!(docs.private_replica_secrets.lock().await.is_empty());
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
        assert!(handle.sync_requested && handle.doc.status().await?.sync);
        drop(handles);
        docs.shutdown().await;
        node.shutdown().await?;
        Ok(())
    }
}
