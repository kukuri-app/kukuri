//! #1407: 読取りは手元に無い namespace を作らない(`iroh_sync.rs` の行数の上限のため分けた)。

use super::*;
use crate::{DocKeyOrder, DocKeyQuery};

async fn namespace_count(node: &IrohDocsNode) -> usize {
    node.docs()
        .list()
        .await
        .expect("list namespaces")
        .count()
        .await
}

#[tokio::test]
async fn reads_of_an_absent_namespace_return_nothing_without_creating_it() {
    let node = IrohDocsNode::memory().await.expect("docs node");
    let docs = IrohDocsSync::new(node.clone());
    let replica = ReplicaId::new("author::absent-author");
    let key = "profile/latest";
    let author = node
        .docs()
        .author_create()
        .await
        .expect("author")
        .to_string();
    let keys = DocKeyQuery {
        prefix: "graph/follows/".into(),
        limit: 8,
        order: DocKeyOrder::Ascending,
    };

    assert!(
        docs.query_replica_with_policy(
            &replica,
            DocQuery::Exact(key.into()),
            DocFetchPolicy::LocalOnly
        )
        .await
        .expect("exact")
        .is_empty()
    );
    assert!(
        docs.query_replica_exact_bounded(&replica, key, 8, DocFetchPolicy::LocalOnly)
            .await
            .expect("bounded")
            .is_empty()
    );
    assert!(
        docs.query_replica_exact_bounded(&replica, key, 0, DocFetchPolicy::LocalOnly)
            .await
            .expect("zero limit")
            .is_empty()
    );
    assert!(
        docs.query_replica_by_author(&replica, &author, key, DocFetchPolicy::LocalOnly)
            .await
            .expect("by author")
            .is_none()
    );
    assert!(
        docs.query_replica_keys(&replica, keys.clone())
            .await
            .expect("keys")
            .entries
            .is_empty()
    );
    assert!(
        docs.query_replica_keys_by_author(&replica, &author, keys.clone())
            .await
            .expect("keys by author")
            .entries
            .is_empty()
    );
    assert!(
        docs.query_replica_keys_by_author(&replica, "not-a-docs-author", keys)
            .await
            .expect("invalid docs author")
            .entries
            .is_empty()
    );
    assert_eq!(namespace_count(&node).await, 0);
    assert!(docs.replicas.lock().await.is_empty());
    docs.shutdown().await;
    node.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn reads_of_a_held_namespace_are_unchanged_and_writes_still_create_it() {
    let node = IrohDocsNode::memory().await.expect("docs node");
    let docs = IrohDocsSync::new(node.clone());
    let replica = ReplicaId::new("author::held-author");
    docs.apply_doc_op(
        &replica,
        DocOp::SetBytes {
            key: "profile/latest".into(),
            value: b"held".to_vec(),
        },
    )
    .await
    .expect("write creates the namespace");
    assert_eq!(namespace_count(&node).await, 1);
    // 閉じた後も、手元にある namespace は読める。
    docs.close_replica(&replica).await.expect("close");
    let records = docs
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact("profile/latest".into()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("read");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].value, b"held");
    assert_eq!(namespace_count(&node).await, 1);
    docs.shutdown().await;
    node.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn reads_of_an_unregistered_private_replica_still_fail() {
    let node = IrohDocsNode::memory().await.expect("docs node");
    let docs = IrohDocsSync::new(node.clone());
    let replica = crate::private_channel_replica_id("unregistered-channel");
    assert!(
        docs.query_replica_exact_bounded(&replica, "key", 0, DocFetchPolicy::LocalOnly)
            .await
            .is_err()
    );
    assert!(
        docs.query_replica_with_policy(
            &replica,
            DocQuery::Exact("key".into()),
            DocFetchPolicy::LocalOnly
        )
        .await
        .is_err()
    );
    assert_eq!(namespace_count(&node).await, 0);
    docs.shutdown().await;
    node.shutdown().await.expect("shutdown");
}
