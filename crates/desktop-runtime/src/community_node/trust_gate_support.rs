//! 採用した CN の信頼値による著者の表示判断（ADR 0026 §8.4、#1061）。
//!
//! - 利用者が優先順に選んだ CN だけへ、表示中の投稿の著者 pubkey を一括照会する（`POST /v1/trust/evaluations`）。
//!   未選択の CN へは送らない。優先順位が空なら、この機能による非表示は行わない。
//! - 上位から順に、対象・閲覧者・期限・版を照合して最初に有効だった評価を採用する。失敗・401・期限切れは
//!   次の選択済み CN へ進み、どれも採れなければ「未評価」（非表示にしない）とする。
//! - 判断は表示だけを変える。mute / block canonical も観測も作らない。
//! - 著者ごとの「常に表示する」例外は端末内（`<db>.trust-display.json`）に保存する。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use kukuri_cn_protocol::{
    TRUST_EVALUATIONS_MAX_TARGETS, TRUST_EVALUATIONS_PATH, TrustEvaluation, TrustEvaluationReason,
    TrustEvaluationsResponse, normalize_http_url, normalize_pubkey,
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{CommunityNodeSessionOutcome, CommunityNodeTrustRelationError};
use crate::runtime::DesktopRuntime;

pub(crate) const TRUST_DISPLAY_STATE_FILE_EXTENSION: &str = "trust-display.json";

/// 著者 1 人分の表示判断。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct AuthorTrustGate {
    pub author_pubkey: String,
    /// この機能で投稿を折りたたむか。未評価・例外設定では false。
    pub hidden: bool,
    /// 判断に使った CN。未評価なら None。
    pub node_base_url: Option<String>,
    /// 評価が下がった理由の種類（CN が返す種類のみ。observer も件数も含まない）。
    #[serde(default)]
    pub reasons: Vec<TrustEvaluationReason>,
    /// 採用した評価の期限（RFC3339）。
    pub expires_at: Option<String>,
    /// 利用者が「常に表示する」を設定している著者か。
    pub always_visible: bool,
}

impl AuthorTrustGate {
    fn unevaluated(author_pubkey: String, always_visible: bool) -> Self {
        Self {
            author_pubkey,
            hidden: false,
            node_base_url: None,
            reasons: Vec::new(),
            expires_at: None,
            always_visible,
        }
    }
}

/// 表示判断の一括照会。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AuthorTrustGateRequest {
    pub author_pubkeys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AuthorTrustGateResult {
    pub gates: Vec<AuthorTrustGate>,
}

/// 著者ごとの表示例外（この端末だけの設定）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SetAuthorTrustDisplayExceptionRequest {
    pub author_pubkey: String,
    /// true にすると、信頼値による折りたたみをこの著者には適用しない。
    pub always_visible: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct TrustDisplayState {
    /// 常に表示する著者 pubkey。
    #[serde(default)]
    always_visible: BTreeSet<String>,
}

/// CN から採った評価の cache 行。
#[derive(Clone, Debug)]
pub(crate) struct CachedAuthorTrustEvaluation {
    pub(crate) viewer_pubkey: String,
    pub(crate) trust: f64,
    pub(crate) evaluation: TrustEvaluation,
    /// 設定・同意・認証の変更で進める世代。古い世代の応答は使わない。
    pub(crate) generation: u64,
}

fn state_path(db_path: &Path) -> PathBuf {
    db_path.with_extension(TRUST_DISPLAY_STATE_FILE_EXTENSION)
}

async fn load_state(db_path: &Path) -> Result<TrustDisplayState> {
    let path = state_path(db_path);
    let Some(raw) = crate::storage::read_file(&path)
        .await
        .with_context(|| format!("failed to read trust display state `{}`", path.display()))?
    else {
        return Ok(TrustDisplayState::default());
    };
    serde_json::from_slice(&raw)
        .with_context(|| format!("failed to parse trust display state `{}`", path.display()))
}

async fn save_state(db_path: &Path, state: &TrustDisplayState) -> Result<()> {
    let path = state_path(db_path);
    let json = serde_json::to_vec_pretty(state).context("failed to encode trust display state")?;
    crate::storage::write_file(&path, &json)
        .await
        .with_context(|| format!("failed to replace trust display state `{}`", path.display()))
}

fn evaluation_is_fresh(evaluation: &TrustEvaluation, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(evaluation.expires_at.as_str())
        .map(|expires_at| expires_at.with_timezone(&Utc) > now)
        .unwrap_or(false)
}

impl DesktopRuntime {
    /// 設定・同意・認証が変わったら世代を進め、保持している評価を捨てる。
    pub(crate) async fn invalidate_author_trust_gate_cache(&self) {
        self.author_trust_gate_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.author_trust_gate_cache.lock().await.clear();
    }

    /// 表示中の著者について、採用 CN の信頼値による折りたたみ判断を返す。
    pub async fn evaluate_author_trust_gates(
        &self,
        request: AuthorTrustGateRequest,
    ) -> Result<AuthorTrustGateResult> {
        let mut targets = Vec::new();
        for pubkey in &request.author_pubkeys {
            match normalize_pubkey(pubkey.as_str()) {
                Ok(pubkey) => targets.push(pubkey),
                Err(error) => {
                    warn!(error = %error, "skipped an invalid author pubkey in a trust gate lookup");
                }
            }
        }
        targets.sort();
        targets.dedup();
        let always_visible = load_state(&self.db_path).await?.always_visible;
        if targets.is_empty() {
            return Ok(AuthorTrustGateResult { gates: Vec::new() });
        }

        // 利用者が優先順に選んだ CN だけを使う。空なら評価しない。
        let config = self.community_node_config.lock().await.clone();
        let selected: Vec<String> = config
            .trust_node_priority
            .iter()
            .filter(|base_url| {
                config
                    .nodes
                    .iter()
                    .any(|node| node.base_url.as_str() == base_url.as_str())
            })
            .cloned()
            .collect();

        let viewer = self.author_keys.public_key_hex();
        let now = Utc::now();
        let mut resolved: BTreeMap<String, (String, f64, TrustEvaluation)> = BTreeMap::new();
        let mut pending: Vec<String> = targets
            .iter()
            .filter(|target| !always_visible.contains(target.as_str()))
            .cloned()
            .collect();
        for base_url in selected {
            if pending.is_empty() {
                break;
            }
            let generation = self
                .author_trust_gate_generation
                .load(std::sync::atomic::Ordering::SeqCst);
            // cache から採れるものを先に埋める。
            {
                let cache = self.author_trust_gate_cache.lock().await;
                pending.retain(|target| {
                    let Some(cached) = cache.get(&(base_url.clone(), target.clone())) else {
                        return true;
                    };
                    if cached.generation != generation
                        || cached.viewer_pubkey != viewer
                        || !evaluation_is_fresh(&cached.evaluation, now)
                    {
                        return true;
                    }
                    resolved.insert(
                        target.clone(),
                        (base_url.clone(), cached.trust, cached.evaluation.clone()),
                    );
                    false
                });
            }
            if pending.is_empty() {
                break;
            }
            let mut requested = pending.clone();
            // CN 側の一括評価の上限。超えた分はこのノードでは評価せず、未評価として扱う
            // (desktop は AUTHOR_TRUST_GATE_LOOKUP_BATCH_SIZE で分割して送る)。
            requested.truncate(TRUST_EVALUATIONS_MAX_TARGETS);
            let evaluations = match self
                .request_author_trust_evaluations(base_url.as_str(), &requested)
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    // 失敗・401・同意待ちは低信頼へ変換せず、次の選択済み CN へ進む。
                    warn!(base_url = %base_url, error = %error, "failed to read trust evaluations; falling back to the next selected node");
                    continue;
                }
            };
            if evaluations.viewer_pubkey.trim().to_ascii_lowercase() != viewer {
                warn!(base_url = %base_url, "community node returned evaluations for another viewer");
                continue;
            }
            let mut cache = self.author_trust_gate_cache.lock().await;
            for item in evaluations.evaluations {
                let target = item.target_pubkey.trim().to_ascii_lowercase();
                if !requested.contains(&target) {
                    warn!(base_url = %base_url, "community node returned an evaluation for an unrequested author");
                    continue;
                }
                if !evaluation_is_fresh(&item.evaluation, now) {
                    continue;
                }
                cache.insert(
                    (base_url.clone(), target.clone()),
                    CachedAuthorTrustEvaluation {
                        viewer_pubkey: viewer.clone(),
                        trust: item.trust,
                        evaluation: item.evaluation.clone(),
                        generation,
                    },
                );
                resolved.insert(
                    target.clone(),
                    (base_url.clone(), item.trust, item.evaluation),
                );
                pending.retain(|value| value != &target);
            }
        }

        let gates = targets
            .into_iter()
            .map(|target| {
                let always_visible = always_visible.contains(target.as_str());
                match resolved.remove(&target) {
                    Some((base_url, _, evaluation)) => AuthorTrustGate {
                        hidden: evaluation.hide_recommended && !always_visible,
                        node_base_url: Some(base_url),
                        reasons: evaluation.reasons,
                        expires_at: Some(evaluation.expires_at),
                        always_visible,
                        author_pubkey: target,
                    },
                    None => AuthorTrustGate::unevaluated(target, always_visible),
                }
            })
            .collect();
        Ok(AuthorTrustGateResult { gates })
    }

    async fn request_author_trust_evaluations(
        &self,
        base_url: &str,
        targets: &[String],
    ) -> Result<TrustEvaluationsResponse, CommunityNodeTrustRelationError> {
        let session = self
            .ensure_community_node_session(base_url)
            .await
            .map_err(|error| {
                CommunityNodeTrustRelationError::new(
                    "COMMUNITY_NODE_SESSION_FAILED",
                    error.to_string(),
                )
            })?;
        if session != CommunityNodeSessionOutcome::Ready {
            return Err(CommunityNodeTrustRelationError::new(
                "COMMUNITY_NODE_SESSION_DEFERRED",
                "community node session is not ready for trust evaluations",
            ));
        }
        let body = serde_json::json!({ "targets": targets });
        self.request_community_node_trust_relation::<TrustEvaluationsResponse>(
            base_url,
            Method::POST,
            TRUST_EVALUATIONS_PATH,
            None,
            Some(&body),
        )
        .await
    }

    /// 著者ごとの「常に表示する」例外を設定・解除する。
    pub async fn set_author_trust_display_exception(
        &self,
        request: SetAuthorTrustDisplayExceptionRequest,
    ) -> Result<AuthorTrustGate> {
        let author_pubkey = normalize_pubkey(request.author_pubkey.as_str())?;
        {
            let _guard = self.trust_display_guard.lock().await;
            let mut state = load_state(&self.db_path).await?;
            if request.always_visible {
                state.always_visible.insert(author_pubkey.clone());
            } else {
                state.always_visible.remove(author_pubkey.as_str());
            }
            save_state(&self.db_path, &state).await?;
        }
        let mut result = self
            .evaluate_author_trust_gates(AuthorTrustGateRequest {
                author_pubkeys: vec![author_pubkey.clone()],
            })
            .await?;
        Ok(result
            .gates
            .pop()
            .unwrap_or_else(|| AuthorTrustGate::unevaluated(author_pubkey, request.always_visible)))
    }

    /// 「常に表示する」例外の一覧（管理導線用）。
    pub async fn list_author_trust_display_exceptions(&self) -> Result<Vec<String>> {
        let _guard = self.trust_display_guard.lock().await;
        Ok(load_state(&self.db_path)
            .await?
            .always_visible
            .into_iter()
            .collect())
    }
}

/// 設定の優先順位を、設定済み node のうち重複しないものへ正規化する。
///
/// 表示設定にすぎないため、読めない値は落とすだけにする。破損した state で runtime の
/// 起動を止めない(#1061 TR-4「有効な設定・根拠のみ復元」)。
pub(crate) fn normalize_trust_node_priority(priority: &[String], nodes: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for base_url in priority {
        let Ok(base_url) = normalize_http_url(base_url.as_str()) else {
            warn!(base_url = %base_url, "trust node priority entry is not a usable url");
            continue;
        };
        if !nodes.contains(&base_url) || !seen.insert(base_url.clone()) {
            continue;
        }
        normalized.push(base_url);
    }
    normalized
}
