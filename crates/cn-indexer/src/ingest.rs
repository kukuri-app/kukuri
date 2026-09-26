//! ingest pipeline（#413 / T5 / ADR 0025 §2.5 / §6.2）。
//!
//! 共有 replica に **実在する** post entry のみを対象に、post 本文 text を `cn-core` の
//! `SafetyScanService`（#406。内部で `cn-safety-runtime` の `SafetyOrchestrator` を駆動し、
//! moderation artifact を署名・永続化する）で scan し、verdict が `allow`
//! （`SafetyVerdict::is_indexable()`）の entry のみを index 投影へ書く。以下を不変条件として守る:
//!   - ghost 注入を作らない: 対象は共有 replica の entry のみ（CN 直渡しは経路が無い。§6.2）。
//!   - fail-closed: unscanned / scan_failed / provider_unavailable / 非 allow は投影しない（§2.5）。
//!   - no permanent blob storage: blob は scan 用の一時 fetch のみで、投影に raw blob を入れない（§2.3）。
//!   - media 参照 post は本文 text に加えて **media blob ごとに scan** し（#420 / ADR 0028）、
//!     いずれか 1 つでも非 allow なら post 全体を index しない（worst-case 合成）。manifest 参照
//!     は replica 上の署名済み manifest を解決して item blob（hash + mime）へ展開する（#609。
//!     解決できなければ index しない）。全 allow の
//!     ときのみ、同一 scan が生成した derived 検索タグ（`SafetyScanReport.derived_tags`）を
//!     本文 text に相乗りさせて投影する（ADR 0025 §2.3。タグ専用列は持たない）。
//!     media provider（VLM）や `MediaFetcher` が未構成の環境では media scan が fail-closed
//!     （Unavailable → hold）になり、従来どおり media 参照 post は index されない。
//!
//! docs replica からの entry 取得は `DocsSync`（`query_replica_with_policy`）越しに行うため、本番
//! （iroh-docs）でも in-memory（テスト）でも同じ pipeline を駆動できる。
//!
//! 書き込みは二段（#404）: `allow` verdict の entry は ① index 真実源
//! （`IndexEntryStore`。Postgres の DB 制約が fail-closed を保証する）→ ② 全文検索投影
//! （`IndexProjection`。ArcadeDB）の順で書く。① が失敗したら ② は書かない（真実源に無い
//! entry は query 境界の突合で surfacing されないため、投影残留も安全側に倒れる）。
//! de-index は真実源 → 投影の順で両方から消す。
//!
//! 1 件の取り込みの失敗は、確定した理由（de-index する）と一時的な失敗（既存 entry を保持し、
//! 新たには索引しない）に分ける（#1090。分類は `failure` を参照）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use futures_util::stream::{FuturesUnordered, StreamExt};
use tracing::{debug, warn};

use kukuri_blob_service::BlobService;
use kukuri_cn_core::{IndexEntryStore, IndexScopeKind, NewIndexEntry};
use kukuri_cn_safety::ReasonCode;
use kukuri_cn_safety::provider::{ProviderScanRequest, ScanReferenceGuard, SubjectKind};
use kukuri_cn_safety_runtime::{SafetyScanOutcome, SafetyScanService, ScanDisposition};
use kukuri_core::{KukuriEnvelope, ObjectStatus, ReplicaId, verify_post_withdrawal};
use kukuri_docs_sync::{DocFetchPolicy, DocRecord, DocsSync, SharedReplicaKeyFamily};

use crate::projection::{IndexProjection, IndexedEntry};
use crate::scheduler::{PostFetchJobKey, PostFetchJobState, PostFetchScheduler};

mod bucket_post;
mod failure;
mod targeted;
pub(crate) use targeted::recent_object_keys;
mod reference_guard;
mod relation;
pub use relation::changed_reactions;
mod source;
mod summary;
pub use summary::IngestSummary;
mod withdrawal;
use failure::{is_transient, transient};
use source::{PostObjectView, SourceResolver};

const RECORDS_PER_EXACT_KEY: usize = 8;

/// 変更通知の key 種別ごとの取り込み方（#1050 / #1065）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDisposition {
    /// key から対象 object を特定して、その object だけを取り込む。
    Object,
    /// 対象 object を特定できず、scope別の有界な見直しが必要。
    ScopeReview,
    /// 索引には影響せず、関係の観測（リアクション）だけに使う（#1221 R5-E。[`changed_reactions`]）。
    Relation,
    /// 索引に影響しないため取り込みの契機にしない。
    Ignore,
}

/// 種別ごとの取り込み方。種別が増えたらここで判断を強制する（ワイルドカードを置かない）。
///
/// `Ignore` にできるのは indexer が読まない種別だけ（[`INDEXER_READ_FAMILIES`] と交わらない）。
/// media manifest は参照元 object を特定できないため現在索引窓へ渡す
/// （投稿stateとmanifestの到着順が前後しても再確認する）。
pub const fn key_disposition(family: SharedReplicaKeyFamily) -> KeyDisposition {
    match family {
        SharedReplicaKeyFamily::PostObject | SharedReplicaKeyFamily::PostWithdrawal => {
            KeyDisposition::Object
        }
        SharedReplicaKeyFamily::MediaManifest => KeyDisposition::ScopeReview,
        SharedReplicaKeyFamily::Reaction => KeyDisposition::Relation,
        SharedReplicaKeyFamily::TimelineIndex
        | SharedReplicaKeyFamily::ThreadIndex
        | SharedReplicaKeyFamily::Envelope
        | SharedReplicaKeyFamily::Session
        | SharedReplicaKeyFamily::Channel
        | SharedReplicaKeyFamily::Metaverse => KeyDisposition::Ignore,
    }
}

/// indexer が共有 replica から読む key 種別（#1065 INVAR-3）。読み取りの prefix もここの種別から作る。
pub const INDEXER_READ_FAMILIES: [SharedReplicaKeyFamily; 4] = [
    SharedReplicaKeyFamily::PostObject,
    SharedReplicaKeyFamily::PostWithdrawal,
    SharedReplicaKeyFamily::MediaManifest,
    SharedReplicaKeyFamily::Reaction,
];

/// 変更通知から取り込み対象を決める鍵の分類（#1050 / #1065）。
///
/// `objects/<id>/…` と `withdrawals/<id>/state` は対象 object を特定できる。索引に影響しない
/// 種別（`indexes/`・`reactions/` 等）は無視する。media manifest と未登録の key は対象を特定
/// できないためscope別の見直しへ渡す。
#[derive(Debug, PartialEq, Eq)]
pub enum ChangedKeys {
    /// 特定できた object id の集合（重複なし・安定順）。
    Objects(Vec<String>),
    /// 索引に影響する key を含まない。
    Ignored,
    /// 対象を特定できない鍵を含むためscope別に見直す。同じbatchの既知object IDは保持する。
    /// `reason` は key の種別 prefix（未登録なら先頭 segment）で、識別子は含めない。
    ScopeReview {
        reason: String,
        objects: Vec<String>,
    },
}

/// 変更通知の鍵を取り込み対象へ分類する純関数。
pub fn classify_changed_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> ChangedKeys {
    let mut ids: Vec<String> = Vec::new();
    let mut review = None;
    let mut any = false;
    for key in keys {
        any = true;
        let Some((family, rest)) = SharedReplicaKeyFamily::parse(key) else {
            let segment = key.split('/').next().unwrap_or_default();
            review.get_or_insert_with(|| format!("unregistered:{segment}"));
            continue;
        };
        let prefix = family.prefix().trim_end_matches('/');
        match key_disposition(family) {
            KeyDisposition::Ignore | KeyDisposition::Relation => {}
            KeyDisposition::ScopeReview => {
                review.get_or_insert_with(|| prefix.to_string());
            }
            KeyDisposition::Object => match rest.split('/').next().filter(|id| !id.is_empty()) {
                Some(id) => {
                    if !ids.iter().any(|known| known == id) {
                        ids.push(id.to_string());
                    }
                }
                None => {
                    review.get_or_insert_with(|| format!("malformed:{prefix}"));
                }
            },
        }
    }
    if !any {
        return ChangedKeys::ScopeReview {
            reason: "empty".to_string(),
            objects: ids,
        };
    }
    if let Some(reason) = review {
        return ChangedKeys::ScopeReview {
            reason,
            objects: ids,
        };
    }
    if ids.is_empty() {
        return ChangedKeys::Ignored;
    }
    ChangedKeys::Objects(ids)
}

/// scope 走査で読み込んだ、per-record 処理の共有文脈。
struct ScopeContext {
    envelopes: HashMap<String, DocRecord>,
    withdrawn_object_ids: HashSet<String>,
}

#[derive(Clone, Copy)]
struct IngestScopeRef<'a> {
    kind: IndexScopeKind,
    id: &'a str,
    replica_id: &'a ReplicaId,
}

/// 1 record の取り込みで行った scan の内訳（#1050）。
#[derive(Debug, Default)]
struct ScanStats {
    fresh: usize,
    reused: usize,
}

/// ingest pipeline。docs replica + safety scan service + index 投影を束ねる。
///
/// safety scan は `SafetyScanService`（#406）経由で行い、scan と同時に moderation artifact
/// （signed moderation event / risk signal）が署名・永続化される。verdict gate（allow のみ投影）
/// は従来どおり本 pipeline が担う。
#[derive(Clone)]
pub struct IngestPipeline {
    docs_sync: Arc<dyn DocsSync>,
    safety: Arc<SafetyScanService>,
    entries: Arc<dyn IndexEntryStore>,
    projection: Arc<dyn IndexProjection>,
    /// `BlobText` 本文を scan 用に一時取得する。未構成時は blob 本文を fail-closed に除外する。
    blob_service: Option<Arc<dyn BlobService>>,
    /// 観測状態（#613 T3）。設定時のみスキャン失敗 / プロバイダ利用不可を分類して数える。
    metrics: Option<Arc<crate::state::IndexerRuntimeState>>,
    post_scheduler: Arc<PostFetchScheduler>,
    max_concurrent_posts: usize,
    /// 2 者間のアクションの保存先（#1221 R5-E）。未構成なら関係を観測しない。
    relation_pool: Option<sqlx::PgPool>,
}

impl IngestPipeline {
    pub fn new(
        docs_sync: Arc<dyn DocsSync>,
        safety: Arc<SafetyScanService>,
        entries: Arc<dyn IndexEntryStore>,
        projection: Arc<dyn IndexProjection>,
    ) -> Self {
        Self {
            docs_sync,
            safety,
            entries,
            projection,
            blob_service: None,
            metrics: None,
            post_scheduler: Arc::new(PostFetchScheduler::new(4)),
            max_concurrent_posts: 4,
            relation_pool: None,
        }
    }

    pub fn with_docs_source(mut self, source: Arc<dyn DocsSync>) -> Self {
        self.docs_sync = source;
        self
    }

    /// `BlobText` 本文の一時取得境界を接続する。
    pub fn with_blob_service(mut self, blob_service: Arc<dyn BlobService>) -> Self {
        self.blob_service = Some(blob_service);
        self
    }

    /// 観測状態を接続する（#613 T3。常駐ワーカーの組み立て時に使う）。
    pub fn with_metrics(mut self, metrics: Arc<crate::state::IndexerRuntimeState>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 2 者間のアクションを保存する DB を接続する（#1221 R5-E）。
    pub fn with_relation_pool(mut self, pool: sqlx::PgPool) -> Self {
        self.relation_pool = Some(pool);
        self
    }

    pub fn with_post_scheduler(
        mut self,
        scheduler: Arc<PostFetchScheduler>,
        max_concurrent_posts: usize,
    ) -> Self {
        self.post_scheduler = scheduler;
        self.max_concurrent_posts = max_concurrent_posts.max(1);
        self
    }

    /// 保存済み verdict を再利用できなければスキャンを実行し、観測状態に分類（スキャン失敗 /
    /// 外部プロバイダ利用不可）を記録する（#1050）。
    ///
    /// `source_fingerprint` は subject の内容識別子（post = state レコードの content hash、
    /// blob = blob hash）。再利用時はproviderを呼ばない。共通内容cacheから別subjectへ
    /// 再利用する場合は、そのsubjectのartifactを新たに生成する。
    ///
    /// media blob の未複製・ピア不在も verdict 上は `ProviderUnavailable` になるが、外部 safety
    /// provider 障害ではない。scan 中に media fetch の利用不可カウンタが増えた場合は、専用の
    /// `media_fetch_unavailable` だけに記録し、provider 障害カウンタへ重複計上しない。
    async fn scan_or_reuse_with_metrics(
        &self,
        request: &ProviderScanRequest,
        subject_author: &str,
        source_fingerprint: &str,
        stats: &mut ScanStats,
        guard: &dyn ScanReferenceGuard,
    ) -> Result<SafetyScanOutcome> {
        let media_fetch_unavailable_before = self
            .metrics
            .as_ref()
            .map(|metrics| metrics.media_fetch_unavailable_count());
        let outcome = match self
            .safety
            .scan_or_reuse_guarded(request, subject_author, source_fingerprint, guard)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Some(metrics) = &self.metrics {
                    metrics.record_scan_error();
                }
                return Err(error);
            }
        };
        match outcome.disposition {
            ScanDisposition::Reused => {
                stats.reused += 1;
                return Ok(outcome);
            }
            ScanDisposition::Fresh => stats.fresh += 1,
        }
        if let Some(metrics) = &self.metrics {
            let media_fetch_became_unavailable = media_fetch_unavailable_before
                .is_some_and(|before| metrics.media_fetch_unavailable_count() > before);
            match outcome.report.verdict.reason_code {
                ReasonCode::ProviderUnavailable if !media_fetch_became_unavailable => {
                    metrics.record_provider_unavailable();
                }
                ReasonCode::ScanFailed | ReasonCode::Unscanned => metrics.record_scan_error(),
                _ => {}
            }
        }
        Ok(outcome)
    }

    /// 変更通知で届いた鍵に対応する object だけを取り込む（#1050 AC-4）。
    ///
    /// `objects/<id>/…` と `withdrawals/<id>/state` は対象 object を特定できるため、その object の
    /// `objects/<id>/` prefix（state + envelope）と撤回だけを読み、scope 全体の prefix 走査を
    /// 行わない。対象を特定できない鍵（media manifest / 未登録 key）が混ざる場合は
    /// 現在索引窓へ渡す。索引に影響しない鍵だけなら何もしない（#1065）。
    pub async fn ingest_changed_keys(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        replica_id: &ReplicaId,
        keys: &[String],
    ) -> Result<IngestSummary> {
        crate::replica_plan::validate_scope_replica(scope_kind, scope_id, replica_id)?;
        let reactions = changed_reactions(keys.iter().map(String::as_str));
        if !reactions.is_empty() {
            self.observe_reactions(scope_kind, scope_id, replica_id, &[], Some(&reactions))
                .await;
        }
        match classify_changed_keys(keys.iter().map(String::as_str)) {
            ChangedKeys::Objects(ids) => {
                self.ingest_object_ids(scope_kind, scope_id, replica_id, &ids)
                    .await
            }
            ChangedKeys::Ignored => {
                debug!(
                    replica_id = %replica_id.as_str(),
                    keys = keys.len(),
                    "changed keys do not affect the index; skipping"
                );
                Ok(IngestSummary::default())
            }
            ChangedKeys::ScopeReview { reason, objects } => {
                debug!(
                    replica_id = %replica_id.as_str(),
                    keys = keys.len(),
                    reason = %reason,
                    "changed keys are not object-scoped; checking the current index window"
                );
                let mut summary = if objects.is_empty() {
                    IngestSummary::default()
                } else {
                    self.ingest_object_ids(scope_kind, scope_id, replica_id, &objects)
                        .await?
                };
                summary.merge(
                    Box::pin(
                        self.ingest_recent_scope_excluding(
                            scope_kind, scope_id, replica_id, &objects,
                        ),
                    )
                    .await?,
                );
                Ok(summary)
            }
        }
    }

    /// prefix 走査結果を state record と共有文脈（envelope map + 検証済み撤回集合）に分ける。
    ///
    /// 撤回は docs が canonical（gossip は起床合図に過ぎない）。元の署名済み envelope に対して
    /// 検証できた撤回だけを抑制対象にする。
    async fn scope_context(
        &self,
        replica_id: &ReplicaId,
        records: Vec<DocRecord>,
        changed_object_ids: &[String],
    ) -> Result<(Vec<DocRecord>, ScopeContext)> {
        // 同一 prefix scan の envelope entry を object_id -> envelope record で index 化し、
        // blob text の本文取得で追加クエリ（N+1）を発生させないようにする。
        let mut envelopes: HashMap<String, DocRecord> = HashMap::new();
        let mut state_records: Vec<DocRecord> = Vec::new();
        for record in records {
            if let Some(object_id) = record.key.strip_suffix("/envelope") {
                if let Some(object_id) = object_id.strip_prefix("objects/") {
                    if replica_id.as_str().starts_with("bucket::") {
                        // 形の違うコピー・署名不正で、同じkeyの正しいenvelopeを上書きしない。
                        if bucket_post::canonical_post(replica_id, object_id, &record).is_some() {
                            envelopes.entry(object_id.to_string()).or_insert(record);
                        }
                        continue;
                    }
                    envelopes.insert(object_id.to_string(), record);
                }
            } else if record.key.ends_with("/state") {
                state_records.push(record);
            }
        }

        let withdrawal_records = {
            let mut records = Vec::new();
            for object_id in changed_object_ids {
                let key = format!(
                    "{}{object_id}/state",
                    SharedReplicaKeyFamily::PostWithdrawal.prefix()
                );
                records.extend(
                    self.docs_sync
                        .query_replica_exact_bounded(
                            replica_id,
                            &key,
                            RECORDS_PER_EXACT_KEY,
                            DocFetchPolicy::LocalThenRemote,
                        )
                        .await
                        .with_context(|| format!("failed to query withdrawal for {object_id}"))?,
                );
            }
            records
        };
        let mut withdrawn_object_ids = HashSet::new();
        for record in withdrawal_records {
            if !record.key.ends_with("/state") {
                continue;
            }
            let Ok(withdrawal) = serde_json::from_slice::<KukuriEnvelope>(&record.value) else {
                continue;
            };
            let Ok(Some(content)) = withdrawal.post_withdrawal_content() else {
                continue;
            };
            let Some(target_record) = envelopes.get(content.target_object_id.as_str()) else {
                continue;
            };
            let Ok(target) = serde_json::from_slice::<KukuriEnvelope>(&target_record.value) else {
                continue;
            };
            if verify_post_withdrawal(&withdrawal, &target).is_ok() {
                withdrawn_object_ids.insert(content.target_object_id.0);
            }
        }
        Ok((
            state_records,
            ScopeContext {
                envelopes,
                withdrawn_object_ids,
            },
        ))
    }

    /// state record 群を 1 件ずつ取り込む（単一 entry の失敗で scope 全体を止めない）。
    async fn ingest_records(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        replica_id: &ReplicaId,
        state_records: &[DocRecord],
        context: &ScopeContext,
    ) -> Result<IngestSummary> {
        // これは一部keyまたは1bucketの窓。ここに無い投稿をscopeから消えたとは判定しない。
        // 完了jobの保持上限はschedulerが所有し、別bucketの実行中leaseを失効させない。
        self.apply_verified_withdrawals(scope_kind, scope_id, &context.withdrawn_object_ids)
            .await?;
        let mut waiting = state_records.iter();
        let mut active = FuturesUnordered::new();
        for _ in 0..self.max_concurrent_posts {
            if let Some(record) = waiting.next() {
                active.push(
                    self.ingest_scheduled_record(scope_kind, scope_id, replica_id, record, context),
                );
            }
        }
        let mut results = Vec::with_capacity(state_records.len());
        while let Some(result) = active.next().await {
            results.push(result);
            if let Some(record) = waiting.next() {
                active.push(
                    self.ingest_scheduled_record(scope_kind, scope_id, replica_id, record, context),
                );
            }
        }

        let mut summary = IngestSummary::default();
        for (record_key, outcome, stats) in results {
            summary.scanned += 1;
            summary.scans_fresh += stats.fresh;
            summary.scans_reused += stats.reused;
            match outcome {
                Ok(IngestOutcome::Indexed) => summary.indexed += 1,
                Ok(IngestOutcome::SkippedNonAllow) => summary.skipped_non_allow += 1,
                Ok(IngestOutcome::Deindexed) => summary.deindexed += 1,
                Ok(IngestOutcome::Ignored) => {}
                Err(error) if is_transient(&error) => {
                    // 一時的な失敗では既存 entry を真実源・投影とも保持し、この走査では新たに
                    // 索引しない（upsert へ到達していない）。次の走査で再評価する（#1090）。
                    warn!(
                        replica_id = %replica_id.as_str(),
                        key = %record_key,
                        error = %format!("{error:#}"),
                        "temporarily failed to ingest object record; keeping any existing entry"
                    );
                    summary.skipped_non_allow += 1;
                }
                Err(error) => {
                    if let Some(id) = post_id_from_state_key(&record_key) {
                        self.deindex_object(scope_kind, scope_id, id).await?;
                    }
                    // 単一 entry の失敗で scope 全体を止めない。fail-closed（投影しない）側に倒す。
                    warn!(
                        replica_id = %replica_id.as_str(),
                        key = %record_key,
                        error = %format!("{error:#}"),
                        "failed to ingest object record; skipping (fail-closed)"
                    );
                    summary.skipped_non_allow += 1;
                }
            }
        }
        Ok(summary)
    }

    async fn ingest_scheduled_record(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        replica_id: &ReplicaId,
        record: &DocRecord,
        context: &ScopeContext,
    ) -> (String, Result<IngestOutcome>, ScanStats) {
        let object_id = post_id_from_state_key(&record.key)
            .unwrap_or(record.key.as_str())
            .to_string();
        let source_revision = if replica_id.as_str().starts_with("bucket::") {
            if context
                .envelopes
                .get(&object_id)
                .and_then(|envelope| bucket_post::canonical_post(replica_id, &object_id, envelope))
                .is_none()
            {
                return (
                    record.key.clone(),
                    Ok(IngestOutcome::Ignored),
                    ScanStats::default(),
                );
            }
            object_id.clone()
        } else {
            record.content_hash.clone()
        };
        let Some(lease) = self.post_scheduler.enqueue(
            PostFetchJobKey {
                scope_kind: scope_kind.as_str().to_string(),
                scope_id: scope_id.to_string(),
                object_id,
            },
            source_revision,
        ) else {
            return (
                record.key.clone(),
                Ok(IngestOutcome::Ignored),
                ScanStats::default(),
            );
        };
        let Some(_permit) = self.post_scheduler.start(&lease).await else {
            return (
                record.key.clone(),
                Ok(IngestOutcome::Ignored),
                ScanStats::default(),
            );
        };
        let mut stats = ScanStats::default();
        let outcome = self
            .ingest_object_record(
                IngestScopeRef {
                    kind: scope_kind,
                    id: scope_id,
                    replica_id,
                },
                record,
                context,
                &lease,
                &mut stats,
            )
            .await;
        let state = match &outcome {
            Ok(IngestOutcome::Indexed | IngestOutcome::Ignored) => PostFetchJobState::Completed,
            Ok(IngestOutcome::SkippedNonAllow | IngestOutcome::Deindexed) => {
                PostFetchJobState::Suppressed
            }
            Err(error) if is_transient(error) => PostFetchJobState::RetryWait,
            Err(_) => PostFetchJobState::Suppressed,
        };
        self.post_scheduler.finish(&lease, state);
        (record.key.clone(), outcome, stats)
    }

    async fn ingest_object_record(
        &self,
        scope: IngestScopeRef<'_>,
        record: &DocRecord,
        context: &ScopeContext,
        job_lease: &crate::scheduler::PostFetchJobLease,
        stats: &mut ScanStats,
    ) -> Result<IngestOutcome> {
        let scope_kind = scope.kind;
        let scope_id = scope.id;
        let replica_id = scope.replica_id;
        // Other key domains are not posts. A corrupt value under a real post identity,
        // however, must reach the error path to remove a previously indexed row.
        let Some(object_id) = post_id_from_state_key(&record.key) else {
            return Ok(IngestOutcome::Ignored);
        };
        let object: PostObjectView =
            if replica_id.as_str().starts_with("bucket::") {
                let Some(object) = context.envelopes.get(object_id).and_then(|envelope| {
                    bucket_post::canonical_post(replica_id, object_id, envelope)
                }) else {
                    // この候補が正本の配置に属する証拠が無い。別bucketの索引を削除する根拠にしない。
                    return Ok(IngestOutcome::Ignored);
                };
                object
            } else {
                serde_json::from_slice(&record.value).context("invalid post object state")?
            };
        if record.key != format!("objects/{}/state", object.object_id) {
            bail!("post object identity does not match its key");
        }

        if context
            .withdrawn_object_ids
            .contains(object.object_id.as_str())
        {
            // ingest_records already persisted this verified withdrawal and removed its projection.
            return Ok(IngestOutcome::Deindexed);
        }
        if self
            .suppress_known_withdrawal(scope_kind, scope_id, object.object_id.as_str())
            .await?
        {
            return Ok(IngestOutcome::Deindexed);
        }

        // tombstone / deleted は de-index する（replica 上で消えた content を真実源にも投影にも
        // 残さない）。
        if matches!(
            object.status,
            ObjectStatus::Deleted | ObjectStatus::Tombstoned
        ) {
            self.deindex_object(scope_kind, scope_id, object.object_id.as_str())
                .await?;
            return Ok(IngestOutcome::Deindexed);
        }

        // A node-local legal decision wins over a successful safety scan. Check before resolving
        // body/media so a prevented subject is neither fetched nor reintroduced by backfill.
        if self
            .entries
            .is_transmission_prevented(object.object_id.as_str())
            .await
            .map_err(transient)?
        {
            self.deindex_object(scope_kind, scope_id, object.object_id.as_str())
                .await?;
            return Ok(IngestOutcome::Deindexed);
        }

        let guard = reference_guard::ReferenceGuard {
            pipeline: self,
            scope_kind,
            scope_id,
            replica: replica_id,
            object: &object,
            record,
            envelope: context.envelopes.get(&object.object_id),
            media_targets: std::sync::Mutex::new(None),
            definitive_failure: std::sync::atomic::AtomicBool::new(false),
            scheduler: self.post_scheduler.as_ref(),
            job_lease,
        };
        guard.verify().await?;
        // 本文 text を取り出す。blob 参照は scan 用の一時 fetch のみ（恒久保存しない）。
        let source = SourceResolver::new(self.docs_sync.as_ref(), self.blob_service.as_deref());
        let text = match source
            .resolve_body_text(replica_id, &object, &context.envelopes)
            .await
        {
            Ok(text) => text,
            Err(error) if is_transient(&error) => return Err(error),
            Err(error) => {
                self.deindex_object(scope_kind, scope_id, object.object_id.as_str())
                    .await?;
                warn!(
                    object_id = %object.object_id,
                    error = %format!("{error:#}"),
                    "failed to resolve post body; not indexing the post (fail-closed)"
                );
                return Ok(IngestOutcome::SkippedNonAllow);
            }
        };
        if !self.post_scheduler.mark_processing(job_lease) {
            return Err(transient(anyhow::anyhow!(
                "post source revision was superseded after fetch"
            )));
        }

        // safety scan（fail-closed）。post 本文 text を scan service に渡す。生成された
        // moderation artifact（risk signal / signed event）は service が署名・永続化する（#406）。
        // 永続化失敗は `?` で呼び出し側の per-entry fail-closed（投影しない）に乗る。
        // v1は検証済み署名のIDを使い、未署名markerやJSONの表記変更で再検査しない。
        // legacyは従来のstate content hashを維持する。
        let fingerprint = if replica_id.as_str().starts_with("bucket::") {
            object.object_id.as_str()
        } else {
            record.content_hash.as_str()
        };
        let request = ProviderScanRequest::for_subject(SubjectKind::Post, object.object_id.clone())
            .with_text(text.clone());
        let outcome = self
            .scan_or_reuse_with_metrics(&request, &object.author, fingerprint, stats, &guard)
            .await
            .map_err(|error| guard.classify_scan_error(error))?;
        let report = &outcome.report;

        if !report.verdict.is_indexable() {
            // unscanned / scan_failed / provider_unavailable / 非 allow は index しない。
            // 既に index 済みなら真実源・投影の両方から de-index する（後から verdict が
            // 変わった場合の整合）。
            self.deindex_object(scope_kind, scope_id, object.object_id.as_str())
                .await?;
            debug!(
                object_id = %object.object_id,
                reason = ?report.verdict.reason_code,
                "verdict is not allow; not indexing (fail-closed)"
            );
            return Ok(IngestOutcome::SkippedNonAllow);
        }

        // media 参照を blob 単位で 1 つずつ scan する（#420 / ADR 0028、manifest 展開は #609）。
        // attachments は `AssetRef` の blob hash + mime、manifest 参照は replica 上の署名済み
        // manifest を解決して item の blob hash + mime に展開する。manifest が解決できない場合は
        // scan 対象を確定できないため post を index しない（fail-closed）。
        // いずれか 1 つでも非 allow なら post 全体を index しない（worst-case 合成）。media
        // provider / fetcher が未構成なら scan は Unavailable → fail-closed hold になり、media
        // 参照 post は従来どおり index されない（挙動後退なし）。全 allow のときのみ derived
        // 検索タグを収集する。
        let media_targets = match source.media_scan_targets(replica_id, &object).await {
            Ok(targets) => targets,
            Err(error) if is_transient(&error) => return Err(error),
            Err(error) => {
                self.deindex_object(scope_kind, scope_id, object.object_id.as_str())
                    .await?;
                warn!(
                    object_id = %object.object_id,
                    error = %format!("{error:#}"),
                    "failed to resolve media references; not indexing the post (fail-closed)"
                );
                return Ok(IngestOutcome::SkippedNonAllow);
            }
        };
        *guard
            .media_targets
            .lock()
            .expect("media reference guard mutex") = Some(media_targets.clone());
        let mut derived_tags: Vec<String> = report.derived_tags.clone();
        // content advisory は本文 text と各 blob の和集合（ADR 0028 §8.3）。blob 単位の要素は
        // `subject_kind = blob_cid` のまま post 行へ同梱し、client の hash 単位取得ゲートに使う。
        let mut advisories = outcome.advisories.clone();
        for target in media_targets {
            let mut request =
                ProviderScanRequest::for_subject(SubjectKind::Blob, target.hash.clone())
                    .with_media_hint(target.hash.clone());
            if let Some(mime) = &target.mime {
                request = request.with_media_mime(mime.clone());
            }
            // blob は不変なので hash 自体が内容 fingerprint。
            let media_outcome = self
                .scan_or_reuse_with_metrics(&request, &object.author, &target.hash, stats, &guard)
                .await
                .map_err(|error| guard.classify_scan_error(error))?;
            let media_report = &media_outcome.report;
            if !media_report.verdict.is_indexable() {
                self.deindex_object(scope_kind, scope_id, object.object_id.as_str())
                    .await?;
                debug!(
                    object_id = %object.object_id,
                    media_hint = %target.hash,
                    reason = ?media_report.verdict.reason_code,
                    "referenced media verdict is not allow; not indexing the post (fail-closed)"
                );
                return Ok(IngestOutcome::SkippedNonAllow);
            }
            for tag in &media_report.derived_tags {
                if !derived_tags.contains(tag) {
                    derived_tags.push(tag.clone());
                }
            }
            for advisory in &media_outcome.advisories {
                if !advisories.contains(advisory) {
                    advisories.push(advisory.clone());
                }
            }
        }
        // post の verdict 行に和集合を確定させる（値が同じなら store 側で no-op）。query 境界は
        // この行から `content_advisories` を導出する（ADR 0025 §7.1）。
        guard.verify().await?;
        self.safety
            .persist_advisories(SubjectKind::Post, object.object_id.as_str(), &advisories)
            .await
            .map_err(transient)?;

        // ① index 真実源（Postgres）へ upsert する。verdict record への FK と CHECK 制約
        // （allow のみ / 非 critical のみ）が fail-closed を DB 層でも保証する（#404）。
        // subject を渡した scan は必ず verdict state を記録するため verdict_id は存在するはずだが、
        // 無ければ index しない（fail-closed）。
        let Some(verdict_id) = outcome.verdict_id.as_deref() else {
            bail!(
                "scan outcome for object `{}` has no verdict record; refusing to index (fail-closed)",
                object.object_id
            );
        };
        guard.verify().await?;
        self.entries
            .upsert_entry(&NewIndexEntry {
                scope_kind,
                scope_id: scope_id.to_string(),
                object_id: object.object_id.clone(),
                author_pubkey: object.author.clone(),
                created_at: object.created_at,
                source_replica_id: replica_id.as_str().to_string(),
                verdict_id: verdict_id.to_string(),
                verdict_action: report.verdict.action.as_str().to_string(),
                critical: report.verdict.critical,
            })
            .await
            .context("failed to record index entry in the authoritative store")
            .map_err(transient)?;

        // ② 全文検索投影（ArcadeDB）へ upsert する。ここが失敗しても真実源には entry が残るが、
        // 投影に無い entry は検索に出ないだけで安全側（fail-closed）に倒れる。
        // derived 検索タグは text へ相乗りさせる（ADR 0025 §2.3。`allow` verdict のみここに
        // 到達し、タグは `derived_tags_for_index` で critical / Match Data / 生スコア除外済み）。
        let entry = IndexedEntry {
            scope_kind,
            scope_id: scope_id.to_string(),
            object_id: object.object_id.clone(),
            author_pubkey: object.author.clone(),
            text: text_with_tags(&text, &derived_tags),
            created_at: object.created_at,
            source_replica_id: replica_id.as_str().to_string(),
            content_advisories: Vec::new(),
        };
        guard.verify().await?;
        self.projection
            .upsert_entry(&entry)
            .await
            .map_err(transient)?;
        if outcome.disposition == ScanDisposition::Fresh
            && let Some(metrics) = &self.metrics
        {
            // 新規に判定して索引に入った投稿の、作成から索引までの遅れ（著者時刻由来の近似値）。
            metrics.record_index_lag(chrono::Utc::now().timestamp() - object.created_at);
        }
        if scope_kind == IndexScopeKind::PublicTopic {
            self.observe_post_actions(scope_id, &object).await;
        }
        Ok(IngestOutcome::Indexed)
    }

    async fn retain_supported_scope(&self, kind: IndexScopeKind, id: &str) -> Result<bool> {
        if self.entries.is_scope_supported(kind, id).await? {
            return Ok(true);
        }
        self.entries.remove_scope(kind, id).await?;
        self.projection.remove_scope(kind, id).await?;
        Ok(false)
    }

    /// object を index 真実源 → 投影の順で両方から消す。
    ///
    /// 真実源を先に消すことで、投影側の削除が失敗して hit が残留しても query 境界の突合
    /// （真実源に無い hit は返さない）が即座に効く。
    async fn deindex_object(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
    ) -> Result<()> {
        self.entries
            .remove_entry(scope_kind, scope_id, object_id)
            .await?;
        self.projection
            .remove_object(scope_kind, scope_id, object_id)
            .await?;
        Ok(())
    }
}

enum IngestOutcome {
    Indexed,
    SkippedNonAllow,
    Deindexed,
    Ignored,
}

fn post_id_from_state_key(key: &str) -> Option<&str> {
    key.strip_prefix("objects/")?
        .strip_suffix("/state")
        .filter(|id| id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// 本文 text に derived 検索タグを相乗りさせた投影用 text を組み立てる。
///
/// タグ専用列は持たない（`IndexedEntry.text` が全文検索の単一入力。ADR 0025 §2.3）。
/// 本文が空（画像のみの投稿）の場合はタグのみになる。
fn text_with_tags(text: &str, derived_tags: &[String]) -> String {
    if derived_tags.is_empty() {
        return text.to_string();
    }
    let tags = derived_tags.join(" ");
    if text.trim().is_empty() {
        tags
    } else {
        format!("{text}\n{tags}")
    }
}
