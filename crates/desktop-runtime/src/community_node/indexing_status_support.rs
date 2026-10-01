//! 索引状況の読取り(#975): 自分の索引申請の状態と、任意の対象が supported set に含まれるか。
//!
//! 送信順序は申請(`indexing_request_support`)と同じ「session → 同意 → 秘密値 → token → HTTP」。
//! 非公開チャンネルの `supported` 判定だけが所属証明(channel secret)を伴い、明示確認なしには
//! 秘密値を組み立てない。自分の申請一覧の取得(target 無指定)は秘密値を送らない。
//! 応答は client に永続化せず、呼出し側の表示 state でだけ保持する(INVAR-4)。

use kukuri_cn_protocol::{
    AUTH_REQUIRED_CODE, ApiErrorBody, CHANNEL_MEMBERSHIP_SECRET_HEADER, CONSENT_REQUIRED_CODE,
    INDEXING_STATUS_PATH, IndexScopeKind, IndexingStatusParams, IndexingStatusResponse,
    normalize_http_url,
};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize};

use super::{
    CommunityNodeIndexingRequestError, CommunityNodeSessionOutcome, community_node_http_client,
    load_community_node_token,
};
use crate::runtime::DesktopRuntime;

/// UI / IPC から受ける索引状況の読取り要求。
///
/// `scope_kind` を指定すると対象の `supported` 判定を併せて取得する。public では `topic_id`、
/// private では `topic_id`(親 topic)と `channel_id` が必要。無指定なら自分の申請一覧だけを返す。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeIndexingStatusRequest {
    pub base_url: String,
    pub scope_kind: Option<IndexScopeKind>,
    /// public の target、private capability の親 topic。`scope_kind` 指定時は必須。
    pub topic_id: Option<String>,
    /// private の target。public では指定しない。
    pub channel_id: Option<String>,
    /// private の所属証明(channel secret)を外部送信することを UI で明示確認した。
    #[serde(default)]
    pub confirm_private_channel_secret_disclosure: bool,
}

/// 読取りの target 指定を検証した結果。秘密値はまだ含まない。
enum StatusTarget {
    None,
    Public {
        topic_id: String,
    },
    Private {
        topic_id: String,
        channel_id: String,
    },
}

fn validate_status_target(
    request: &CommunityNodeIndexingStatusRequest,
) -> Result<StatusTarget, CommunityNodeIndexingRequestError> {
    let invalid = |message: &str| {
        CommunityNodeIndexingRequestError::new("INVALID_INDEXING_STATUS_REQUEST", message)
    };
    let topic_id = request
        .topic_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let channel_id = request
        .channel_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match request.scope_kind {
        None => {
            if channel_id.is_some() {
                return Err(invalid("channel_id requires scope_kind"));
            }
            Ok(StatusTarget::None)
        }
        Some(IndexScopeKind::PublicTopic) => {
            if channel_id.is_some() {
                return Err(invalid(
                    "channel_id must not be specified for public topics",
                ));
            }
            let topic_id = topic_id.ok_or_else(|| invalid("topic_id is required"))?;
            Ok(StatusTarget::Public {
                topic_id: topic_id.to_string(),
            })
        }
        Some(IndexScopeKind::PrivateChannel) => {
            let topic_id = topic_id.ok_or_else(|| invalid("topic_id is required"))?;
            let channel_id =
                channel_id.ok_or_else(|| invalid("channel_id is required for private channels"))?;
            if !request.confirm_private_channel_secret_disclosure {
                return Err(CommunityNodeIndexingRequestError::new(
                    "PRIVATE_CHANNEL_SECRET_DISCLOSURE_NOT_CONFIRMED",
                    "private channel indexing status requires explicit secret disclosure confirmation",
                ));
            }
            Ok(StatusTarget::Private {
                topic_id: topic_id.to_string(),
                channel_id: channel_id.to_string(),
            })
        }
    }
}

impl DesktopRuntime {
    pub(crate) async fn fetch_community_node_indexing_status(
        &self,
        request: CommunityNodeIndexingStatusRequest,
    ) -> Result<IndexingStatusResponse, CommunityNodeIndexingRequestError> {
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
        // 入力検証は I/O を伴わないため session より先に行う。非公開の確認 flag もここで固定する。
        let target = validate_status_target(&request)?;

        // 申請と同じ順序: session 確立と同意確認を秘密値の組立てより前に行う(#698)。
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
                    "community node required policies must be accepted before reading indexing status",
                ));
            }
            CommunityNodeSessionOutcome::Deferred(phase) => {
                return Err(CommunityNodeIndexingRequestError::new(
                    "COMMUNITY_NODE_SESSION_DEFERRED",
                    format!("community node session is not ready ({phase:?})"),
                ));
            }
        }

        let (params, channel_secret) = match target {
            StatusTarget::None => (IndexingStatusParams::default(), None),
            StatusTarget::Public { topic_id } => (
                IndexingStatusParams {
                    scope_kind: Some(IndexScopeKind::PublicTopic.as_str().to_string()),
                    scope_id: Some(topic_id),
                },
                None,
            ),
            StatusTarget::Private {
                topic_id,
                channel_id,
            } => {
                // 参加中でない(capability が無い)チャンネルは送信前に拒否する(#711)。
                let secret = self
                    .app_service
                    .private_channel_indexing_secret(topic_id.as_str(), channel_id.as_str())
                    .await
                    .map_err(|error| {
                        CommunityNodeIndexingRequestError::new(
                            "PRIVATE_CHANNEL_CAPABILITY_UNAVAILABLE",
                            error.to_string(),
                        )
                    })?;
                (
                    IndexingStatusParams {
                        scope_kind: Some(IndexScopeKind::PrivateChannel.as_str().to_string()),
                        scope_id: Some(channel_id),
                    },
                    Some(secret),
                )
            }
        };

        let token = load_community_node_token(&self.db_path, self.identity_mode, base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new("AUTH_TOKEN_LOAD_FAILED", error.to_string())
            })?
            .ok_or_else(|| {
                CommunityNodeIndexingRequestError::new(
                    AUTH_REQUIRED_CODE,
                    "community node authentication is required",
                )
            })?;

        match self
            .send_community_node_indexing_status(
                base_url.as_str(),
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
                        CommunityNodeIndexingRequestError::new(
                            "COMMUNITY_NODE_REAUTHENTICATION_FAILED",
                            error.to_string(),
                        )
                    })?;
                self.send_community_node_indexing_status(
                    base_url.as_str(),
                    &params,
                    refreshed.access_token.as_str(),
                    channel_secret.as_deref(),
                )
                .await
            }
            result => result,
        }
    }

    async fn send_community_node_indexing_status(
        &self,
        base_url: &str,
        params: &IndexingStatusParams,
        access_token: &str,
        channel_secret: Option<&str>,
    ) -> Result<IndexingStatusResponse, CommunityNodeIndexingRequestError> {
        let client = community_node_http_client().map_err(|error| {
            CommunityNodeIndexingRequestError::new("INDEXING_HTTP_CLIENT_FAILED", error.to_string())
        })?;
        let mut request_builder = client
            .get(format!("{base_url}{INDEXING_STATUS_PATH}"))
            .bearer_auth(access_token)
            .query(params);
        // 所属証明は URL クエリでなくヘッダで送る(アクセスログへの露出を避ける。#711)。
        if let Some(channel_secret) = channel_secret {
            request_builder =
                request_builder.header(CHANNEL_MEMBERSHIP_SECRET_HEADER, channel_secret);
        }
        let response = request_builder.send().await.map_err(|error| {
            CommunityNodeIndexingRequestError::new(
                "INDEXING_STATUS_TRANSPORT_FAILED",
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
            .json::<IndexingStatusResponse>()
            .await
            .map_err(|error| {
                CommunityNodeIndexingRequestError::new(
                    "INDEXING_STATUS_DECODE_FAILED",
                    error.to_string(),
                )
            })
    }
}
