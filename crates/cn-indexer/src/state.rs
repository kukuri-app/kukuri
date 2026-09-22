//! 観測用の実行状態（#613 T3）。
//!
//! 常駐ワーカーが更新し、テスト・起動完了判定（#612）・HTTP 状態エンドポイント（`status`）が
//! 読む共有状態。観測用の状態の置き場はここだけにする（他の場所に分散させない）。
//!
//! 時刻はすべて呼び出し側が unix 秒で渡す（この構造体は時計を持たない）。

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::ingest::IngestSummary;
use crate::scheduler::{PostFetchScheduler, PostFetchSchedulerSnapshot};

/// 観測状態の写し。`GET /v1/status` はこの形をそのまま JSON で返す。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct IndexerStateSnapshot {
    #[serde(default)]
    pub moderation: kukuri_cn_safety::metrics::ModerationMetricsSnapshot,
    /// ワーカーが動いているか。
    pub worker_running: bool,
    /// 取り込みが有効か（安全性プロバイダ未設定なら false のまま常駐する）。
    pub ingest_enabled: bool,
    /// 開いているスコープ数。
    pub opened_scopes: u64,
    /// 最後に全件見直し（restore → 取り込み 1 巡）が成功した時刻（unix 秒）。
    pub last_sync_at: Option<i64>,
    /// 最後にスコープ取り込みが成功した時刻（unix 秒）。
    pub last_ingest_at: Option<i64>,
    /// 最後のエラー内容。
    pub last_error: Option<String>,
    /// 最後のエラーが起きた対象スコープ（`replica id` 表現。全体エラーなら None）。
    pub last_error_scope: Option<String>,
    /// 走査した項目数の累計。
    pub scanned: u64,
    /// 許可されて索引に入れた件数の累計。
    pub indexed: u64,
    /// 不許可などで索引に入れなかった件数の累計（下の 2 つの分類を含む）。
    pub skipped_non_allow: u64,
    /// スキャン失敗（scan_failed / unscanned / スキャン呼び出しの失敗）の件数。
    pub scan_errors: u64,
    /// media 取得不能を除く、外部 safety provider 利用不可の件数。
    pub provider_unavailable: u64,
    /// 索引から外した件数の累計。
    pub deindexed: u64,
    /// メディア取得の成功件数。
    pub media_fetch_success: u64,
    /// メディア取得の利用不可（未複製・ピア不在など）件数。
    pub media_fetch_unavailable: u64,
    /// メディア取得の時間切れ件数。
    pub media_fetch_timeout: u64,
    /// メディア取得の大きさ超過件数。
    pub media_fetch_oversize: u64,
    /// provider を呼んで判定した scan 数の累計（#1050）。
    #[serde(default)]
    pub scans_fresh: u64,
    /// 保存済み verdict を再利用して provider を呼ばなかった scan 数の累計（#1050）。
    #[serde(default)]
    pub scans_reused: u64,
    /// 最後の全件見直し 1 巡にかかった時間（ミリ秒。#1050）。
    #[serde(default)]
    pub last_pass_duration_ms: Option<u64>,
    /// 最後の変更通知駆動の取り込みにかかった時間（ミリ秒。#1050）。
    #[serde(default)]
    pub last_event_ingest_duration_ms: Option<u64>,
    /// 最後に新規判定で索引に入った投稿の、作成時刻から索引までの遅れ（秒。著者時刻由来の
    /// 近似値。#1050）。
    #[serde(default)]
    pub last_index_lag_secs: Option<i64>,
    /// 変更通知の key から対象 object を特定できず scope 全体の見直しへ倒した回数の累計（#1065）。
    #[serde(default)]
    pub event_whole_scope_fallbacks: u64,
    /// 直近の全体見直しへ倒した理由（key の種別 prefix。object id 等は含めない。#1065）。
    #[serde(default)]
    pub last_whole_scope_fallback_reason: Option<String>,
    /// 投稿取得schedulerの現在状態。本文・hash・peer識別子は含めない。
    #[serde(default)]
    pub post_scheduler: PostFetchSchedulerSnapshot,
}

/// 共有の観測状態。ワーカー・取り込みパイプライン・メディア取得器が更新する。
#[derive(Debug, Default)]
pub struct IndexerRuntimeState {
    moderation: RwLock<Option<std::sync::Arc<kukuri_cn_safety::metrics::ModerationMetrics>>>,
    worker_running: AtomicBool,
    ingest_enabled: AtomicBool,
    opened_scopes: AtomicU64,
    last_sync_at: RwLock<Option<i64>>,
    last_ingest_at: RwLock<Option<i64>>,
    last_error: RwLock<Option<(String, Option<String>)>>,
    scanned: AtomicU64,
    indexed: AtomicU64,
    skipped_non_allow: AtomicU64,
    scan_errors: AtomicU64,
    provider_unavailable: AtomicU64,
    deindexed: AtomicU64,
    media_fetch_success: AtomicU64,
    media_fetch_unavailable: AtomicU64,
    media_fetch_timeout: AtomicU64,
    media_fetch_oversize: AtomicU64,
    scans_fresh: AtomicU64,
    scans_reused: AtomicU64,
    last_pass_duration_ms: RwLock<Option<u64>>,
    last_event_ingest_duration_ms: RwLock<Option<u64>>,
    last_index_lag_secs: RwLock<Option<i64>>,
    event_whole_scope_fallbacks: AtomicU64,
    last_whole_scope_fallback_reason: RwLock<Option<String>>,
    post_scheduler: RwLock<Option<std::sync::Arc<PostFetchScheduler>>>,
}

impl IndexerRuntimeState {
    pub fn set_post_scheduler(&self, scheduler: std::sync::Arc<PostFetchScheduler>) {
        *self
            .post_scheduler
            .write()
            .expect("post scheduler lock poisoned") = Some(scheduler);
    }
    pub fn set_moderation_metrics(
        &self,
        metrics: Option<std::sync::Arc<kukuri_cn_safety::metrics::ModerationMetrics>>,
    ) {
        *self.moderation.write().expect("moderation metrics lock") = metrics;
    }
    pub fn set_worker_running(&self, running: bool) {
        self.worker_running.store(running, Ordering::Relaxed);
    }

    pub fn set_ingest_enabled(&self, enabled: bool) {
        self.ingest_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn set_opened_scopes(&self, count: u64) {
        self.opened_scopes.store(count, Ordering::Relaxed);
    }

    /// 全件見直しの成功を記録する（unix 秒）。
    pub fn record_sync_success(&self, at_unix: i64) {
        *self.last_sync_at.write().expect("last_sync_at poisoned") = Some(at_unix);
    }

    /// スコープ取り込みの成功と、その取り込み結果の件数を記録する。
    pub fn record_ingest_success(&self, at_unix: i64, summary: &IngestSummary) {
        *self
            .last_ingest_at
            .write()
            .expect("last_ingest_at poisoned") = Some(at_unix);
        self.scanned
            .fetch_add(summary.scanned as u64, Ordering::Relaxed);
        self.indexed
            .fetch_add(summary.indexed as u64, Ordering::Relaxed);
        self.skipped_non_allow
            .fetch_add(summary.skipped_non_allow as u64, Ordering::Relaxed);
        self.deindexed
            .fetch_add(summary.deindexed as u64, Ordering::Relaxed);
        self.scans_fresh
            .fetch_add(summary.scans_fresh as u64, Ordering::Relaxed);
        self.scans_reused
            .fetch_add(summary.scans_reused as u64, Ordering::Relaxed);
    }

    /// 全件見直し 1 巡の所要時間を記録する（#1050）。
    pub fn record_pass_duration(&self, millis: u64) {
        *self
            .last_pass_duration_ms
            .write()
            .expect("last_pass_duration_ms poisoned") = Some(millis);
    }

    /// 変更通知駆動の取り込みの所要時間を記録する（#1050）。
    pub fn record_event_ingest_duration(&self, millis: u64) {
        *self
            .last_event_ingest_duration_ms
            .write()
            .expect("last_event_ingest_duration_ms poisoned") = Some(millis);
    }

    /// 新規判定で索引に入った投稿の作成→索引の遅れを記録する（秒。#1050）。
    pub fn record_index_lag(&self, secs: i64) {
        *self
            .last_index_lag_secs
            .write()
            .expect("last_index_lag_secs poisoned") = Some(secs.max(0));
    }

    /// エラーを記録する（scope は replica id 表現。全体エラーなら None）。
    pub fn record_error(&self, scope: Option<&str>, error: &str) {
        *self.last_error.write().expect("last_error poisoned") =
            Some((error.to_string(), scope.map(str::to_string)));
    }

    /// 索引解除（スコープ単位の削除など、取り込み結果の外で行った分）を記録する。
    pub fn record_deindexed(&self, count: u64) {
        self.deindexed.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_scan_error(&self) {
        self.scan_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_provider_unavailable(&self) {
        self.provider_unavailable.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_media_fetch_success(&self) {
        self.media_fetch_success.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_media_fetch_unavailable(&self) {
        self.media_fetch_unavailable.fetch_add(1, Ordering::Relaxed);
    }

    /// media 取得不能の現在値を返す。
    ///
    /// scan 前後の差分から、`ProviderUnavailable` が外部 provider ではなく media fetch 境界で
    /// 発生したかを分類するために使う。常駐 worker は scope / entry を逐次 scan する。
    pub(crate) fn media_fetch_unavailable_count(&self) -> u64 {
        self.media_fetch_unavailable.load(Ordering::Relaxed)
    }

    /// 変更通知の取り込みが scope 全体の見直しへ倒れたことと、その理由 prefix を記録する（#1065）。
    pub fn record_whole_scope_fallback(&self, reason: &str) {
        self.event_whole_scope_fallbacks
            .fetch_add(1, Ordering::Relaxed);
        *self
            .last_whole_scope_fallback_reason
            .write()
            .expect("last_whole_scope_fallback_reason poisoned") = Some(reason.to_string());
    }

    pub fn record_media_fetch_timeout(&self) {
        self.media_fetch_timeout.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_media_fetch_oversize(&self) {
        self.media_fetch_oversize.fetch_add(1, Ordering::Relaxed);
    }

    /// 現在の状態の写しを返す。
    pub fn snapshot(&self) -> IndexerStateSnapshot {
        let (last_error, last_error_scope) =
            match self.last_error.read().expect("last_error poisoned").clone() {
                Some((error, scope)) => (Some(error), scope),
                None => (None, None),
            };
        IndexerStateSnapshot {
            moderation: self
                .moderation
                .read()
                .expect("moderation metrics lock")
                .as_ref()
                .map(|metrics| metrics.snapshot())
                .unwrap_or_default(),
            worker_running: self.worker_running.load(Ordering::Relaxed),
            ingest_enabled: self.ingest_enabled.load(Ordering::Relaxed),
            opened_scopes: self.opened_scopes.load(Ordering::Relaxed),
            last_sync_at: *self.last_sync_at.read().expect("last_sync_at poisoned"),
            last_ingest_at: *self.last_ingest_at.read().expect("last_ingest_at poisoned"),
            last_error,
            last_error_scope,
            scanned: self.scanned.load(Ordering::Relaxed),
            indexed: self.indexed.load(Ordering::Relaxed),
            skipped_non_allow: self.skipped_non_allow.load(Ordering::Relaxed),
            scan_errors: self.scan_errors.load(Ordering::Relaxed),
            provider_unavailable: self.provider_unavailable.load(Ordering::Relaxed),
            deindexed: self.deindexed.load(Ordering::Relaxed),
            media_fetch_success: self.media_fetch_success.load(Ordering::Relaxed),
            media_fetch_unavailable: self.media_fetch_unavailable.load(Ordering::Relaxed),
            media_fetch_timeout: self.media_fetch_timeout.load(Ordering::Relaxed),
            media_fetch_oversize: self.media_fetch_oversize.load(Ordering::Relaxed),
            scans_fresh: self.scans_fresh.load(Ordering::Relaxed),
            scans_reused: self.scans_reused.load(Ordering::Relaxed),
            last_pass_duration_ms: *self
                .last_pass_duration_ms
                .read()
                .expect("last_pass_duration_ms poisoned"),
            last_event_ingest_duration_ms: *self
                .last_event_ingest_duration_ms
                .read()
                .expect("last_event_ingest_duration_ms poisoned"),
            last_index_lag_secs: *self
                .last_index_lag_secs
                .read()
                .expect("last_index_lag_secs poisoned"),
            event_whole_scope_fallbacks: self.event_whole_scope_fallbacks.load(Ordering::Relaxed),
            last_whole_scope_fallback_reason: self
                .last_whole_scope_fallback_reason
                .read()
                .expect("last_whole_scope_fallback_reason poisoned")
                .clone(),
            post_scheduler: self
                .post_scheduler
                .read()
                .expect("post scheduler lock poisoned")
                .as_ref()
                .map(|scheduler| scheduler.snapshot())
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reflects_flags_and_counters() {
        let state = IndexerRuntimeState::default();
        assert_eq!(state.snapshot(), IndexerStateSnapshot::default());

        state.set_worker_running(true);
        state.set_ingest_enabled(true);
        state.set_opened_scopes(2);
        state.record_sync_success(100);
        state.record_ingest_success(
            101,
            &IngestSummary {
                scanned: 3,
                indexed: 2,
                skipped_non_allow: 1,
                deindexed: 0,
                scans_fresh: 4,
                scans_reused: 5,
            },
        );
        state.record_pass_duration(1200);
        state.record_event_ingest_duration(80);
        state.record_index_lag(-3);
        state.record_scan_error();
        state.record_provider_unavailable();
        state.record_deindexed(4);
        state.record_media_fetch_success();
        state.record_media_fetch_unavailable();
        state.record_media_fetch_timeout();
        state.record_media_fetch_oversize();
        state.record_error(Some("topic::rust"), "boom");
        state.record_whole_scope_fallback("unregistered:a");
        state.record_whole_scope_fallback("manifests/media");
        let scheduler = std::sync::Arc::new(PostFetchScheduler::new(2));
        let _lease = scheduler.enqueue(
            crate::scheduler::PostFetchJobKey {
                scope_kind: "public_topic".into(),
                scope_id: "rust".into(),
                object_id: "post-1".into(),
            },
            "revision-1".into(),
        );
        state.set_post_scheduler(scheduler);

        let snapshot = state.snapshot();
        assert!(snapshot.worker_running);
        assert!(snapshot.ingest_enabled);
        assert_eq!(snapshot.opened_scopes, 2);
        assert_eq!(snapshot.last_sync_at, Some(100));
        assert_eq!(snapshot.last_ingest_at, Some(101));
        assert_eq!(snapshot.last_error.as_deref(), Some("boom"));
        assert_eq!(snapshot.last_error_scope.as_deref(), Some("topic::rust"));
        assert_eq!(snapshot.scanned, 3);
        assert_eq!(snapshot.indexed, 2);
        assert_eq!(snapshot.skipped_non_allow, 1);
        assert_eq!(snapshot.scan_errors, 1);
        assert_eq!(snapshot.provider_unavailable, 1);
        assert_eq!(snapshot.deindexed, 4);
        assert_eq!(snapshot.media_fetch_success, 1);
        assert_eq!(snapshot.media_fetch_unavailable, 1);
        assert_eq!(snapshot.media_fetch_timeout, 1);
        assert_eq!(snapshot.media_fetch_oversize, 1);
        assert_eq!(snapshot.scans_fresh, 4);
        assert_eq!(snapshot.scans_reused, 5);
        assert_eq!(snapshot.last_pass_duration_ms, Some(1200));
        assert_eq!(snapshot.last_event_ingest_duration_ms, Some(80));
        assert_eq!(
            snapshot.last_index_lag_secs,
            Some(0),
            "negative lag is clamped"
        );
        assert_eq!(snapshot.event_whole_scope_fallbacks, 2);
        assert_eq!(
            snapshot.last_whole_scope_fallback_reason.as_deref(),
            Some("manifests/media")
        );
        assert_eq!(snapshot.post_scheduler.queued, 1);
    }

    #[test]
    fn snapshot_deserializes_from_json_without_new_fields() {
        // #1050 以前の indexer が返す JSON（新フィールド無し）も読める（readiness 側の互換）。
        let legacy = serde_json::json!({
            "worker_running": true,
            "ingest_enabled": true,
            "opened_scopes": 3,
            "last_sync_at": 1,
            "last_ingest_at": 2,
            "last_error": null,
            "last_error_scope": null,
            "scanned": 10,
            "indexed": 8,
            "skipped_non_allow": 2,
            "scan_errors": 0,
            "provider_unavailable": 0,
            "deindexed": 0,
            "media_fetch_success": 1,
            "media_fetch_unavailable": 0,
            "media_fetch_timeout": 0,
            "media_fetch_oversize": 0
        });
        let snapshot: IndexerStateSnapshot = serde_json::from_value(legacy).expect("legacy json");
        assert_eq!(snapshot.opened_scopes, 3);
        assert_eq!(snapshot.scans_fresh, 0);
        assert_eq!(snapshot.scans_reused, 0);
        assert_eq!(snapshot.last_pass_duration_ms, None);
        assert_eq!(snapshot.last_index_lag_secs, None);
        assert_eq!(snapshot.event_whole_scope_fallbacks, 0);
        assert_eq!(snapshot.last_whole_scope_fallback_reason, None);
        assert_eq!(
            snapshot.post_scheduler,
            PostFetchSchedulerSnapshot::default()
        );
    }

    #[test]
    fn snapshot_serializes_to_json_with_stable_field_names() {
        let state = IndexerRuntimeState::default();
        let json = serde_json::to_value(state.snapshot()).expect("serialize");
        // 起動完了判定（#612）が機械的に読む代表フィールド名を固定する。
        assert!(json.get("worker_running").is_some());
        assert!(json.get("ingest_enabled").is_some());
        assert!(json.get("opened_scopes").is_some());
        assert!(json.get("last_sync_at").is_some());
        assert!(json.get("provider_unavailable").is_some());
        assert!(json.get("media_fetch_timeout").is_some());
        assert!(json.get("event_whole_scope_fallbacks").is_some());
        assert!(json.get("last_whole_scope_fallback_reason").is_some());
        assert!(json.get("post_scheduler").is_some());
    }
}
