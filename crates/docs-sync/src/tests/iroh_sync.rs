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
    // #1407: 手元に無い namespace は読取りで作らない(作られなければ同期も始まらない)。
    let created = node.docs().open(namespace).await.is_ok();
    docs.shutdown().await;
    node.shutdown().await?;
    assert!(!created, "a LocalOnly lookup must not create the namespace");
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
    // #1407: 手元に無い namespace は読取りで作らない。
    assert!(
        node.docs().open(namespace).await.is_err(),
        "reads must not create the namespace"
    );
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
    // R5-H: 旧 sync は撤去した。書込みで開いた namespace も同期を始めない(旧 replica の同期 I/O は 0)。
    assert!(!probe.status().await?.sync, "writes must not start sync");
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

// 手元に無い namespace を「無い」と答え、作らない。書いた namespace は、handle を閉じた後も「ある」と答える(#1221 R5-H)。
#[tokio::test]
async fn has_local_replica_does_not_create_a_namespace() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let missing = author_replica_id("aa".repeat(32).as_str());
    assert!(!docs.has_local_replica(&missing).await?);
    let namespace = crate::replicas::public_replica_secret(&missing)
        .unwrap()
        .id();
    assert!(node.docs().open(namespace).await.ok().flatten().is_none());
    let written = author_replica_id("bb".repeat(32).as_str());
    docs.apply_doc_op(
        &written,
        DocOp::SetJson {
            key: "profile/latest".into(),
            value: serde_json::json!({}),
        },
    )
    .await?;
    docs.close_replica(&written).await?;
    assert!(docs.has_local_replica(&written).await?);
    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

// ADR 0061 §1: account 同期は、アカウント鍵だけから導出した namespace を登録して使う（通常の epoch と独立）。
// 登録が無いと書けず、登録すれば封をした item を書いて読み戻せる。
#[tokio::test]
async fn the_account_sync_replica_needs_the_derived_secret() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let keys = kukuri_core::KukuriKeys::parse(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
    )?;
    let derived = keys.derive_account_sync();
    let item = kukuri_core::AccountSyncItem {
        key: kukuri_core::AccountSyncItemKey::Profile,
        op_id: "0123456789abcdef0123456789abcdef".to_string(),
        updated_at: 1,
        value: Some(serde_json::json!({ "name": "alice" })),
    };
    let docs_key = item.key.docs_key();
    let sealed = serde_json::to_value(derived.seal(&keys.public_key(), &item)?)?;
    let write = |value: serde_json::Value| DocOp::SetJson {
        key: docs_key.clone(),
        value,
    };
    assert!(
        docs.apply_doc_op(derived.replica_id(), write(sealed.clone()))
            .await
            .is_err(),
        "the account replica must not fall back to a public namespace"
    );
    docs.register_private_replica_secret(
        derived.replica_id(),
        &derived.expose_namespace_secret_hex(),
    )
    .await?;
    docs.apply_doc_op(derived.replica_id(), write(sealed))
        .await?;
    let records = docs
        .query_replica_with_policy(
            derived.replica_id(),
            DocQuery::Exact(docs_key.clone()),
            crate::DocFetchPolicy::LocalOnly,
        )
        .await?;
    assert_eq!(records.len(), 1);
    let read: kukuri_core::SealedAccountSyncItem = serde_json::from_slice(&records[0].value)?;
    assert_eq!(derived.open(&keys.public_key(), &docs_key, &read)?, item);
    node.shutdown().await?;
    Ok(())
}
