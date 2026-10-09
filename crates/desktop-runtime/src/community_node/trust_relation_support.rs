use std::fmt;

use kukuri_cn_protocol::{
    AUTH_REQUIRED_CODE, ApiErrorBody, CONSENT_REQUIRED_CODE, RELATION_NEIGHBORS_PATH,
    RELATION_OPTOUT_PATH, RELATION_USERS_PATH_PREFIX, RelationNeighborsResponse,
    RelationOptoutResponse, RelationReadResponse, TRUST_USERS_PATH_PREFIX, TrustUserReadResponse,
    normalize_http_url, normalize_pubkey,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{CommunityNodeSessionOutcome, community_node_http_client, load_community_node_token};
use crate::runtime::DesktopRuntime;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeUserAdvisoryRequest {
    pub base_url: String,
    pub target_pubkey: String,
    /// trust の basis の続き（前の応答の `basis_next_cursor`、#1702）。relation の照会は使わない。
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeRelationNeighborsRequest {
    pub base_url: String,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeTrustRelationError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
}

impl CommunityNodeTrustRelationError {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
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
            StatusCode::NOT_FOUND => "TRUST_RELATION_UNAVAILABLE",
            _ => "TRUST_RELATION_REQUEST_FAILED",
        };
        let fallback_message =
            format!("community node trust / relation request failed with {status}");
        Self {
            code: body
                .as_ref()
                .map_or_else(|| fallback_code.to_string(), |body| body.code.clone()),
            message: body.map_or(fallback_message, |body| body.message),
            status: Some(status.as_u16()),
        }
    }
}

impl fmt::Display for CommunityNodeTrustRelationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommunityNodeTrustRelationError {}

/// 応答本文の対象識別子が要求した利用者(正規化済み)と一致することを確認する(#699)。
///
/// 規約に従わないノードが別の利用者の評価・関係を返した場合に、その内容を表示したり
/// 別の判定を異議申し立て対象にしたりしないための実行時層の照合。比較は `normalize_pubkey`
/// と同じ正規化(前後空白除去・小文字化)で行う。
pub(crate) fn ensure_trust_relation_target(
    requested_normalized: &str,
    response_target: &str,
) -> Result<(), CommunityNodeTrustRelationError> {
    let response_target = response_target.trim().to_ascii_lowercase();
    if response_target == requested_normalized {
        return Ok(());
    }
    Err(CommunityNodeTrustRelationError::new(
        "TRUST_RELATION_RESPONSE_MISMATCH",
        format!(
            "community node returned a response for `{response_target}` while `{requested_normalized}` was requested"
        ),
    ))
}

impl DesktopRuntime {
    pub(crate) async fn request_community_node_trust_user(
        &self,
        request: CommunityNodeUserAdvisoryRequest,
    ) -> Result<TrustUserReadResponse, CommunityNodeTrustRelationError> {
        let target = normalize_pubkey(request.target_pubkey.as_str()).map_err(|error| {
            CommunityNodeTrustRelationError::new("INVALID_TRUST_QUERY", error.to_string())
        })?;
        let response: TrustUserReadResponse = self
            .request_community_node_trust_relation(
                request.base_url.as_str(),
                Method::GET,
                format!("{TRUST_USERS_PATH_PREFIX}{target}").as_str(),
                request.cursor.map(|cursor| ("cursor", cursor)),
                None,
            )
            .await?;
        ensure_trust_relation_target(target.as_str(), response.view.target_id.as_str())?;
        Ok(response)
    }

    pub(crate) async fn request_community_node_relation_user(
        &self,
        request: CommunityNodeUserAdvisoryRequest,
    ) -> Result<RelationReadResponse, CommunityNodeTrustRelationError> {
        let target = normalize_pubkey(request.target_pubkey.as_str()).map_err(|error| {
            CommunityNodeTrustRelationError::new("INVALID_TRUST_QUERY", error.to_string())
        })?;
        let response: RelationReadResponse = self
            .request_community_node_trust_relation(
                request.base_url.as_str(),
                Method::GET,
                format!("{RELATION_USERS_PATH_PREFIX}{target}").as_str(),
                None,
                None,
            )
            .await?;
        ensure_trust_relation_target(target.as_str(), response.target_pubkey.as_str())?;
        Ok(response)
    }

    pub(crate) async fn request_community_node_relation_neighbors(
        &self,
        request: CommunityNodeRelationNeighborsRequest,
    ) -> Result<RelationNeighborsResponse, CommunityNodeTrustRelationError> {
        self.request_community_node_trust_relation(
            request.base_url.as_str(),
            Method::GET,
            RELATION_NEIGHBORS_PATH,
            request.limit.map(|limit| ("limit", limit.to_string())),
            None,
        )
        .await
    }

    pub(crate) async fn request_community_node_relation_optout(
        &self,
        base_url: &str,
        method: Method,
    ) -> Result<RelationOptoutResponse, CommunityNodeTrustRelationError> {
        self.request_community_node_trust_relation(
            base_url,
            method,
            RELATION_OPTOUT_PATH,
            None,
            None,
        )
        .await
    }

    /// 認証済み trust / relation 系 request の共通処理。session が Ready でなければ HTTP を送らず、
    /// 401 は 1 回だけ再認証して再送する。`query` は 1 つの query 引数（`limit`、#1702 の `cursor`）、
    /// `body` は JSON 本文（#1061 の観測提供・一括評価）。
    pub(crate) async fn request_community_node_trust_relation<T: DeserializeOwned>(
        &self,
        base_url: &str,
        method: Method,
        path: &str,
        query: Option<(&str, String)>,
        body: Option<&serde_json::Value>,
    ) -> Result<T, CommunityNodeTrustRelationError> {
        let base_url = normalize_http_url(base_url).map_err(|error| {
            CommunityNodeTrustRelationError::new("INVALID_COMMUNITY_NODE_URL", error.to_string())
        })?;
        self.require_community_node(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeTrustRelationError::new(
                    "COMMUNITY_NODE_NOT_CONFIGURED",
                    error.to_string(),
                )
            })?;
        let session_outcome = self
            .ensure_community_node_session(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeTrustRelationError::new(
                    "COMMUNITY_NODE_SESSION_FAILED",
                    error.to_string(),
                )
            })?;
        match session_outcome {
            CommunityNodeSessionOutcome::Ready => {}
            CommunityNodeSessionOutcome::ConsentRequired => {
                return Err(CommunityNodeTrustRelationError::new(
                    CONSENT_REQUIRED_CODE,
                    "community node required policies must be accepted before trust and relation requests",
                ));
            }
            CommunityNodeSessionOutcome::Deferred(phase) => {
                return Err(CommunityNodeTrustRelationError::new(
                    "COMMUNITY_NODE_SESSION_DEFERRED",
                    format!("community node session is not ready ({phase:?})"),
                ));
            }
        }
        let token = load_community_node_token(&self.db_path, self.identity_mode, base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeTrustRelationError::new("AUTH_TOKEN_LOAD_FAILED", error.to_string())
            })?
            .ok_or_else(|| {
                CommunityNodeTrustRelationError::new(
                    AUTH_REQUIRED_CODE,
                    "community node authentication is required",
                )
            })?;

        match self
            .send_community_node_trust_relation(
                base_url.as_str(),
                method.clone(),
                path,
                query.clone(),
                body,
                token.access_token.as_str(),
            )
            .await
        {
            Err(error) if error.status == Some(StatusCode::UNAUTHORIZED.as_u16()) => {
                let refreshed = self
                    .request_community_node_authentication_token(base_url.as_str())
                    .await
                    .map_err(|error| {
                        CommunityNodeTrustRelationError::new(
                            "COMMUNITY_NODE_REAUTHENTICATION_FAILED",
                            error.to_string(),
                        )
                    })?;
                self.send_community_node_trust_relation(
                    base_url.as_str(),
                    method,
                    path,
                    query,
                    body,
                    refreshed.access_token.as_str(),
                )
                .await
            }
            result => result,
        }
    }

    async fn send_community_node_trust_relation<T: DeserializeOwned>(
        &self,
        base_url: &str,
        method: Method,
        path: &str,
        query: Option<(&str, String)>,
        body: Option<&serde_json::Value>,
        access_token: &str,
    ) -> Result<T, CommunityNodeTrustRelationError> {
        let client = community_node_http_client().map_err(|error| {
            CommunityNodeTrustRelationError::new(
                "TRUST_RELATION_HTTP_CLIENT_FAILED",
                error.to_string(),
            )
        })?;
        let mut request = client
            .request(method, format!("{base_url}{path}"))
            .bearer_auth(access_token);
        if let Some(query) = query {
            request = request.query(&[query]);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|error| {
            CommunityNodeTrustRelationError::new(
                "TRUST_RELATION_TRANSPORT_FAILED",
                error.to_string(),
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.json::<ApiErrorBody>().await.ok();
            return Err(CommunityNodeTrustRelationError::from_response(status, body));
        }
        response.json::<T>().await.map_err(|error| {
            CommunityNodeTrustRelationError::new("TRUST_RELATION_DECODE_FAILED", error.to_string())
        })
    }
}
