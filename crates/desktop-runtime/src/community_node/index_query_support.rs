use std::fmt;

use chrono::Utc;
use kukuri_cn_protocol::{
    AUTH_REQUIRED_CODE, AdvisorySubjectKind, ApiErrorBody, CHANNEL_MEMBERSHIP_SECRET_HEADER,
    CONSENT_REQUIRED_CODE, INDEX_DISCOVERY_PATH, INDEX_RECOMMENDATIONS_PATH, INDEX_SEARCH_PATH,
    IndexQueryParams, IndexQueryResponse, IndexScopeKind, normalize_http_url,
};
use kukuri_store::ContentObservationRow;
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{CommunityNodeSessionOutcome, community_node_http_client, load_community_node_token};
use crate::runtime::DesktopRuntime;

/// #1055 / ADR 0046 §6.4: content advisory の成人向けゲートへの合成。利用規約 第3条 4 項の改訂と
/// 再同意(C4 = #1056)と同じ変更で有効化した。node 単位の採用は `content_advisory_enabled` で決める。
pub(crate) const CONTENT_ADVISORY_SYNTHESIS_DEFAULT: bool = true;

/// #1055: client が成人向けゲートの対象として扱う advisory の表示ラベル(ADR 0028 §8.6)。
/// 未知ラベルは無視する(前方互換。node が将来増やしても勝手にゲートしない)。
pub(crate) const GATING_ADVISORY_LABELS: [&str; 2] = ["adult", "sensitive"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeIndexQueryRequest {
    pub base_url: String,
    pub query: Option<String>,
    pub scope_kind: Option<IndexScopeKind>,
    pub scope_id: Option<String>,
    /// scope_kind が private_channel のとき必須。所属証明(channel secret)を参加中
    /// チャンネルの capability から引くために使う(#711)。
    pub topic_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeIndexQueryError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
    pub retry_after_seconds: Option<u64>,
}

impl CommunityNodeIndexQueryError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            status: None,
            retry_after_seconds: None,
        }
    }

    fn from_response(
        status: StatusCode,
        retry_after_seconds: Option<u64>,
        body: Option<ApiErrorBody>,
    ) -> Self {
        let fallback_code = match status {
            StatusCode::UNAUTHORIZED => AUTH_REQUIRED_CODE,
            StatusCode::FORBIDDEN => CONSENT_REQUIRED_CODE,
            StatusCode::TOO_MANY_REQUESTS => "RATE_LIMITED",
            _ => "INDEX_QUERY_FAILED",
        };
        let fallback_message = format!("community node index request failed with {status}");
        Self {
            code: body
                .as_ref()
                .map_or_else(|| fallback_code.to_string(), |body| body.code.clone()),
            message: body.map_or(fallback_message, |body| body.message),
            status: Some(status.as_u16()),
            retry_after_seconds,
        }
    }
}

impl fmt::Display for CommunityNodeIndexQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommunityNodeIndexQueryError {}

#[derive(Clone, Copy)]
pub(crate) enum IndexOperation {
    Search,
    Discovery,
    Recommendations,
}

impl IndexOperation {
    fn path(self) -> &'static str {
        match self {
            Self::Search => INDEX_SEARCH_PATH,
            Self::Discovery => INDEX_DISCOVERY_PATH,
            Self::Recommendations => INDEX_RECOMMENDATIONS_PATH,
        }
    }

    fn observation_capability(self) -> &'static str {
        match self {
            Self::Search | Self::Discovery => "community_index",
            Self::Recommendations => "recommendation",
        }
    }
}

impl DesktopRuntime {
    pub(crate) async fn query_community_node_index(
        &self,
        operation: IndexOperation,
        request: CommunityNodeIndexQueryRequest,
    ) -> Result<IndexQueryResponse, CommunityNodeIndexQueryError> {
        let base_url = normalize_http_url(request.base_url.as_str()).map_err(|error| {
            CommunityNodeIndexQueryError::new("INVALID_COMMUNITY_NODE_URL", error.to_string())
        })?;
        self.require_community_node(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeIndexQueryError::new(
                    "COMMUNITY_NODE_NOT_CONFIGURED",
                    error.to_string(),
                )
            })?;

        let has_scope_kind = request.scope_kind.is_some();
        let has_scope_id = request
            .scope_id
            .as_deref()
            .is_some_and(|scope_id| !scope_id.trim().is_empty());
        if has_scope_kind != has_scope_id {
            return Err(CommunityNodeIndexQueryError::new(
                "INVALID_INDEX_QUERY",
                "scope_kind and scope_id must be specified together",
            ));
        }
        if matches!(operation, IndexOperation::Search)
            && request
                .query
                .as_deref()
                .is_none_or(|query| query.trim().is_empty())
        {
            return Err(CommunityNodeIndexQueryError::new(
                "INVALID_INDEX_QUERY",
                "search query must not be empty",
            ));
        }

        let session_outcome = self
            .ensure_community_node_session(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeIndexQueryError::new(
                    "COMMUNITY_NODE_SESSION_FAILED",
                    error.to_string(),
                )
            })?;
        match session_outcome {
            CommunityNodeSessionOutcome::Ready => {}
            CommunityNodeSessionOutcome::ConsentRequired => {
                return Err(CommunityNodeIndexQueryError::new(
                    CONSENT_REQUIRED_CODE,
                    "community node required policies must be accepted before index queries",
                ));
            }
            CommunityNodeSessionOutcome::Deferred(phase) => {
                return Err(CommunityNodeIndexQueryError::new(
                    "COMMUNITY_NODE_SESSION_DEFERRED",
                    format!("community node session is not ready ({phase:?})"),
                ));
            }
        }
        // 非公開チャンネルの範囲指定は所属証明(channel secret)を同伴する(#711)。
        // 参加中でない(capability が無い)チャンネルへは送信前に拒否する。
        // 秘密値の組み立ては同意確認の後に行う(#698 と同じ順序)。
        let channel_secret = if request.scope_kind == Some(IndexScopeKind::PrivateChannel) {
            let channel_id = request
                .scope_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            let topic_id = request
                .topic_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    CommunityNodeIndexQueryError::new(
                        "INVALID_INDEX_QUERY",
                        "topic_id is required for private channel scoped queries",
                    )
                })?;
            let secret = self
                .app_service
                .private_channel_indexing_secret(topic_id, channel_id)
                .await
                .map_err(|error| {
                    CommunityNodeIndexQueryError::new(
                        "PRIVATE_CHANNEL_CAPABILITY_UNAVAILABLE",
                        error.to_string(),
                    )
                })?;
            Some(secret)
        } else {
            None
        };
        let token = load_community_node_token(&self.db_path, self.identity_mode, base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeIndexQueryError::new("AUTH_TOKEN_LOAD_FAILED", error.to_string())
            })?
            .ok_or_else(|| {
                CommunityNodeIndexQueryError::new(
                    AUTH_REQUIRED_CODE,
                    "community node authentication is required",
                )
            })?;
        let params = IndexQueryParams {
            q: request.query,
            scope_kind: request
                .scope_kind
                .map(|scope_kind| scope_kind.as_str().to_string()),
            scope_id: request.scope_id,
            limit: request.limit,
        };

        let response = match self
            .send_community_node_index_query(
                base_url.as_str(),
                operation,
                &params,
                token.access_token.as_str(),
                channel_secret.as_deref(),
            )
            .await
        {
            Err(error) if error.status == Some(StatusCode::UNAUTHORIZED.as_u16()) => {
                let refreshed = self
                    .request_community_node_authentication_token(base_url.as_str())
                    .await
                    .map_err(|error| {
                        CommunityNodeIndexQueryError::new(
                            "COMMUNITY_NODE_REAUTHENTICATION_FAILED",
                            error.to_string(),
                        )
                    })?;
                self.send_community_node_index_query(
                    base_url.as_str(),
                    operation,
                    &params,
                    refreshed.access_token.as_str(),
                    channel_secret.as_deref(),
                )
                .await
            }
            result => result,
        }?;
        let mut response = response;
        self.apply_content_advisories(base_url.as_str(), &mut response)
            .await;
        self.record_index_observations(base_url.as_str(), operation, &response)
            .await?;
        Ok(response)
    }

    /// #1055 / ADR 0046 §6.4: content advisory の成人向けゲートへの合成を切り替える。
    /// 本番は `CONTENT_ADVISORY_SYNTHESIS_DEFAULT`(#1056 で有効化)に従い、実行時の setter は
    /// 無効時の挙動を検証する test だけが使う。
    #[cfg(test)]
    pub(crate) fn set_content_advisory_synthesis_enabled(&self, enabled: bool) {
        self.content_advisory_synthesis_enabled
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    /// #1055 / ADR 0046 §6: index 応答に同梱された content advisory を、client が採用できる形へ
    /// 揃える単一の choke point。
    ///
    /// - 合成が無効、または index を返した node の採用が OFF(#1056)なら、すべての advisory を
    ///   落として取得ゲートにも登録しない。
    /// - 有効なら、index を返した設定済み node の manifest `node_id` と `issuer_node_id` が一致する
    ///   advisory だけを残す(AC-4)。manifest を取得できない場合も採用しない(fail-closed)。
    /// - 残った advisory のうち blob 対象のものを、`blob_media_payload` の取得ゲートへ登録する。
    ///
    /// advisory は投稿の canonical でも署名対象でもないため、`content_labels` へは書き戻さない。
    async fn apply_content_advisories(&self, base_url: &str, response: &mut IndexQueryResponse) {
        let synthesis_enabled = self
            .content_advisory_synthesis_enabled
            .load(std::sync::atomic::Ordering::SeqCst);
        // #1056: 利用者が採用を OFF にした node の advisory は採用しない。
        let node_adopted = self
            .community_node_config
            .lock()
            .await
            .nodes
            .iter()
            .any(|node| node.base_url == base_url && node.content_advisory_enabled);
        if !synthesis_enabled || !node_adopted {
            for entry in &mut response.entries {
                entry.content_advisories.clear();
            }
            return;
        }
        if response
            .entries
            .iter()
            .all(|entry| entry.content_advisories.is_empty())
        {
            return;
        }

        // advisory を含む応答のときだけ manifest を引く。issuer を確認できなければ採用しない。
        let issuer_node_id = match self.request_community_node_manifest(base_url).await {
            Ok(fetch) => fetch
                .manifest
                .map(|manifest| manifest.node_id.trim().to_string())
                .filter(|node_id| !node_id.is_empty()),
            Err(error) => {
                warn!(
                    base_url = %base_url,
                    error = %error,
                    "content advisories dropped because the issuing node manifest was unavailable"
                );
                None
            }
        };
        let Some(issuer_node_id) = issuer_node_id else {
            for entry in &mut response.entries {
                entry.content_advisories.clear();
            }
            return;
        };

        let mut gated_blob_hashes = Vec::new();
        for entry in &mut response.entries {
            entry
                .content_advisories
                .retain(|advisory| advisory.issuer_node_id.trim() == issuer_node_id);
            for advisory in &entry.content_advisories {
                if !GATING_ADVISORY_LABELS.contains(&advisory.label.trim()) {
                    continue;
                }
                if advisory.subject_kind != AdvisorySubjectKind::BlobCid {
                    continue;
                }
                let subject_id = advisory.subject_id.trim();
                if subject_id.is_empty() {
                    continue;
                }
                gated_blob_hashes.push(subject_id.to_string());
            }
        }
        self.app_service
            .register_advisory_media_hashes(&gated_blob_hashes)
            .await;
    }

    async fn record_index_observations(
        &self,
        base_url: &str,
        operation: IndexOperation,
        response: &IndexQueryResponse,
    ) -> Result<(), CommunityNodeIndexQueryError> {
        let observed_at = Utc::now().timestamp_millis();
        for entry in &response.entries {
            let observations = [
                ("post", entry.object_id.as_str()),
                ("profile", entry.author_pubkey.as_str()),
            ];
            for (subject_kind, subject_id) in observations {
                self.store
                    .put_content_observation(ContentObservationRow {
                        subject_kind: subject_kind.to_string(),
                        subject_id: subject_id.to_string(),
                        node_base_url: base_url.to_string(),
                        capability: operation.observation_capability().to_string(),
                        observed_at,
                    })
                    .await
                    .map_err(|error| {
                        CommunityNodeIndexQueryError::new(
                            "INDEX_OBSERVATION_STORE_FAILED",
                            error.to_string(),
                        )
                    })?;
            }
        }
        Ok(())
    }

    async fn send_community_node_index_query(
        &self,
        base_url: &str,
        operation: IndexOperation,
        params: &IndexQueryParams,
        access_token: &str,
        channel_secret: Option<&str>,
    ) -> Result<IndexQueryResponse, CommunityNodeIndexQueryError> {
        let client = community_node_http_client().map_err(|error| {
            CommunityNodeIndexQueryError::new("INDEX_HTTP_CLIENT_FAILED", error.to_string())
        })?;
        let mut request_builder = client
            .get(format!("{base_url}{}", operation.path()))
            .bearer_auth(access_token)
            .query(params);
        // 所属証明は URL クエリでなくヘッダで送る(アクセスログへの露出を避ける。#711)。
        if let Some(channel_secret) = channel_secret {
            request_builder =
                request_builder.header(CHANNEL_MEMBERSHIP_SECRET_HEADER, channel_secret);
        }
        let response = request_builder.send().await.map_err(|error| {
            CommunityNodeIndexQueryError::new("INDEX_QUERY_TRANSPORT_FAILED", error.to_string())
        })?;
        let status = response.status();
        let retry_after_seconds = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        if !status.is_success() {
            let body = response.json::<ApiErrorBody>().await.ok();
            return Err(CommunityNodeIndexQueryError::from_response(
                status,
                retry_after_seconds,
                body,
            ));
        }
        response
            .json::<IndexQueryResponse>()
            .await
            .map_err(|error| {
                CommunityNodeIndexQueryError::new("INDEX_QUERY_DECODE_FAILED", error.to_string())
            })
    }
}
