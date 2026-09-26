use anyhow::Result;
use kukuri_iroh_node::IrohDocsNode;

use crate::{
    DocOp, DocQuery, DocsSync, IrohDocsSync, author_replica_id, device_replica_id, topic_replica_id,
};

#[tokio::test]
async fn local_only_bucket_lookup_does_not_start_replica_sync() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    // 旧形式も含むLocalOnlyの契約。statusは実iroh-docsの同期状態を読む。
    let replica = topic_replica_id("kukuri:topic:local-only-bucket-contract");
    let rows = docs
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact("objects/missing/state".into()),
            crate::DocFetchPolicy::LocalOnly,
        )
        .await?;
    assert!(rows.is_empty());
    let namespace = crate::replicas::public_replica_secret(&replica)
        .unwrap()
        .id();
    let doc = node.docs().open(namespace).await?.expect("local namespace");
    let syncing = doc.status().await?.sync;
    doc.close().await?;
    docs.shutdown().await;
    node.shutdown().await?;
    assert!(!syncing, "a LocalOnly lookup must not start docs sync");
    Ok(())
}

#[tokio::test]
async fn local_bucket_reads_stay_idle_after_seed_reapply_and_close_preserves_data() -> Result<()> {
    use crate::{DocFetchPolicy, DocKeyOrder, DocKeyQuery};
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let replica = topic_replica_id("kukuri:topic:bucket-lifecycle");
    let namespace = crate::replicas::public_replica_secret(&replica)
        .unwrap()
        .id();
    let author = iroh_docs::Author::from_bytes(&[1; 32]).id().to_string();
    docs.query_replica_by_author(&replica, &author, "missing", DocFetchPolicy::LocalOnly)
        .await?;
    docs.query_replica_exact_bounded(&replica, "missing", 1, DocFetchPolicy::LocalOnly)
        .await?;
    docs.query_replica_keys(
        &replica,
        DocKeyQuery {
            prefix: "objects/".into(),
            order: DocKeyOrder::Ascending,
            limit: 1,
        },
    )
    .await?;
    docs.query_replica_keys_by_author(
        &replica,
        "invalid author",
        DocKeyQuery {
            prefix: "objects/".into(),
            order: DocKeyOrder::Ascending,
            limit: 1,
        },
    )
    .await?;
    docs.set_seed_peers(Vec::new()).await?;
    let probe = node.docs().open(namespace).await?.unwrap();
    assert!(
        !probe.status().await?.sync,
        "read or peer reapply started idle sync"
    );
    probe.close().await?;
    docs.open_replica(&replica).await?;
    docs.apply_doc_op(
        &replica,
        DocOp::SetBytes {
            key: "kept".into(),
            value: b"value".to_vec(),
        },
    )
    .await?;
    let probe = node.docs().open(namespace).await?.unwrap();
    assert!(probe.status().await?.sync, "explicit open must enable sync");
    probe.close().await?;
    docs.close_replica(&replica).await?;
    docs.close_replica(&replica).await?;
    let rows = docs
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact("kept".into()),
            DocFetchPolicy::LocalOnly,
        )
        .await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].value, b"value");
    docs.set_seed_peers(Vec::new()).await?;
    let probe = node.docs().open(namespace).await?.unwrap();
    assert!(
        !probe.status().await?.sync,
        "reopening a saved local row restarted sync"
    );
    probe.close().await?;
    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn docs_topic_index_roundtrip() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let replica = topic_replica_id("kukuri:topic:docs");

    docs.open_replica(&replica).await?;
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: crate::stable_key("timeline", "0001-event"),
            value: serde_json::json!({
                "object_id": "event-1",
                "topic_id": "kukuri:topic:docs"
            }),
        },
    )
    .await?;

    let rows = docs
        .query_replica(&replica, DocQuery::Prefix("timeline/".into()))
        .await?;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, "timeline/0001-event");
    assert!(String::from_utf8(rows[0].value.clone())?.contains("event-1"));

    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn private_cursor_not_in_public_replica() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let topic_replica = topic_replica_id("kukuri:topic:docs");
    let author_replica = author_replica_id("f".repeat(64).as_str());
    let device_replica = device_replica_id("f".repeat(64).as_str(), "device-a");

    docs.open_replica(&topic_replica).await?;
    docs.open_replica(&author_replica).await?;
    docs.open_replica(&device_replica).await?;

    docs.apply_doc_op(
        &device_replica,
        DocOp::SetJson {
            key: "cursor/topic/kukuri:topic:docs".into(),
            value: serde_json::json!({ "created_at": 1 }),
        },
    )
    .await?;

    let topic_rows = docs
        .query_replica(&topic_replica, DocQuery::Prefix("cursor/".into()))
        .await?;
    let author_rows = docs
        .query_replica(&author_replica, DocQuery::Prefix("cursor/".into()))
        .await?;
    let device_rows = docs
        .query_replica(&device_replica, DocQuery::Prefix("cursor/".into()))
        .await?;

    assert!(topic_rows.is_empty());
    assert!(author_rows.is_empty());
    assert_eq!(device_rows.len(), 1);
    assert_eq!(device_rows[0].key, "cursor/topic/kukuri:topic:docs");

    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

// #1239 の計測用。`cargo test -p kukuri-docs-sync --lib measure_exact_query_cost -- --ignored --nocapture` で実行する。
// 1 つの replica の entry 数を増やし、key を 1 つ指定した読み出しの所要時間が総数に依存するかを出す。
#[tokio::test]
#[ignore = "measurement only"]
async fn measure_exact_query_cost() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    for count in [1_000usize, 10_000] {
        let replica = topic_replica_id(format!("kukuri:topic:exact-cost-{count}").as_str());
        docs.open_replica(&replica).await?;
        for index in 0..count {
            docs.apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: crate::stable_key("objects", &format!("{index:08}/state")),
                    value: serde_json::json!({ "index": index }),
                },
            )
            .await?;
        }
        let target = crate::stable_key("objects", &format!("{:08}/state", count / 2));
        let started = std::time::Instant::now();
        let rounds = 50;
        for _ in 0..rounds {
            let rows = docs
                .query_replica(&replica, DocQuery::Exact(target.clone()))
                .await?;
            assert_eq!(rows.len(), 1);
        }
        println!(
            "[exact-cost] entries={count} exact query avg={:?}",
            started.elapsed() / rounds
        );
    }
    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

// #1221 R5-G: 旧領域から保護所有先へ移した private の record は、旧領域が無くても key 指定の読み出しで読める。
// capability を外した後は読めない。
#[tokio::test]
async fn private_exact_reads_include_moved_records_only_with_the_capability() {
    let node = IrohDocsNode::memory().await.expect("docs node");
    let store = std::sync::Arc::new(
        kukuri_store::SqliteStore::connect_memory()
            .await
            .expect("store"),
    );
    let docs = IrohDocsSync::with_account_store(node.clone(), store.clone());
    let replica = crate::private_channel_epoch_replica_id("channel", "epoch");
    let key = "channels/metadata";
    let value = br#"{"moved":true}"#.to_vec();
    let record = kukuri_iroh_node::DocReadRecord {
        key: key.into(),
        value: value.clone(),
        content_hash: blake3::hash(&value).to_hex().to_string(),
        content_len: value.len() as u64,
        docs_author: "owner-author".into(),
    };
    store
        .put_remote_record(
            replica.as_str(),
            key,
            "owner-author",
            &serde_json::to_vec(&record).expect("record"),
        )
        .await
        .expect("cache record");
    docs.register_private_replica_secret(&replica, &hex::encode([7u8; 32]))
        .await
        .expect("capability");
    let exact = docs
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact(key.into()),
            crate::DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("exact read");
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].value, value);
    let bounded = docs
        .query_replica_exact_bounded(&replica, key, 8, crate::DocFetchPolicy::LocalOnly)
        .await
        .expect("bounded read");
    assert_eq!(bounded.len(), 1);
    docs.remove_private_replica_secret(&replica)
        .await
        .expect("remove capability");
    assert!(
        docs.query_replica_with_policy(
            &replica,
            DocQuery::Exact(key.into()),
            crate::DocFetchPolicy::LocalOnly,
        )
        .await
        .is_err()
    );
    docs.shutdown().await;
    node.shutdown().await.expect("shutdown");
}
