//! ユーザー向け query 境界（search / discovery / recommendation。#404 / ADR 0025 §2.7）。
//!
//! 読み口は 3 つ:
//! - topic 内検索（基本 UX。`search_scope`）
//! - supported set 横断検索（別画面。`search_all`。投影には supported scope の entry しか
//!   入らないため、投影全体 = supported set 全体）
//! - 新着列挙（`list_recent`。discovery / recommendation の最小 surface。ranking / 関連度
//!   スコアリングの具体は ADR 0025 §4 でスコープ外）
//!
//! fail-closed query gate（[`FailClosedIndexQuery`]）が唯一のユーザー向け入口である。投影
//! （ArcadeDB）の hit を index 真実源（`cn_index.index_entries` + 最新 verdict join）と突合し、
//! 真実源に無い / 現在の verdict が非 allow / critical の hit を結果から落とす。これにより
//! 「`allow` 以外の verdict が search / discovery / recommendation に入らない」
//! （`search_discovery_recommendation_excludes_non_allow`）が、投影の残留や de-index の遅延に
//! 依存せず成立する。

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use kukuri_cn_core::{IndexEntryStore, IndexScopeKind, SurfaceableEntry};

use crate::projection::IndexedEntry;

/// query 1 回で返す hit 数の上限（投影への問い合わせと真実源突合の両方を有界にする）。
pub const MAX_QUERY_LIMIT: usize = 100;

/// 要求 limit を `1..=MAX_QUERY_LIMIT` に丸める。
pub fn clamp_query_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_QUERY_LIMIT)
}

/// index 投影への読み口（search / 新着列挙）。ArcadeDB / in-memory が同じ API を満たす。
///
/// この trait は投影の生 read であり、fail-closed gate を通っていない。ユーザー向けには必ず
/// [`FailClosedIndexQuery`] 越しに使うこと。
#[async_trait]
pub trait IndexQuery: Send + Sync {
    /// topic 内検索（scope 指定つき全文検索）。
    async fn search_scope(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<IndexedEntry>>;

    /// supported set 横断検索（投影全体への全文検索）。
    async fn search_all(&self, query: &str, limit: usize) -> Result<Vec<IndexedEntry>>;

    /// 新着列挙（created_at 降順）。scope 指定ありは scope 内、なしは横断。
    async fn list_recent(
        &self,
        scope: Option<(IndexScopeKind, &str)>,
        limit: usize,
    ) -> Result<Vec<IndexedEntry>>;
}

/// fail-closed query gate（#404 の単一判定点への突合）。
///
/// 投影（`inner`）の hit を真実源（`entries`）の `filter_surfaceable`（entry 存在 + 最新 verdict
/// が `allow` かつ非 critical）で絞ってから返す。落ちた hit はエラーにせず結果から除くだけ
/// （fail-closed。返さない側に倒す）。
pub struct FailClosedIndexQuery {
    inner: Arc<dyn IndexQuery>,
    entries: Arc<dyn IndexEntryStore>,
}

impl FailClosedIndexQuery {
    pub fn new(inner: Arc<dyn IndexQuery>, entries: Arc<dyn IndexEntryStore>) -> Self {
        Self { inner, entries }
    }

    /// hit 群を真実源と突合し、surfacing してよいものだけを元の順序で返す。
    ///
    /// 突合で得た最新 verdict 由来の content advisory を hit に同梱する
    /// （ADR 0025 §7.1 `index_entry_advisories_derive_from_latest_verdict`。投影の値は使わない）。
    async fn gate(&self, hits: Vec<IndexedEntry>) -> Result<Vec<IndexedEntry>> {
        if hits.is_empty() {
            return Ok(hits);
        }
        // scope_kind ごとに候補をまとめて 1 回ずつ突合する（hit 数は limit で有界）。
        let mut surfaceable: Vec<(IndexScopeKind, SurfaceableEntry)> = Vec::new();
        for scope_kind in [IndexScopeKind::PublicTopic, IndexScopeKind::PrivateChannel] {
            let candidates: Vec<(String, String)> = hits
                .iter()
                .filter(|hit| hit.scope_kind == scope_kind)
                .map(|hit| (hit.scope_id.clone(), hit.object_id.clone()))
                .collect();
            if candidates.is_empty() {
                continue;
            }
            for entry in self
                .entries
                .filter_surfaceable(scope_kind, &candidates)
                .await?
            {
                surfaceable.push((scope_kind, entry));
            }
        }
        Ok(hits
            .into_iter()
            .filter_map(|mut hit| {
                let (_, entry) = surfaceable.iter().find(|(kind, entry)| {
                    *kind == hit.scope_kind
                        && entry.scope_id == hit.scope_id
                        && entry.object_id == hit.object_id
                })?;
                hit.content_advisories = entry.content_advisories.clone();
                // 保存先と著者は確定済み索引から復元する。遅延した投影を取得先の根拠にしない。
                hit.source_replica_id = entry.source_replica_id.clone();
                hit.author_pubkey = entry.author_pubkey.clone();
                hit.created_at = entry.created_at;
                Some(hit)
            })
            .collect())
    }
}

#[async_trait]
impl IndexQuery for FailClosedIndexQuery {
    async fn search_scope(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<IndexedEntry>> {
        let limit = clamp_query_limit(limit);
        let hits = self
            .inner
            .search_scope(scope_kind, scope_id, query, limit)
            .await?;
        self.gate(hits).await
    }

    async fn search_all(&self, query: &str, limit: usize) -> Result<Vec<IndexedEntry>> {
        let limit = clamp_query_limit(limit);
        let hits = exclude_private_channel(self.inner.search_all(query, limit).await?);
        self.gate(hits).await
    }

    async fn list_recent(
        &self,
        scope: Option<(IndexScopeKind, &str)>,
        limit: usize,
    ) -> Result<Vec<IndexedEntry>> {
        let limit = clamp_query_limit(limit);
        let hits = self.inner.list_recent(scope, limit).await?;
        let hits = if scope.is_none() {
            exclude_private_channel(hits)
        } else {
            hits
        };
        self.gate(hits).await
    }
}

/// 横断読みから非公開チャンネルの項目を除外する(#711 / ADR 0025 §6.3)。
///
/// 非公開チャンネルの索引は参加者に閉じるため、scope 無指定の search / discovery /
/// recommendation には項目も識別子(`scope_id`)も出さない。所属証明つきの範囲指定読み
/// (`search_scope` / scope 指定の `list_recent`)だけが読み口になる(所属証明の検証は
/// cn-user-api の門が担う)。
fn exclude_private_channel(hits: Vec<IndexedEntry>) -> Vec<IndexedEntry> {
    hits.into_iter()
        .filter(|hit| hit.scope_kind != IndexScopeKind::PrivateChannel)
        .collect()
}
