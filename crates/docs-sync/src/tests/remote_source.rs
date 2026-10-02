use anyhow::Result;
use kukuri_iroh_node::IrohDocsNode;
use kukuri_store::SqliteStore;
use kukuri_transport::SeedPeer;
use std::sync::Arc;

use crate::{
    BucketReplica, BucketScope, DocFetchPolicy, DocKeyOrder, DocKeyQuery, DocOp, DocsSync,
    IrohDocsSync, TimeBucket,
};

#[tokio::test]
async fn private_bucket_lease_uses_only_its_explicit_epoch_secret() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let writer = IrohDocsSync::new(provider.clone());
    let reader = IrohDocsSync::new(requester.clone());
    let bucket = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: "private-room".into(),
            epoch_id: "epoch-1".into(),
        },
        TimeBucket::from_index(1)?,
    )?;
    let replica = bucket.replica_id();
    let secret = bucket.derive_private_secret(&[7; 32])?;
    writer
        .register_private_replica_secret(&replica, &hex::encode(secret))
        .await?;
    writer
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: "indexes/timeline/0001/post".into(),
                value: b"private index".to_vec(),
            },
        )
        .await?;
    let lease = reader.remote_source_with_private_secret(
        provider.endpoint().addr(),
        replica.clone(),
        secret,
    );
    let page = lease
        .query_replica_keys(
            &replica,
            DocKeyQuery {
                prefix: "indexes/timeline/".into(),
                order: DocKeyOrder::Descending,
                limit: 1,
            },
        )
        .await?;
    assert_eq!(page.entries.len(), 1);
    assert!(
        lease
            .query_replica_keys(
                &BucketReplica::new(
                    BucketScope::PrivateChannel {
                        channel_id: "other-room".into(),
                        epoch_id: "epoch-1".into(),
                    },
                    TimeBucket::from_index(1)?,
                )?
                .replica_id(),
                DocKeyQuery {
                    prefix: "indexes/timeline/".into(),
                    order: DocKeyOrder::Descending,
                    limit: 1
                },
            )
            .await
            .is_err()
    );
    writer.shutdown().await;
    reader.shutdown().await;
    provider.shutdown().await?;
    requester.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn private_readers_use_only_the_selected_scope_peer() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(requester.clone());
    let replica = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: "room".into(),
            epoch_id: "e1".into(),
        },
        TimeBucket::from_index(1)?,
    )?
    .replica_id();
    assert!(
        docs.remote_readers(&replica, Some([7; 32]), Vec::new())
            .await?
            .is_empty()
    );
    let readers = docs
        .remote_readers(
            &replica,
            Some([7; 32]),
            vec![SeedPeer {
                endpoint_id: provider.endpoint().addr().id.to_string(),
                addr_hint: None,
            }],
        )
        .await?;
    assert_eq!(readers.len(), 1);
    assert!(
        docs.remote_readers(&replica, None, Vec::new())
            .await
            .is_err()
    );
    assert!(
        docs.remote_readers(&crate::topic_replica_id("rust"), Some([7; 32]), Vec::new())
            .await
            .is_err()
    );
    docs.shutdown().await;
    requester.shutdown().await?;
    provider.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn remote_object_lease_reads_provider_keys_and_keeps_local_snapshot() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let writer = IrohDocsSync::new(provider.clone());
    let reader = IrohDocsSync::new(requester.clone());
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: "remote-object-lease".into(),
        },
        TimeBucket::from_index(1)?,
    )?
    .replica_id();
    let key = "objects/post/state";
    writer
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: "indexes/timeline/00000000000000000001-post/post".into(),
                value: b"index".to_vec(),
            },
        )
        .await?;
    writer
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: key.into(),
                value: b"first".to_vec(),
            },
        )
        .await?;
    let peer = provider.endpoint().addr();
    let lease = reader.remote_source(peer.clone());
    let page = lease
        .query_replica_keys(
            &replica,
            DocKeyQuery {
                prefix: "indexes/timeline/".into(),
                order: DocKeyOrder::Descending,
                limit: 10,
            },
        )
        .await?;
    assert_eq!(page.entries.len(), 1);
    assert!(
        lease
            .query_replica_exact_bounded(&replica, key, 8, DocFetchPolicy::LocalOnly)
            .await?
            .is_empty()
    );
    let fetched = lease
        .query_replica_exact_bounded(&replica, key, 8, DocFetchPolicy::LocalThenRemote)
        .await?;
    assert_eq!(fetched[0].value, b"first");

    writer
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: key.into(),
                value: b"second".to_vec(),
            },
        )
        .await?;
    let snapshot = lease
        .query_replica_exact_bounded(&replica, key, 8, DocFetchPolicy::LocalOnly)
        .await?;
    assert_eq!(snapshot[0].content_hash, fetched[0].content_hash);
    let next = reader
        .remote_source(peer)
        .query_replica_exact_bounded(&replica, key, 8, DocFetchPolicy::LocalThenRemote)
        .await?;
    assert_eq!(next[0].value, b"second");

    writer.shutdown().await;
    reader.shutdown().await;
    requester.shutdown().await?;
    provider.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn fetched_public_record_remains_local_and_can_be_reprovided() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let third = IrohDocsNode::memory().await?;
    let cache = Arc::new(SqliteStore::connect_memory().await?);
    requester.install_remote_cache(cache.clone())?;
    let writer = IrohDocsSync::new(provider.clone());
    let reader = IrohDocsSync::with_account_store(requester.clone(), cache.clone());
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: "cached-public-record".into(),
        },
        TimeBucket::from_index(1)?,
    )?
    .replica_id();
    let key = "objects/post/envelope";
    writer
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: key.into(),
                value: b"cached record".to_vec(),
            },
        )
        .await?;
    let lease = reader.remote_source(provider.endpoint().addr());
    let fetched = lease
        .query_replica_exact_bounded(&replica, key, 1, DocFetchPolicy::LocalThenRemote)
        .await?;
    assert_eq!(fetched[0].value, b"cached record");
    assert!(
        cache
            .get_remote_records(
                replica.as_str(),
                key,
                fetched[0].docs_author.as_deref(),
                1,
                false
            )
            .await?
            .is_empty(),
        "remote read must not persist before the caller's save guard"
    );
    lease
        .persist_verified_record(&replica, key, fetched[0].docs_author.as_deref(), &[])
        .await?;
    assert_eq!(
        reader.query_local_source(&replica, key, None, 1).await?[0].value,
        b"cached record"
    );
    let secret = crate::replicas::public_replica_secret(&replica).expect("public bucket secret");
    let response = third
        .query_remote_docs(
            requester.endpoint().addr(),
            &replica,
            &secret,
            kukuri_iroh_node::DocReadQuery::Exact {
                key: key.into(),
                limit: 1,
                author: fetched[0].docs_author.clone(),
            },
        )
        .await?;
    let kukuri_iroh_node::DocReadResponse::Records(records) = response else {
        anyhow::bail!("expected records")
    };
    assert_eq!(records[0].value, b"cached record");
    writer.shutdown().await;
    reader.shutdown().await;
    third.shutdown().await?;
    requester.shutdown().await?;
    provider.shutdown().await?;
    Ok(())
}

// 相手へ private の保持分を返すのは、要求の replica に手元で登録した capability で証明を確かめられたときだけ。
// 別の private replica の namespace の証明では、要求の replica の保持分を返さない(#1459)。
#[tokio::test]
async fn private_held_records_require_the_requested_replica_capability() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let cache = Arc::new(SqliteStore::connect_memory().await?);
    provider.install_remote_cache(cache.clone())?;
    let docs = IrohDocsSync::with_account_store(provider.clone(), cache.clone());
    let joined = crate::private_channel_epoch_replica_id("joined", "e1");
    let other = crate::private_channel_epoch_replica_id("other", "e1");
    let joined_secret = iroh_docs::NamespaceSecret::from_bytes(&[7; 32]);
    let other_secret = iroh_docs::NamespaceSecret::from_bytes(&[8; 32]);
    for (replica, secret) in [(&joined, &joined_secret), (&other, &other_secret)] {
        docs.register_private_replica_secret(replica, &hex::encode(secret.to_bytes()))
            .await?;
        docs.apply_doc_op(
            replica,
            DocOp::SetBytes {
                key: "channels/metadata".into(),
                value: b"{}".to_vec(),
            },
        )
        .await?;
    }
    let key = "objects/post/envelope";
    let value = b"other channel post".to_vec();
    let record = kukuri_iroh_node::DocReadRecord {
        key: key.into(),
        value: value.clone(),
        content_hash: blake3::hash(&value).to_hex().to_string(),
        content_len: value.len() as u64,
        docs_author: "other-author".into(),
    };
    cache
        .put_remote_record(
            other.as_str(),
            key,
            "other-author",
            &serde_json::to_vec(&record)?,
        )
        .await?;
    let query = kukuri_iroh_node::DocReadQuery::Exact {
        key: key.into(),
        limit: 8,
        author: None,
    };
    let response = requester
        .query_remote_docs(
            provider.endpoint().addr(),
            &other,
            &other_secret,
            query.clone(),
        )
        .await?;
    let kukuri_iroh_node::DocReadResponse::Records(records) = response else {
        anyhow::bail!("expected records")
    };
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].value, value);
    let crossed = requester
        .query_remote_docs(provider.endpoint().addr(), &other, &joined_secret, query)
        .await;
    assert!(
        crossed.is_err(),
        "another replica's capability must not read held records: {crossed:?}"
    );
    docs.shutdown().await;
    requester.shutdown().await?;
    provider.shutdown().await?;
    Ok(())
}
