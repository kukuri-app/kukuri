//! 空の専用 ArcadeDB 26.10.1 で実行する（ほかの ArcadeDB 試験と並行させない）。
use std::sync::Arc;

use anyhow::Result;
use kukuri_cn_core::IndexScopeKind;
use kukuri_cn_indexer::backfill::BackfillCursor;
use kukuri_cn_indexer::search::{SearchIndex, SearchProjection, SearchReader};
use kukuri_cn_indexer::{ArcadeDbConfig, ArcadeDbProjection, IndexProjection, IndexedEntry};

mod arcadedb_support;
use arcadedb_support::{command, read_records as reads};

fn entry(i: usize) -> IndexedEntry {
    IndexedEntry {
        scope_kind: if i.is_multiple_of(3) {
            IndexScopeKind::PrivateChannel
        } else {
            IndexScopeKind::PublicTopic
        },
        scope_id: format!("backfill-{}", i % 7),
        object_id: i.to_string(),
        author_pubkey: "author".into(),
        text: format!("移行の投稿 marker{i:06}"),
        created_at: (i % 2 + 1) as i64,
        source_replica_id: "replica".into(),
        content_advisories: vec![],
    }
}

#[tokio::test]
async fn backfill_resumes_copies_all_ties_and_reads_a_bounded_page() -> Result<()> {
    if !kukuri_test_support::env_flag_enabled("KUKURI_CN_RUN_ARCADEDB_TESTS") {
        eprintln!(
            "skipping backfill test; set KUKURI_CN_RUN_ARCADEDB_TESTS=1 with an empty dedicated ArcadeDB"
        );
        return Ok(());
    }
    let config = ArcadeDbConfig::from_env();
    let source = Arc::new(ArcadeDbProjection::new(config.clone())?);
    source.ensure_schema().await?;
    anyhow::ensure!(
        source.count_all().await? == 0,
        "backfill test requires an empty dedicated ArcadeDB"
    );
    for i in 0..300 {
        source.upsert_entry(&entry(i)).await?;
    }
    let dir = tempfile::tempdir()?;
    let saved;
    {
        let search = Arc::new(SearchIndex::open(dir.path())?);
        let projection = SearchProjection::new(source.clone(), search.clone());
        let before = reads(&config).await?;
        assert_eq!(projection.backfill_page(&source).await?, 128);
        assert_eq!(reads(&config).await? - before, 128);
        saved = search.backfill_cursor()?;
    }
    {
        let search = Arc::new(SearchIndex::open(dir.path())?);
        assert_eq!(search.backfill_cursor()?, saved);
        let projection = SearchProjection::new(source.clone(), search.clone());
        while search.backfill_cursor()? != BackfillCursor::Complete {
            let before = reads(&config).await?;
            let count = projection.backfill_page(&source).await?;
            assert!(count <= 128);
            assert_eq!(reads(&config).await? - before, count as u64);
        }
        let reader = SearchReader::open(dir.path())?;
        for i in 0..300 {
            let entry = entry(i);
            let hits = reader.search(
                Some((entry.scope_kind, &entry.scope_id)),
                &format!("marker{i:06}"),
                100,
            )?;
            assert_eq!(hits.len(), 1, "post {i}");
            assert_eq!(hits[0].object_id, entry.object_id);
        }
        // live の commit が、写し終わりの記録を消さない。
        projection.upsert_entry(&entry(0)).await?;
        search.commit()?;
    }
    {
        let search = Arc::new(SearchIndex::open(dir.path())?);
        let projection = SearchProjection::new(source.clone(), search.clone());
        assert_eq!(search.backfill_cursor()?, BackfillCursor::Complete);
        let before = reads(&config).await?;
        assert_eq!(projection.backfill_page(&source).await?, 0);
        assert_eq!(reads(&config).await?, before);
    }
    for i in 0..300 {
        let e = entry(i);
        source
            .remove_object(e.scope_kind, &e.scope_id, &e.object_id)
            .await?;
    }
    // 実投影で2,000→20,000件。全件に同じ作成時刻を持たせる。
    let mut inserted = 0;
    for total in [2_000, 20_000] {
        while inserted < total {
            let docs: Vec<_> = (inserted..inserted + 1_000)
                .map(|i| {
                    let mut e = entry(i);
                    e.created_at = 1;
                    e
                })
                .collect();
            command(
                &config,
                "sqlscript",
                &format!(
                    "INSERT INTO IndexedEntry CONTENT {}; RETURN 1;",
                    serde_json::to_string(&docs)?
                ),
            )
            .await?;
            inserted += 1_000;
        }
        let before = reads(&config).await?;
        let page = source.read_backfill_page(&BackfillCursor::Start).await?;
        assert_eq!(page.entries.len(), 128);
        assert_eq!(reads(&config).await? - before, 128, "{total} entries");
        let before = reads(&config).await?;
        assert_eq!(
            source.read_backfill_page(&page.cursor).await?.entries.len(),
            128
        );
        assert_eq!(
            reads(&config).await? - before,
            128,
            "resume at {total} entries"
        );
    }
    for kind in [IndexScopeKind::PublicTopic, IndexScopeKind::PrivateChannel] {
        for i in 0..7 {
            while source
                .remove_scope_page(kind, &format!("backfill-{i}"), 128)
                .await?
                != 0
            {}
        }
    }
    assert_eq!(source.count_all().await?, 0);
    Ok(())
}
