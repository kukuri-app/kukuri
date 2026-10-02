mod artifacts;
mod runtime;
mod scenario;
mod scenarios;
mod waiters;

#[cfg(test)]
mod tests;

pub use artifacts::{HarnessResult, HarnessStatus, StepResult, summarize_metrics};
pub use scenario::{
    ScenarioArtifacts, ScenarioFixtures, ScenarioKind, ScenarioScoreUpdate, ScenarioSpec,
    ScenarioStep, ScenarioTimeouts, load_scenario,
};
pub use scenarios::{run_named_scenario, run_scenario};

pub(crate) use artifacts::{push_named_step, write_result_artifact};
pub(crate) use runtime::{
    CommunityNodeStack, ScenarioRuntime, cleanup_runtime_artifacts, persist_runtime_identity,
    shutdown_runtime,
};
pub(crate) use waiters::*;

pub(crate) use std::collections::BTreeMap;
pub(crate) use std::net::SocketAddr;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::Arc;
pub(crate) use std::time::{Duration, Instant};

pub(crate) use anyhow::{Context, Result};
pub(crate) use base64::Engine;
pub(crate) use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
pub(crate) use kukuri_app_api::{
    AcceptDomeConnectionProposalInput, ActivateCommunityNodeDomeHostingInput, AppService,
    CloseDomeHostingInput, CommitDomeLayoutInput, CommitDomeTransitionInput,
    CreateDomeConnectionProposalInput, CreateGameRoomInput, CreateLiveSessionInput,
    CreateMetaverseRoomInput, DirectMessageConversationView, DirectMessageMessageView,
    DirectMessageStatusView, DomeLayoutCommitOutcome, GameScoreView, MoveDomeInput,
    PrepareCommunityNodeDomeHostingInput, PrepareDomeTransitionInput, RevokeDomeConnectionInput,
    ServiceHandles, StartOwnerDomeHostingInput, SubmitDomeSessionInput, SyncStatus,
    UpdateGameRoomInput, UpdateMetaverseRoomInput,
};
pub(crate) use kukuri_blob_service::MemoryBlobService;
pub(crate) use kukuri_cn_core::{JwtConfig, TestDatabase};
pub(crate) use kukuri_cn_iroh_relay::{IrohRelayConfig, SpawnedIrohRelay};
pub(crate) use kukuri_cn_user_api::{
    UserApiConfig, app_router as user_api_app_router, build_state as build_user_api_state,
};
pub(crate) use kukuri_core::{
    ChannelAudienceKind, ChannelId, ChannelRef, CreatePrivateChannelInput, DomeDirection,
    DomeHostingStateKindV1, DomeMaterialPreset, DomeSessionInputKindV1,
    DomeTransitionAdmissionRequestV1, GameRoomStatus, KukuriKeys, SignedDomeHostingLeaseV1,
    SpatialContextV1, TimelineScope, TopicId, accept_dome_hosting_lease, generate_keys,
};
pub(crate) use kukuri_desktop_runtime::{
    AcceptCommunityNodeConsentsRequest, AuthorRequest, CommunityNodeTargetRequest,
    CreateAttachmentRequest, CreateGameRoomRequest, CreateLiveSessionRequest, CreatePostRequest,
    CreatePrivateChannelRequest, CreateRepostRequest, DeleteDirectMessageMessageRequest,
    DesktopRuntime, DirectMessageRequest, ExportPrivateChannelInviteRequest, GetBlobMediaRequest,
    ImportPeerTicketRequest, ImportPrivateChannelInviteRequest, ListDirectMessageMessagesRequest,
    ListGameRoomsRequest, ListJoinedPrivateChannelsRequest, ListLiveSessionsRequest,
    ListThreadRequest, ListTimelineRequest, LiveSessionCommandRequest, ReactionKeyRequest,
    SendDirectMessageRequest, SetCommunityNodeConfigRequest, SetMyProfileRequest,
    SubmitCommunityNodeReportRequest, ToggleReactionRequest,
};
pub(crate) use kukuri_docs_sync::MemoryDocsSync;
pub(crate) use kukuri_store::{MemoryStore, SqliteStore};
pub(crate) use kukuri_transport::{
    ConnectMode, FakeNetwork, FakeTransport, TransportNetworkConfig,
};
pub(crate) use serde::{Deserialize, Serialize};
pub(crate) use tokio::time::{sleep, timeout};
