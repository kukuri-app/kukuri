use kukuri_desktop_runtime::{
    AcceptCommunityNodeConsentsRequest, AuthorTrustGate, AuthorTrustGateRequest,
    AuthorTrustGateResult, CommunityNodeConfig,
    CommunityNodeContentAdvisoryLookupRequest, CommunityNodeContentAdvisoryLookupResult,
    CommunityNodeIndexQueryRequest,
    CommunityNodeIndexingRequest, CommunityNodeIndexingStatusRequest, CommunityNodeManifestFetch,
    CommunityNodeNodeStatus, CommunityNodeObservationSharingStatus,
    CommunityNodeRelationNeighborsRequest, CommunityNodeTargetRequest,
    CommunityNodeTesterFeedbackResponse, CommunityNodeTesterFeedbackSubmission,
    CommunityNodeUserAdvisoryRequest, CreatePrivateChannelRequest, DiscoveryConfig,
    EnableCommunityNodeObservationSharingRequest, SetAuthorTrustDisplayExceptionRequest,
    ExportChannelAccessTokenRequest, ExportFriendOnlyGrantRequest, ExportFriendPlusShareRequest,
    ExportPrivateChannelInviteRequest, FetchCommunityNodePoliciesRequest,
    FreezePrivateChannelRequest,
    ImportChannelAccessTokenRequest, ImportFriendOnlyGrantRequest, ImportFriendPlusShareRequest,
    ImportPeerTicketRequest, ImportPrivateChannelInviteRequest, IndexQueryResponse,
    IndexingStatusResponse, LeavePrivateChannelRequest, ListJoinedPrivateChannelsRequest, PreviewChannelAccessTokenRequest,
    RelationNeighborsResponse, RelationOptoutResponse, RelationReadResponse,
    RotatePrivateChannelRequest, SetChannelGossipEnabledRequest, SetCommunityNodeConfigRequest,
    SetCommunityNodeInviteCodeRequest, SetDiscoverySeedsRequest, SetPrivateChannelEntryDomeRequest,
    SetTopicGossipEnabledRequest, SubmitCommunityNodeReportRequest,
    SubmitCommunityNodeReportResult, SubmitIndexingRequestResponse, TrustUserReadResponse,
    UnsubscribeTopicRequest,
};

use kukuri_desktop_runtime::CommunityNodePoliciesResponse;

use crate::state::{CommandError, DesktopState, map_error};

#[tauri::command]
pub async fn create_private_channel(
    state: tauri::State<'_, DesktopState>,
    request: CreatePrivateChannelRequest,
) -> Result<kukuri_app_api::JoinedPrivateChannelView, CommandError> {
    state
        .runtime()
        .create_private_channel(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn export_private_channel_invite(
    state: tauri::State<'_, DesktopState>,
    request: ExportPrivateChannelInviteRequest,
) -> Result<String, CommandError> {
    state
        .runtime()
        .export_private_channel_invite(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn import_private_channel_invite(
    state: tauri::State<'_, DesktopState>,
    request: ImportPrivateChannelInviteRequest,
) -> Result<kukuri_core::PrivateChannelInvitePreview, CommandError> {
    state
        .runtime()
        .import_private_channel_invite(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn export_channel_access_token(
    state: tauri::State<'_, DesktopState>,
    request: ExportChannelAccessTokenRequest,
) -> Result<kukuri_app_api::ChannelAccessTokenExport, CommandError> {
    state
        .runtime()
        .export_channel_access_token(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn import_channel_access_token(
    state: tauri::State<'_, DesktopState>,
    request: ImportChannelAccessTokenRequest,
) -> Result<kukuri_app_api::ChannelAccessTokenPreview, CommandError> {
    state
        .runtime()
        .import_channel_access_token(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn preview_channel_access_token(
    state: tauri::State<'_, DesktopState>,
    request: PreviewChannelAccessTokenRequest,
) -> Result<kukuri_app_api::ChannelAccessTokenPreview, CommandError> {
    state
        .runtime()
        .preview_channel_access_token(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn export_friend_only_grant(
    state: tauri::State<'_, DesktopState>,
    request: ExportFriendOnlyGrantRequest,
) -> Result<String, CommandError> {
    state
        .runtime()
        .export_friend_only_grant(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn import_friend_only_grant(
    state: tauri::State<'_, DesktopState>,
    request: ImportFriendOnlyGrantRequest,
) -> Result<kukuri_core::FriendOnlyGrantPreview, CommandError> {
    state
        .runtime()
        .import_friend_only_grant(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn export_friend_plus_share(
    state: tauri::State<'_, DesktopState>,
    request: ExportFriendPlusShareRequest,
) -> Result<String, CommandError> {
    state
        .runtime()
        .export_friend_plus_share(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn import_friend_plus_share(
    state: tauri::State<'_, DesktopState>,
    request: ImportFriendPlusShareRequest,
) -> Result<kukuri_core::FriendPlusSharePreview, CommandError> {
    state
        .runtime()
        .import_friend_plus_share(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn freeze_private_channel(
    state: tauri::State<'_, DesktopState>,
    request: FreezePrivateChannelRequest,
) -> Result<kukuri_app_api::JoinedPrivateChannelView, CommandError> {
    state
        .runtime()
        .freeze_private_channel(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn rotate_private_channel(
    state: tauri::State<'_, DesktopState>,
    request: RotatePrivateChannelRequest,
) -> Result<kukuri_app_api::JoinedPrivateChannelView, CommandError> {
    state
        .runtime()
        .rotate_private_channel(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn set_private_channel_entry_dome(
    state: tauri::State<'_, DesktopState>,
    request: SetPrivateChannelEntryDomeRequest,
) -> Result<kukuri_app_api::JoinedPrivateChannelView, CommandError> {
    state
        .runtime()
        .set_private_channel_entry_dome(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn leave_private_channel(
    state: tauri::State<'_, DesktopState>,
    request: LeavePrivateChannelRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .leave_private_channel(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn list_joined_private_channels(
    state: tauri::State<'_, DesktopState>,
    request: ListJoinedPrivateChannelsRequest,
) -> Result<Vec<kukuri_app_api::JoinedPrivateChannelView>, CommandError> {
    state
        .runtime()
        .list_joined_private_channels(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn get_sync_status(
    state: tauri::State<'_, DesktopState>,
) -> Result<kukuri_app_api::SyncStatus, CommandError> {
    state.runtime().get_sync_status().await.map_err(map_error)
}

#[tauri::command]
pub async fn get_discovery_config(
    state: tauri::State<'_, DesktopState>,
) -> Result<DiscoveryConfig, CommandError> {
    state
        .runtime()
        .get_discovery_config()
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn import_peer_ticket(
    state: tauri::State<'_, DesktopState>,
    request: ImportPeerTicketRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .import_peer_ticket(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn set_discovery_seeds(
    state: tauri::State<'_, DesktopState>,
    request: SetDiscoverySeedsRequest,
) -> Result<DiscoveryConfig, CommandError> {
    state
        .runtime()
        .set_discovery_seeds(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn unsubscribe_topic(
    state: tauri::State<'_, DesktopState>,
    request: UnsubscribeTopicRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .unsubscribe_topic(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn set_topic_gossip_enabled(
    state: tauri::State<'_, DesktopState>,
    request: SetTopicGossipEnabledRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .set_topic_gossip_enabled(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn set_channel_gossip_enabled(
    state: tauri::State<'_, DesktopState>,
    request: SetChannelGossipEnabledRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .set_channel_gossip_enabled(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn get_local_peer_ticket(
    state: tauri::State<'_, DesktopState>,
) -> Result<Option<String>, CommandError> {
    state.runtime().local_peer_ticket().await.map_err(map_error)
}

#[tauri::command]
pub async fn get_community_node_config(
    state: tauri::State<'_, DesktopState>,
) -> Result<CommunityNodeConfig, CommandError> {
    state
        .runtime()
        .get_community_node_config()
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn get_community_node_statuses(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<CommunityNodeNodeStatus>, CommandError> {
    state
        .runtime()
        .get_community_node_statuses()
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn set_community_node_config(
    state: tauri::State<'_, DesktopState>,
    request: SetCommunityNodeConfigRequest,
) -> Result<CommunityNodeConfig, CommandError> {
    state
        .runtime()
        .set_community_node_config(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn clear_community_node_config(
    state: tauri::State<'_, DesktopState>,
) -> Result<(), CommandError> {
    state
        .runtime()
        .clear_community_node_config()
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn authenticate_community_node(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<CommunityNodeNodeStatus, CommandError> {
    state
        .runtime()
        .authenticate_community_node(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn set_community_node_invite_code(
    state: tauri::State<'_, DesktopState>,
    request: SetCommunityNodeInviteCodeRequest,
) -> Result<CommunityNodeNodeStatus, CommandError> {
    state
        .runtime()
        .set_community_node_invite_code(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn clear_community_node_token(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<CommunityNodeNodeStatus, CommandError> {
    state
        .runtime()
        .clear_community_node_token(request)
        .await
        .map_err(map_error)
}

/// #857: 認証不要の公開 policy カタログ。同意モーダルの提示内容を組み立てるために呼ぶ。
#[tauri::command]
pub async fn fetch_community_node_policies(
    state: tauri::State<'_, DesktopState>,
    request: FetchCommunityNodePoliciesRequest,
) -> Result<CommunityNodePoliciesResponse, CommandError> {
    state
        .runtime()
        .fetch_community_node_policies(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn accept_community_node_consents(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    request: AcceptCommunityNodeConsentsRequest,
) -> Result<CommunityNodeNodeStatus, CommandError> {
    // #857: 同意記録のアプリ版はフロントの申告ではなく実行中バイナリの版を使う。
    let app_version = app_handle.package_info().version.to_string();
    state
        .runtime()
        .accept_community_node_consents(request, app_version.as_str())
        .await
        .map_err(map_error)
}

/// #857: Node 同意の撤回。記録は履歴として残し、トークンを破棄して接続を停止する。
#[tauri::command]
pub async fn withdraw_community_node_consents(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<CommunityNodeNodeStatus, CommandError> {
    state
        .runtime()
        .withdraw_community_node_consents(request)
        .await
        .map_err(map_error)
}

/// #1061: 表示中の著者について、採用 CN の信頼値による折りたたみ判断を返す。
#[tauri::command]
pub async fn evaluate_author_trust_gates(
    state: tauri::State<'_, DesktopState>,
    request: AuthorTrustGateRequest,
) -> Result<AuthorTrustGateResult, CommandError> {
    state
        .runtime()
        .evaluate_author_trust_gates(request)
        .await
        .map_err(map_error)
}

/// #1061: 著者ごとの「常に表示する」例外を設定・解除する。
#[tauri::command]
pub async fn set_author_trust_display_exception(
    state: tauri::State<'_, DesktopState>,
    request: SetAuthorTrustDisplayExceptionRequest,
) -> Result<AuthorTrustGate, CommandError> {
    state
        .runtime()
        .set_author_trust_display_exception(request)
        .await
        .map_err(map_error)
}

/// #1061: 「常に表示する」例外の一覧。
#[tauri::command]
pub async fn list_author_trust_display_exceptions(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<String>, CommandError> {
    state
        .runtime()
        .list_author_trust_display_exceptions()
        .await
        .map_err(map_error)
}

/// #1061: ブロック / ミュート観測の提供状態（任意文書の公開有無を含む）。
#[tauri::command]
pub async fn get_community_node_observation_sharing(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<CommunityNodeObservationSharingStatus, CommandError> {
    state
        .runtime()
        .get_community_node_observation_sharing(request)
        .await
        .map_err(map_error)
}

/// #1061: 任意文書への同意で観測提供を有効にする。
#[tauri::command]
pub async fn enable_community_node_observation_sharing(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    request: EnableCommunityNodeObservationSharingRequest,
) -> Result<CommunityNodeObservationSharingStatus, CommandError> {
    let app_version = app_handle.package_info().version.to_string();
    state
        .runtime()
        .enable_community_node_observation_sharing(request, app_version.as_str())
        .await
        .map_err(map_error)
}

/// #1061: 観測提供を止め、保存済み観測の削除を要求する。
#[tauri::command]
pub async fn disable_community_node_observation_sharing(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<CommunityNodeObservationSharingStatus, CommandError> {
    state
        .runtime()
        .disable_community_node_observation_sharing(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn refresh_community_node_metadata(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<CommunityNodeNodeStatus, CommandError> {
    state
        .runtime()
        .refresh_community_node_metadata(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn fetch_community_node_manifest(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<CommunityNodeManifestFetch, CommandError> {
    state
        .runtime()
        .fetch_community_node_manifest(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn submit_community_node_report(
    state: tauri::State<'_, DesktopState>,
    request: SubmitCommunityNodeReportRequest,
) -> Result<SubmitCommunityNodeReportResult, CommandError> {
    state
        .runtime()
        .submit_community_node_report(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn submit_community_node_tester_feedback(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTesterFeedbackSubmission,
) -> Result<CommunityNodeTesterFeedbackResponse, CommandError> {
    state
        .runtime()
        .submit_community_node_tester_feedback(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn submit_community_node_indexing_request(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeIndexingRequest,
) -> Result<SubmitIndexingRequestResponse, CommandError> {
    state
        .runtime()
        .submit_community_node_indexing_request(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn revoke_community_node_indexing_request(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeIndexingRequest,
) -> Result<(), CommandError> {
    state.runtime()
        .revoke_community_node_indexing_request(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn read_community_node_indexing_status(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeIndexingStatusRequest,
) -> Result<IndexingStatusResponse, CommandError> {
    state
        .runtime()
        .read_community_node_indexing_status(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn search_community_node_index(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeIndexQueryRequest,
) -> Result<IndexQueryResponse, CommandError> {
    state
        .runtime()
        .search_community_node_index(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn discover_community_node_index(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeIndexQueryRequest,
) -> Result<IndexQueryResponse, CommandError> {
    state
        .runtime()
        .discover_community_node_index(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn recommend_community_node_index(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeIndexQueryRequest,
) -> Result<IndexQueryResponse, CommandError> {
    state
        .runtime()
        .recommend_community_node_index(request)
        .await
        .map_err(CommandError::from)
}

/// #1056: 可視 post id / blob hash の content advisory を採用 ON の設定済み node へ一括照会する。
#[tauri::command]
pub async fn lookup_community_node_content_advisories(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeContentAdvisoryLookupRequest,
) -> Result<CommunityNodeContentAdvisoryLookupResult, CommandError> {
    state
        .runtime()
        .lookup_community_node_content_advisories(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn read_community_node_trust_user(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeUserAdvisoryRequest,
) -> Result<TrustUserReadResponse, CommandError> {
    state
        .runtime()
        .read_community_node_trust_user(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn read_community_node_relation_user(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeUserAdvisoryRequest,
) -> Result<RelationReadResponse, CommandError> {
    state
        .runtime()
        .read_community_node_relation_user(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn list_community_node_relation_neighbors(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeRelationNeighborsRequest,
) -> Result<RelationNeighborsResponse, CommandError> {
    state
        .runtime()
        .list_community_node_relation_neighbors(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_community_node_relation_optout(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<RelationOptoutResponse, CommandError> {
    state
        .runtime()
        .get_community_node_relation_optout(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn set_community_node_relation_optout(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<RelationOptoutResponse, CommandError> {
    state
        .runtime()
        .set_community_node_relation_optout(request)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn clear_community_node_relation_optout(
    state: tauri::State<'_, DesktopState>,
    request: CommunityNodeTargetRequest,
) -> Result<RelationOptoutResponse, CommandError> {
    state
        .runtime()
        .clear_community_node_relation_optout(request)
        .await
        .map_err(CommandError::from)
}
