use std::sync::Arc;

use anyhow::Result;
use kukuri_cn_core::IndexScopeKind;
use kukuri_cn_indexer::search::{SearchIndex, SearchProjection, SearchReader};
use kukuri_cn_indexer::{IndexProjection, IndexedEntry, MemoryIndexProjection};

mod ingest_support;

fn entry(scope: &str, id: usize, created_at: i64) -> IndexedEntry {
    IndexedEntry {
        scope_kind: IndexScopeKind::PublicTopic,
        scope_id: scope.into(),
        object_id: id.to_string(),
        author_pubkey: "author".into(),
        text: "京都の天気とＫＵＫＵＲＩを共有".into(),
        created_at,
        source_replica_id: "replica".into(),
        content_advisories: vec![],
    }
}

#[tokio::test]
async fn writes_and_bounded_deletes_reach_an_independent_reader() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let search = Arc::new(SearchIndex::open(dir.path())?);
    let projection = SearchProjection::new(Arc::new(MemoryIndexProjection::new()), search.clone());
    let reader = SearchReader::open(dir.path())?;
    for id in 0..300 {
        projection.upsert_entry(&entry("first", id, 1)).await?;
    }
    projection.upsert_entry(&entry("kept", 0, 20)).await?;
    search.commit()?;
    reader.reload()?;
    assert_eq!(reader.search(None, "kukuri", 100)?.len(), 100);
    for id in 300..302 {
        projection.upsert_entry(&entry("first", id, 1)).await?;
    }
    // commit 前の書込みも削除対象に含め、同じページを繰り返し数えない。
    for expected in [128, 128, 46, 0] {
        assert_eq!(
            projection
                .remove_scope_page(IndexScopeKind::PublicTopic, "first", 128)
                .await?,
            expected
        );
    }
    search.commit()?;
    reader.reload()?;
    assert_eq!(reader.search(None, "kukuri", 100)?.len(), 1);
    let mut replacement = entry("kept", 0, 20);
    replacement.text = "東京タワー".into();
    projection.upsert_entry(&replacement).await?;
    search.commit()?;
    reader.reload()?;
    assert!(reader.search(None, "kukuri", 100)?.is_empty());
    assert_eq!(reader.search(None, "東京タワー", 100)?.len(), 1);
    projection
        .remove_object(IndexScopeKind::PublicTopic, "kept", "0")
        .await?;
    search.commit()?;
    reader.reload()?;
    assert!(reader.search(None, "東京タワー", 100)?.is_empty());
    for id in 0..300 {
        projection.upsert_entry(&entry("expired", id, 1)).await?;
    }
    projection.upsert_entry(&entry("kept", 0, 20)).await?;
    for expected in [128, 128, 44, 0] {
        assert_eq!(projection.remove_older_than(10, 128).await?, expected);
    }
    search.commit()?;
    reader.reload()?;
    assert_eq!(reader.search(None, "京都の天気", 100)?.len(), 1);
    assert!(!search.commit()?, "変更が無い間は commit しない");
    Ok(())
}

#[tokio::test]
async fn ingestion_withdrawal_and_deindex_update_search() -> Result<()> {
    use kukuri_cn_core::MemoryIndexEntryStore;
    use kukuri_cn_indexer::IngestPipeline;
    use kukuri_core::{TopicId, WithdrawalReasonVisibility, build_post_withdrawal_envelope};
    use kukuri_docs_sync::{DocOp, DocsSync, MemoryDocsSync, topic_replica_id};

    let dir = tempfile::tempdir()?;
    let search = Arc::new(SearchIndex::open(dir.path())?);
    let projection = Arc::new(SearchProjection::new(
        Arc::new(MemoryIndexProjection::new()),
        search.clone(),
    ));
    let reader = SearchReader::open(dir.path())?;
    let docs = Arc::new(MemoryDocsSync::default());
    let replica = topic_replica_id("rust");
    let (id, keys, envelope, _) = ingest_support::persist_post_with_source(
        &docs,
        &replica,
        &TopicId::new("rust"),
        "京都の天気",
    )
    .await;
    let (service, store) = ingest_support::allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(store));
    let pipeline = IngestPipeline::new(docs.clone(), service, entries.clone(), projection.clone());
    pipeline
        .ingest_changed_keys(IndexScopeKind::PublicTopic, "rust", &replica, &[])
        .await?;
    search.commit()?;
    reader.reload()?;
    assert_eq!(reader.search(None, "京都の天気", 100)?.len(), 1);
    let (service, _) = ingest_support::service_with(
        kukuri_cn_safety::MockSafetyProvider::known_csam("mock-known-csam")
            .with_known_hash_match(&id),
    );
    IngestPipeline::new(docs.clone(), service, entries, projection.clone())
        .ingest_changed_keys(IndexScopeKind::PublicTopic, "rust", &replica, &[])
        .await?;
    search.commit()?;
    reader.reload()?;
    assert!(reader.search(None, "京都の天気", 100)?.is_empty());
    // allow の書込みを戻し、実際の署名済み撤回を同じ pipeline へ渡す。
    pipeline
        .ingest_changed_keys(IndexScopeKind::PublicTopic, "rust", &replica, &[])
        .await?;
    search.commit()?;
    reader.reload()?;
    assert_eq!(reader.search(None, "京都の天気", 100)?.len(), 1);
    let withdrawal = build_post_withdrawal_envelope(
        &keys,
        &envelope,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(kukuri_core::PostWithdrawalReason::AuthorRequest),
    )?;
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: format!("withdrawals/{id}/state"),
            value: serde_json::to_value(withdrawal)?,
        },
    )
    .await?;
    pipeline
        .ingest_changed_keys(IndexScopeKind::PublicTopic, "rust", &replica, &[])
        .await?;
    search.commit()?;
    reader.reload()?;
    assert!(reader.search(None, "京都の天気", 100)?.is_empty());
    Ok(())
}
