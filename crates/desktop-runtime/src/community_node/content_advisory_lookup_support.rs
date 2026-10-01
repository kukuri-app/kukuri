//! #1056: タイムライン向け content advisory 一括照会の client(ADR 0046 §6.1 / §6.3)。
//!
//! 可視の post id / blob hash を、利用者が採用を選んだ(`content_advisory_enabled`)設定済み node へ
//! まとめて照会し、その node 自身が発行した advisory だけを採用する。
//!
//! 外部送信の門(HTTP sink より前に置く):
//! - 合成が無効、採用 OFF、未設定の node へは送らない。
//! - 同意未成立・session 未確立・token 無しの node へは送らない(先例: trust read)。
//! - manifest を取得できず発行元を確認できない node へは送らない(fail-closed)。
//!
//! 送信する識別子は post id と blob hash のみ(INVAR-1)。応答は永続化せず、blob 対象の advisory だけを
//! `blob_media_payload` の取得ゲート(プロセス内集合)へ登録する。

use std::collections::BTreeSet;
use std::fmt;

use kukuri_cn_protocol::{
    ADVISORY_LOOKUP_MAX_SUBJECTS, ADVISORY_LOOKUP_PATH, AUTH_REQUIRED_CODE, AdvisoryLookupRequest,
    AdvisoryLookupResponse, AdvisorySubjectKind, ApiErrorBody, Basis, CONSENT_REQUIRED_CODE,
    ContentAdvisory, is_advisory_blob_hash,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::index_query_support::GATING_ADVISORY_LABELS;
use super::{CommunityNodeSessionOutcome, community_node_http_client, load_community_node_token};
use crate::runtime::DesktopRuntime;

/// 1 回の呼出しで受け付ける subject の上限(frontend の分割漏れで無制限に送らないため)。
pub(crate) const CONTENT_ADVISORY_LOOKUP_MAX_SUBJECTS_PER_CALL: usize = 1_000;
const POST_ID_MAX_LEN: usize = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommunityNodeContentAdvisoryLookupRequest {
    /// 可視の投稿 id(引用元・返信先を含む)。
    #[serde(default)]
    pub post_ids: Vec<String>,
    /// 可視の添付 blob hash。
    #[serde(default)]
    pub blob_hashes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeContentAdvisoryLookupError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
}

impl CommunityNodeContentAdvisoryLookupError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            status: None,
        }
    }

    fn from_response(status: StatusCode, body: Option<ApiErrorBody>) -> Self {
        let fallback_code = match status {
            StatusCode::UNAUTHORIZED => AUTH_REQUIRED_CODE,
            StatusCode::FORBIDDEN => CONSENT_REQUIRED_CODE,
            StatusCode::NOT_FOUND => "CONTENT_ADVISORY_UNAVAILABLE",
            _ => "CONTENT_ADVISORY_LOOKUP_FAILED",
        };
        Self {
            code: body
                .as_ref()
                .map_or_else(|| fallback_code.to_string(), |body| body.code.clone()),
            message: body.map_or_else(
                || format!("community node advisory lookup failed with {status}"),
                |body| body.message,
            ),
            status: Some(status.as_u16()),
        }
    }
}

impl fmt::Display for CommunityNodeContentAdvisoryLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommunityNodeContentAdvisoryLookupError {}

/// node 1 つ分の照会結果。`error` があっても、それまでに得た advisory は含む。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeContentAdvisoryNodeResult {
    pub base_url: String,
    /// manifest で確認した発行元(= advisory の `issuer_node_id`)。
    pub node_id: Option<String>,
    pub advisories: Vec<ContentAdvisory>,
    pub error: Option<CommunityNodeContentAdvisoryLookupError>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommunityNodeContentAdvisoryLookupResult {
    /// 照会対象になった(採用 ON の)node ごとの結果。
    pub nodes: Vec<CommunityNodeContentAdvisoryNodeResult>,
}

/// 正規化済みの照会対象。
struct NormalizedSubjects {
    post_ids: Vec<String>,
    blob_hashes: Vec<String>,
}

impl NormalizedSubjects {
    fn is_empty(&self) -> bool {
        self.post_ids.is_empty() && self.blob_hashes.is_empty()
    }

    fn contains(&self, advisory: &ContentAdvisory) -> bool {
        match advisory.subject_kind {
            AdvisorySubjectKind::PostId => self
                .post_ids
                .binary_search(&advisory.subject_id.trim().to_string())
                .is_ok(),
            AdvisorySubjectKind::BlobCid => self
                .blob_hashes
                .binary_search(&advisory.subject_id.trim().to_ascii_lowercase())
                .is_ok(),
        }
    }

    /// server の 1 request 上限に合わせて分割する。
    fn chunks(&self) -> Vec<AdvisoryLookupRequest> {
        let mut requests = Vec::new();
        let mut current = AdvisoryLookupRequest::default();
        let subjects = self
            .post_ids
            .iter()
            .map(|id| (true, id))
            .chain(self.blob_hashes.iter().map(|hash| (false, hash)));
        for (is_post, value) in subjects {
            if current.subject_count() == ADVISORY_LOOKUP_MAX_SUBJECTS {
                requests.push(std::mem::take(&mut current));
            }
            if is_post {
                current.post_ids.push(value.clone());
            } else {
                current.blob_hashes.push(value.clone());
            }
        }
        if current.subject_count() > 0 {
            requests.push(current);
        }
        requests
    }
}

fn normalize_subjects(
    request: CommunityNodeContentAdvisoryLookupRequest,
) -> Result<NormalizedSubjects, CommunityNodeContentAdvisoryLookupError> {
    let post_ids: BTreeSet<String> = request
        .post_ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && id.len() <= POST_ID_MAX_LEN)
        .collect();
    let blob_hashes: BTreeSet<String> = request
        .blob_hashes
        .into_iter()
        .map(|hash| hash.trim().to_ascii_lowercase())
        .filter(|hash| is_advisory_blob_hash(hash.as_str()))
        .collect();
    if post_ids.len() + blob_hashes.len() > CONTENT_ADVISORY_LOOKUP_MAX_SUBJECTS_PER_CALL {
        return Err(CommunityNodeContentAdvisoryLookupError::new(
            "INVALID_ADVISORY_LOOKUP",
            format!(
                "at most {CONTENT_ADVISORY_LOOKUP_MAX_SUBJECTS_PER_CALL} subjects can be looked up at once"
            ),
        ));
    }
    Ok(NormalizedSubjects {
        post_ids: post_ids.into_iter().collect(),
        blob_hashes: blob_hashes.into_iter().collect(),
    })
}

/// client が採用する advisory か(表示語彙・根拠・発行元・要求範囲)。
fn is_adoptable(advisory: &ContentAdvisory, issuer: &str, subjects: &NormalizedSubjects) -> bool {
    advisory.issuer_node_id.trim() == issuer
        && GATING_ADVISORY_LABELS.contains(&advisory.label.trim())
        && advisory.basis == Basis::ClassifierScore
        && subjects.contains(advisory)
}

impl DesktopRuntime {
    /// 可視 subject の advisory を採用 ON の設定済み node へ一括照会する。
    pub(crate) async fn lookup_content_advisories(
        &self,
        request: CommunityNodeContentAdvisoryLookupRequest,
    ) -> Result<CommunityNodeContentAdvisoryLookupResult, CommunityNodeContentAdvisoryLookupError>
    {
        let subjects = normalize_subjects(request)?;
        if subjects.is_empty()
            || !self
                .content_advisory_synthesis_enabled
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(CommunityNodeContentAdvisoryLookupResult::default());
        }
        let enabled_nodes: Vec<String> = self
            .community_node_config
            .lock()
            .await
            .nodes
            .iter()
            .filter(|node| node.content_advisory_enabled)
            .map(|node| node.base_url.clone())
            .collect();
        let mut nodes = Vec::with_capacity(enabled_nodes.len());
        for base_url in enabled_nodes {
            let result = self
                .lookup_content_advisories_on_node(base_url.as_str(), &subjects)
                .await;
            if let Some(error) = result.error.as_ref() {
                warn!(
                    base_url = %base_url,
                    code = %error.code,
                    "content advisory lookup skipped or failed"
                );
            }
            let gated_blob_hashes: Vec<String> = result
                .advisories
                .iter()
                .filter(|advisory| advisory.subject_kind == AdvisorySubjectKind::BlobCid)
                .map(|advisory| advisory.subject_id.trim().to_ascii_lowercase())
                .collect();
            self.app_service
                .register_advisory_media_hashes(&gated_blob_hashes)
                .await;
            nodes.push(result);
        }
        Ok(CommunityNodeContentAdvisoryLookupResult { nodes })
    }

    async fn lookup_content_advisories_on_node(
        &self,
        base_url: &str,
        subjects: &NormalizedSubjects,
    ) -> CommunityNodeContentAdvisoryNodeResult {
        let mut result = CommunityNodeContentAdvisoryNodeResult {
            base_url: base_url.to_string(),
            node_id: None,
            advisories: Vec::new(),
            error: None,
        };
        let token = match self.prepare_content_advisory_lookup(base_url).await {
            Ok(token) => token,
            Err(error) => {
                result.error = Some(error);
                return result;
            }
        };
        // 発行元を確認できない node へは識別子を送らない(fail-closed)。
        let issuer = match self.content_advisory_issuer(base_url).await {
            Ok(issuer) => issuer,
            Err(error) => {
                result.error = Some(error);
                return result;
            }
        };
        result.node_id = Some(issuer.clone());
        let mut access_token = token;
        let mut reauthenticated = false;
        for chunk in subjects.chunks() {
            let response = match self
                .send_content_advisory_lookup(base_url, &chunk, access_token.as_str())
                .await
            {
                Err(error)
                    if error.status == Some(StatusCode::UNAUTHORIZED.as_u16())
                        && !reauthenticated =>
                {
                    reauthenticated = true;
                    match self
                        .request_community_node_authentication_token(base_url)
                        .await
                    {
                        Ok(refreshed) => {
                            access_token = refreshed.access_token;
                            self.send_content_advisory_lookup(
                                base_url,
                                &chunk,
                                access_token.as_str(),
                            )
                            .await
                        }
                        Err(error) => Err(CommunityNodeContentAdvisoryLookupError::new(
                            "COMMUNITY_NODE_REAUTHENTICATION_FAILED",
                            error.to_string(),
                        )),
                    }
                }
                other => other,
            };
            match response {
                Ok(response) => result.advisories.extend(
                    response
                        .advisories
                        .into_iter()
                        .filter(|advisory| is_adoptable(advisory, issuer.as_str(), subjects)),
                ),
                Err(error) => {
                    result.error = Some(error);
                    break;
                }
            }
        }
        result
    }

    /// 送信前の門: 設定済み・同意成立・session 確立・token 取得。
    async fn prepare_content_advisory_lookup(
        &self,
        base_url: &str,
    ) -> Result<String, CommunityNodeContentAdvisoryLookupError> {
        self.require_community_node(base_url)
            .await
            .map_err(|error| {
                CommunityNodeContentAdvisoryLookupError::new(
                    "COMMUNITY_NODE_NOT_CONFIGURED",
                    error.to_string(),
                )
            })?;
        match self
            .ensure_community_node_session(base_url)
            .await
            .map_err(|error| {
                CommunityNodeContentAdvisoryLookupError::new(
                    "COMMUNITY_NODE_SESSION_FAILED",
                    error.to_string(),
                )
            })? {
            CommunityNodeSessionOutcome::Ready => {}
            CommunityNodeSessionOutcome::ConsentRequired => {
                return Err(CommunityNodeContentAdvisoryLookupError::new(
                    CONSENT_REQUIRED_CODE,
                    "community node required policies must be accepted before advisory lookups",
                ));
            }
            CommunityNodeSessionOutcome::Deferred(phase) => {
                return Err(CommunityNodeContentAdvisoryLookupError::new(
                    "COMMUNITY_NODE_SESSION_DEFERRED",
                    format!("community node session is not ready ({phase:?})"),
                ));
            }
        }
        load_community_node_token(&self.db_path, self.identity_mode, base_url)
            .await
            .map_err(|error| {
                CommunityNodeContentAdvisoryLookupError::new(
                    "AUTH_TOKEN_LOAD_FAILED",
                    error.to_string(),
                )
            })?
            .map(|token| token.access_token)
            .ok_or_else(|| {
                CommunityNodeContentAdvisoryLookupError::new(
                    AUTH_REQUIRED_CODE,
                    "community node authentication is required",
                )
            })
    }

    /// manifest の `node_id`(発行元)を引く。node 設定の保存で破棄されるプロセス内 cache を使う。
    async fn content_advisory_issuer(
        &self,
        base_url: &str,
    ) -> Result<String, CommunityNodeContentAdvisoryLookupError> {
        if let Some(issuer) = self
            .content_advisory_issuer_cache
            .lock()
            .await
            .get(base_url)
            .cloned()
        {
            return Ok(issuer);
        }
        let fetch = self
            .request_community_node_manifest(base_url)
            .await
            .map_err(|error| {
                CommunityNodeContentAdvisoryLookupError::new(
                    "COMMUNITY_NODE_MANIFEST_UNAVAILABLE",
                    error.to_string(),
                )
            })?;
        let issuer = fetch
            .manifest
            .map(|manifest| manifest.node_id.trim().to_string())
            .filter(|node_id| !node_id.is_empty())
            .ok_or_else(|| {
                CommunityNodeContentAdvisoryLookupError::new(
                    "COMMUNITY_NODE_MANIFEST_UNAVAILABLE",
                    "community node does not publish an issuer identity",
                )
            })?;
        self.content_advisory_issuer_cache
            .lock()
            .await
            .insert(base_url.to_string(), issuer.clone());
        Ok(issuer)
    }

    async fn send_content_advisory_lookup(
        &self,
        base_url: &str,
        request: &AdvisoryLookupRequest,
        access_token: &str,
    ) -> Result<AdvisoryLookupResponse, CommunityNodeContentAdvisoryLookupError> {
        let client = community_node_http_client().map_err(|error| {
            CommunityNodeContentAdvisoryLookupError::new(
                "CONTENT_ADVISORY_HTTP_CLIENT_FAILED",
                error.to_string(),
            )
        })?;
        let response = client
            .post(format!("{base_url}{ADVISORY_LOOKUP_PATH}"))
            .bearer_auth(access_token)
            .json(request)
            .send()
            .await
            .map_err(|error| {
                CommunityNodeContentAdvisoryLookupError::new(
                    "CONTENT_ADVISORY_TRANSPORT_FAILED",
                    error.to_string(),
                )
            })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.json::<ApiErrorBody>().await.ok();
            return Err(CommunityNodeContentAdvisoryLookupError::from_response(
                status, body,
            ));
        }
        response
            .json::<AdvisoryLookupResponse>()
            .await
            .map_err(|error| {
                CommunityNodeContentAdvisoryLookupError::new(
                    "CONTENT_ADVISORY_DECODE_FAILED",
                    error.to_string(),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_are_trimmed_deduplicated_and_validated() {
        let subjects = normalize_subjects(CommunityNodeContentAdvisoryLookupRequest {
            post_ids: vec![" post-1 ".into(), "post-1".into(), " ".into()],
            blob_hashes: vec!["A".repeat(64), "a".repeat(64), "not-a-hash".into()],
        })
        .expect("normalize");
        assert_eq!(subjects.post_ids, vec!["post-1".to_string()]);
        assert_eq!(subjects.blob_hashes, vec!["a".repeat(64)]);
    }

    #[test]
    fn subjects_are_split_by_server_limit() {
        let subjects = normalize_subjects(CommunityNodeContentAdvisoryLookupRequest {
            post_ids: (0..450).map(|index| format!("post-{index:03}")).collect(),
            blob_hashes: Vec::new(),
        })
        .expect("normalize");
        let chunks = subjects.chunks();
        assert_eq!(
            chunks
                .iter()
                .map(AdvisoryLookupRequest::subject_count)
                .collect::<Vec<_>>(),
            vec![200, 200, 50]
        );
    }

    #[test]
    fn oversized_calls_are_rejected_before_any_request() {
        let error = normalize_subjects(CommunityNodeContentAdvisoryLookupRequest {
            post_ids: (0..=CONTENT_ADVISORY_LOOKUP_MAX_SUBJECTS_PER_CALL)
                .map(|index| format!("post-{index}"))
                .collect(),
            blob_hashes: Vec::new(),
        })
        .err()
        .expect("oversized");
        assert_eq!(error.code, "INVALID_ADVISORY_LOOKUP");
    }
}
