use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use kukuri_app_api::{
    AbortDomeTransitionInput, AcceptDomeConnectionProposalInput,
    ActivateCommunityNodeDomeHostingInput, AppService, AuthorSocialView, BlobMediaPayload,
    BookmarkedCustomReactionView, BookmarkedPostPageView, BookmarkedPostView,
    ChannelAccessTokenExport, ChannelAccessTokenPreview, CloseDomeHostingInput,
    CommitDomeLayoutInput, CommitDomeTransitionInput, CommunityIndexPostResolveResponse,
    CreateCustomReactionAssetInput, CreateDomeConnectionProposalInput, CreateGameRoomInput,
    CreateLiveSessionInput, CreateMetaverseRoomInput, CustomReactionAssetView,
    DirectMessageConversationView, DirectMessageStatusView, DirectMessageTimelineView,
    DirectMessageTopicStatusView, DomeHostingView, DomeLayoutCommitView, GameRoomView,
    ImportMetaverseRoomAssetInput, JoinedPrivateChannelView, LiveSessionView,
    MetaverseAssetRefView, MetaverseRoomEventView, MoveDomeInput, NotificationPageView,
    NotificationStatusView, NotificationView, PostView, PrepareCommunityNodeDomeHostingInput,
    PrepareDomeTransitionInput, PrivateChannelCapability, ProfileInput,
    PublishMetaverseRoomEventInput, ReactionStateView, RecentReactionView,
    ResyncDomeSnapshotsInput, RevokeDomeConnectionInput, ServiceHandles,
    StartOwnerDomeHostingInput, SubmitDomeSessionInput, SyncStatus, TimelineView,
    UpdateGameRoomInput, UpdateMetaverseRoomInput, WithdrawDomeConnectionProposalInput,
};
use kukuri_cn_protocol::{
    DomeHostingActivationRequest, DomeHostingAssetBlob, DomeHostingAssignmentRequest,
    DomeHostingLayoutCandidateRequest, DomeHostingReleaseRequest, DomeHostingSessionInputRequest,
    DomeHostingSnapshotResyncRequest, DomeTransitionAbortRequest, DomeTransitionCommitRequest,
    DomeTransitionPrepareRequest, normalize_http_url,
};
use kukuri_core::{
    BlobHash, CreatePrivateChannelInput, CustomReactionAssetSnapshotV1, DomeHostTargetV1,
    DomeInstanceManifestV1, DomePresetManifestV1, EnvelopeId, FriendOnlyGrantPreview,
    FriendPlusSharePreview, KukuriKeys, PrivateChannelInvitePreview, Profile,
    SignedDomeHostingActivationV1, SignedDomeHostingCloseV1, SignedDomeHostingLeaseV1,
    TimelineScope, TopicId, build_signed_dome_session_input, verify_signed_dome_host_heartbeat,
    verify_signed_dome_physics_snapshot,
};
use kukuri_docs_sync::{DocQuery, DocsSync};
use kukuri_store::SqliteStore;
use kukuri_transport::{DhtDiscoveryOptions, DiscoveryMode, TransportNetworkConfig};
use tokio::sync::Mutex;

use crate::attachments::{
    normalize_custom_reaction_upload, pending_attachment_from_request, reaction_key_from_request,
};
use crate::community_node::{
    AcceptCommunityNodeConsentsRequest, COMMUNITY_NODE_TOKEN_PURPOSE,
    CONTENT_ADVISORY_SYNTHESIS_DEFAULT, CommunityNodeConfig, CommunityNodeConsentPreflight,
    CommunityNodeIndexQueryError, CommunityNodeIndexQueryRequest, CommunityNodeIndexingRequest,
    CommunityNodeIndexingRequestError, CommunityNodeManifestFetch, CommunityNodeNodeConfig,
    CommunityNodeNodeStatus, CommunityNodeReconnectState, CommunityNodeRelationNeighborsRequest,
    CommunityNodeReportError, CommunityNodeSessionPhase, CommunityNodeSessionState,
    CommunityNodeTargetRequest, CommunityNodeTesterFeedbackError,
    CommunityNodeTesterFeedbackResponse, CommunityNodeTesterFeedbackSubmission,
    CommunityNodeTrustRelationError, CommunityNodeUserAdvisoryRequest,
    FetchCommunityNodePoliciesRequest, IndexOperation, IndexQueryResponse,
    RelationNeighborsResponse, RelationOptoutResponse, RelationReadResponse,
    SetCommunityNodeConfigRequest, SetCommunityNodeInviteCodeRequest,
    SubmitCommunityNodeReportRequest, SubmitCommunityNodeReportResult,
    SubmitIndexingRequestResponse, TrustUserReadResponse,
    community_node_local_consent_covers_status, community_node_seed_peers,
    delete_community_node_invite_code, effective_seed_peer_apply_state,
    load_community_node_config_from_file, load_community_node_local_consents,
    normalize_community_node_config, persist_community_node_invite_code,
    persist_community_node_local_consents, record_community_node_local_consents,
    relay_config_from_community_node_config, runtime_connectivity_assist_state,
    save_community_node_config,
};
use crate::discovery::{
    DiscoveryConfig, SetDiscoverySeedsRequest, parse_seed_entries,
    resolve_discovery_config_from_env, save_discovery_config,
};
use crate::host::{DesiredSubscription, DesiredSubscriptionScope};
use crate::identity::{
    IdentityStorageMode, delete_optional_secret, load_optional_secret, load_or_create_keys,
    persist_optional_secret,
};
use crate::requests::*;
use crate::stack::SharedIrohStack;

mod community_node_api;
mod content_profile_api;
mod identity_api;
mod notifications_messages_api;
mod private_channels_game_api;
mod sync_live_api;
mod sync_status_observer;

pub(crate) const PRIVATE_CHANNEL_CAPABILITIES_PURPOSE: &str = "private-channel-capabilities";
pub(crate) const PRIVATE_CHANNEL_CAPABILITIES_KEY: &str = "registry";
pub(crate) const GOSSIP_SUBSCRIPTION_STATE_PURPOSE: &str = "gossip-subscription-state";
pub(crate) const GOSSIP_SUBSCRIPTION_STATE_KEY: &str = "registry";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    NotificationStatusChanged,
    AdultMediaLabelEvicted {
        hash: Option<String>,
    },
    SyncStatusChanged {
        sync_status: Option<Box<SyncStatus>>,
        community_node_statuses: Option<Vec<CommunityNodeNodeStatus>>,
    },
}

pub struct DesktopRuntime {
    pub(crate) app_service: AppService,
    pub(crate) author_keys: Arc<KukuriKeys>,
    pub(crate) db_path: PathBuf,
    pub(crate) identity_mode: IdentityStorageMode,
    pub(crate) store: Arc<SqliteStore>,
    pub(crate) iroh_stack: SharedIrohStack,
    pub(crate) discovery_config: Arc<Mutex<DiscoveryConfig>>,
    pub(crate) community_node_config: Arc<Mutex<CommunityNodeConfig>>,
    pub(crate) community_node_sessions: Arc<Mutex<HashMap<String, CommunityNodeSessionState>>>,
    pub(crate) community_node_dome_heartbeats:
        Arc<Mutex<HashMap<String, kukuri_core::SignedDomeHostHeartbeatV1>>>,
    pub(crate) community_node_rendezvous_seed_peers:
        Arc<Mutex<HashMap<String, Vec<kukuri_transport::SeedPeer>>>>,
    pub(crate) community_node_session_guard: crate::community_node::SessionLocks,
    pub(crate) community_node_connectivity_guard: Mutex<()>,
    pub(crate) community_node_reconnect_state: Arc<Mutex<CommunityNodeReconnectState>>,
    pub(crate) community_node_reconnect_guard: Arc<Mutex<()>>,
    pub(crate) community_node_scheduler_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(crate) sync_status_observer_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Notification forwarding belongs to this account runtime, including Drop without shutdown.
    notification_event_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    pub(crate) active_connectivity_urls: Arc<Mutex<Vec<String>>>,
    pub(crate) last_runtime_connectivity_assist_state:
        Arc<Mutex<Option<crate::community_node::RuntimeConnectivityAssistState>>>,
    pub(crate) last_effective_seed_peer_apply_state:
        Arc<Mutex<Option<crate::community_node::EffectiveSeedPeerApplyState>>>,
    pub(crate) runtime_connectivity_apply_version: Arc<AtomicU64>,
    pub(crate) effective_seed_peer_apply_version: Arc<AtomicU64>,
    /// #1055: Community Node の content advisory を成人向けゲートへ合成するかどうか。
    /// ADR 0046 §6.4 の利用規約改訂と再同意(C4 = #1056)で既定 ON にした。
    pub(crate) content_advisory_synthesis_enabled: Arc<AtomicBool>,
    /// #1056: 一括照会で使う発行元(manifest `node_id`)の cache。node 設定の保存で破棄する。
    pub(crate) content_advisory_issuer_cache: Arc<Mutex<HashMap<String, String>>>,
    /// #1061: ブロック / ミュート観測の提供状態ファイルの読み書きを直列化する。
    pub(crate) trust_observation_guard: Arc<Mutex<()>>,
    pub(crate) private_index_grant_guard: Mutex<()>,
    /// #1061: 著者表示例外ファイルの読み書きを直列化する。
    pub(crate) trust_display_guard: Arc<Mutex<()>>,
    /// #1061: 採用 CN から採った評価の cache（key = (base_url, target)）。
    pub(crate) author_trust_gate_cache:
        Arc<Mutex<HashMap<(String, String), crate::community_node::CachedAuthorTrustEvaluation>>>,
    /// cache の世代。設定・同意・認証の変更で進め、古い応答を採らない。
    pub(crate) author_trust_gate_generation: Arc<AtomicU64>,
    event_sender: tokio::sync::broadcast::Sender<RuntimeEvent>,
}

fn load_private_channel_capabilities(
    db_path: &Path,
    mode: IdentityStorageMode,
) -> Result<Vec<PrivateChannelCapability>> {
    let Some(raw) = load_optional_secret(
        db_path,
        mode,
        PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
        PRIVATE_CHANNEL_CAPABILITIES_KEY,
    )?
    else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&raw).context("failed to decode private channel capabilities")
}

fn persist_private_channel_capabilities(
    db_path: &Path,
    mode: IdentityStorageMode,
    capabilities: &[PrivateChannelCapability],
) -> Result<()> {
    let encoded = serde_json::to_string(capabilities)
        .context("failed to encode private channel capabilities")?;
    persist_optional_secret(
        db_path,
        mode,
        PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
        PRIVATE_CHANNEL_CAPABILITIES_KEY,
        encoded.as_str(),
    )
}

/// #858: 成人向け表現の表示設定の永続形。`<db_path>.content-display.json` に保存する
/// (app-consent と同様、DB・identity storage から独立した平文ローカル設定)。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct ContentDisplaySettingsState {
    #[serde(default)]
    pub(crate) adult_content_enabled: bool,
}

const CONTENT_DISPLAY_SETTINGS_FILE_EXTENSION: &str = "content-display.json";

fn content_display_settings_path(db_path: &Path) -> PathBuf {
    db_path.with_extension(CONTENT_DISPLAY_SETTINGS_FILE_EXTENSION)
}

/// 欠落・破損は既定値(表示 OFF)として扱う(fail-closed)。
pub(crate) fn load_content_display_settings(db_path: &Path) -> ContentDisplaySettingsState {
    let Ok(bytes) = std::fs::read(content_display_settings_path(db_path)) else {
        return ContentDisplaySettingsState::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub(crate) fn save_content_display_settings(
    db_path: &Path,
    state: &ContentDisplaySettingsState,
) -> Result<()> {
    let path = content_display_settings_path(db_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("failed to create content display settings dir")?;
    }
    let bytes =
        serde_json::to_vec_pretty(state).context("failed to encode content display settings")?;
    std::fs::write(&path, bytes).context("failed to write content display settings")
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct GossipSubscriptionState {
    #[serde(default)]
    disabled_topics: Vec<String>,
    #[serde(default)]
    disabled_channels: Vec<String>,
}

fn load_gossip_subscription_state(
    db_path: &Path,
    mode: IdentityStorageMode,
) -> Result<GossipSubscriptionState> {
    let Some(raw) = load_optional_secret(
        db_path,
        mode,
        GOSSIP_SUBSCRIPTION_STATE_PURPOSE,
        GOSSIP_SUBSCRIPTION_STATE_KEY,
    )?
    else {
        return Ok(GossipSubscriptionState::default());
    };
    serde_json::from_str(&raw).context("failed to decode gossip subscription state")
}

pub(crate) fn validate_persisted_runtime_state(
    db_path: &Path,
    mode: IdentityStorageMode,
) -> Result<()> {
    for capability in load_private_channel_capabilities(db_path, mode)? {
        let current_epoch_id = if capability.current_epoch_id.trim().is_empty() {
            "legacy"
        } else {
            capability.current_epoch_id.as_str()
        };
        let current_secret = if capability.current_epoch_secret_hex.trim().is_empty() {
            capability.namespace_secret_hex.as_str()
        } else {
            capability.current_epoch_secret_hex.as_str()
        };
        validate_private_channel_namespace_secret(current_secret)?;
        let mut seen_epochs = std::collections::BTreeSet::from([current_epoch_id]);
        for epoch in &capability.archived_epochs {
            if seen_epochs.insert(epoch.epoch_id.as_str()) {
                validate_private_channel_namespace_secret(&epoch.namespace_secret_hex)?;
            }
        }
    }
    load_gossip_subscription_state(db_path, mode)?;
    Ok(())
}

fn validate_private_channel_namespace_secret(secret: &str) -> Result<()> {
    let secret = secret.trim();
    if secret.len() != 64 || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("private channel namespace secret must be 32-byte hex");
    }
    Ok(())
}

fn persist_gossip_subscription_state(
    db_path: &Path,
    mode: IdentityStorageMode,
    state: &GossipSubscriptionState,
) -> Result<()> {
    let encoded =
        serde_json::to_string(state).context("failed to encode gossip subscription state")?;
    persist_optional_secret(
        db_path,
        mode,
        GOSSIP_SUBSCRIPTION_STATE_PURPOSE,
        GOSSIP_SUBSCRIPTION_STATE_KEY,
        encoded.as_str(),
    )
}

impl DesktopRuntime {
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self> {
        Self::new_with_config_and_identity_and_discovery(
            db_path,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::from_env(),
            DiscoveryConfig::static_peer_default(),
            DhtDiscoveryOptions::disabled(),
            None,
        )
        .await
    }

    pub async fn new_with_config(
        db_path: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
    ) -> Result<Self> {
        Self::new_with_config_and_identity_and_discovery(
            db_path,
            network_config,
            IdentityStorageMode::from_env(),
            DiscoveryConfig::static_peer_default(),
            DhtDiscoveryOptions::disabled(),
            None,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn new_with_config_and_identity(
        db_path: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
        identity_mode: IdentityStorageMode,
    ) -> Result<Self> {
        Self::new_with_config_and_identity_and_discovery(
            db_path,
            network_config,
            identity_mode,
            DiscoveryConfig::static_peer_default(),
            DhtDiscoveryOptions::disabled(),
            None,
        )
        .await
    }

    pub(crate) async fn new_with_config_and_identity_and_discovery(
        db_path: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
        identity_mode: IdentityStorageMode,
        discovery_config: DiscoveryConfig,
        dht_options: DhtDiscoveryOptions,
        initial_community_node_config: Option<CommunityNodeConfig>,
    ) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        let community_node_config = match load_community_node_config_from_file(&db_path)? {
            Some(config) => config,
            None if initial_community_node_config.is_some() => {
                let config = normalize_community_node_config(
                    initial_community_node_config.expect("checked as present"),
                )?;
                save_community_node_config(&db_path, &config)?;
                config
            }
            None => CommunityNodeConfig::default(),
        };
        // 保存済みの Node relay / seed は、公開 policy とサーバ同意を確認する前の
        // 起動入力へ渡さない。設定と resolved_urls は保持し、scheduler 成功後に再適用する。
        let active_community_node_config = CommunityNodeConfig::default();
        let relay_config = relay_config_from_community_node_config(&active_community_node_config);
        let community_node_seed_peers =
            community_node_seed_peers(&active_community_node_config).collect::<Vec<_>>();
        let initial_runtime_connectivity_state =
            runtime_connectivity_assist_state(&discovery_config, &active_community_node_config);
        let initial_effective_seed_peer_state =
            effective_seed_peer_apply_state(&discovery_config, &active_community_node_config);
        let docs_root = db_path.with_extension("iroh-data");
        let store = Arc::new(SqliteStore::connect_file(&db_path).await?);
        let iroh_stack = SharedIrohStack::new(
            &docs_root,
            network_config.clone(),
            &discovery_config,
            &community_node_seed_peers,
            dht_options,
            relay_config.clone(),
            Some(store.clone()),
        )
        .await?;
        let keys = load_or_create_keys(&db_path, identity_mode)?;
        // docs へ何かを書く前に、書き込みの名義をアカウントの docs author にする(ADR 0053 §1)。
        iroh_stack
            .use_account_docs_author(keys.derive_docs_author_seed())
            .await?;
        iroh_stack
            .use_account_receive_binding(Arc::new(keys.clone()))
            .await?;
        let author_keys = Arc::new(keys.clone());
        let services = ServiceHandles::new(
            store.clone(),
            store.clone(),
            iroh_stack.transport.clone(),
            iroh_stack.transport.clone(),
            iroh_stack.docs_sync.clone(),
            iroh_stack.blob_service.clone(),
            keys,
        );
        let metaverse_budget = kukuri_core::MetaverseResourceBudgetConfig::from_json_override(
            std::env::var("KUKURI_METAVERSE_RESOURCE_BUDGET_JSON")
                .ok()
                .as_deref(),
        )?;
        let app_service =
            AppService::from_handles_with_metaverse_budget(services, metaverse_budget)?;
        for capability in load_private_channel_capabilities(&db_path, identity_mode)? {
            app_service
                .restore_private_channel_capability(capability)
                .await?;
        }
        // 復元完了後に write-through 永続化を接続する(復元前に接続すると復元途中の
        // 部分リストが persist され、途中クラッシュでディスク上の registry が縮む)。
        // 以後、capability registry の変異(register/remove 経由の全経路)は AppService
        // 層でそのまま identity storage へ永続化され、ラッパー側の手動 persist 規約は不要。
        {
            let persist_db_path = db_path.clone();
            app_service.set_private_channel_capability_persist(Arc::new(move |capabilities| {
                persist_private_channel_capabilities(&persist_db_path, identity_mode, capabilities)
            }));
        }
        let gossip_subscription_state = load_gossip_subscription_state(&db_path, identity_mode)?;
        app_service
            .restore_gossip_disabled_state(
                gossip_subscription_state.disabled_topics,
                gossip_subscription_state.disabled_channels,
            )
            .await;
        // #858: 成人向け表現の表示設定(既定 OFF)を起動時に反映する。設定が読めない
        // 場合も OFF のまま(fail-closed)。
        app_service.set_adult_content_display_enabled(
            load_content_display_settings(&db_path).adult_content_enabled,
        );
        app_service.warm_social_graph().await?;
        if let Err(error) = app_service.start_account_receive_offers().await {
            tracing::warn!(%error, "account receive route could not start; legacy receivers remain active");
        }
        app_service.resume_direct_message_state().await?;

        let (event_sender, _) = tokio::sync::broadcast::channel(64);
        let notification_event_task = {
            let notify = app_service.notification_inserted_notify();
            let mut label_evictions = store.subscribe_adult_label_evictions();
            let sender = event_sender.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = notify.notified() => {
                            let _ = sender.send(RuntimeEvent::NotificationStatusChanged);
                        }
                        label = label_evictions.recv() => match label {
                            Ok(hash) => {
                                let _ = sender.send(RuntimeEvent::AdultMediaLabelEvicted { hash: Some(hash) });
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                let _ = sender.send(RuntimeEvent::AdultMediaLabelEvicted { hash: None });
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        },
                    }
                }
            })
        };

        Ok(Self {
            app_service,
            author_keys,
            db_path,
            identity_mode,
            store,
            iroh_stack,
            discovery_config: Arc::new(Mutex::new(discovery_config)),
            community_node_config: Arc::new(Mutex::new(community_node_config)),
            community_node_sessions: Arc::new(Mutex::new(HashMap::new())),
            community_node_dome_heartbeats: Arc::new(Mutex::new(HashMap::new())),
            community_node_rendezvous_seed_peers: Arc::new(Mutex::new(HashMap::new())),
            community_node_session_guard: Default::default(),
            community_node_connectivity_guard: Mutex::new(()),
            community_node_reconnect_state: Arc::new(Mutex::new(
                CommunityNodeReconnectState::default(),
            )),
            community_node_reconnect_guard: Arc::new(Mutex::new(())),
            community_node_scheduler_task: Mutex::new(None),
            sync_status_observer_task: Mutex::new(None),
            notification_event_task: StdMutex::new(Some(notification_event_task)),
            active_connectivity_urls: Arc::new(Mutex::new(relay_config.iroh_relay_urls.clone())),
            last_runtime_connectivity_assist_state: Arc::new(Mutex::new(Some(
                initial_runtime_connectivity_state,
            ))),
            last_effective_seed_peer_apply_state: Arc::new(Mutex::new(Some(
                initial_effective_seed_peer_state,
            ))),
            runtime_connectivity_apply_version: Arc::new(AtomicU64::new(0)),
            effective_seed_peer_apply_version: Arc::new(AtomicU64::new(0)),
            content_advisory_synthesis_enabled: Arc::new(AtomicBool::new(
                CONTENT_ADVISORY_SYNTHESIS_DEFAULT,
            )),
            content_advisory_issuer_cache: Arc::new(Mutex::new(HashMap::new())),
            trust_observation_guard: Arc::new(Mutex::new(())),
            private_index_grant_guard: Mutex::new(()),
            trust_display_guard: Arc::new(Mutex::new(())),
            author_trust_gate_cache: Arc::new(Mutex::new(HashMap::new())),
            author_trust_gate_generation: Arc::new(AtomicU64::new(0)),
            event_sender,
        })
    }

    pub async fn from_env(
        db_path: impl AsRef<Path>,
        initial_community_node_config: CommunityNodeConfig,
    ) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        let discovery_config = resolve_discovery_config_from_env(&db_path)?;
        let dht_options = match discovery_config.mode {
            DiscoveryMode::SeededDht => DhtDiscoveryOptions::seeded_dht(),
            DiscoveryMode::StaticPeer => DhtDiscoveryOptions::disabled(),
        };
        Self::new_with_config_and_identity_and_discovery(
            &db_path,
            TransportNetworkConfig::from_env()?,
            IdentityStorageMode::from_env(),
            discovery_config,
            dht_options,
            Some(initial_community_node_config),
        )
        .await
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<RuntimeEvent> {
        self.event_sender.subscribe()
    }

    pub(crate) fn emit_event(&self, event: RuntimeEvent) {
        let _ = self.event_sender.send(event);
    }

    pub(crate) fn take_notification_event_task(&self) -> Option<tokio::task::JoinHandle<()>> {
        self.notification_event_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub(crate) async fn persist_gossip_subscription_state_from_app(&self) -> Result<()> {
        persist_gossip_subscription_state(
            &self.db_path,
            self.identity_mode,
            &GossipSubscriptionState {
                disabled_topics: self.app_service.list_gossip_disabled_topics().await,
                disabled_channels: self.app_service.list_gossip_disabled_channels().await,
            },
        )
    }
}

impl Drop for DesktopRuntime {
    fn drop(&mut self) {
        if let Some(task) = self
            .notification_event_task
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}
