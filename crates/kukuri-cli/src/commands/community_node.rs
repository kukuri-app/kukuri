use super::{command, command_error, community_schema, decode, encode, host_guards, runtime};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput, error_code},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};
use async_trait::async_trait;
use kukuri_desktop_runtime::{
    AcceptCommunityNodeConsentsRequest, AuthorTrustGateRequest,
    CommunityNodeContentAdvisoryLookupRequest, CommunityNodeIndexQueryRequest,
    CommunityNodeIndexingRequest, CommunityNodeIndexingStatusRequest, CommunityNodeNodeStatus,
    CommunityNodeRelationNeighborsRequest, CommunityNodeTargetRequest,
    CommunityNodeTesterFeedbackSubmission, CommunityNodeUserAdvisoryRequest,
    EnableCommunityNodeObservationSharingRequest, FetchCommunityNodePoliciesRequest,
    SetAuthorTrustDisplayExceptionRequest, SetCommunityNodeConfigRequest,
    SetCommunityNodeInviteCodeRequest, SubmitCommunityNodeReportRequest,
};
use serde_json::Value;
use std::sync::Arc;

struct Handler(&'static str);

fn safe_status(mut status: CommunityNodeNodeStatus) -> CommunityNodeNodeStatus {
    // 保存済みの通信エラーにも招待コード等が混ざり得るため、生の診断本文は返さない。
    if status.last_error.is_some() {
        status.last_error = Some("Community Nodeとの処理でエラーが発生しました".into());
    }
    if let Some(rejection) = &mut status.admission_rejection {
        rejection.message = "Community Nodeの参加条件を満たしていません".into();
    }
    status
}

#[async_trait]
impl CommandHandler for Handler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        payload: Value,
        secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let runtime = runtime(&context)?;
        match self.0 {
            "get_community_node_config" => encode(
                runtime
                    .get_community_node_config()
                    .await
                    .map_err(command_error)?,
            ),
            "get_community_node_statuses" => encode(
                runtime
                    .get_community_node_statuses()
                    .await
                    .map_err(command_error)?
                    .into_iter()
                    .map(safe_status)
                    .collect::<Vec<_>>(),
            ),
            "set_community_node_config" => encode(
                runtime
                    .set_community_node_config(decode::<SetCommunityNodeConfigRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "clear_community_node_config" => encode(
                runtime
                    .clear_community_node_config()
                    .await
                    .map_err(command_error)?,
            ),
            "authenticate_community_node" => encode(safe_status(
                runtime
                    .authenticate_community_node(decode::<CommunityNodeTargetRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            )),
            "clear_community_node_token" => encode(safe_status(
                runtime
                    .clear_community_node_token(decode::<CommunityNodeTargetRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            )),
            "fetch_community_node_policies" => encode(
                runtime
                    .fetch_community_node_policies(decode::<FetchCommunityNodePoliciesRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
            ),
            "withdraw_community_node_consents" => encode(safe_status(
                runtime
                    .withdraw_community_node_consents(decode::<CommunityNodeTargetRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
            )),
            "refresh_community_node_metadata" => encode(safe_status(
                runtime
                    .refresh_community_node_metadata(decode::<CommunityNodeTargetRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            )),
            "fetch_community_node_manifest" => encode(
                runtime
                    .fetch_community_node_manifest(decode::<CommunityNodeTargetRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "submit_community_node_report" => encode(
                runtime
                    .submit_community_node_report(decode::<SubmitCommunityNodeReportRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "submit_community_node_tester_feedback" => encode(
                runtime
                    .submit_community_node_tester_feedback(decode::<
                        CommunityNodeTesterFeedbackSubmission,
                    >(payload)?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "submit_community_node_indexing_request" => encode(
                runtime
                    .submit_community_node_indexing_request(decode::<CommunityNodeIndexingRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "revoke_community_node_indexing_request" => encode(
                runtime
                    .revoke_community_node_indexing_request(decode::<CommunityNodeIndexingRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "read_community_node_indexing_status" => encode(
                runtime
                    .read_community_node_indexing_status(decode::<
                        CommunityNodeIndexingStatusRequest,
                    >(payload)?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "search_community_node_index" => encode(
                runtime
                    .search_community_node_index(decode::<CommunityNodeIndexQueryRequest>(payload)?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "discover_community_node_index" => encode(
                runtime
                    .discover_community_node_index(decode::<CommunityNodeIndexQueryRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "recommend_community_node_index" => encode(
                runtime
                    .recommend_community_node_index(decode::<CommunityNodeIndexQueryRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "lookup_community_node_content_advisories" => encode(
                runtime
                    .lookup_community_node_content_advisories(decode::<
                        CommunityNodeContentAdvisoryLookupRequest,
                    >(payload)?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "read_community_node_trust_user" => encode(
                runtime
                    .read_community_node_trust_user(decode::<CommunityNodeUserAdvisoryRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "read_community_node_relation_user" => encode(
                runtime
                    .read_community_node_relation_user(decode::<CommunityNodeUserAdvisoryRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "list_community_node_relation_neighbors" => encode(
                runtime
                    .list_community_node_relation_neighbors(decode::<
                        CommunityNodeRelationNeighborsRequest,
                    >(payload)?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "get_community_node_relation_optout" => encode(
                runtime
                    .get_community_node_relation_optout(decode::<CommunityNodeTargetRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "set_community_node_relation_optout" => encode(
                runtime
                    .set_community_node_relation_optout(decode::<CommunityNodeTargetRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "clear_community_node_relation_optout" => encode(
                runtime
                    .clear_community_node_relation_optout(decode::<CommunityNodeTargetRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(|error| command_error(error.into()))?,
            ),
            "evaluate_author_trust_gates" => encode(
                runtime
                    .evaluate_author_trust_gates(decode::<AuthorTrustGateRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "set_author_trust_display_exception" => encode(
                runtime
                    .set_author_trust_display_exception(decode::<
                        SetAuthorTrustDisplayExceptionRequest,
                    >(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "list_author_trust_display_exceptions" => encode(
                runtime
                    .list_author_trust_display_exceptions()
                    .await
                    .map_err(command_error)?,
            ),
            "get_community_node_observation_sharing" => encode(
                runtime
                    .get_community_node_observation_sharing(decode::<CommunityNodeTargetRequest>(
                        payload,
                    )?)
                    .await
                    .map_err(command_error)?,
            ),
            "enable_community_node_observation_sharing" => encode(
                runtime
                    .enable_community_node_observation_sharing(
                        decode::<EnableCommunityNodeObservationSharingRequest>(payload)?,
                        env!("CARGO_PKG_VERSION"),
                    )
                    .await
                    .map_err(command_error)?,
            ),
            "disable_community_node_observation_sharing" => encode(
                runtime
                    .disable_community_node_observation_sharing(
                        decode::<CommunityNodeTargetRequest>(payload)?,
                    )
                    .await
                    .map_err(command_error)?,
            ),
            "accept_community_node_consents" => encode(safe_status(
                runtime
                    .accept_community_node_consents(
                        decode::<AcceptCommunityNodeConsentsRequest>(payload)?,
                        env!("CARGO_PKG_VERSION"),
                    )
                    .await
                    .map_err(command_error)?,
            )),
            "set_community_node_invite_code" => {
                let target: CommunityNodeTargetRequest = decode(payload)?;
                let bytes = secret
                    .ok_or_else(|| {
                        ProtocolError::new(
                            error_code::VALIDATION_FAILED,
                            "招待コードの専用入力が必要です",
                        )
                    })?
                    .expose();
                let invite_code = std::str::from_utf8(bytes).map_err(|_| {
                    ProtocolError::new(
                        error_code::VALIDATION_FAILED,
                        "招待コードはUTF-8で指定してください",
                    )
                })?;
                encode(safe_status(
                    runtime
                        .set_community_node_invite_code(SetCommunityNodeInviteCodeRequest {
                            base_url: target.base_url,
                            invite_code: Some(invite_code.to_owned()),
                        })
                        .await
                        .map_err(command_error)?,
                ))
            }
            _ => unreachable!("登録済みのCommunity Node command"),
        }
    }
}

pub(super) fn registrations() -> Vec<CommandRegistration> {
    use CommandEffect::{Destructive, Read, Write};
    [
        ("get_community_node_config", Read, false),
        ("get_community_node_statuses", Read, false),
        ("set_community_node_config", Write, false),
        ("clear_community_node_config", Write, false),
        ("authenticate_community_node", Write, false),
        ("clear_community_node_token", Destructive, false),
        ("fetch_community_node_policies", Read, false),
        ("withdraw_community_node_consents", Destructive, false),
        ("refresh_community_node_metadata", Write, false),
        ("fetch_community_node_manifest", Read, false),
        ("submit_community_node_report", Write, false),
        ("submit_community_node_tester_feedback", Write, false),
        ("submit_community_node_indexing_request", Write, false),
        ("revoke_community_node_indexing_request", Destructive, false),
        ("read_community_node_indexing_status", Read, false),
        ("search_community_node_index", Read, false),
        ("discover_community_node_index", Read, false),
        ("recommend_community_node_index", Read, false),
        ("lookup_community_node_content_advisories", Read, false),
        ("read_community_node_trust_user", Read, false),
        ("read_community_node_relation_user", Read, false),
        ("list_community_node_relation_neighbors", Read, false),
        ("get_community_node_relation_optout", Read, false),
        ("set_community_node_relation_optout", Write, false),
        ("clear_community_node_relation_optout", Destructive, false),
        ("accept_community_node_consents", Write, false),
        ("get_community_node_observation_sharing", Read, false),
        ("evaluate_author_trust_gates", Read, false),
        ("set_author_trust_display_exception", Write, false),
        ("list_author_trust_display_exceptions", Read, false),
        ("enable_community_node_observation_sharing", Write, false),
        (
            "disable_community_node_observation_sharing",
            Destructive,
            false,
        ),
        ("set_community_node_invite_code", Write, true),
    ]
    .into_iter()
    .map(|(name, effect, secret)| {
        command(
            name,
            effect,
            secret,
            false,
            host_guards(),
            (
                community_schema::input(name),
                community_schema::output(name),
            ),
            Arc::new(Handler(name)),
        )
    })
    .collect()
}
