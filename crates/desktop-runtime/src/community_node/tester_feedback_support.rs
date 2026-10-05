//! テスターフィードバック送信(#802 / ADR 0039)。
//!
//! UI からは送信先 base_url と 3 つの自由記述だけを受け取り、client version / OS は
//! この層で自動付与する(UI に入力させない)。送信は bearer 認証 + 401 時の再認証
//! 1 回リトライ(indexing request と同じパターン)。

use std::fmt;

use kukuri_cn_protocol::{
    AUTH_REQUIRED_CODE, ApiErrorBody, CONSENT_REQUIRED_CODE, CommunityNodeTesterFeedbackRequest,
    CommunityNodeTesterFeedbackResponse, TESTER_FEEDBACK_MAX_CHARS, TESTER_FEEDBACK_PATH,
    normalize_http_url,
};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize};

use super::{CommunityNodeSessionOutcome, community_node_http_client, load_community_node_token};
use crate::runtime::DesktopRuntime;

/// UI / IPC から受けるテスターフィードバック送信。version / OS は含めない(自動付与)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommunityNodeTesterFeedbackSubmission {
    pub base_url: String,
    /// やろうとしたこと。
    pub what_attempted: String,
    /// 何が起きたか。
    pub what_happened: String,
    /// 何が変だと思ったか。
    pub what_seemed_wrong: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeTesterFeedbackError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
    pub retry_after_seconds: Option<u64>,
}

impl CommunityNodeTesterFeedbackError {
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
            _ => "TESTER_FEEDBACK_FAILED",
        };
        let fallback_message = format!("community node tester feedback failed with {status}");
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

impl fmt::Display for CommunityNodeTesterFeedbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommunityNodeTesterFeedbackError {}

impl DesktopRuntime {
    pub(crate) async fn submit_tester_feedback(
        &self,
        submission: CommunityNodeTesterFeedbackSubmission,
    ) -> Result<CommunityNodeTesterFeedbackResponse, CommunityNodeTesterFeedbackError> {
        let base_url = normalize_http_url(submission.base_url.as_str()).map_err(|error| {
            CommunityNodeTesterFeedbackError::new("INVALID_COMMUNITY_NODE_URL", error.to_string())
        })?;
        self.require_community_node(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeTesterFeedbackError::new(
                    "COMMUNITY_NODE_NOT_CONFIGURED",
                    error.to_string(),
                )
            })?;

        let what_attempted = submission.what_attempted.trim();
        let what_happened = submission.what_happened.trim();
        let what_seemed_wrong = submission.what_seemed_wrong.trim();
        if what_attempted.is_empty() || what_happened.is_empty() || what_seemed_wrong.is_empty() {
            return Err(CommunityNodeTesterFeedbackError::new(
                "INVALID_TESTER_FEEDBACK",
                "what_attempted, what_happened and what_seemed_wrong are required",
            ));
        }
        for text in [what_attempted, what_happened, what_seemed_wrong] {
            if text.chars().count() > TESTER_FEEDBACK_MAX_CHARS {
                return Err(CommunityNodeTesterFeedbackError::new(
                    "INVALID_TESTER_FEEDBACK",
                    format!("each field must be at most {TESTER_FEEDBACK_MAX_CHARS} characters"),
                ));
            }
        }

        let session_outcome = self
            .ensure_community_node_session(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeTesterFeedbackError::new(
                    "COMMUNITY_NODE_SESSION_FAILED",
                    error.to_string(),
                )
            })?;
        match session_outcome {
            CommunityNodeSessionOutcome::Ready => {}
            CommunityNodeSessionOutcome::ConsentRequired => {
                return Err(CommunityNodeTesterFeedbackError::new(
                    CONSENT_REQUIRED_CODE,
                    "community node required policies must be accepted before sending tester feedback",
                ));
            }
            CommunityNodeSessionOutcome::Deferred(phase) => {
                return Err(CommunityNodeTesterFeedbackError::new(
                    "COMMUNITY_NODE_SESSION_DEFERRED",
                    format!("community node session is not ready ({phase:?})"),
                ));
            }
        }

        let token = load_community_node_token(&self.db_path, self.identity_mode, base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeTesterFeedbackError::new("AUTH_TOKEN_LOAD_FAILED", error.to_string())
            })?
            .ok_or_else(|| {
                CommunityNodeTesterFeedbackError::new(
                    AUTH_REQUIRED_CODE,
                    "community node authentication is required",
                )
            })?;
        // client version / OS はこの層で自動付与する(ADR 0039)。UI のバグや改変で
        // 欠落・偽装されないよう、UI 入力は受け取らない。
        let wire_request = CommunityNodeTesterFeedbackRequest {
            what_attempted: what_attempted.to_string(),
            what_happened: what_happened.to_string(),
            what_seemed_wrong: what_seemed_wrong.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
        };

        match self
            .send_tester_feedback(
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
                        CommunityNodeTesterFeedbackError::new(
                            "COMMUNITY_NODE_REAUTHENTICATION_FAILED",
                            error.to_string(),
                        )
                    })?;
                self.send_tester_feedback(
                    base_url.as_str(),
                    &wire_request,
                    refreshed.access_token.as_str(),
                )
                .await
            }
            result => result,
        }
    }

    async fn send_tester_feedback(
        &self,
        base_url: &str,
        request: &CommunityNodeTesterFeedbackRequest,
        access_token: &str,
    ) -> Result<CommunityNodeTesterFeedbackResponse, CommunityNodeTesterFeedbackError> {
        let client = community_node_http_client().map_err(|error| {
            CommunityNodeTesterFeedbackError::new(
                "TESTER_FEEDBACK_HTTP_CLIENT_FAILED",
                error.to_string(),
            )
        })?;
        let response = client
            .post(format!("{base_url}{TESTER_FEEDBACK_PATH}"))
            .bearer_auth(access_token)
            .json(request)
            .send()
            .await
            .map_err(|error| {
                CommunityNodeTesterFeedbackError::new(
                    "TESTER_FEEDBACK_TRANSPORT_FAILED",
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
            return Err(CommunityNodeTesterFeedbackError::from_response(
                status,
                retry_after_seconds,
                body,
            ));
        }
        response
            .json::<CommunityNodeTesterFeedbackResponse>()
            .await
            .map_err(|error| {
                CommunityNodeTesterFeedbackError::new(
                    "TESTER_FEEDBACK_DECODE_FAILED",
                    error.to_string(),
                )
            })
    }
}
