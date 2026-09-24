mod accounts;
mod attachments;
mod backup;
mod community_node;
mod discovery;
mod host;
mod identity;
#[cfg(feature = "ts")]
mod ipc_ts_export;
mod paths;
mod requests;
mod runtime;
mod stack;

#[cfg(test)]
mod tests;

pub use accounts::display::AccountDisplay;
pub use accounts::lifecycle::profile_setup_required;
pub use accounts::{
    AccountKeyExport, AccountKeyImportPreview, AccountRecord, AccountsSnapshot, account_db_path,
    add_account_from_env, ensure_accounts_initialized_from_env, import_account_key_from_env,
    list_accounts, preview_account_key_import, set_active_account,
};
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
pub use discovery::{DiscoveryConfig, SetDiscoverySeedsRequest};
pub use host::{
    AGE_ATTESTATION_VERSION, APP_LEGAL_AUTHORITATIVE_LANGUAGE, APP_LEGAL_DOCUMENTS,
    APP_LEGAL_EFFECTIVE_DATE, AgeAttestationRecord, AgeAttestationStatus, AppConsentDocumentRecord,
    AppConsentDocumentStatus, AppConsentStore, ClientEventReceiver, ClientHost, ClientHostStart,
    ClientProfile, ClientProfileKind, ClientStartupError, ClientStartupErrorKind,
    ClientStartupErrorView, ClientStartupState, ClientStartupStatus, DesiredSubscription,
    DesiredSubscriptionScope, LEGAL_BUNDLE_VERSION, ProfileError, ProfileErrorKind, ProfileLease,
    SubscriptionStateError, SubscriptionStateErrorKind, age_attestation_satisfied,
    age_attestation_status, app_consent_documents_satisfied, app_consent_documents_status,
    app_consent_path, app_consent_satisfied, consent_required_status, current_unix_seconds,
    desired_subscriptions_path, distribution_community_node_config, failed_startup_status,
    gui_profile, load_app_consent_store, reset_app_consent_at_path, resolve_cli_profile,
    save_app_consent_store,
};
pub use host::{
    AcceptedAppConsentDocument, AppConsentStatus, app_consent_status, record_app_consents,
    require_consent_acceptance_state, validate_app_consent_documents,
};
pub use host::{
    ClientOperationState, RestoreActivationFailure, RestoreActivationOrchestrationFailure,
    RestoreStartupAction, advance_committed_restore_to_consent, orchestrate_restore_activation,
    persist_restore_activation_phase, recover_device_restore_before_startup,
    require_runtime_operation_ready, restore_startup_action, runtime_access_allowed,
};
pub use kukuri_app_api::SessionDisplayRequest;
pub use requests::CreateAccountRequest;
// 起動エラーの typed 分類(WP-Q2)。src-tauri は downcast で DatabaseOpen/Migration を判定する。
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
    DelegateDomeHostingRequest, DeleteDirectMessageMessageRequest, DirectMessageRequest,
    ExportAccountKeyRequest, ExportChannelAccessTokenRequest, ExportFriendOnlyGrantRequest,
    ExportFriendPlusShareRequest, ExportPrivateChannelInviteRequest, FreezePrivateChannelRequest,
    GetBlobMediaRequest, GetBlobPreviewRequest, GetDomeHostingRequest, ImportAccountKeyRequest,
    ImportChannelAccessTokenRequest, ImportFriendOnlyGrantRequest, ImportFriendPlusShareRequest,
    ImportMetaverseRoomAssetRequest, ImportPeerTicketRequest, ImportPrivateChannelInviteRequest,
    InitialProfileRequest, LeavePrivateChannelRequest, ListBookmarkedPostsRequest,
    ListDirectMessageMessagesRequest, ListDomeConnectionTopologyRequest, ListGameRoomsRequest,
    ListJoinedPrivateChannelsRequest, ListLiveSessionsRequest, ListMetaverseRoomEventsRequest,
    ListProfileTimelineRequest, ListRecentReactionsRequest, ListSocialConnectionsRequest,
    ListThreadRequest, ListTimelineRequest, LiveSessionCommandRequest, MoveDomeRequest,
    NotificationIdRequest, PostWithdrawalReasonRequest, PrepareDomeTransitionRequest,
    PreviewAccountKeyImportRequest, PreviewChannelAccessTokenRequest,
    PublishMetaverseRoomEventRequest, ReactionKeyRequest, RemoveBookmarkedCustomReactionRequest,
    RemoveBookmarkedPostRequest, ResolveCommunityIndexPostsRequest, ResyncDomeSnapshotsRequest,
    RetryPostElementsRequest, RevokeDomeConnectionRequest, RotatePrivateChannelRequest,
    SendDirectMessageRequest, SetChannelGossipEnabledRequest, SetMyProfileRequest,
    SetPrivateChannelEntryDomeRequest, SetTopicGossipEnabledRequest, StartOwnerDomeHostingRequest,
    SubmitDomeSessionInputRequest, SwitchAccountRequest, ToggleReactionRequest,
    UnsubscribeTopicRequest, UpdateGameRoomRequest, UpdateMetaverseRoomRequest,
    WithdrawDomeConnectionProposalRequest, WithdrawPostRequest, WithdrawalReasonVisibilityRequest,
};
pub use runtime::{DesktopRuntime, RuntimeEvent};

/// Generation-bound owner deletion request.
pub use kukuri_app_api::DeleteDomeInput as DeleteDomeRequest;
