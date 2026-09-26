//! Issue #1090: 取り込み途中の一時的な失敗と、確定した理由による de-index を区別する contract。
//!
//! 再走査の n 回目以降の照会・読み取りだけを失敗させ、参照再確認（`ReferenceGuard`）の全呼び出し
//! 位置で次を固定する。
//!   - 一時的な失敗: 索引済み entry は真実源・投影とも保持し、新規投稿は索引しない。
//!   - 確定した理由（再確認中の state 変化）: 従来どおり de-index する。
mod ingest_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use ingest_support::*;
use kukuri_cn_core::{
    IndexEntryStore, IndexScopeKind, MemoryIndexEntryStore, NewIndexEntry, SurfaceableEntry,
};
use kukuri_cn_indexer::ingest::{IngestPipeline, IngestSummary};
use kukuri_cn_indexer::projection::{IndexProjection, MemoryIndexProjection};
use kukuri_core::{ReplicaId, TopicId};
use kukuri_docs_sync::{
    DocFetchPolicy, DocOp, DocQuery, DocRecord, DocsSync, MemoryDocsSync, topic_replica_id,
};

/// 照会の障害の種類。
#[derive(Clone, Copy, Debug)]
enum Fault {
    /// 照会そのものが失敗する（一時的な失敗）。
    QueryError,
    /// state の照会が別内容の record を返す（再確認中の state 変化 = 確定した理由）。
    StateChanged,
}

/// 対象の照会を数え、`fail_from` 回目以降だけ障害を起こす docs 同期。
struct FaultyDocs {
    inner: Arc<MemoryDocsSync>,
    /// 障害の対象にする照会か。
    target: fn(&DocQuery, DocFetchPolicy) -> bool,
    fault: Fault,
    seen: AtomicUsize,
    bounded_withdrawal_reads: AtomicUsize,
    fail_from: AtomicUsize,
}

impl FaultyDocs {
    fn new(
        inner: Arc<MemoryDocsSync>,
        target: fn(&DocQuery, DocFetchPolicy) -> bool,
        fault: Fault,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            target,
            fault,
            seen: AtomicUsize::new(0),
            bounded_withdrawal_reads: AtomicUsize::new(0),
            fail_from: AtomicUsize::new(usize::MAX),
        })
    }

    /// 以後の対象照会を数え直し、`n` 回目（0 始まり）以降を障害にする。`None` で解除する。
    fn arm(&self, n: Option<usize>) {
        self.seen.store(0, Ordering::SeqCst);
        self.fail_from
            .store(n.unwrap_or(usize::MAX), Ordering::SeqCst);
    }

    fn seen(&self) -> usize {
        self.seen.load(Ordering::SeqCst)
    }

    fn bounded_withdrawal_reads(&self) -> usize {
        self.bounded_withdrawal_reads.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DocsSync for FaultyDocs {
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
    ) -> Result<Vec<DocRecord>> {
        let is_state = matches!(&query, DocQuery::Exact(key) if key.ends_with("/state"))
            && matches!(&query, DocQuery::Exact(key) if key.starts_with("objects/"));
        let counted = match self.fault {
            Fault::QueryError => (self.target)(&query, policy),
            Fault::StateChanged => (self.target)(&query, policy) && is_state,
        };
        let faulty = counted
            && self.seen.fetch_add(1, Ordering::SeqCst) >= self.fail_from.load(Ordering::SeqCst);
        if faulty && matches!(self.fault, Fault::QueryError) {
            anyhow::bail!("simulated transient replica query failure");
        }
        let mut records = self
            .inner
            .query_replica_with_policy(replica_id, query, policy)
            .await?;
        if faulty {
            for record in &mut records {
                record.content_hash = format!("changed-{}", record.content_hash);
            }
        }
        Ok(records)
    }

    async fn query_replica_exact_bounded(
        &self,
        replica_id: &ReplicaId,
        key: &str,
        limit: usize,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        if key.starts_with("withdrawals/") || key.starts_with("objects/") {
            assert!(limit <= 8);
            if key.starts_with("withdrawals/") {
                self.bounded_withdrawal_reads.fetch_add(1, Ordering::SeqCst);
            }
        }
        let mut records = self
            .query_replica_with_policy(replica_id, DocQuery::Exact(key.to_string()), policy)
            .await?;
        records.truncate(limit);
        Ok(records)
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
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

/// 参照再確認（`ReferenceGuard`）が使う照会。
fn guard_queries(_: &DocQuery, policy: DocFetchPolicy) -> bool {
    policy == DocFetchPolicy::LocalOnly
}

/// media manifest の照会（取り込み本体と参照再確認の両方）。
fn manifest_queries(query: &DocQuery, _: DocFetchPolicy) -> bool {
    matches!(query, DocQuery::Exact(key) if key.starts_with("manifests/media/"))
}

fn whole_object_or_withdrawal_queries(query: &DocQuery, _: DocFetchPolicy) -> bool {
    matches!(query, DocQuery::Prefix(prefix) if prefix.starts_with("objects/") || prefix == "withdrawals/")
}

/// n 回目以降の読み取りだけ失敗する真実源。
struct FlakyEntries {
    inner: Arc<MemoryIndexEntryStore>,
    seen: AtomicUsize,
    fail_from: AtomicUsize,
    failed: AtomicBool,
}

impl FlakyEntries {
    fn arm(&self, n: Option<usize>) {
        self.seen.store(0, Ordering::SeqCst);
        self.failed.store(false, Ordering::SeqCst);
        self.fail_from
            .store(n.unwrap_or(usize::MAX), Ordering::SeqCst);
    }

    fn read(&self) -> Result<()> {
        if self.seen.fetch_add(1, Ordering::SeqCst) >= self.fail_from.load(Ordering::SeqCst) {
            self.failed.store(true, Ordering::SeqCst);
            anyhow::bail!("simulated transient index store failure");
        }
        Ok(())
    }
}

#[async_trait]
impl IndexEntryStore for FlakyEntries {
    async fn is_scope_supported(&self, kind: IndexScopeKind, id: &str) -> Result<bool> {
        self.read()?;
        self.inner.is_scope_supported(kind, id).await
    }

    async fn upsert_entry(&self, entry: &NewIndexEntry) -> Result<()> {
        self.inner.upsert_entry(entry).await
    }

    async fn remove_entry(&self, kind: IndexScopeKind, id: &str, object_id: &str) -> Result<()> {
        self.inner.remove_entry(kind, id, object_id).await
    }

    async fn remove_scope(&self, kind: IndexScopeKind, id: &str) -> Result<()> {
        self.inner.remove_scope(kind, id).await
    }

    async fn record_verified_withdrawal(
        &self,
        kind: IndexScopeKind,
        id: &str,
        object_id: &str,
    ) -> Result<()> {
        self.inner
            .record_verified_withdrawal(kind, id, object_id)
            .await
    }

    async fn is_known_withdrawn(
        &self,
        kind: IndexScopeKind,
        id: &str,
        object_id: &str,
    ) -> Result<bool> {
        self.read()?;
        self.inner.is_known_withdrawn(kind, id, object_id).await
    }

    async fn filter_surfaceable(
        &self,
        kind: IndexScopeKind,
        candidates: &[(String, String)],
    ) -> Result<Vec<SurfaceableEntry>> {
        self.inner.filter_surfaceable(kind, candidates).await
    }

    async fn next_scope_after(
        &self,
        after_kind: &str,
        after_id: &str,
    ) -> Result<Option<(IndexScopeKind, String)>> {
        self.inner.next_scope_after(after_kind, after_id).await
    }

    async fn is_transmission_prevented(&self, object_id: &str) -> Result<bool> {
        self.read()?;
        self.inner.is_transmission_prevented(object_id).await
    }
}

struct Fixture {
    docs: Arc<MemoryDocsSync>,
    projection: Arc<MemoryIndexProjection>,
    entries: Arc<MemoryIndexEntryStore>,
    pipeline: IngestPipeline,
    replica: ReplicaId,
    topic: TopicId,
}

impl Fixture {
    fn new(faulty: Option<Arc<FaultyDocs>>, docs: Arc<MemoryDocsSync>) -> Self {
        let projection = Arc::new(MemoryIndexProjection::new());
        let (service, store) = allow_service();
        let entries = Arc::new(MemoryIndexEntryStore::new(store));
        let sync: Arc<dyn DocsSync> = match faulty {
            Some(faulty) => faulty,
            None => docs.clone(),
        };
        let pipeline = IngestPipeline::new(sync, service, entries.clone(), projection.clone());
        Self {
            docs,
            projection,
            entries,
            pipeline,
            replica: topic_replica_id("rust"),
            topic: TopicId::new("rust"),
        }
    }

    async fn ingest(&self) -> Result<IngestSummary> {
        self.pipeline
            .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &self.replica)
            .await
    }

    async fn is_indexed(&self, object_id: &str) -> Result<bool> {
        let truth = self
            .entries
            .contains(IndexScopeKind::PublicTopic, "rust", object_id);
        let projected = self
            .projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", object_id)
            .await?;
        assert_eq!(
            truth, projected,
            "truth and projection must agree for {object_id}"
        );
        Ok(truth)
    }
}

#[tokio::test]
async fn changed_object_reads_only_bounded_exact_keys() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let observed = FaultyDocs::new(
        docs.clone(),
        whole_object_or_withdrawal_queries,
        Fault::QueryError,
    );
    let fixture = Fixture::new(Some(observed.clone()), docs);
    let object_id = persist_post(&fixture.docs, &fixture.replica, &fixture.topic, "new post").await;

    observed.arm(Some(0));
    let summary = fixture
        .pipeline
        .ingest_changed_keys(
            IndexScopeKind::PublicTopic,
            "rust",
            &fixture.replica,
            &[format!("objects/{object_id}/state")],
        )
        .await?;
    assert_eq!((summary.scanned, summary.indexed), (1, 1));
    assert_eq!(observed.seen(), 0);
    assert!(observed.bounded_withdrawal_reads() >= 2);
    assert!(fixture.is_indexed(&object_id).await?);
    Ok(())
}

/// 索引済み投稿の再走査で、n 回目以降の参照再確認の照会が失敗しても entry を保持する。
/// 同じ走査に新規投稿が混ざっても、その投稿は索引しない。障害解除後は両方が索引される。
#[tokio::test]
async fn transient_guard_query_failure_keeps_indexed_post_and_holds_new_post() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let faulty = FaultyDocs::new(docs.clone(), guard_queries, Fault::QueryError);
    let fixture = Fixture::new(Some(faulty.clone()), docs);
    let indexed = persist_post(&fixture.docs, &fixture.replica, &fixture.topic, "indexed").await;
    assert_eq!(fixture.ingest().await?.indexed, 1);

    // 障害なしの再走査で参照再確認の照会回数を数える（全呼び出し位置を網羅する上限）。
    faulty.arm(None);
    assert_eq!(fixture.ingest().await?.indexed, 1);
    let per_pass = faulty.seen();
    assert!(per_pass > 0, "the rescan must recheck references");

    for n in 0..per_pass {
        faulty.arm(Some(n));
        let summary = fixture.ingest().await?;
        assert_eq!(summary.indexed, 0, "n={n}: {summary:?}");
        assert_eq!(summary.deindexed, 0, "n={n}: {summary:?}");
        assert!(
            fixture.is_indexed(&indexed).await?,
            "n={n}: a transient recheck failure must keep the indexed post"
        );
    }

    let fresh = persist_post(&fixture.docs, &fixture.replica, &fixture.topic, "fresh").await;
    faulty.arm(Some(0));
    let summary = fixture.ingest().await?;
    assert_eq!(summary.indexed, 0, "{summary:?}");
    assert!(fixture.is_indexed(&indexed).await?);
    assert!(
        !fixture.is_indexed(&fresh).await?,
        "a new post must not be indexed while rechecks fail"
    );

    faulty.arm(None);
    assert_eq!(fixture.ingest().await?.indexed, 2);
    assert!(fixture.is_indexed(&indexed).await?);
    assert!(fixture.is_indexed(&fresh).await?);
    Ok(())
}

/// 参照再確認で state が変わっていた場合（確定した理由）は、どの確認位置でも de-index する。
#[tokio::test]
async fn state_change_detected_by_any_recheck_deindexes_indexed_post() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let faulty = FaultyDocs::new(docs.clone(), guard_queries, Fault::StateChanged);
    let fixture = Fixture::new(Some(faulty.clone()), docs);
    let indexed = persist_post(&fixture.docs, &fixture.replica, &fixture.topic, "indexed").await;
    assert_eq!(fixture.ingest().await?.indexed, 1);

    faulty.arm(None);
    fixture.ingest().await?;
    let per_pass = faulty.seen();
    assert!(per_pass > 1, "state must be rechecked more than once");

    for n in 0..per_pass {
        faulty.arm(None);
        assert_eq!(fixture.ingest().await?.indexed, 1, "n={n}: restore");
        faulty.arm(Some(n));
        let summary = fixture.ingest().await?;
        assert_eq!(summary.indexed, 0, "n={n}: {summary:?}");
        assert!(
            !fixture.is_indexed(&indexed).await?,
            "n={n}: a state change during the recheck must de-index the post"
        );
    }
    Ok(())
}

/// 真実源の読み取り（送信防止・scope 対応の確認）が走査途中で失敗しても entry を保持する。
#[tokio::test]
async fn transient_index_store_read_failure_keeps_indexed_post() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let (service, store) = allow_service();
    let memory = Arc::new(MemoryIndexEntryStore::new(store));
    let entries = Arc::new(FlakyEntries {
        inner: memory.clone(),
        seen: AtomicUsize::new(0),
        fail_from: AtomicUsize::new(usize::MAX),
        failed: AtomicBool::new(false),
    });
    let pipeline = IngestPipeline::new(docs.clone(), service, entries.clone(), projection.clone());
    let replica = topic_replica_id("rust");
    let topic = TopicId::new("rust");
    let indexed = persist_post(&docs, &replica, &topic, "indexed").await;
    let ingest = || pipeline.ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica);
    assert_eq!(ingest().await?.indexed, 1);

    entries.arm(None);
    ingest().await?;
    let per_pass = entries.seen.load(Ordering::SeqCst);
    // 0 回目は走査開始前の scope 確認で、失敗すると走査全体が止まる（entry には触れない）。
    for n in 0..per_pass {
        entries.arm(Some(n));
        let outcome = ingest().await;
        assert!(
            entries.failed.load(Ordering::SeqCst),
            "n={n}: fault injected"
        );
        if let Ok(summary) = &outcome {
            assert_eq!(summary.indexed, 0, "n={n}: {summary:?}");
            assert_eq!(summary.deindexed, 0, "n={n}: {summary:?}");
        }
        assert!(
            memory.contains(IndexScopeKind::PublicTopic, "rust", &indexed),
            "n={n}: a transient store read failure must keep the truth entry"
        );
        assert!(
            projection
                .contains_object(IndexScopeKind::PublicTopic, "rust", &indexed)
                .await?,
            "n={n}: a transient store read failure must keep the projection"
        );
    }
    Ok(())
}

/// media manifest の照会が失敗しても、索引済みの media 投稿は保持する。
#[tokio::test]
async fn transient_manifest_query_failure_keeps_indexed_media_post() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let faulty = FaultyDocs::new(docs.clone(), manifest_queries, Fault::QueryError);
    let fixture = Fixture::new(Some(faulty.clone()), docs);
    let indexed = persist_media_post(
        &fixture.docs,
        &fixture.replica,
        &fixture.topic,
        "media caption",
        true,
    )
    .await;
    assert_eq!(fixture.ingest().await?.indexed, 1);

    faulty.arm(None);
    fixture.ingest().await?;
    let per_pass = faulty.seen();
    assert!(per_pass > 1, "manifest must be resolved and rechecked");

    for n in 0..per_pass {
        faulty.arm(Some(n));
        let summary = fixture.ingest().await?;
        assert_eq!(summary.indexed, 0, "n={n}: {summary:?}");
        assert!(
            fixture.is_indexed(&indexed).await?,
            "n={n}: a transient manifest query failure must keep the media post"
        );
    }
    Ok(())
}

/// 確定した理由（manifest が replica に無い）は、索引済みの media 投稿でも de-index する。
#[tokio::test]
async fn missing_manifest_still_deindexes_indexed_media_post() -> Result<()> {
    let docs = Arc::new(MemoryDocsSync::default());
    let fixture = Fixture::new(None, docs);
    let indexed = persist_media_post(
        &fixture.docs,
        &fixture.replica,
        &fixture.topic,
        "media caption",
        true,
    )
    .await;
    assert_eq!(fixture.ingest().await?.indexed, 1);
    fixture
        .docs
        .apply_doc_op(
            &fixture.replica,
            DocOp::DeletePrefix {
                prefix: format!("manifests/media/{MEDIA_MANIFEST_ID}/"),
            },
        )
        .await?;
    let summary = fixture.ingest().await?;
    assert_eq!(summary.indexed, 0, "{summary:?}");
    assert!(!fixture.is_indexed(&indexed).await?);
    Ok(())
}
