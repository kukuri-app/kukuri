//! 同じ key に複数の docs author の record を返す docs の test double(#1248・#1250・#1258)。

use super::*;

/// 同じ key に複数の record を返す docs(iroh-docs は、同じ key の entry を docs author ごとに持つ)。
/// `shadows` に入れた値を、その key の正しい record より先に返す。
#[derive(Clone, Default)]
pub(super) struct ShadowingDocsSync {
    inner: MemoryDocsSync,
    shadows: Arc<TokioMutex<HashMap<String, Vec<Vec<u8>>>>>,
    /// この key の読み出しを失敗させる(I/O の失敗)。
    pub(super) failing_key: Arc<TokioMutex<Option<String>>>,
    /// 上限つきの読み出しの key と上限(#1250)。
    pub(super) bounded_reads: Arc<TokioMutex<Vec<(String, usize)>>>,
    /// `inner` の record を書いた docs author(#1258)。`None` なら docs author を持たない docs として振る舞う。
    /// shadow は、key ごとに `shadow_docs_author(n)`(この docs author より前に並ぶ別の名義)が書いたものとして返す。
    pub(super) account_docs_author: Option<String>,
    /// 「docs author と key の組」の読み出しの記録(#1258)。
    pub(super) author_reads: Arc<TokioMutex<Vec<(String, String)>>>,
}

/// `ShadowingDocsSync` が n 番目の shadow の名義として返す docs author の id。
pub(super) fn shadow_docs_author(index: usize) -> String {
    format!("{index:064x}")
}

impl ShadowingDocsSync {
    /// `inner` の record が、この docs author の名義で書かれている docs。
    pub(super) fn with_account_docs_author(docs_author: impl Into<String>) -> Self {
        Self {
            account_docs_author: Some(docs_author.into()),
            ..Self::default()
        }
    }

    pub(super) async fn shadow(&self, key: &str, value: serde_json::Value) {
        self.shadows
            .lock()
            .await
            .entry(key.to_string())
            .or_default()
            .push(serde_json::to_vec(&value).expect("shadow json"));
    }
}

#[async_trait]
impl DocsSync for ShadowingDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        if let DocQuery::Exact(key) = &query
            && self.failing_key.lock().await.as_deref() == Some(key.as_str())
        {
            anyhow::bail!("simulated docs read failure");
        }
        let exact_key = match &query {
            DocQuery::Exact(key) => Some(key.clone()),
            _ => None,
        };
        let records = self
            .inner
            .query_replica_with_policy(replica_id, query, policy)
            .await?;
        let shadows = self.shadows.lock().await;
        let mut merged = Vec::new();
        // docs author つきの docs では、`inner` に record が無い key にも、別の名義の record だけを置ける(#1258)。
        if let (true, Some(key), Some(_)) =
            (records.is_empty(), exact_key, &self.account_docs_author)
        {
            for (index, value) in shadows.get(key.as_str()).into_iter().flatten().enumerate() {
                merged.push(kukuri_docs_sync::DocRecord {
                    key: key.clone(),
                    content_hash: kukuri_docs_sync::value_hash(value),
                    content_len: value.len() as u64,
                    value: value.clone(),
                    docs_author: Some(shadow_docs_author(index)),
                });
            }
        }
        for mut record in records {
            for (index, value) in shadows
                .get(record.key.as_str())
                .into_iter()
                .flatten()
                .enumerate()
            {
                merged.push(kukuri_docs_sync::DocRecord {
                    key: record.key.clone(),
                    content_hash: kukuri_docs_sync::value_hash(value),
                    content_len: value.len() as u64,
                    value: value.clone(),
                    docs_author: self
                        .account_docs_author
                        .as_ref()
                        .map(|_| shadow_docs_author(index)),
                });
            }
            record.docs_author = self.account_docs_author.clone();
            merged.push(record);
        }
        Ok(merged)
    }

    async fn local_docs_author(&self) -> Result<Option<String>> {
        Ok(self.account_docs_author.clone())
    }

    async fn query_replica_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<kukuri_docs_sync::DocRecord>> {
        self.author_reads
            .lock()
            .await
            .push((docs_author.to_string(), key.to_string()));
        // その名義の record だけを返す。同じ key の他の名義の record は、何件あっても読まない。
        Ok(self
            .query_replica_with_policy(replica_id, DocQuery::Exact(key.to_string()), policy)
            .await?
            .into_iter()
            .find(|record| record.docs_author.as_deref() == Some(docs_author)))
    }

    // #1239: タイムラインの取得は、ページの範囲を時系列の索引と照合する(key だけの上限つきの読み出し)。
    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn query_replica_exact_bounded(
        &self,
        replica_id: &ReplicaId,
        key: &str,
        limit: usize,
        policy: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        self.bounded_reads
            .lock()
            .await
            .push((key.to_string(), limit));
        let mut records = self
            .query_replica_with_policy(replica_id, DocQuery::Exact(key.to_string()), policy)
            .await?;
        records.truncate(limit);
        Ok(records)
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

pub(super) fn app_over_docs(docs_sync: Arc<dyn DocsSync>) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    (app, store)
}

pub(super) fn honest_header(envelope: &KukuriEnvelope) -> CanonicalPostHeader {
    envelope
        .to_post_object()
        .expect("post object")
        .expect("post object")
}
