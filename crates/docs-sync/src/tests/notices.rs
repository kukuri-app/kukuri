//! #1239: replica の購読が、取りこぼしと同期の区切りを受け取る。

use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use kukuri_iroh_node::IrohDocsNode;

use crate::{DocOp, DocsSync, IrohDocsSync, MemoryDocsSync, ReplicaNotice, topic_replica_id};

/// 購読側が読まないうちに、buffer(256 件)を超える entry を書く。
async fn overflow(docs: &dyn DocsSync, replica: &kukuri_core::ReplicaId) -> Result<()> {
    for index in 0..300usize {
        docs.apply_doc_op(
            replica,
            DocOp::SetJson {
                key: format!("objects/{index:04}/state"),
                value: serde_json::json!({}),
            },
        )
        .await?;
    }
    Ok(())
}

async fn assert_overflow_is_reported(docs: &dyn DocsSync, name: &str) -> Result<()> {
    let replica = topic_replica_id(format!("kukuri:topic:notices-{name}").as_str());
    let mut notices = docs.subscribe_replica_notices(&replica).await?;
    let mut entries = docs.subscribe_replica(&replica).await?;
    overflow(docs, &replica).await?;

    let first = tokio::time::timeout(Duration::from_secs(5), notices.next())
        .await?
        .expect("stream is open")?;
    let ReplicaNotice::Lagged { missed } = first else {
        panic!("the overflow must be reported first: {first:?}");
    };
    assert!(
        missed >= 44,
        "300 entries through a buffer of 256: {missed}"
    );
    // 取りこぼしの後は、buffer に残った entry が順に届く。
    let next = tokio::time::timeout(Duration::from_secs(5), notices.next())
        .await?
        .expect("stream is open")?;
    assert!(matches!(next, ReplicaNotice::Entry(_)), "{next:?}");

    // entry だけの購読(`subscribe_replica`)は、取りこぼしを流さない(従来の挙動)。
    let event = tokio::time::timeout(Duration::from_secs(5), entries.next())
        .await?
        .expect("stream is open")?;
    assert!(event.key.starts_with("objects/"));
    Ok(())
}

#[tokio::test]
async fn overflow_is_reported_as_lagged_on_memory_docs() -> Result<()> {
    assert_overflow_is_reported(&MemoryDocsSync::default(), "memory").await
}

#[tokio::test]
async fn overflow_is_reported_as_lagged_on_iroh_docs() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let result = assert_overflow_is_reported(&docs, "iroh").await;
    docs.shutdown().await;
    node.shutdown().await?;
    result
}

/// 通知を知らせない実装(trait の既定実装)は、entry だけを流す。
#[tokio::test]
async fn default_notices_carry_entries_only() -> Result<()> {
    struct EntriesOnly(MemoryDocsSync);

    #[async_trait::async_trait]
    impl DocsSync for EntriesOnly {
        async fn open_replica(&self, replica_id: &kukuri_core::ReplicaId) -> Result<()> {
            self.0.open_replica(replica_id).await
        }
        async fn apply_doc_op(&self, replica_id: &kukuri_core::ReplicaId, op: DocOp) -> Result<()> {
            self.0.apply_doc_op(replica_id, op).await
        }
        async fn query_replica_with_policy(
            &self,
            replica_id: &kukuri_core::ReplicaId,
            query: crate::DocQuery,
            policy: crate::DocFetchPolicy,
        ) -> Result<Vec<crate::DocRecord>> {
            self.0
                .query_replica_with_policy(replica_id, query, policy)
                .await
        }
        async fn subscribe_replica(
            &self,
            replica_id: &kukuri_core::ReplicaId,
        ) -> Result<crate::DocEventStream> {
            self.0.subscribe_replica(replica_id).await
        }
        async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
            self.0.import_peer_ticket(ticket).await
        }
    }

    let docs = EntriesOnly(MemoryDocsSync::default());
    let replica = topic_replica_id("kukuri:topic:notices-default");
    let mut notices = docs.subscribe_replica_notices(&replica).await?;
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: "objects/a/state".into(),
            value: serde_json::json!({}),
        },
    )
    .await?;
    let notice = tokio::time::timeout(Duration::from_secs(5), notices.next())
        .await?
        .expect("stream is open")?;
    assert!(
        matches!(&notice, ReplicaNotice::Entry(event) if event.key == "objects/a/state"),
        "{notice:?}"
    );
    Ok(())
}
