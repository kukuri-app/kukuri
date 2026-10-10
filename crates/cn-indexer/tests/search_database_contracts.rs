//! 検索の読取りが実 ArcadeDB の一致数で増えないことを確かめる。
use anyhow::Result;
use kukuri_cn_core::IndexScopeKind;
use kukuri_cn_indexer::search::SearchIndex;
use kukuri_cn_indexer::{ArcadeDbConfig, ArcadeDbProjection, IndexProjection, IndexedEntry};

mod arcadedb_support;
use arcadedb_support::{command, read_records as reads};

#[tokio::test]
async fn scoped_and_cross_search_never_read_arcadedb_documents() -> Result<()> {
    if !kukuri_test_support::env_flag_enabled("KUKURI_CN_RUN_ARCADEDB_TESTS") {
        return Ok(());
    }
    let config = ArcadeDbConfig::from_env();
    let source = ArcadeDbProjection::new(config.clone())?;
    source.ensure_schema().await?;
    command(
        &config,
        "sql",
        "CREATE INDEX IF NOT EXISTS ON IndexedEntry (text) FULL_TEXT ENGINE LUCENE",
    )
    .await?;
    let before = source.count_all().await?;
    source.ensure_schema().await?;
    assert_eq!(source.count_all().await?, before);
    let indexes = command(&config, "sql", "SELECT name FROM schema:indexes").await?;
    assert!(
        indexes["result"]
            .as_array()
            .expect("indexes")
            .iter()
            .all(|row| row["name"] != "IndexedEntry[text]")
    );
    let scope = format!(
        "search-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let dir = tempfile::tempdir()?;
    let index = SearchIndex::open(dir.path())?;
    let mut inserted = 0;
    for total in [2000, 20000] {
        while inserted < total {
            let entries: Vec<_> = (inserted..inserted + 1000)
                .map(|i| IndexedEntry {
                    scope_kind: IndexScopeKind::PublicTopic,
                    scope_id: scope.clone(),
                    object_id: i.to_string(),
                    author_pubkey: "author".into(),
                    text: "検索の検証".into(),
                    created_at: i as i64,
                    source_replica_id: "replica".into(),
                    content_advisories: vec![],
                })
                .collect();
            command(
                &config,
                "sqlscript",
                &format!(
                    "INSERT INTO IndexedEntry CONTENT {}; RETURN 1;",
                    serde_json::to_string(&entries)?
                ),
            )
            .await?;
            for entry in entries {
                index.upsert(&entry)?;
            }
            inserted += 1000;
        }
        index.commit()?;
        for filter in [Some((IndexScopeKind::PublicTopic, scope.as_str())), None] {
            let before = reads(&config).await?;
            assert_eq!(index.reader().search(filter, "検索", 100)?.len(), 100);
            assert_eq!(
                reads(&config).await? - before,
                0,
                "{total} documents, {filter:?}"
            );
        }
    }
    while source
        .remove_scope_page(IndexScopeKind::PublicTopic, &scope, 128)
        .await?
        != 0
    {}
    Ok(())
}
