//! index された entry の真実源（fail-closed indexing の権威レコード。#404）。
//!
//! `cn_index.index_entries` は「この node が index に入れた content」の真実源であり、ArcadeDB の
//! 全文検索投影はこの集合の derived な写像である。fail-closed 不変条件は DB 制約で保証する:
//!
//! - `verdict_id` NOT NULL + FK（`cn_safety.scan_verdicts`）: verdict 無しの index entry を
//!   作らない。unscanned の content は verdict 行が無いため構造的に index できない。
//! - `CHECK (verdict_action = 'allow')`: 非 allow（hold / quarantine / exclude）と fail-closed
//!   経路（scan_failed / provider_unavailable）の entry は書き込めない。
//! - `CHECK (NOT critical)`: critical verdict は search / discovery / recommendation に入らない。
//!
//! query 境界（search / discovery / recommendation）は投影の hit を本テーブル + 最新 verdict
//! （`verdict_id` join。verdict 行は対象ごとに upsert されるため常に最新値）と突合し、真実源に
//! 無い / 現在の verdict が非 allow / critical の hit を落とす（fail-closed query gate）。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};

use crate::index_scope::IndexScopeKind;
use kukuri_cn_safety::ContentAdvisory;
use kukuri_cn_safety_runtime::MemorySafetyArtifactStore;

/// query 境界の突合結果 1 件（surfacing してよい entry と、最新 verdict 由来の content advisory）。
///
/// advisory は `cn_safety.scan_verdicts.advisory_labels`（post 行 = 本文 + 参照 blob の和集合）から
/// 導出し、index entry 自体には持たせない（ADR 0025 §7.1
/// `index_entry_advisories_derive_from_latest_verdict`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceableEntry {
    pub scope_id: String,
    pub object_id: String,
    pub source_replica_id: String,
    pub author_pubkey: String,
    pub created_at: i64,
    pub content_advisories: Vec<ContentAdvisory>,
}

/// 真実源に upsert する index entry（`cn-indexer` の投影 entry と同じ内容 + verdict 参照）。
///
/// 検索対象 text は持たない（text は ArcadeDB 投影のみに置き、replica の再 ingest + 再 scan で
/// 再構築する。ADR 0025 §2.1）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewIndexEntry {
    pub scope_kind: IndexScopeKind,
    pub scope_id: String,
    pub object_id: String,
    pub author_pubkey: String,
    /// content の作成時刻（unix 秒）。
    pub created_at: i64,
    pub source_replica_id: String,
    /// 対応する verdict record id（`cn_safety.scan_verdicts`）。
    pub verdict_id: String,
    /// index 時点の verdict action（DB CHECK により `allow` のみ通る）。
    pub verdict_action: String,
    /// index 時点の critical フラグ（DB CHECK により false のみ通る）。
    pub critical: bool,
}

/// 永続化された index entry。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredIndexEntry {
    pub scope_kind: IndexScopeKind,
    pub scope_id: String,
    pub object_id: String,
    pub author_pubkey: String,
    pub created_at: i64,
    pub source_replica_id: String,
    pub verdict_id: String,
    pub verdict_action: String,
    pub critical: bool,
    pub indexed_at: DateTime<Utc>,
}

/// index entry を真実源へ upsert する（(scope_kind, scope_id, object_id) で冪等）。
///
/// 非 allow / critical / 存在しない verdict_id は DB 制約（CHECK / FK）が拒否する。呼び出し側の
/// verdict gate（`SafetyVerdict::is_indexable()`）をすり抜けた書き込みもここで止まる（防御の重ね）。
pub async fn upsert_index_entry(pool: &PgPool, entry: &NewIndexEntry) -> Result<StoredIndexEntry> {
    if entry.object_id.trim().is_empty() {
        bail!("index entry object_id must not be empty");
    }
    if entry.scope_id.trim().is_empty() {
        bail!("index entry scope_id must not be empty");
    }
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 761))")
        .bind(&entry.object_id)
        .execute(&mut *tx)
        .await?;
    let prevented: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cn_legal.transmission_preventions
         WHERE subject_kind = 'post' AND subject_id = $1 AND released_at IS NULL
           AND (expires_at IS NULL OR expires_at > NOW())
           AND capabilities && ARRAY['community_index','search','discovery','recommendation']::text[])",
    )
    .bind(&entry.object_id)
    .fetch_one(&mut *tx)
    .await?;
    if prevented {
        bail!("index entry is blocked by an active transmission-prevention decision");
    }
    let row = sqlx::query(
        "INSERT INTO cn_index.index_entries
            (scope_kind, scope_id, object_id, author_pubkey, created_at, source_replica_id,
             verdict_id, verdict_action, critical)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (scope_kind, scope_id, object_id) DO UPDATE
         SET author_pubkey = EXCLUDED.author_pubkey,
             created_at = EXCLUDED.created_at,
             source_replica_id = EXCLUDED.source_replica_id,
             verdict_id = EXCLUDED.verdict_id,
             verdict_action = EXCLUDED.verdict_action,
             critical = EXCLUDED.critical,
             indexed_at = NOW()
         RETURNING scope_kind, scope_id, object_id, author_pubkey, created_at, source_replica_id,
                   verdict_id, verdict_action, critical, indexed_at",
    )
    .bind(entry.scope_kind.as_str())
    .bind(&entry.scope_id)
    .bind(&entry.object_id)
    .bind(&entry.author_pubkey)
    .bind(entry.created_at)
    .bind(&entry.source_replica_id)
    .bind(&entry.verdict_id)
    .bind(&entry.verdict_action)
    .bind(entry.critical)
    .fetch_one(&mut *tx)
    .await?;
    let stored = index_entry_from_row(&row)?;
    tx.commit().await?;
    Ok(stored)
}

/// 単一 object を真実源から削除する（tombstone / 非 allow への verdict 変化時の de-index）。
pub async fn remove_index_entry(
    pool: &PgPool,
    scope_kind: IndexScopeKind,
    scope_id: &str,
    object_id: &str,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM cn_index.index_entries
         WHERE scope_kind = $1 AND scope_id = $2 AND object_id = $3",
    )
    .bind(scope_kind.as_str())
    .bind(scope_id)
    .bind(object_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 解除した scope の真実源の行を最大 `limit` 件消し、消した件数を返す（#1221 R5-F。1 回の回収は有界）。
pub async fn remove_index_scope_page(
    pool: &PgPool,
    scope_kind: IndexScopeKind,
    scope_id: &str,
    limit: usize,
) -> Result<usize> {
    let result = sqlx::query(
        "DELETE FROM cn_index.index_entries WHERE (scope_kind, scope_id, object_id) IN (
             SELECT scope_kind, scope_id, object_id FROM cn_index.index_entries
             WHERE scope_kind = $1 AND scope_id = $2 LIMIT $3)",
    )
    .bind(scope_kind.as_str())
    .bind(scope_id)
    .bind(i64::try_from(limit)?)
    .execute(pool)
    .await?;
    Ok(usize::try_from(result.rows_affected())?)
}

/// 単一 object の真実源 entry を取得する。
pub async fn get_index_entry(
    pool: &PgPool,
    scope_kind: IndexScopeKind,
    scope_id: &str,
    object_id: &str,
) -> Result<Option<StoredIndexEntry>> {
    let row = sqlx::query(
        "SELECT scope_kind, scope_id, object_id, author_pubkey, created_at, source_replica_id,
                verdict_id, verdict_action, critical, indexed_at
         FROM cn_index.index_entries
         WHERE scope_kind = $1 AND scope_id = $2 AND object_id = $3",
    )
    .bind(scope_kind.as_str())
    .bind(scope_id)
    .bind(object_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(index_entry_from_row).transpose()
}

/// fail-closed query gate の突合（#404）。
///
/// query 境界が投影（ArcadeDB）から得た hit 候補 `(scope_id, object_id)` のうち、いま surfacing
/// してよいものだけを返す。条件は AND:
/// - 真実源 `index_entries` に entry が存在する（投影残留 / ghost を落とす）
/// - `verdict_id` が指す**最新** verdict（対象ごとに upsert される行）が `allow` かつ非 critical
///   （scan し直しで verdict が変わり、de-index がまだ追いついていない場合もここで落ちる）
///
/// 候補は limit で有界（検索 1 回分）である前提。返り値は入力と同じ `(scope_id, object_id)` の組。
pub async fn filter_surfaceable_objects(
    pool: &PgPool,
    scope_kind: IndexScopeKind,
    candidates: &[(String, String)],
) -> Result<Vec<SurfaceableEntry>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let scope_ids: Vec<&str> = candidates.iter().map(|(s, _)| s.as_str()).collect();
    let object_ids: Vec<&str> = candidates.iter().map(|(_, o)| o.as_str()).collect();
    let rows = sqlx::query(
        "SELECT e.scope_id, e.object_id, e.source_replica_id, e.author_pubkey, e.created_at, v.advisory_labels
         FROM UNNEST($2::text[], $3::text[]) AS candidate (scope_id, object_id)
         JOIN cn_index.index_entries e
           ON e.scope_kind = $1
          AND e.scope_id = candidate.scope_id
          AND e.object_id = candidate.object_id
         JOIN cn_safety.scan_verdicts v
           ON v.id = e.verdict_id
         WHERE v.action = 'allow'
           AND NOT v.critical
           AND EXISTS (
               SELECT 1 FROM cn_index.supported_topics s
               WHERE s.kind = e.scope_kind AND s.id = e.scope_id
                 AND (s.kind = 'public_topic' OR EXISTS (
                     SELECT 1 FROM cn_index.channel_secrets c WHERE c.channel_id = s.id))
           )
           AND NOT EXISTS (
               SELECT 1 FROM cn_legal.transmission_preventions p
               WHERE p.subject_kind = 'post' AND p.subject_id = e.object_id
                 AND p.released_at IS NULL
                 AND (p.expires_at IS NULL OR p.expires_at > NOW())
                 AND p.capabilities && ARRAY['community_index','search','discovery','recommendation']::text[]
           )",
    )
    .bind(scope_kind.as_str())
    .bind(&scope_ids)
    .bind(&object_ids)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            let advisory_labels: serde_json::Value = row.try_get("advisory_labels")?;
            Ok(SurfaceableEntry {
                scope_id: row.try_get::<String, _>("scope_id")?,
                object_id: row.try_get::<String, _>("object_id")?,
                source_replica_id: row.try_get("source_replica_id")?,
                author_pubkey: row.try_get("author_pubkey")?,
                created_at: row.try_get("created_at")?,
                content_advisories: serde_json::from_value(advisory_labels)?,
            })
        })
        .collect()
}

/// index 真実源への書き込み / 突合の抽象（#404）。
///
/// 本番は Postgres（[`PgIndexEntryStore`]、`cn_index.index_entries` の DB 制約が fail-closed を
/// 保証する）、contract test は in-memory（[`MemoryIndexEntryStore`]、同じ不変条件をコードで模す）。
/// ingest pipeline は「① 真実源 upsert → ② 投影 upsert」の順で書き、query 境界は投影 hit を
/// `filter_surfaceable` で突合してから返す。
#[async_trait]
pub trait IndexEntryStore: Send + Sync {
    async fn is_scope_supported(
        &self,
        _scope_kind: IndexScopeKind,
        _scope_id: &str,
    ) -> Result<bool> {
        Ok(false)
    }
    /// allow entry を真実源へ upsert する（非 allow / critical / verdict 無しは Err）。
    async fn upsert_entry(&self, entry: &NewIndexEntry) -> Result<()>;

    /// 単一 object を真実源から削除する。
    async fn remove_entry(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
    ) -> Result<()>;

    /// 解除した scope の真実源の行を最大 `limit` 件消し、消した件数を返す（#1221 R5-F）。
    async fn remove_scope_page(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        limit: usize,
    ) -> Result<usize>;

    /// Keep a validated withdrawal across provider changes and remove any old indexed row.
    /// `created_at` は撤回対象の署名済み envelope の作成時刻（受入下限を越えるまで marker を残す。#1221 R5-F）。
    async fn record_verified_withdrawal(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
        created_at: i64,
    ) -> Result<()>;
    /// 撤回済み、または作成時刻が受入下限未満で索引へ入れない投稿か。
    async fn is_known_withdrawn(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
        created_at: i64,
    ) -> Result<bool>;

    /// 投影 hit 候補 `(scope_id, object_id)` のうち、いま surfacing してよいものだけを返す
    /// （真実源に存在し、最新 verdict が `allow` かつ非 critical）。最新 verdict 由来の
    /// content advisory を同伴する（ADR 0025 §7.1）。
    async fn filter_surfaceable(
        &self,
        scope_kind: IndexScopeKind,
        candidates: &[(String, String)],
    ) -> Result<Vec<SurfaceableEntry>>;

    /// Composite-key seek to the next distinct indexed logical scope.
    async fn next_scope_after(
        &self,
        after_kind: &str,
        after_id: &str,
    ) -> Result<Option<(IndexScopeKind, String)>>;

    /// Active legal decisions are checked before any body or media fetch.
    async fn is_transmission_prevented(&self, object_id: &str) -> Result<bool>;
}

/// Postgres 実装。`cn_index.index_entries` の persist API に委譲する。
#[derive(Clone, Debug)]
pub struct PgIndexEntryStore {
    pool: PgPool,
}

impl PgIndexEntryStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl IndexEntryStore for PgIndexEntryStore {
    async fn is_scope_supported(&self, scope_kind: IndexScopeKind, scope_id: &str) -> Result<bool> {
        if !crate::is_topic_supported(&self.pool, scope_kind, scope_id).await? {
            return Ok(false);
        }
        if scope_kind == IndexScopeKind::PrivateChannel {
            return Ok(sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM cn_index.channel_secrets WHERE channel_id=$1)",
            )
            .bind(scope_id)
            .fetch_one(&self.pool)
            .await?);
        }
        Ok(true)
    }
    async fn upsert_entry(&self, entry: &NewIndexEntry) -> Result<()> {
        upsert_index_entry(&self.pool, entry).await.map(|_| ())
    }

    async fn remove_entry(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
    ) -> Result<()> {
        remove_index_entry(&self.pool, scope_kind, scope_id, object_id).await
    }

    async fn remove_scope_page(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        limit: usize,
    ) -> Result<usize> {
        remove_index_scope_page(&self.pool, scope_kind, scope_id, limit).await
    }

    async fn record_verified_withdrawal(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
        created_at: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO cn_index.known_post_withdrawals (scope_kind, scope_id, object_id, created_at)
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(scope_kind.as_str())
        .bind(scope_id)
        .bind(object_id)
        .bind(created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn is_known_withdrawn(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
        created_at: i64,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM cn_index.known_post_withdrawals
                 WHERE scope_kind = $1 AND scope_id = $2 AND object_id = $3)
                 OR EXISTS(SELECT 1 FROM cn_index.retention_state WHERE $4 < floor)",
        )
        .bind(scope_kind.as_str())
        .bind(scope_id)
        .bind(object_id)
        .bind(created_at)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn filter_surfaceable(
        &self,
        scope_kind: IndexScopeKind,
        candidates: &[(String, String)],
    ) -> Result<Vec<SurfaceableEntry>> {
        filter_surfaceable_objects(&self.pool, scope_kind, candidates).await
    }

    async fn next_scope_after(
        &self,
        after_kind: &str,
        after_id: &str,
    ) -> Result<Option<(IndexScopeKind, String)>> {
        let row = sqlx::query(
            "SELECT scope_kind, scope_id FROM cn_index.index_entries
             WHERE (scope_kind, scope_id) > ($1, $2)
             ORDER BY scope_kind, scope_id, object_id LIMIT 1",
        )
        .bind(after_kind)
        .bind(after_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok((
                IndexScopeKind::parse(&row.try_get::<String, _>("scope_kind")?)?,
                row.try_get("scope_id")?,
            ))
        })
        .transpose()
    }

    async fn is_transmission_prevented(&self, object_id: &str) -> Result<bool> {
        crate::is_transmission_prevented_for_any(
            &self.pool,
            "post",
            object_id,
            &[
                crate::TransmissionPreventionCapability::CommunityIndex,
                crate::TransmissionPreventionCapability::Search,
                crate::TransmissionPreventionCapability::Discovery,
                crate::TransmissionPreventionCapability::Recommendation,
            ],
        )
        .await
    }
}

/// contract test 用の in-memory 実装。
///
/// Postgres の DB 制約（CHECK / FK）と同じ不変条件をコードで模し、verdict の現在値は
/// [`MemorySafetyArtifactStore`] の verdict record（`verdict_by_id`）を参照する。これにより
/// 「index 後に verdict が非 allow / critical へ変わった entry は surfacing されない」という
/// join セマンティクスが in-memory でも成立する。
/// (scope_kind, scope_id, object_id) → entry の in-memory map。
type MemoryEntryMap = HashMap<(IndexScopeKind, String, String), NewIndexEntry>;

#[derive(Clone)]
pub struct MemoryIndexEntryStore {
    verdicts: Arc<MemorySafetyArtifactStore>,
    entries: Arc<Mutex<MemoryEntryMap>>,
    prevented: Arc<Mutex<HashSet<String>>>,
    unsupported: Arc<Mutex<HashSet<(IndexScopeKind, String)>>>,
    withdrawn: Arc<Mutex<HashSet<(IndexScopeKind, String, String)>>>,
}

impl MemoryIndexEntryStore {
    pub fn set_scope_supported(&self, kind: IndexScopeKind, id: &str, supported: bool) {
        let key = (kind, id.to_owned());
        let mut scopes = self.unsupported.lock().expect("scope mutex");
        if supported {
            scopes.remove(&key);
        } else {
            scopes.insert(key);
            self.entries
                .lock()
                .expect("entries mutex")
                .retain(|(entry_kind, entry_id, _), _| *entry_kind != kind || entry_id != id);
        }
    }

    pub fn new(verdicts: Arc<MemorySafetyArtifactStore>) -> Self {
        Self {
            verdicts,
            entries: Arc::new(Mutex::new(HashMap::new())),
            prevented: Arc::new(Mutex::new(HashSet::new())),
            unsupported: Arc::new(Mutex::new(HashSet::new())),
            withdrawn: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// 現在の entry 一覧のスナップショット（co-participation 集計の in-memory 実装が使う）。
    pub fn entries_snapshot(&self) -> Vec<NewIndexEntry> {
        self.entries
            .lock()
            .expect("entries mutex poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// entry が真実源に存在するか（テスト用の read ヘルパ）。
    pub fn contains(&self, scope_kind: IndexScopeKind, scope_id: &str, object_id: &str) -> bool {
        self.entries
            .lock()
            .expect("entries mutex poisoned")
            .contains_key(&(scope_kind, scope_id.to_string(), object_id.to_string()))
    }

    pub fn prevent_subject(&self, object_id: impl Into<String>) {
        let object_id = object_id.into();
        self.prevented
            .lock()
            .expect("prevented mutex poisoned")
            .insert(object_id.clone());
        self.entries
            .lock()
            .expect("entries mutex poisoned")
            .retain(|(_, _, entry_object_id), _| entry_object_id != &object_id);
    }

    pub fn release_subject(&self, object_id: &str) {
        self.prevented
            .lock()
            .expect("prevented mutex poisoned")
            .remove(object_id);
    }
}

#[async_trait]
impl IndexEntryStore for MemoryIndexEntryStore {
    async fn is_scope_supported(&self, kind: IndexScopeKind, id: &str) -> Result<bool> {
        Ok(!self
            .unsupported
            .lock()
            .expect("scope mutex")
            .contains(&(kind, id.to_owned())))
    }

    async fn upsert_entry(&self, entry: &NewIndexEntry) -> Result<()> {
        if self
            .prevented
            .lock()
            .expect("prevented mutex poisoned")
            .contains(&entry.object_id)
        {
            bail!("index entry is blocked by an active transmission-prevention decision");
        }
        // Postgres の CHECK / FK 制約と同じ不変条件を模す（fail-closed contract の等価性）。
        if entry.verdict_action != "allow" {
            bail!(
                "index entry violates check constraint: verdict_action `{}` is not `allow`",
                entry.verdict_action
            );
        }
        if entry.critical {
            bail!("index entry violates check constraint: critical entry is not indexable");
        }
        if self
            .verdicts
            .verdict_by_id(entry.verdict_id.as_str())
            .is_none()
        {
            bail!(
                "index entry violates foreign key constraint: verdict `{}` does not exist",
                entry.verdict_id
            );
        }
        let key = (
            entry.scope_kind,
            entry.scope_id.clone(),
            entry.object_id.clone(),
        );
        let withdrawn = self.withdrawn.lock().expect("withdrawn mutex poisoned");
        if withdrawn.contains(&key) {
            bail!("known withdrawn post cannot be indexed");
        }
        self.entries
            .lock()
            .expect("entries mutex poisoned")
            .insert(key, entry.clone());
        Ok(())
    }

    async fn remove_entry(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
    ) -> Result<()> {
        self.entries
            .lock()
            .expect("entries mutex poisoned")
            .remove(&(scope_kind, scope_id.to_string(), object_id.to_string()));
        Ok(())
    }

    async fn remove_scope_page(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        limit: usize,
    ) -> Result<usize> {
        let mut entries = self.entries.lock().expect("entries mutex poisoned");
        let page = entries
            .keys()
            .filter(|(kind, id, _)| *kind == scope_kind && id == scope_id)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        for key in &page {
            entries.remove(key);
        }
        Ok(page.len())
    }

    async fn record_verified_withdrawal(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
        _created_at: i64,
    ) -> Result<()> {
        let key = (scope_kind, scope_id.to_string(), object_id.to_string());
        let mut withdrawn = self.withdrawn.lock().expect("withdrawn mutex poisoned");
        withdrawn.insert(key.clone());
        self.entries
            .lock()
            .expect("entries mutex poisoned")
            .remove(&key);
        Ok(())
    }

    async fn is_known_withdrawn(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
        _created_at: i64,
    ) -> Result<bool> {
        Ok(self
            .withdrawn
            .lock()
            .expect("withdrawn mutex poisoned")
            .contains(&(scope_kind, scope_id.to_string(), object_id.to_string())))
    }

    async fn filter_surfaceable(
        &self,
        scope_kind: IndexScopeKind,
        candidates: &[(String, String)],
    ) -> Result<Vec<SurfaceableEntry>> {
        let entries = self.entries.lock().expect("entries mutex poisoned");
        Ok(candidates
            .iter()
            .filter_map(|(scope_id, object_id)| {
                let entry = entries.get(&(scope_kind, scope_id.clone(), object_id.clone()))?;
                // Postgres 実装の join（verdict_id → 最新 verdict）と同じセマンティクス。
                let stored = self
                    .verdicts
                    .stored_verdict_by_id(entry.verdict_id.as_str())?;
                (stored.verdict.is_indexable() && !stored.verdict.critical).then(|| {
                    SurfaceableEntry {
                        scope_id: scope_id.clone(),
                        object_id: object_id.clone(),
                        source_replica_id: entry.source_replica_id.clone(),
                        author_pubkey: entry.author_pubkey.clone(),
                        created_at: entry.created_at,
                        content_advisories: stored.advisories.clone(),
                    }
                })
            })
            .collect())
    }

    async fn next_scope_after(
        &self,
        after_kind: &str,
        after_id: &str,
    ) -> Result<Option<(IndexScopeKind, String)>> {
        let entries = self.entries.lock().expect("entries mutex poisoned");
        Ok(entries
            .keys()
            .filter(|(kind, scope_id, _)| {
                (kind.as_str(), scope_id.as_str()) > (after_kind, after_id)
            })
            .min_by(|(kind_a, id_a, _), (kind_b, id_b, _)| {
                (kind_a.as_str(), id_a.as_str()).cmp(&(kind_b.as_str(), id_b.as_str()))
            })
            .map(|(kind, scope_id, _)| (*kind, scope_id.clone())))
    }

    async fn is_transmission_prevented(&self, object_id: &str) -> Result<bool> {
        Ok(self
            .prevented
            .lock()
            .expect("prevented mutex poisoned")
            .contains(object_id))
    }
}

fn index_entry_from_row(row: &PgRow) -> Result<StoredIndexEntry> {
    Ok(StoredIndexEntry {
        scope_kind: IndexScopeKind::parse(&row.try_get::<String, _>("scope_kind")?)?,
        scope_id: row.try_get("scope_id")?,
        object_id: row.try_get("object_id")?,
        author_pubkey: row.try_get("author_pubkey")?,
        created_at: row.try_get("created_at")?,
        source_replica_id: row.try_get("source_replica_id")?,
        verdict_id: row.try_get("verdict_id")?,
        verdict_action: row.try_get("verdict_action")?,
        critical: row.try_get("critical")?,
        indexed_at: row.try_get("indexed_at")?,
    })
}
