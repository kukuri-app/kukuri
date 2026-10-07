// ブラウザでも動く共用 crate（ADR 0056 §3）。tokio・std の時刻と task を直接使わない（native では
// n0_future・web_time がそれらの再公開なので、wasm32 の clippy で確かめる）。
#![cfg_attr(
    all(target_family = "wasm", not(test)),
    warn(clippy::disallowed_methods)
)]
mod accounts;
mod attachments;
#[cfg(not(target_family = "wasm"))]
mod backup;
mod command;
mod community_node;
mod discovery;
mod host;
mod identity;
#[cfg(feature = "ts")]
mod ipc_ts_export;
mod kdf;
mod paths;
mod requests;
mod runtime;
mod stack;
mod storage;

#[cfg(test)]
mod tests;

#[cfg(not(target_family = "wasm"))]
pub use accounts::display::AccountDisplay;
pub use accounts::lifecycle::profile_setup_required;
pub use accounts::{
    AccountKeyExport, AccountKeyImportPreview, AccountRecord, AccountTransferLink,
    AccountsSnapshot, account_db_path, add_account_from_env, ensure_accounts_initialized_from_env,
    import_account_key_from_env, list_accounts, preview_account_key_import, set_active_account,
};
#[cfg(not(target_family = "wasm"))]
pub use backup::{
    CreateDeviceBackupRequest, DeviceBackupCancellation, DeviceBackupPhase, DeviceBackupPreview,
    DeviceBackupProgress, DeviceBackupRestoreResult, DeviceBackupSummary, DeviceRestorePhase,
    InstalledDeviceRestore, PreparedDeviceRestore, PreviewDeviceBackupRequest,
    RestoreDeviceBackupRequest, acknowledge_pending_device_restore_frontend_state,
    commit_device_restore, create_device_backup, finalize_device_restore,
    finalize_pending_device_restore, install_prepared_device_restore,
    mark_device_restore_activated, mark_device_restore_awaiting_consent,
    pending_device_restore_frontend_state, pending_device_restore_phase, prepare_device_restore,
    preview_device_backup, recover_interrupted_restore, rollback_device_restore,
    rollback_pending_device_restore, validate_prepared_device_restore,
};
pub use command::{
    COMMAND_FAILED_CODE, ClientGate, CommandError, DispatchContext,
    PRIVATE_CHANNEL_CONTROLLER_PENDING_CODE, PostRetryView, SCOPE_LIMIT_REACHED_CODE,
    STALE_RUNTIME_CODE, UNSUPPORTED_PLATFORM_CODE, dispatch_command, dispatched_commands,
    map_error,
};
/// 通報の送信の HTTP（web-runtime の browser の試験が、転送の拒否を確かめる）。
#[cfg(target_family = "wasm")]
#[doc(hidden)]
pub use community_node::post_report;
pub use community_node::{
    AcceptCommunityNodeConsentsRequest, AuthorTrustGate, AuthorTrustGateRequest,
    AuthorTrustGateResult, CommunityNodeAdmissionRejection, CommunityNodeAdmissionRejectionCode,
    CommunityNodeAuthState, CommunityNodeAuthorityScope, CommunityNodeCapabilityScope,
    CommunityNodeConfig, CommunityNodeConsentDocumentRef, CommunityNodeContentAdvisoryLookupError,
    CommunityNodeContentAdvisoryLookupRequest, CommunityNodeContentAdvisoryLookupResult,
    CommunityNodeContentAdvisoryNodeResult, CommunityNodeIndexQueryError,
    CommunityNodeIndexQueryRequest, CommunityNodeIndexingRequest,
    CommunityNodeIndexingRequestError, CommunityNodeIndexingStatusRequest,
    CommunityNodeLegalDocument, CommunityNodeLocalConsentRecord, CommunityNodeLocalConsentState,
    CommunityNodeManifest, CommunityNodeManifestFetch, CommunityNodeManifestFetchStatus,
    CommunityNodeNodeConfig, CommunityNodeNodeStatus, CommunityNodeObservationSharingStatus,
    CommunityNodeP2pBoundary, CommunityNodePoliciesResponse, CommunityNodePolicyDocument,
    CommunityNodeRelationNeighborsRequest, CommunityNodeReportAppeal, CommunityNodeReportError,
    CommunityNodeSessionPhase, CommunityNodeTargetRequest, CommunityNodeTesterFeedbackError,
    CommunityNodeTesterFeedbackResponse, CommunityNodeTesterFeedbackSubmission,
    CommunityNodeTrustRelationError, CommunityNodeUserAdvisoryRequest, DomeHostingRequestError,
    EnableCommunityNodeObservationSharingRequest, FetchCommunityNodePoliciesRequest,
    IndexEntryView, IndexQueryResponse, IndexScopeKind, IndexingRequestView,
    IndexingStatusResponse, IndexingTargetStatus, RelationNeighborsResponse,
    RelationOptoutResponse, RelationReadResponse, SetAuthorTrustDisplayExceptionRequest,
    SetCommunityNodeConfigNode, SetCommunityNodeConfigRequest, SetCommunityNodeInviteCodeRequest,
    SubmitCommunityNodeReportRequest, SubmitCommunityNodeReportResult,
    SubmitCommunityNodeReportStatus, SubmitIndexingRequestResponse, TrustUserReadResponse,
};
pub use discovery::{DiscoveryConfig, SetDiscoverySeedsRequest, SetPublicBlobDiscoveryRequest};
pub use host::{
    AGE_ATTESTATION_VERSION, APP_LEGAL_AUTHORITATIVE_LANGUAGE, APP_LEGAL_DOCUMENTS,
    APP_LEGAL_EFFECTIVE_DATE, AgeAttestationRecord, AgeAttestationStatus, AppConsentDocumentRecord,
    AppConsentDocumentStatus, AppConsentStore, ClientEventReceiver, ClientHost, ClientHostStart,
    ClientStartupError, ClientStartupErrorKind, ClientStartupErrorView, ClientStartupState,
    ClientStartupStatus, DesiredSubscription, DesiredSubscriptionScope, LEGAL_BUNDLE_VERSION,
    SubscriptionStateError, SubscriptionStateErrorKind, age_attestation_satisfied,
    age_attestation_status, app_consent_documents_satisfied, app_consent_documents_status,
    app_consent_path, app_consent_satisfied, consent_required_status, current_unix_seconds,
    desired_subscriptions_path, distribution_community_node_config, failed_startup_status,
    load_app_consent_store, reset_app_consent_at_path, save_app_consent_store,
};
pub use host::{
    AcceptedAppConsentDocument, AppConsentStatus, app_consent_status, record_app_consents,
    require_consent_acceptance_state, validate_app_consent_documents,
};
#[cfg(not(target_family = "wasm"))]
pub use host::{
    ClientOperationState, RestoreActivationFailure, RestoreActivationOrchestrationFailure,
    RestoreStartupAction, advance_committed_restore_to_consent, orchestrate_restore_activation,
    persist_restore_activation_phase, recover_device_restore_before_startup,
    restore_startup_action,
};
#[cfg(not(target_family = "wasm"))]
pub use host::{
    ClientProfile, ClientProfileKind, ProfileError, ProfileErrorKind, ProfileLease, gui_profile,
    resolve_cli_profile,
};
pub use host::{
    NON_READY_COMMAND_ALLOWLIST, RuntimeBuilder, admit_command, require_runtime_operation_ready,
    runtime_access_allowed,
};
pub use identity::{load_endpoint_secret, save_endpoint_secret};
pub use kukuri_app_api::SessionDisplayRequest;
pub use kukuri_app_api::{
    ConnectivityPeersRequest, MAX_ACTIVE_SCOPES, PrivateChannelControllerPending,
    ScopeDisplayRequest, ScopeDisplayTarget, ScopeLimitReached,
};
pub use kukuri_transport::{ConnectivityPeerKind, PeerPage};
pub use requests::CreateAccountRequest;
pub use stack::{NodeSource, StackStore};
#[cfg(target_family = "wasm")]
pub use storage::install_platform_storage;
pub use storage::{ClientStorage, KeyringUnavailable};
// 起動エラーの typed 分類(WP-Q2)。src-tauri は downcast で DatabaseOpen/Migration を判定する。
#[cfg(not(target_family = "wasm"))]
pub use kukuri_store::StoreStartupError;
pub use paths::{
    AppBuildProfile, default_app_data_dir, resolve_app_data_dir_from_env, resolve_db_path_from_env,
};
pub use requests::{
    AbortDomeTransitionRequest, AcceptDomeConnectionProposalRequest, AuthorRequest,
    BookmarkCustomReactionRequest, BookmarkPostRequest, BookmarkedPostIdsRequest,
    CloseDomeHostingRequest, CommitDomeLayoutRequest, CommitDomeTransitionRequest,
    CreateAttachmentRequest, CreateCustomReactionAssetRequest, CreateDomeConnectionProposalRequest,
    CreateGameRoomRequest, CreateLiveSessionRequest, CreateMetaverseRoomRequest, CreatePostRequest,
    CreatePrivateChannelRequest, CreateRepostRequest, CustomReactionCropRect,
    DecideAccountTransferRequest, DelegateDomeHostingRequest, DeleteDirectMessageMessageRequest,
    DirectMessageRequest, ExportAccountKeyRequest, ExportChannelAccessTokenRequest,
    ExportFriendOnlyGrantRequest, ExportFriendPlusShareRequest, ExportPrivateChannelInviteRequest,
    FreezePrivateChannelRequest, GetBlobMediaRequest, GetBlobPreviewRequest, GetDomeHostingRequest,
    ImportAccountKeyRequest, ImportChannelAccessTokenRequest, ImportFriendOnlyGrantRequest,
    ImportFriendPlusShareRequest, ImportMetaverseRoomAssetRequest, ImportPeerTicketRequest,
    ImportPrivateChannelInviteRequest, InitialProfileRequest, LeavePrivateChannelRequest,
    ListBookmarkedPostsRequest, ListDirectMessageMessagesRequest,
    ListDomeConnectionTopologyRequest, ListGameRoomsRequest, ListJoinedPrivateChannelsRequest,
    ListLiveSessionsRequest, ListMetaverseRoomEventsRequest, ListNotificationsPageRequest,
    ListProfileTimelineRequest, ListRecentReactionsRequest, ListSocialConnectionsRequest,
    ListThreadRequest, ListTimelineRequest, LiveSessionCommandRequest, MoveDomeRequest,
    NotificationIdRequest, OpenAccountTransferRequest, PostWithdrawalReasonRequest,
    PrepareDomeTransitionRequest, PreviewAccountKeyImportRequest, PreviewChannelAccessTokenRequest,
    PublishMetaverseRoomEventRequest, ReactionKeyRequest, RemoveBookmarkedCustomReactionRequest,
    RemoveBookmarkedPostRequest, ResolveCommunityIndexPostsRequest, ResyncDomeSnapshotsRequest,
    RetryPostElementsRequest, RevokeDomeConnectionRequest, RotatePrivateChannelRequest,
    SendDirectMessageRequest, SetChannelGossipEnabledRequest, SetMyProfileRequest,
    SetPrivateChannelEntryDomeRequest, SetTopicGossipEnabledRequest, StartOwnerDomeHostingRequest,
    SubmitDomeSessionInputRequest, SwitchAccountRequest, TakePrivateChannelControllerRequest,
    ToggleReactionRequest, UnsubscribeTopicRequest, UpdateGameRoomRequest,
    UpdateMetaverseRoomRequest, WithdrawDomeConnectionProposalRequest, WithdrawPostRequest,
    WithdrawalReasonVisibilityRequest,
};
pub use runtime::{DesktopRuntime, RuntimeEvent};

/// Generation-bound owner deletion request.
pub use kukuri_app_api::DeleteDomeInput as DeleteDomeRequest;
