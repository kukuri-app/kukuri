//! #1239 / ADR 0053 §6: 自分の replica の、ADR 0053 以前の端末ごとの名義(旧名義)で書かれた edge も、背景の読み出しで読む
//! (独立監査 B-5)。読み出しの中では書き直さない(独立監査 B-6。同期の途中で古い状態を新しい時刻で書き戻すため)。

use super::*;
use crate::service::author_state_support::sweep_own_author_edges_with;
use std::collections::HashSet;

const ACCOUNT_DOCS_AUTHOR: &str =
    "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

/// 追跡を始める前に書いた record は旧名義、始めた後に書いた key は自分の docs author の名義として振る舞う docs。
#[derive(Clone, Default)]
struct LegacyNameDocsSync {
    inner: CountingDocsSync,
    tracking: Arc<std::sync::atomic::AtomicBool>,
    account_keys: Arc<TokioMutex<HashSet<String>>>,
}

impl LegacyNameDocsSync {
    fn start_tracking(&self) {
        self.tracking
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    async fn rewritten(&self, prefix: &str) -> usize {
        self.account_keys
            .lock()
            .await
            .iter()
            .filter(|key| key.starts_with(prefix))
            .count()
    }
}

#[async_trait]
impl DocsSync for LegacyNameDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        if self.tracking.load(std::sync::atomic::Ordering::SeqCst)
            && let DocOp::SetJson { key, .. } = &op
        {
            self.account_keys.lock().await.insert(key.clone());
        }
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn query_replica_keys_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        if docs_author != ACCOUNT_DOCS_AUTHOR {
            return Ok(kukuri_docs_sync::DocKeyPage::default());
        }
        let account_keys = self.account_keys.lock().await.clone();
        let mut page = self.inner.query_replica_keys(replica_id, query).await?;
        page.entries
            .retain(|entry| account_keys.contains(&entry.key));
        Ok(page)
    }

    async fn local_docs_author(&self) -> Result<Option<String>> {
        Ok(Some(ACCOUNT_DOCS_AUTHOR.to_string()))
    }

    async fn query_replica_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        key: &str,
        policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Option<kukuri_docs_sync::DocRecord>> {
        if docs_author != ACCOUNT_DOCS_AUTHOR || !self.account_keys.lock().await.contains(key) {
            return Ok(None);
        }
        Ok(self
            .inner
            .query_replica_with_policy(replica_id, DocQuery::Exact(key.to_string()), policy)
            .await?
            .into_iter()
            .next())
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

// 旧名義の自分の follow は、背景の読み出しで入る。読み出しは docs に何も書かない(旧名義の edge を書き直さない)。
#[tokio::test]
async fn own_legacy_edges_are_read_without_being_rewritten() {
    let docs_sync = Arc::new(LegacyNameDocsSync::default());
    let local_keys = generate_keys();
    let local_author_pubkey = local_keys.public_key_hex();
    let edges = 20;
    for _ in 0..edges {
        let envelope = build_follow_edge_envelope(
            &local_keys,
            &Pubkey::from(generate_keys().public_key_hex().as_str()),
            FollowEdgeStatus::Active,
        )
        .expect("legacy follow");
        let edge = parse_follow_edge(&envelope)
            .expect("parse")
            .expect("follow");
        persist_follow_edge_doc(docs_sync.as_ref(), &edge, &envelope)
            .await
            .expect("persist legacy follow");
    }
    docs_sync.start_tracking();
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        local_keys,
    );

    let outcome = sweep_own_author_edges_with(&app.services, local_author_pubkey.as_str(), 16, 64)
        .await
        .expect("sweep");

    assert_eq!(outcome.reflected, edges, "legacy edges are read");
    assert_eq!(
        store
            .list_follow_edges_by_subject(local_author_pubkey.as_str())
            .await
            .expect("own follows")
            .len(),
        edges
    );
    assert_eq!(
        docs_sync.rewritten("").await,
        0,
        "the reading does not write to docs"
    );
}
