//! index 投影 store の境界（#413 / T6）。
//!
//! index 投影は canonical source ではない derived な写像である（ADR 0025 §2.1）。全文検索は ArcadeDB
//! （Lucene）に置く。backend を差し替え可能にするため trait `IndexProjection` を境界に置き、本番は
//! ArcadeDB adapter（`arcadedb.rs`）、テストは in-memory 実装で駆動する。
//!
//! 投影に入るのは safety verdict が `allow` の entry のみ（fail-closed は ingest 側で担保）。scope
//! （supported topic 除外 / channel secret 失効）で de-index するため、topic/channel 単位の削除を持つ。

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use kukuri_cn_core::IndexScopeKind;
use kukuri_cn_safety::ContentAdvisory;

use crate::query::IndexQuery;

/// 投影 1 件（検索対象エントリ）。
///
/// canonical ではない derived な写像。text は post 本文（media は将来 VLM 派生タグ）で、raw blob は
/// 含めない（no permanent blob storage / ADR 0025 §2.3）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedEntry {
    /// scope 種別（public_topic / private_channel）。
    pub scope_kind: IndexScopeKind,
    /// scope 識別子（topic_id / channel_id）。de-index の単位。
    pub scope_id: String,
    /// 対象 object id（post id 等）。scope 内で一意。
    pub object_id: String,
    /// 著者 pubkey。
    pub author_pubkey: String,
    /// 検索対象テキスト（post 本文 / 将来は media 派生タグ）。
    pub text: String,
    /// 作成時刻（unix 秒）。
    pub created_at: i64,
    /// 由来の共有 replica id（監査用。ghost 注入でないことの追跡）。
    pub source_replica_id: String,
    /// content advisory（ADR 0028 §8.6）。投影には書かず、query 境界（`FailClosedIndexQuery`）が
    /// 真実源の最新 verdict から充填する（ADR 0025 §7.1）。投影からの読み出しでは常に空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_advisories: Vec<ContentAdvisory>,
}

/// index 投影 store の境界。ArcadeDB / in-memory が同じ API を満たす。
///
/// クエリ表現は Cypher 互換に落とせることを前提とする（ADR 0026 §6.1 と同じ方針）が、#413 の
/// スコープは ingest→投影 + 投影レベル read（allow entry の存在確認）までで、ユーザー向け search API は
/// #404 が載せる。
#[async_trait]
pub trait IndexProjection: Send + Sync {
    /// `allow` entry を投影へ upsert する（object_id で冪等）。
    async fn upsert_entry(&self, entry: &IndexedEntry) -> Result<()>;

    /// scope 内の単一 object が投影に存在するか（投影レベル read）。
    async fn contains_object(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
    ) -> Result<bool>;

    /// scope 内の投影 entry 数（テスト / 監査用）。
    async fn count_scope(&self, scope_kind: IndexScopeKind, scope_id: &str) -> Result<usize>;

    /// 解除した scope の entry を最大 `limit` 件 de-index し、消した件数を返す（#1221 R5-F。1 回の回収は有界）。
    async fn remove_scope_page(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        limit: usize,
    ) -> Result<usize>;

    /// 作成時刻が受入下限 `floor` 未満の entry を最大 `limit` 件 de-index し、消した件数を返す（#1221 R5-F）。
    async fn remove_older_than(&self, floor: i64, limit: usize) -> Result<usize>;

    /// 単一 object を de-index する（replica 上の tombstone / 削除時）。
    async fn remove_object(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
    ) -> Result<()>;
}

/// in-memory 実装（contract / unit テスト用）。
#[derive(Clone, Default)]
pub struct MemoryIndexProjection {
    entries: std::sync::Arc<tokio::sync::Mutex<Vec<IndexedEntry>>>,
}

impl MemoryIndexProjection {
    pub fn new() -> Self {
        Self::default()
    }

    /// scope 内の全 entry を取得する（テスト用の read ヘルパ）。
    pub async fn entries_in_scope(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
    ) -> Vec<IndexedEntry> {
        self.entries
            .lock()
            .await
            .iter()
            .filter(|entry| entry.scope_kind == scope_kind && entry.scope_id == scope_id)
            .cloned()
            .collect()
    }
}

fn same_object(
    entry: &IndexedEntry,
    scope_kind: IndexScopeKind,
    scope_id: &str,
    object_id: &str,
) -> bool {
    entry.scope_kind == scope_kind && entry.scope_id == scope_id && entry.object_id == object_id
}

#[async_trait]
impl IndexProjection for MemoryIndexProjection {
    async fn upsert_entry(&self, entry: &IndexedEntry) -> Result<()> {
        let mut entries = self.entries.lock().await;
        if let Some(existing) = entries.iter_mut().find(|existing| {
            same_object(
                existing,
                entry.scope_kind,
                entry.scope_id.as_str(),
                entry.object_id.as_str(),
            )
        }) {
            *existing = entry.clone();
        } else {
            entries.push(entry.clone());
        }
        Ok(())
    }

    async fn contains_object(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
    ) -> Result<bool> {
        Ok(self
            .entries
            .lock()
            .await
            .iter()
            .any(|entry| same_object(entry, scope_kind, scope_id, object_id)))
    }

    async fn count_scope(&self, scope_kind: IndexScopeKind, scope_id: &str) -> Result<usize> {
        Ok(self
            .entries
            .lock()
            .await
            .iter()
            .filter(|entry| entry.scope_kind == scope_kind && entry.scope_id == scope_id)
            .count())
    }

    async fn remove_scope_page(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        limit: usize,
    ) -> Result<usize> {
        let mut removed = 0;
        self.entries.lock().await.retain(|entry| {
            let remove =
                removed < limit && entry.scope_kind == scope_kind && entry.scope_id == scope_id;
            removed += usize::from(remove);
            !remove
        });
        Ok(removed)
    }

    async fn remove_older_than(&self, floor: i64, limit: usize) -> Result<usize> {
        let mut removed = 0;
        self.entries.lock().await.retain(|entry| {
            let remove = removed < limit && entry.created_at < floor;
            removed += usize::from(remove);
            !remove
        });
        Ok(removed)
    }

    async fn remove_object(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
    ) -> Result<()> {
        self.entries
            .lock()
            .await
            .retain(|entry| !same_object(entry, scope_kind, scope_id, object_id));
        Ok(())
    }
}

/// in-memory の全文検索近似: query を空白区切りの term に分け、いずれかの term を含む text に
/// hit する（Lucene の既定 OR 演算に対応する最小近似。contract テスト用）。
fn memory_text_matches(text: &str, query: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .any(|term| !term.is_empty() && text.contains(&term.to_lowercase()))
}

#[async_trait]
impl IndexQuery for MemoryIndexProjection {
    async fn search_scope(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<IndexedEntry>> {
        let mut hits: Vec<IndexedEntry> = self
            .entries
            .lock()
            .await
            .iter()
            .filter(|entry| {
                entry.scope_kind == scope_kind
                    && entry.scope_id == scope_id
                    && memory_text_matches(&entry.text, query)
            })
            .cloned()
            .collect();
        hits.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        hits.truncate(limit);
        Ok(hits)
    }

    async fn search_all(&self, query: &str, limit: usize) -> Result<Vec<IndexedEntry>> {
        let mut hits: Vec<IndexedEntry> = self
            .entries
            .lock()
            .await
            .iter()
            .filter(|entry| memory_text_matches(&entry.text, query))
            .cloned()
            .collect();
        hits.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        hits.truncate(limit);
        Ok(hits)
    }

    async fn list_recent(
        &self,
        scope: Option<(IndexScopeKind, &str)>,
        limit: usize,
    ) -> Result<Vec<IndexedEntry>> {
        let mut hits: Vec<IndexedEntry> = self
            .entries
            .lock()
            .await
            .iter()
            .filter(|entry| match scope {
                Some((kind, id)) => entry.scope_kind == kind && entry.scope_id == id,
                None => true,
            })
            .cloned()
            .collect();
        hits.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        hits.truncate(limit);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(scope_id: &str, object_id: &str, text: &str) -> IndexedEntry {
        IndexedEntry {
            scope_kind: IndexScopeKind::PublicTopic,
            scope_id: scope_id.to_string(),
            object_id: object_id.to_string(),
            author_pubkey: "author".to_string(),
            text: text.to_string(),
            created_at: 1,
            source_replica_id: format!("topic::{scope_id}"),
            content_advisories: Vec::new(),
        }
    }

    #[tokio::test]
    async fn upsert_is_idempotent_by_object_id() {
        let projection = MemoryIndexProjection::new();
        projection
            .upsert_entry(&entry("t1", "o1", "a"))
            .await
            .unwrap();
        projection
            .upsert_entry(&entry("t1", "o1", "b"))
            .await
            .unwrap();
        assert_eq!(
            projection
                .count_scope(IndexScopeKind::PublicTopic, "t1")
                .await
                .unwrap(),
            1
        );
        let stored = projection
            .entries_in_scope(IndexScopeKind::PublicTopic, "t1")
            .await;
        assert_eq!(stored[0].text, "b");
    }

    #[tokio::test]
    async fn contains_object_reflects_upsert_and_remove() {
        let projection = MemoryIndexProjection::new();
        projection
            .upsert_entry(&entry("t1", "o1", "a"))
            .await
            .unwrap();
        assert!(
            projection
                .contains_object(IndexScopeKind::PublicTopic, "t1", "o1")
                .await
                .unwrap()
        );
        projection
            .remove_object(IndexScopeKind::PublicTopic, "t1", "o1")
            .await
            .unwrap();
        assert!(
            !projection
                .contains_object(IndexScopeKind::PublicTopic, "t1", "o1")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn remove_scope_page_deindexes_a_bounded_page_of_the_scope() {
        let projection = MemoryIndexProjection::new();
        projection
            .upsert_entry(&entry("t1", "o1", "a"))
            .await
            .unwrap();
        projection
            .upsert_entry(&entry("t1", "o2", "b"))
            .await
            .unwrap();
        projection
            .upsert_entry(&entry("t2", "o3", "c"))
            .await
            .unwrap();
        for expected in [1, 1, 0] {
            assert_eq!(
                projection
                    .remove_scope_page(IndexScopeKind::PublicTopic, "t1", 1)
                    .await
                    .unwrap(),
                expected
            );
        }
        assert_eq!(
            projection
                .count_scope(IndexScopeKind::PublicTopic, "t1")
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            projection
                .count_scope(IndexScopeKind::PublicTopic, "t2")
                .await
                .unwrap(),
            1
        );
    }
}
