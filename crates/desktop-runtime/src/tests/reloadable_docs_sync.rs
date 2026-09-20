//! `ReloadableDocsSync` が、宣言の要る trait メソッドを内側の docs sync へ転送することを固定する(#1157)。
//! 宣言の無いメソッドは trait の既定実装に落ち、上限つきの読み出しや通知が黙って効かなくなる。

use std::sync::Arc;

use kukuri_docs_sync::{DocsSync, IrohDocsSync};
use kukuri_iroh_node::IrohDocsNode;

use crate::stack::ReloadableDocsSync;

// #1239: 取りこぼしの通知は、内側の docs sync の購読からしか出ない。転送の宣言が無いと、既定実装
// (entry だけを流す)に落ちて、購読側は取りこぼしに気づけない。
#[tokio::test]
async fn reloadable_docs_sync_forwards_replica_notices() {
    use futures_util::StreamExt;

    let node = IrohDocsNode::memory().await.expect("docs node");
    let docs = ReloadableDocsSync::new(Arc::new(IrohDocsSync::new(node.clone())));
    let replica = kukuri_docs_sync::topic_replica_id("kukuri:topic:reloadable-notices");
    let mut notices = docs
        .subscribe_replica_notices(&replica)
        .await
        .expect("subscribe notices");
    // 購読側が読まないうちに、buffer(256 件)を超える entry を書く。
    for index in 0..300usize {
        docs.apply_doc_op(
            &replica,
            kukuri_docs_sync::DocOp::SetJson {
                key: format!("objects/{index:04}/state"),
                value: serde_json::json!({}),
            },
        )
        .await
        .expect("write entry");
    }
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), notices.next())
        .await
        .expect("a notice arrives")
        .expect("stream is open")
        .expect("notice");
    assert!(
        matches!(first, kukuri_docs_sync::ReplicaNotice::Lagged { missed } if missed > 0),
        "the overflow must be reported to the subscriber: {first:?}"
    );
    node.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn reloadable_docs_sync_forwards_bounded_key_queries() {
    let node = IrohDocsNode::memory().await.expect("docs node");
    let docs = ReloadableDocsSync::new(Arc::new(IrohDocsSync::new(node.clone())));
    let replica = kukuri_docs_sync::topic_replica_id("kukuri:topic:reloadable-key-query");
    docs.open_replica(&replica).await.expect("open replica");
    for key in [
        "indexes/timeline/a",
        "indexes/timeline/b",
        "objects/a/state",
    ] {
        docs.apply_doc_op(
            &replica,
            kukuri_docs_sync::DocOp::SetJson {
                key: key.into(),
                value: serde_json::json!({}),
            },
        )
        .await
        .expect("write entry");
    }

    let entries = docs
        .query_replica_keys(
            &replica,
            kukuri_docs_sync::DocKeyQuery {
                prefix: "indexes/timeline/".into(),
                order: kukuri_docs_sync::DocKeyOrder::Descending,
                limit: 1,
            },
        )
        .await
        .expect("bounded key query must be forwarded to the inner docs sync");

    // #1257: 打ち切りの情報も、内側の結果をそのまま返す。
    assert!(entries.reached_limit);
    assert_eq!(
        entries
            .entries
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>(),
        vec!["indexes/timeline/b"]
    );
    docs.current().await.shutdown().await;
    node.shutdown().await.expect("shutdown node");
}

// #1152 / ADR 0046 §6.2: desktop が実際に使う `ReloadableBlobService` 越しでも、
// ephemeral 取得は remote の bytes をローカルへ保存せず、状態確認は remote から取得しない。
// 既定実装へ落ちる method があると、黙って永続化する `fetch_blob` に戻る。
