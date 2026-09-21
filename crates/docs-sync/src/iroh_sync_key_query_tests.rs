//! #1239 / ADR 0053 §6: 名義を指定した key の一覧の test(`iroh_sync.rs` の行数の上限のため分けた)。

use super::*;
use crate::{DocKeyOrder, DocKeyQuery};

// #1239 / ADR 0053 §6: 名義を指定した key の一覧は、その名義の entry だけを返し、他の名義の entry は何件あっても
// 読まず、`limit` にも数えない。
#[tokio::test]
async fn key_query_by_author_skips_entries_of_other_authors() {
    let node = IrohDocsNode::memory().await.expect("docs node");
    let docs = IrohDocsSync::new(node.clone());
    let replica = crate::topic_replica_id("kukuri:topic:keys-by-author");
    let doc = docs.ensure_replica(&replica).await.expect("open replica");
    let owner = node.docs().author_create().await.expect("owner author");
    let other = node.docs().author_create().await.expect("other author");
    for index in 0..5u8 {
        doc.set_bytes(
            other,
            format!("graph/follows/0{index}").into_bytes(),
            vec![index],
        )
        .await
        .expect("write as another author");
    }
    for index in 0..2u8 {
        doc.set_bytes(
            owner,
            format!("graph/follows/9{index}").into_bytes(),
            vec![index],
        )
        .await
        .expect("write as the owner");
    }
    let query = || DocKeyQuery {
        prefix: "graph/follows/".into(),
        order: DocKeyOrder::Ascending,
        limit: 3,
    };

    let plain = docs
        .query_replica_keys(&replica, query())
        .await
        .expect("keys");
    assert!(
        plain
            .entries
            .iter()
            .all(|entry| entry.key.starts_with("graph/follows/0")),
        "entries of the other author fill the window of the plain query"
    );
    let by_owner = docs
        .query_replica_keys_by_author(&replica, owner.to_string().as_str(), query())
        .await
        .expect("keys by author");
    assert_eq!(
        by_owner
            .entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect::<Vec<_>>(),
        vec!["graph/follows/90", "graph/follows/91"]
    );
    assert!(!by_owner.reached_limit);
    assert!(
        by_owner
            .entries
            .iter()
            .all(|entry| entry.docs_author.as_deref() == Some(owner.to_string().as_str()))
    );
    assert!(
        docs.query_replica_keys_by_author(&replica, "not-an-author", query())
            .await
            .expect("an invalid author")
            .entries
            .is_empty()
    );

    docs.shutdown().await;
    node.shutdown().await.expect("shutdown node");
}
