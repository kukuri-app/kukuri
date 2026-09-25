use std::fmt;

use kukuri_cn_protocol::{
    AUTH_REQUIRED_CODE, ApiErrorBody, CONSENT_REQUIRED_CODE, INDEXING_REQUESTS_PATH,
    IndexScopeKind, RevokeIndexingRequestRequest, SubmitIndexingRequestRequest,
    SubmitIndexingRequestResponse, normalize_http_url,
};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize};

use super::{CommunityNodeSessionOutcome, community_node_http_client, load_community_node_token};
use crate::identity::{delete_optional_secret, load_optional_secret, persist_optional_secret};
use crate::runtime::DesktopRuntime;
use kukuri_store::PrivateIndexGrant;

const PRIVATE_INDEX_GRANT_SECRET_PURPOSE: &str = "community-node-private-index-grant";

fn grant_secret_key(grant: &PrivateIndexGrant) -> String {
    format!("{}|{}|{}", grant.base_url, grant.topic_id, grant.channel_id)
}

/// UI / IPC から受ける secret 非公開の indexing 申請。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeIndexingRequest {
    pub base_url: String,
    pub scope_kind: IndexScopeKind,
    /// public の target、private capability の親 topic。
    pub topic_id: String,
    /// private の target。public では指定しない。
    pub channel_id: Option<String>,
    /// private secret の外部送信を UI で明示確認したことを示す。
    #[serde(default)]
    pub confirm_private_channel_secret_disclosure: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeIndexingRequestError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
    pub retry_after_seconds: Option<u64>,
}

impl CommunityNodeIndexingRequestError {
    pub(super) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            status: None,
            retry_after_seconds: None,
        }
    }

    pub(super) fn from_response(
        status: StatusCode,
        retry_after_seconds: Option<u64>,
        body: Option<ApiErrorBody>,
    ) -> Self {
        let fallback_code = match status {
            StatusCode::UNAUTHORIZED => AUTH_REQUIRED_CODE,
            StatusCode::FORBIDDEN => CONSENT_REQUIRED_CODE,
            StatusCode::TOO_MANY_REQUESTS => "RATE_LIMITED",
            _ => "INDEXING_REQUEST_FAILED",
        };
        let fallback_message = format!("community node indexing request failed with {status}");
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

impl fmt::Display for CommunityNodeIndexingRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommunityNodeIndexingRequestError {}

impl DesktopRuntime {
    pub(crate) async fn revoke_community_node_indexing(
        &self,
        request: CommunityNodeIndexingRequest,
    ) -> Result<(), CommunityNodeIndexingRequestError> {
        let _grant_guard = self.private_index_grant_guard.lock().await;
        let base_url = normalize_http_url(&request.base_url).map_err(|error| {
            CommunityNodeIndexingRequestError::new("INVALID_COMMUNITY_NODE_URL", error.to_string())
        })?;
        self.require_community_node(&base_url)
            .await
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "COMMUNITY_NODE_NOT_CONFIGURED",
                    error.to_string(),
                )
            })?;
        let target_id = if request.scope_kind == IndexScopeKind::PrivateChannel {
            request.channel_id.as_deref().unwrap_or_default().trim()
        } else {
            request.topic_id.trim()
        }
        .to_owned();
        if target_id.is_empty() {
            return Err(CommunityNodeIndexingRequestError::new(
                "INVALID_INDEXING_REQUEST",
                "target is required",
            ));
        }
        if request.scope_kind == IndexScopeKind::PrivateChannel {
            self.store
                .stop_private_index_grant(&base_url, &request.topic_id, &target_id)
                .await
                .map_err(|error| {
                    CommunityNodeIndexingRequestError::new(
                        "INDEXING_GRANT_SAVE_FAILED",
                        error.to_string(),
                    )
                })?;
        }
        let token = load_community_node_token(&self.db_path, self.identity_mode, &base_url)
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new("AUTH_TOKEN_LOAD_FAILED", error.to_string())
            })?
            .ok_or_else(|| {
                CommunityNodeIndexingRequestError::new(
                    AUTH_REQUIRED_CODE,
                    "community node authentication is required",
                )
            })?;
        let response = community_node_http_client()
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "INDEXING_HTTP_CLIENT_FAILED",
                    error.to_string(),
                )
            })?
            .delete(format!("{base_url}{INDEXING_REQUESTS_PATH}"))
            .bearer_auth(token.access_token)
            .json(&RevokeIndexingRequestRequest {
                kind: request.scope_kind,
                target_id: target_id.clone(),
            })
            .send()
            .await
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "INDEXING_REQUEST_TRANSPORT_FAILED",
                    error.to_string(),
                )
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.json::<ApiErrorBody>().await.ok();
            return Err(CommunityNodeIndexingRequestError::from_response(
                status, None, body,
            ));
        }
        if request.scope_kind == IndexScopeKind::PrivateChannel {
            let grant = PrivateIndexGrant {
                base_url,
                topic_id: request.topic_id,
                channel_id: target_id,
                applied_epoch_id: String::new(),
            };
            delete_optional_secret(
                &self.db_path,
                self.identity_mode,
                PRIVATE_INDEX_GRANT_SECRET_PURPOSE,
                &grant_secret_key(&grant),
            )
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "INDEXING_GRANT_SAVE_FAILED",
                    error.to_string(),
                )
            })?;
        }
        Ok(())
    }

    pub(crate) async fn request_community_node_indexing(
        &self,
        request: CommunityNodeIndexingRequest,
    ) -> Result<SubmitIndexingRequestResponse, CommunityNodeIndexingRequestError> {
        let _grant_guard = self.private_index_grant_guard.lock().await;
        let base_url = normalize_http_url(request.base_url.as_str()).map_err(|error| {
            CommunityNodeIndexingRequestError::new("INVALID_COMMUNITY_NODE_URL", error.to_string())
        })?;
        self.require_community_node(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "COMMUNITY_NODE_NOT_CONFIGURED",
                    error.to_string(),
                )
            })?;

        let topic_id = request.topic_id.trim();
        if topic_id.is_empty() {
            return Err(CommunityNodeIndexingRequestError::new(
                "INVALID_INDEXING_REQUEST",
                "topic_id is required",
            ));
        }
        // セッション確立と同意確認は秘密値を組み立てる前に行う。必須同意が未承認のノードへは
        // 非公開チャンネルの秘密値を含む申請を送らない(#698)。
        let session_outcome = self
            .ensure_community_node_session(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "COMMUNITY_NODE_SESSION_FAILED",
                    error.to_string(),
                )
            })?;
        match session_outcome {
            CommunityNodeSessionOutcome::Ready => {}
            CommunityNodeSessionOutcome::ConsentRequired => {
                return Err(CommunityNodeIndexingRequestError::new(
                    CONSENT_REQUIRED_CODE,
                    "community node required policies must be accepted before indexing requests",
                ));
            }
            CommunityNodeSessionOutcome::Deferred(phase) => {
                return Err(CommunityNodeIndexingRequestError::new(
                    "COMMUNITY_NODE_SESSION_DEFERRED",
                    format!("community node session is not ready ({phase:?})"),
                ));
            }
        }
        let (target_id, epoch_id, channel_secret_hex) = match request.scope_kind {
            IndexScopeKind::PublicTopic => {
                if request
                    .channel_id
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    return Err(CommunityNodeIndexingRequestError::new(
                        "INVALID_INDEXING_REQUEST",
                        "channel_id must not be specified for public topics",
                    ));
                }
                (topic_id.to_string(), None, None)
            }
            IndexScopeKind::PrivateChannel => {
                if !request.confirm_private_channel_secret_disclosure {
                    return Err(CommunityNodeIndexingRequestError::new(
                        "PRIVATE_CHANNEL_SECRET_DISCLOSURE_NOT_CONFIRMED",
                        "private channel indexing requires explicit secret disclosure confirmation",
                    ));
                }
                let channel_id = request
                    .channel_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        CommunityNodeIndexingRequestError::new(
                            "INVALID_INDEXING_REQUEST",
                            "channel_id is required for private channels",
                        )
                    })?;
                let (epoch_id, secret) = self
                    .app_service
                    .private_channel_indexing_capability(topic_id, channel_id)
                    .await
                    .map_err(|error| {
                        CommunityNodeIndexingRequestError::new(
                            "PRIVATE_CHANNEL_CAPABILITY_UNAVAILABLE",
                            error.to_string(),
                        )
                    })?;
                (channel_id.to_string(), Some(epoch_id), Some(secret))
            }
        };

        let token = load_community_node_token(&self.db_path, self.identity_mode, base_url.as_str())
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new("AUTH_TOKEN_LOAD_FAILED", error.to_string())
            })?
            .ok_or_else(|| {
                CommunityNodeIndexingRequestError::new(
                    AUTH_REQUIRED_CODE,
                    "community node authentication is required",
                )
            })?;
        let wire_request = SubmitIndexingRequestRequest {
            kind: request.scope_kind.as_str().to_string(),
            target_id,
            channel_secret_hex,
            epoch_id,
            previous_epoch_id: None,
            previous_channel_secret_hex: None,
        };

        let result = match self
            .send_community_node_indexing_request(
                base_url.as_str(),
                &wire_request,
                token.access_token.as_str(),
            )
            .await
        {
            Err(error) if error.status == Some(StatusCode::UNAUTHORIZED.as_u16()) => {
                let refreshed = self
                    .request_community_node_authentication_token(base_url.as_str())
                    .await
                    .map_err(|error| {
                        CommunityNodeIndexingRequestError::new(
                            "COMMUNITY_NODE_REAUTHENTICATION_FAILED",
                            error.to_string(),
                        )
                    })?;
                self.send_community_node_indexing_request(
                    base_url.as_str(),
                    &wire_request,
                    refreshed.access_token.as_str(),
                )
                .await
            }
            result => result,
        }?;
        if request.scope_kind == IndexScopeKind::PrivateChannel {
            let grant = PrivateIndexGrant {
                base_url: base_url.clone(),
                topic_id: topic_id.to_string(),
                channel_id: wire_request.target_id.clone(),
                applied_epoch_id: wire_request.epoch_id.clone().unwrap_or_default(),
            };
            persist_optional_secret(
                &self.db_path,
                self.identity_mode,
                PRIVATE_INDEX_GRANT_SECRET_PURPOSE,
                &grant_secret_key(&grant),
                wire_request
                    .channel_secret_hex
                    .as_deref()
                    .unwrap_or_default(),
            )
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "INDEXING_GRANT_SAVE_FAILED",
                    error.to_string(),
                )
            })?;
            self.store
                .save_private_index_grant(&grant)
                .await
                .map_err(|error| {
                    CommunityNodeIndexingRequestError::new(
                        "INDEXING_GRANT_SAVE_FAILED",
                        error.to_string(),
                    )
                })?;
        }
        Ok(result)
    }

    /// At most one grant per CN maintenance tick. No secret is sent unless the
    /// same account still holds this channel and the same CN session is ready.
    pub(crate) async fn refresh_private_index_grant_once(
        &self,
        base_url: &str,
    ) -> anyhow::Result<()> {
        let _grant_guard = self.private_index_grant_guard.lock().await;
        let Some(grant) = self
            .store
            .next_private_index_grant(base_url, chrono::Utc::now().timestamp_millis())
            .await?
        else {
            return Ok(());
        };
        if self.ensure_community_node_session(base_url).await? != CommunityNodeSessionOutcome::Ready
        {
            return Ok(());
        }
        let (epoch_id, secret) = match self
            .app_service
            .private_channel_indexing_capability_known(&grant.topic_id, &grant.channel_id)
            .await
        {
            Ok(capability) => capability,
            Err(_) => {
                self.store
                    .stop_private_index_grant(base_url, &grant.topic_id, &grant.channel_id)
                    .await?;
                return Ok(());
            }
        };
        if epoch_id == grant.applied_epoch_id {
            return Ok(());
        }
        let previous_secret = load_optional_secret(
            &self.db_path,
            self.identity_mode,
            PRIVATE_INDEX_GRANT_SECRET_PURPOSE,
            &grant_secret_key(&grant),
        )?
        .ok_or_else(|| anyhow::anyhow!("private indexing grant secret is missing"))?;
        let token = load_community_node_token(&self.db_path, self.identity_mode, base_url)?
            .ok_or_else(|| anyhow::anyhow!("community node token is missing"))?;
        let result = self
            .send_community_node_indexing_request(
                base_url,
                &SubmitIndexingRequestRequest {
                    kind: IndexScopeKind::PrivateChannel.as_str().to_owned(),
                    target_id: grant.channel_id.clone(),
                    channel_secret_hex: Some(secret.clone()),
                    epoch_id: Some(epoch_id.clone()),
                    previous_epoch_id: Some(grant.applied_epoch_id.clone()),
                    previous_channel_secret_hex: Some(previous_secret),
                },
                &token.access_token,
            )
            .await;
        if let Err(error) = &result
            && error.status == Some(StatusCode::CONFLICT.as_u16())
        {
            // Operator replacement or a different current capability is a
            // terminal boundary; repeatedly sending the old secret cannot fix it.
            self.store
                .stop_private_index_grant(base_url, &grant.topic_id, &grant.channel_id)
                .await?;
        }
        result?;
        persist_optional_secret(
            &self.db_path,
            self.identity_mode,
            PRIVATE_INDEX_GRANT_SECRET_PURPOSE,
            &grant_secret_key(&grant),
            &secret,
        )?;
        self.store
            .mark_private_index_grant_applied(&grant, &epoch_id)
            .await?;
        Ok(())
    }

    async fn send_community_node_indexing_request(
        &self,
        base_url: &str,
        request: &SubmitIndexingRequestRequest,
        access_token: &str,
    ) -> Result<SubmitIndexingRequestResponse, CommunityNodeIndexingRequestError> {
        let client = community_node_http_client().map_err(|error| {
            CommunityNodeIndexingRequestError::new("INDEXING_HTTP_CLIENT_FAILED", error.to_string())
        })?;
        let response = client
            .post(format!("{base_url}{INDEXING_REQUESTS_PATH}"))
            .bearer_auth(access_token)
            .json(request)
            .send()
            .await
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "INDEXING_REQUEST_TRANSPORT_FAILED",
                    error.to_string(),
                )
            })?;
        let status = response.status();
        let retry_after_seconds = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        if !status.is_success() {
            let body = response.json::<ApiErrorBody>().await.ok();
            return Err(CommunityNodeIndexingRequestError::from_response(
                status,
                retry_after_seconds,
                body,
            ));
        }
        response
            .json::<SubmitIndexingRequestResponse>()
            .await
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "INDEXING_REQUEST_DECODE_FAILED",
                    error.to_string(),
                )
            })
    }
}
