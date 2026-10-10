//! CN の検索索引。投影への変更をまとめて commit し、読み手は同じ索引を別に開く。

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use lindera::{dictionary::load_dictionary, mode::Mode, segmenter::Segmenter};
use lindera_tantivy::tokenizer::LinderaTokenizer;
use tantivy::{
    DocAddress, DocSet, Index, IndexReader, IndexWriter, ReloadPolicy, TERMINATED, TantivyDocument,
    Term,
    collector::{Count, TopDocs},
    query::{BooleanQuery, EmptyQuery, Occur, PhraseQuery, Query, TermQuery},
    schema::{
        Field, INDEXED, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions,
        Value,
    },
    tokenizer::TokenStream,
};
use unicode_normalization::UnicodeNormalization;

use crate::backfill::{BackfillCursor, BackfillPage};
use crate::{ArcadeDbProjection, IndexProjection, IndexedEntry};
use kukuri_cn_core::IndexScopeKind;

pub const SEARCH_DIRECTORY: &str = "search";
const PENDING_LIMIT: usize = 512;
const DELETE_LIMIT: usize = 128;

#[derive(Clone, Copy)]
struct Fields {
    key: Field,
    scope: Field,
    kind: Field,
    text: Field,
    created: Field,
    entry: Field,
}

fn schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let fields = Fields {
        key: builder.add_text_field("key", STRING | STORED),
        scope: builder.add_text_field("scope", STRING),
        kind: builder.add_text_field("kind", STRING),
        text: builder.add_text_field(
            "text",
            TextOptions::default().set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("neologd")
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            ),
        ),
        created: builder.add_i64_field("created_at", INDEXED),
        entry: builder.add_text_field("entry", STORED),
    };
    (builder.build(), fields)
}

fn scope_key(kind: IndexScopeKind, id: &str) -> String {
    serde_json::to_string(&(kind.as_str(), id)).expect("strings serialize")
}

fn object_key(kind: IndexScopeKind, scope: &str, id: &str) -> String {
    serde_json::to_string(&(kind.as_str(), scope, id)).expect("strings serialize")
}

fn configure(index: &Index) -> Result<()> {
    let tokenizer = LinderaTokenizer::from_segmenter(Segmenter::new(
        Mode::Normal,
        load_dictionary("embedded://ipadic-neologd")?,
        None,
    ));
    index.tokenizers().register("neologd", tokenizer);
    Ok(())
}

#[derive(Clone)]
pub struct SearchReader {
    index: Index,
    reader: IndexReader,
    fields: Fields,
}

impl SearchReader {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_index(Index::open_in_dir(path)?)
    }

    pub fn open_or_create(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        let (schema, _) = schema();
        Self::from_index(Index::open_or_create(
            tantivy::directory::MmapDirectory::open(path)?,
            schema,
        )?)
    }

    fn from_index(index: Index) -> Result<Self> {
        configure(&index)?;
        let (_, fields) = schema();
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            fields,
        })
    }

    pub fn reload(&self) -> Result<()> {
        self.reader.reload()?;
        Ok(())
    }

    pub fn search(
        &self,
        scope: Option<(IndexScopeKind, &str)>,
        text: &str,
        limit: usize,
    ) -> Result<Vec<IndexedEntry>> {
        self.reload()?;
        let query = self.query(scope, text)?;
        let searcher = self.reader.searcher();
        searcher
            .search(
                &*query,
                &TopDocs::with_limit(crate::clamp_query_limit(limit)).order_by_score(),
            )?
            .into_iter()
            .map(|(_, addr)| {
                let doc = searcher.doc::<TantivyDocument>(addr)?;
                serde_json::from_str(
                    doc.get_first(self.fields.entry)
                        .and_then(|v| v.as_str())
                        .context("missing search entry")?,
                )
                .context("invalid search entry")
            })
            .collect()
    }

    fn query(&self, scope: Option<(IndexScopeKind, &str)>, text: &str) -> Result<Box<dyn Query>> {
        let mut clauses = vec![(
            Occur::Must,
            term_query(match scope {
                Some((kind, id)) => Term::from_field_text(self.fields.scope, &scope_key(kind, id)),
                None => {
                    Term::from_field_text(self.fields.kind, IndexScopeKind::PublicTopic.as_str())
                }
            }),
        )];
        let mut analyzer = self.index.tokenizer_for_field(self.fields.text)?;
        for word in text.split_whitespace() {
            let normalized = normalize(word);
            let mut stream = analyzer.token_stream(&normalized);
            let mut terms = vec![];
            while stream.advance() {
                terms.push((
                    stream.token().position,
                    Term::from_field_text(self.fields.text, &stream.token().text),
                ));
            }
            let query: Box<dyn Query> = match terms.len() {
                0 => Box::new(EmptyQuery),
                1 => term_query(terms.pop().expect("one term").1),
                _ => Box::new(PhraseQuery::new_with_offset(terms)),
            };
            clauses.push((Occur::Must, query));
        }
        if clauses.len() == 1 {
            return Ok(Box::new(EmptyQuery));
        }
        Ok(Box::new(BooleanQuery::new(clauses)))
    }

    pub fn contains_object(&self, kind: IndexScopeKind, scope: &str, object: &str) -> Result<bool> {
        self.reload()?;
        Ok(self.reader.searcher().search(
            &term_query(Term::from_field_text(
                self.fields.key,
                &object_key(kind, scope, object),
            )),
            &Count,
        )? > 0)
    }
}

fn term_query(term: Term) -> Box<dyn Query> {
    Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs))
}

fn normalize(text: &str) -> String {
    text.nfkc().flat_map(char::to_lowercase).collect()
}

struct Writer {
    writer: IndexWriter,
    // commit 前の upsert と削除を含めて、有界な回収の次のページを選ぶ。
    pending: BTreeMap<String, Option<IndexedEntry>>,
    cursor: BackfillCursor,
    cursor_dirty: bool,
}

pub struct SearchIndex {
    reader: SearchReader,
    writer: Mutex<Writer>,
}

impl SearchIndex {
    pub fn open(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        let (schema, _) = schema();
        let index = Index::open_or_create(tantivy::directory::MmapDirectory::open(path)?, schema)?;
        Self::from_index(index)
    }

    pub fn in_ram() -> Result<Self> {
        Self::from_index(Index::create_in_ram(schema().0))
    }

    fn from_index(index: Index) -> Result<Self> {
        configure(&index)?;
        let writer = index.writer_with_num_threads(1, 50_000_000)?;
        let cursor = index
            .load_metas()?
            .payload
            .map(|payload| serde_json::from_str(&payload))
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            reader: SearchReader::from_index(index)?,
            writer: Mutex::new(Writer {
                writer,
                pending: BTreeMap::new(),
                cursor,
                cursor_dirty: false,
            }),
        })
    }

    pub fn reader(&self) -> SearchReader {
        self.reader.clone()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Writer>> {
        self.writer
            .lock()
            .map_err(|_| anyhow::anyhow!("search writer poisoned"))
    }

    fn commit_locked(&self, writer: &mut Writer) -> Result<bool> {
        if writer.pending.is_empty() && !writer.cursor_dirty {
            return Ok(false);
        }
        let mut commit = writer.writer.prepare_commit()?;
        commit.set_payload(&serde_json::to_string(&writer.cursor)?);
        commit.commit()?;
        writer.pending.clear();
        writer.cursor_dirty = false;
        self.reader.reload()?;
        Ok(true)
    }

    pub fn commit(&self) -> Result<bool> {
        self.commit_locked(&mut *self.lock()?)
    }

    fn reserve(&self, writer: &mut Writer) -> Result<()> {
        // 一定間隔の commit に加え、書込み集中時も未確定の台帳を上限内に保つ。
        if writer.pending.len() >= PENDING_LIMIT {
            self.commit_locked(writer)?;
        }
        Ok(())
    }

    pub fn upsert(&self, entry: &IndexedEntry) -> Result<()> {
        let mut writer = self.lock()?;
        self.reserve(&mut writer)?;
        let fields = self.reader.fields;
        let key = object_key(entry.scope_kind, &entry.scope_id, &entry.object_id);
        let mut doc = TantivyDocument::default();
        doc.add_text(fields.key, &key);
        doc.add_text(fields.scope, scope_key(entry.scope_kind, &entry.scope_id));
        doc.add_text(fields.kind, entry.scope_kind.as_str());
        doc.add_text(fields.text, normalize(&entry.text));
        doc.add_i64(fields.created, entry.created_at);
        doc.add_text(fields.entry, serde_json::to_string(entry)?);
        writer
            .writer
            .delete_term(Term::from_field_text(fields.key, &key));
        writer.writer.add_document(doc)?;
        writer.pending.insert(key, Some(entry.clone()));
        Ok(())
    }

    pub fn remove_object(&self, kind: IndexScopeKind, scope: &str, id: &str) -> Result<()> {
        let mut writer = self.lock()?;
        self.reserve(&mut writer)?;
        self.delete(&mut writer, object_key(kind, scope, id));
        Ok(())
    }

    fn delete(&self, writer: &mut Writer, key: String) {
        writer
            .writer
            .delete_term(Term::from_field_text(self.reader.fields.key, &key));
        writer.pending.insert(key, None);
    }

    fn remove_page(
        &self,
        scope: Option<(IndexScopeKind, &str)>,
        floor: Option<i64>,
        limit: usize,
    ) -> Result<usize> {
        let mut writer = self.lock()?;
        // 1 回の回収（128 件）の途中で commit しない。
        if writer.pending.len() + DELETE_LIMIT > PENDING_LIMIT {
            self.commit_locked(&mut writer)?;
        }
        let limit = limit.min(DELETE_LIMIT);
        let mut keys: Vec<String> = writer
            .pending
            .iter()
            .filter_map(|(key, entry)| {
                entry
                    .as_ref()
                    .filter(|entry| {
                        scope.is_none_or(|(kind, id)| {
                            entry.scope_kind == kind && entry.scope_id == id
                        }) && floor.is_none_or(|floor| entry.created_at < floor)
                    })
                    .map(|_| key.clone())
            })
            .take(limit)
            .collect();
        let searcher = self.reader.reader.searcher();
        // posting を必要件数で止める。全件の候補収集やソートをしない。
        'segments: for (segment_ord, segment) in searcher.segment_readers().iter().enumerate() {
            if keys.len() == limit {
                break;
            }
            let (field, lower, upper) = match scope {
                Some((kind, id)) => {
                    let term =
                        Term::from_field_text(self.reader.fields.scope, &scope_key(kind, id));
                    (
                        self.reader.fields.scope,
                        Some(term.clone()),
                        Some((term, true)),
                    )
                }
                None => (
                    self.reader.fields.created,
                    None,
                    Some((
                        Term::from_field_i64(
                            self.reader.fields.created,
                            floor.context("missing retention floor")?,
                        ),
                        false,
                    )),
                ),
            };
            let inverted = segment.inverted_index(field)?;
            let mut builder = inverted.terms().range();
            if let Some(lower) = lower {
                builder = builder.ge(lower.serialized_value_bytes());
            }
            if let Some((upper, inclusive)) = upper {
                builder = if inclusive {
                    builder.le(upper.serialized_value_bytes())
                } else {
                    builder.lt(upper.serialized_value_bytes())
                };
            }
            let mut terms = builder.into_stream()?;
            while terms.advance() {
                let mut postings = inverted
                    .read_postings_from_terminfo(terms.value(), IndexRecordOption::Basic)?;
                while postings.doc() != TERMINATED {
                    let doc_id = postings.doc();
                    if !segment.is_deleted(doc_id) {
                        let doc = searcher
                            .doc::<TantivyDocument>(DocAddress::new(segment_ord as u32, doc_id))?;
                        let key = doc
                            .get_first(self.reader.fields.key)
                            .and_then(|v| v.as_str())
                            .context("missing search key")?;
                        if !writer.pending.contains_key(key) {
                            keys.push(key.to_owned());
                        }
                        if keys.len() == limit {
                            break 'segments;
                        }
                    }
                    postings.advance();
                }
            }
        }
        let count = keys.len();
        for key in keys {
            self.delete(&mut writer, key);
        }
        Ok(count)
    }

    pub fn backfill_cursor(&self) -> Result<BackfillCursor> {
        Ok(self.lock()?.cursor.clone())
    }

    fn apply_backfill(&self, page: BackfillPage) -> Result<usize> {
        let count = page.entries.len();
        for entry in page.entries {
            self.upsert(&entry)?;
        }
        let mut writer = self.lock()?;
        writer.cursor = page.cursor;
        writer.cursor_dirty = true;
        self.commit_locked(&mut writer)?;
        Ok(count)
    }
}

/// 全取込・回収経路で使う二つの投影の書込み境界。
pub struct SearchProjection {
    projection: Arc<dyn IndexProjection>,
    search: Arc<SearchIndex>,
    mutation: tokio::sync::Mutex<()>,
}

impl SearchProjection {
    pub fn new(projection: Arc<dyn IndexProjection>, search: Arc<SearchIndex>) -> Self {
        Self {
            projection,
            search,
            mutation: tokio::sync::Mutex::new(()),
        }
    }

    /// live の書込み・削除と直列に写し、古い読み取りで削除を戻さない。
    pub async fn backfill_page(&self, source: &ArcadeDbProjection) -> Result<usize> {
        let _mutation = self.mutation.lock().await;
        let cursor = self.search.backfill_cursor()?;
        if cursor == BackfillCursor::Complete {
            return Ok(0);
        }
        let page = source.read_backfill_page(&cursor).await?;
        let search = self.search.clone();
        tokio::task::spawn_blocking(move || search.apply_backfill(page)).await?
    }

    pub fn spawn_committer(
        self: &Arc<Self>,
        source: Arc<ArcadeDbProjection>,
    ) -> tokio::task::JoinHandle<()> {
        let projection = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let Some(projection) = projection.upgrade() else {
                    break;
                };
                if let Err(error) = projection.backfill_page(&source).await {
                    tracing::warn!(%error, "failed to copy a search index page; will resume");
                }
                let search = projection.search.clone();
                match tokio::task::spawn_blocking(move || search.commit()).await {
                    Ok(Ok(_)) => {}
                    error => tracing::warn!(?error, "failed to commit search index"),
                }
            }
        })
    }
}

#[async_trait]
impl IndexProjection for SearchProjection {
    async fn upsert_entry(&self, entry: &IndexedEntry) -> Result<()> {
        let _mutation = self.mutation.lock().await;
        self.projection.upsert_entry(entry).await?;
        let search = self.search.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || search.upsert(&entry)).await?
    }
    async fn contains_object(&self, kind: IndexScopeKind, scope: &str, id: &str) -> Result<bool> {
        self.projection.contains_object(kind, scope, id).await
    }
    async fn count_scope(&self, kind: IndexScopeKind, scope: &str) -> Result<usize> {
        self.projection.count_scope(kind, scope).await
    }
    async fn remove_object(&self, kind: IndexScopeKind, scope: &str, id: &str) -> Result<()> {
        let _mutation = self.mutation.lock().await;
        self.projection.remove_object(kind, scope, id).await?;
        let search = self.search.clone();
        let scope = scope.to_owned();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || search.remove_object(kind, &scope, &id)).await?
    }
    async fn remove_scope_page(
        &self,
        kind: IndexScopeKind,
        scope: &str,
        limit: usize,
    ) -> Result<usize> {
        let _mutation = self.mutation.lock().await;
        let projected = self
            .projection
            .remove_scope_page(kind, scope, limit.min(DELETE_LIMIT))
            .await?;
        let search = self.search.clone();
        let scope = scope.to_owned();
        let removed = tokio::task::spawn_blocking(move || {
            search.remove_page(Some((kind, &scope)), None, limit)
        })
        .await??;
        Ok(projected.max(removed))
    }
    async fn remove_older_than(&self, floor: i64, limit: usize) -> Result<usize> {
        let _mutation = self.mutation.lock().await;
        let projected = self
            .projection
            .remove_older_than(floor, limit.min(DELETE_LIMIT))
            .await?;
        let search = self.search.clone();
        let removed =
            tokio::task::spawn_blocking(move || search.remove_page(None, Some(floor), limit))
                .await??;
        Ok(projected.max(removed))
    }
}

#[cfg(test)]
mod backfill_tests {
    use super::*;

    #[tokio::test]
    async fn completed_cursor_survives_live_commit_and_skips_an_unreachable_source() -> Result<()> {
        let dir = tempfile::tempdir()?;
        {
            let search = SearchIndex::open(dir.path())?;
            search.apply_backfill(BackfillPage {
                entries: vec![],
                cursor: BackfillCursor::Complete,
            })?;
            search.remove_object(IndexScopeKind::PublicTopic, "topic", "post")?;
            search.commit()?;
        }
        let search = Arc::new(SearchIndex::open(dir.path())?);
        assert_eq!(search.backfill_cursor()?, BackfillCursor::Complete);
        let source = ArcadeDbProjection::new(crate::ArcadeDbConfig {
            base_url: "http://127.0.0.1:9".into(),
            database: "unused".into(),
            username: "unused".into(),
            password: "unused".into(),
        })?;
        let projection =
            SearchProjection::new(Arc::new(crate::MemoryIndexProjection::new()), search);
        assert_eq!(projection.backfill_page(&source).await?, 0);
        Ok(())
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;

    fn entry(scope: &str, id: &str, text: &str) -> IndexedEntry {
        IndexedEntry {
            scope_kind: IndexScopeKind::PublicTopic,
            scope_id: scope.into(),
            object_id: id.into(),
            author_pubkey: "author".into(),
            text: text.into(),
            created_at: 1,
            source_replica_id: "replica".into(),
            content_advisories: vec![],
        }
    }

    #[test]
    fn phrases_all_words_relevance_and_scope_use_the_same_tokenizer() -> Result<()> {
        let index = SearchIndex::in_ram()?;
        for (scope, id, text) in [
            ("first", "once", "京都の天気 写真"),
            ("first", "repeated", "京都の天気 京都の天気 写真 写真 写真"),
            ("other", "other", "京都の天気 写真"),
            ("first", "reversed", "天気の京都 写真"),
            ("first", "missing", "京都の天気"),
            ("first", "name", "東京タワー ＫＵＫＵＲＩ"),
        ] {
            index.upsert(&entry(scope, id, text))?;
        }
        let mut private = entry("secret", "private", "京都の天気 写真");
        private.scope_kind = IndexScopeKind::PrivateChannel;
        index.upsert(&private)?;
        index.commit()?;
        let reader = index.reader();
        let hits = reader.search(
            Some((IndexScopeKind::PublicTopic, "first")),
            "京都の天気 写真",
            100,
        )?;
        assert_eq!(
            hits.iter()
                .map(|e| e.object_id.as_str())
                .collect::<Vec<_>>(),
            vec!["repeated", "once"]
        );
        let cross = reader.search(None, "京都の天気 写真", 100)?;
        assert_eq!(cross.len(), 3);
        assert!(
            cross
                .iter()
                .all(|e| e.scope_kind == IndexScopeKind::PublicTopic)
        );
        assert_eq!(
            reader
                .search(
                    Some((IndexScopeKind::PrivateChannel, "secret")),
                    "京都の天気 写真",
                    100
                )?
                .len(),
            1
        );
        assert!(reader.search(None, "東京", 100)?.is_empty());
        assert_eq!(reader.search(None, "kukuri", 100)?[0].object_id, "name");
        Ok(())
    }

    #[test]
    fn rare_matches_and_returned_documents_stay_bounded_as_the_index_grows() -> Result<()> {
        let index = SearchIndex::in_ram()?;
        let mut inserted = 0;
        for total in [2_000, 20_000] {
            for i in inserted..total {
                let text = if i < 50 {
                    "くくり 珍品"
                } else {
                    "くくり"
                };
                index.upsert(&entry(
                    if i < 200 { "small" } else { "other" },
                    &i.to_string(),
                    text,
                ))?;
            }
            inserted = total;
            index.commit()?;
            let reader = index.reader();
            let query = reader.query(None, "珍品")?;
            assert_eq!(reader.reader.searcher().search(&*query, &Count)?, 50);
            assert_eq!(reader.search(None, "珍品", 100)?.len(), 50);
            assert_eq!(reader.search(None, "くくり", 100)?.len(), 100);
            assert_eq!(
                reader
                    .search(Some((IndexScopeKind::PublicTopic, "small")), "くくり", 100)?
                    .len(),
                100
            );
        }
        Ok(())
    }
}
