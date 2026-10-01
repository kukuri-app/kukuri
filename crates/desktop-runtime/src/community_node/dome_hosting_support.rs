use anyhow::{Context, Result, anyhow};
use kukuri_cn_protocol::{
    ApiErrorBody, CONSENT_REQUIRED_CODE, DOME_HOSTING_ACTIVATE_PATH, DOME_HOSTING_ASSIGNMENTS_PATH,
    DOME_HOSTING_LAYOUT_CANDIDATE_PATH, DOME_HOSTING_RELEASE_PATH, DOME_HOSTING_SESSION_INPUT_PATH,
    DOME_HOSTING_SNAPSHOT_RESYNC_PATH, DOME_HOSTING_STATUS_ROUTE, DOME_TRANSITION_ABORT_PATH,
    DOME_TRANSITION_COMMIT_PATH, DOME_TRANSITION_PREPARE_PATH, DomeHostingActivationRequest,
    DomeHostingAssignmentRequest, DomeHostingAssignmentResponse, DomeHostingLayoutCandidateRequest,
    DomeHostingLayoutCandidateResponse, DomeHostingReleaseRequest, DomeHostingSessionInputRequest,
    DomeHostingSessionSnapshotResponse, DomeHostingSnapshotResyncRequest,
    DomeHostingSnapshotResyncResponse, DomeHostingStatusResponse, DomeTransitionAbortRequest,
    DomeTransitionCommitRequest, DomeTransitionMutationResponse, DomeTransitionPrepareRequest,
    DomeTransitionPrepareResponse, normalize_http_url,
};
use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub struct DomeHostingRequestError {
    pub code: String,
    pub message: String,
    pub status: u16,
}

impl std::fmt::Display for DomeHostingRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DomeHostingRequestError {}

use super::{CommunityNodeConsentPreflight, community_node_http_client, load_community_node_token};
use crate::runtime::DesktopRuntime;

impl DesktopRuntime {
    async fn require_dome_hosting_community_node_consent(
        &self,
        raw_base_url: &str,
    ) -> Result<String> {
        let base_url = normalize_http_url(raw_base_url)?;
        self.require_community_node(base_url.as_str())
            .await
            .map_err(|error| {
                anyhow::Error::new(DomeHostingRequestError {
                    code: "DOME_HOSTING_TARGET_NOT_CONFIGURED".to_string(),
                    message: error.to_string(),
                    status: 400,
                })
            })?;
        let preflight = self
            .preflight_community_node_consent(base_url.as_str())
            .await?;
        if let CommunityNodeConsentPreflight::Required { policy_update, .. } = preflight {
            self.set_community_node_local_consent_update_pending(base_url.as_str(), policy_update)
                .await;
            let message = if policy_update {
                "current community node policies must be accepted before Dome hosting"
            } else {
                "community node consent is required before Dome hosting"
            };
            return Err(DomeHostingRequestError {
                code: CONSENT_REQUIRED_CODE.to_string(),
                message: message.to_string(),
                status: 403,
            }
            .into());
        }
        self.set_community_node_local_consent_update_pending(base_url.as_str(), false)
            .await;
        Ok(base_url)
    }

    pub(crate) async fn assign_dome_hosting_to_community_node(
        &self,
        base_url: &str,
        request: &DomeHostingAssignmentRequest,
    ) -> Result<DomeHostingAssignmentResponse> {
        self.send_dome_hosting_request(base_url, DOME_HOSTING_ASSIGNMENTS_PATH, request)
            .await
    }

    pub(crate) async fn activate_dome_hosting_on_community_node(
        &self,
        base_url: &str,
        request: &DomeHostingActivationRequest,
    ) -> Result<DomeHostingStatusResponse> {
        self.send_dome_hosting_request(base_url, DOME_HOSTING_ACTIVATE_PATH, request)
            .await
    }

    pub(crate) async fn release_dome_hosting_on_community_node(
        &self,
        base_url: &str,
        request: &DomeHostingReleaseRequest,
    ) -> Result<DomeHostingStatusResponse> {
        self.send_dome_hosting_request(base_url, DOME_HOSTING_RELEASE_PATH, request)
            .await
    }

    pub(crate) async fn submit_dome_hosting_input_to_community_node(
        &self,
        base_url: &str,
        request: &DomeHostingSessionInputRequest,
    ) -> Result<DomeHostingSessionSnapshotResponse> {
        self.send_dome_hosting_request(base_url, DOME_HOSTING_SESSION_INPUT_PATH, request)
            .await
    }

    pub(crate) async fn prepare_dome_transition_on_community_node(
        &self,
        base_url: &str,
        request: &DomeTransitionPrepareRequest,
    ) -> Result<DomeTransitionPrepareResponse> {
        self.send_dome_hosting_request(base_url, DOME_TRANSITION_PREPARE_PATH, request)
            .await
    }

    pub(crate) async fn commit_dome_transition_on_community_node(
        &self,
        base_url: &str,
        request: &DomeTransitionCommitRequest,
    ) -> Result<DomeTransitionMutationResponse> {
        self.send_dome_hosting_request(base_url, DOME_TRANSITION_COMMIT_PATH, request)
            .await
    }

    pub(crate) async fn abort_dome_transition_on_community_node(
        &self,
        base_url: &str,
        request: &DomeTransitionAbortRequest,
    ) -> Result<DomeTransitionMutationResponse> {
        self.send_dome_hosting_request(base_url, DOME_TRANSITION_ABORT_PATH, request)
            .await
    }

    pub(crate) async fn capture_dome_layout_candidate_from_community_node(
        &self,
        base_url: &str,
        request: &DomeHostingLayoutCandidateRequest,
    ) -> Result<DomeHostingLayoutCandidateResponse> {
        self.send_dome_hosting_request(base_url, DOME_HOSTING_LAYOUT_CANDIDATE_PATH, request)
            .await
    }

    pub(crate) async fn resync_dome_snapshots_from_community_node(
        &self,
        base_url: &str,
        request: &DomeHostingSnapshotResyncRequest,
    ) -> Result<DomeHostingSnapshotResyncResponse> {
        self.send_dome_hosting_request(base_url, DOME_HOSTING_SNAPSHOT_RESYNC_PATH, request)
            .await
    }

    pub(crate) async fn get_dome_hosting_status_from_community_node(
        &self,
        base_url: &str,
        instance_id: &str,
    ) -> Result<DomeHostingStatusResponse> {
        let path = DOME_HOSTING_STATUS_ROUTE.replace("{instance_id}", instance_id);
        self.send_dome_hosting_get(base_url, &path).await
    }

    async fn send_dome_hosting_get<Response>(
        &self,
        raw_base_url: &str,
        path: &str,
    ) -> Result<Response>
    where
        Response: DeserializeOwned,
    {
        let base_url = self
            .require_dome_hosting_community_node_consent(raw_base_url)
            .await?;
        let token =
            match load_community_node_token(&self.db_path, self.identity_mode, &base_url).await? {
                Some(token) => token,
                None => {
                    self.request_community_node_authentication_token(&base_url)
                        .await?
                }
            };
        match send_get(&base_url, path, &token.access_token).await {
            Err(DomeHostingHttpError::Unauthorized) => {
                let refreshed = self
                    .request_community_node_authentication_token(&base_url)
                    .await?;
                send_get(&base_url, path, &refreshed.access_token)
                    .await
                    .map_err(DomeHostingHttpError::into_anyhow)
            }
            Err(error) => Err(error.into_anyhow()),
            Ok(response) => Ok(response),
        }
    }

    async fn send_dome_hosting_request<Request, Response>(
        &self,
        raw_base_url: &str,
        path: &str,
        request: &Request,
    ) -> Result<Response>
    where
        Request: Serialize + ?Sized,
        Response: DeserializeOwned,
    {
        let base_url = self
            .require_dome_hosting_community_node_consent(raw_base_url)
            .await?;
        let token =
            match load_community_node_token(&self.db_path, self.identity_mode, &base_url).await? {
                Some(token) => token,
                None => {
                    self.request_community_node_authentication_token(&base_url)
                        .await?
                }
            };
        match send_json(&base_url, path, request, &token.access_token).await {
            Err(DomeHostingHttpError::Unauthorized) => {
                let refreshed = self
                    .request_community_node_authentication_token(&base_url)
                    .await?;
                send_json(&base_url, path, request, &refreshed.access_token)
                    .await
                    .map_err(DomeHostingHttpError::into_anyhow)
            }
            Err(error) => Err(error.into_anyhow()),
            Ok(response) => Ok(response),
        }
    }
}

enum DomeHostingHttpError {
    Unauthorized,
    Other(anyhow::Error),
}

impl DomeHostingHttpError {
    fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Unauthorized => anyhow!("community node authentication is required"),
            Self::Other(error) => error,
        }
    }
}

async fn send_json<Request, Response>(
    base_url: &str,
    path: &str,
    request: &Request,
    access_token: &str,
) -> std::result::Result<Response, DomeHostingHttpError>
where
    Request: Serialize + ?Sized,
    Response: DeserializeOwned,
{
    let response = community_node_http_client()
        .map_err(DomeHostingHttpError::Other)?
        .post(format!("{base_url}{path}"))
        .bearer_auth(access_token)
        .json(request)
        .send()
        .await
        .context("failed to send Community Node Dome hosting request")
        .map_err(DomeHostingHttpError::Other)?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(DomeHostingHttpError::Unauthorized);
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.json::<ApiErrorBody>().await.ok();
        if let Some(body) = body {
            let (code, message) = if body.code == "METAVERSE_RESOURCE_BUDGET_REJECTED" {
                serde_json::from_str::<kukuri_core::MetaverseResourceRejection>(&body.message)
                    .map(|rejection| (rejection.code(), rejection.to_string()))
                    .unwrap_or((body.code, body.message))
            } else {
                (body.code, body.message)
            };
            return Err(DomeHostingHttpError::Other(
                DomeHostingRequestError {
                    code,
                    message,
                    status: status.as_u16(),
                }
                .into(),
            ));
        }
        return Err(DomeHostingHttpError::Other(anyhow!(
            "Community Node Dome hosting request failed: {status}"
        )));
    }
    response
        .json::<Response>()
        .await
        .context("failed to decode Community Node Dome hosting response")
        .map_err(DomeHostingHttpError::Other)
}

async fn send_get<Response>(
    base_url: &str,
    path: &str,
    access_token: &str,
) -> std::result::Result<Response, DomeHostingHttpError>
where
    Response: DeserializeOwned,
{
    let response = community_node_http_client()
        .map_err(DomeHostingHttpError::Other)?
        .get(format!("{base_url}{path}"))
        .bearer_auth(access_token)
        .send()
        .await
        .context("failed to fetch Community Node Dome hosting status")
        .map_err(DomeHostingHttpError::Other)?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(DomeHostingHttpError::Unauthorized);
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.json::<ApiErrorBody>().await.ok();
        let detail = body.map_or_else(
            || status.to_string(),
            |body| format!("{}: {}", body.code, body.message),
        );
        return Err(DomeHostingHttpError::Other(anyhow!(
            "Community Node Dome hosting status failed: {detail}"
        )));
    }
    response
        .json::<Response>()
        .await
        .context("failed to decode Community Node Dome hosting status")
        .map_err(DomeHostingHttpError::Other)
}
