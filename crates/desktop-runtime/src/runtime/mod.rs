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
    DomeHostingView, DomeLayoutCommitView, GameRoomView, ImportMetaverseRoomAssetInput,
    JoinedPrivateChannelPage, JoinedPrivateChannelView, LiveSessionView, MetaverseAssetRefView,
    MetaverseRoomEventView, MoveDomeInput, NotificationPageView, NotificationStatusView,
    NotificationView, PostView, PrepareCommunityNodeDomeHostingInput, PrepareDomeTransitionInput,
    PrivateChannelCapability, ProfileInput, PublishMetaverseRoomEventInput, ReactionStateView,
    RecentReactionView, ResyncDomeSnapshotsInput, RevokeDomeConnectionInput, ServiceHandles,
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
#[cfg(not(target_family = "wasm"))]
use kukuri_store::SqliteStore;
use kukuri_transport::{DhtDiscoveryOptions, TransportNetworkConfig, TransportRelayConfig};
use tokio::sync::Mutex;

use crate::attachments::{
    normalize_custom_reaction_upload, pending_attachment_from_request, reaction_key_from_request,
};
use crate::community_node::{
    AcceptCommunityNodeConsentsRequest, COMMUNITY_NODE_TOKEN_PURPOSE,
    CONTENT_ADVISORY_SYNTHESIS_DEFAULT, CommunityNodeConfig, CommunityNodeConsentPreflight,
    CommunityNodeIndexQueryError, CommunityNodeIndexQueryRequest, CommunityNodeIndexingRequest,
    CommunityNodeIndexingRequestError, CommunityNodeManifestFetch, CommunityNodeNodeConfig,
    CommunityNodeNodeStatus, CommunityNodeRelationNeighborsRequest, CommunityNodeReportError,
    CommunityNodeSessionPhase, CommunityNodeSessionState, CommunityNodeTargetRequest,
    CommunityNodeTesterFeedbackError, CommunityNodeTesterFeedbackResponse,
    CommunityNodeTesterFeedbackSubmission, CommunityNodeTrustRelationError,
    CommunityNodeUserAdvisoryRequest, FetchCommunityNodePoliciesRequest, IndexOperation,
    IndexQueryResponse, RelationNeighborsResponse, RelationOptoutResponse, RelationReadResponse,
    SetCommunityNodeConfigRequest, SetCommunityNodeInviteCodeRequest,
    SubmitCommunityNodeReportRequest, SubmitCommunityNodeReportResult,
    SubmitIndexingRequestResponse, TrustUserReadResponse,
    community_node_local_consent_covers_status, delete_community_node_invite_code,
    load_community_node_config_from_file, load_community_node_local_consents,
    normalize_community_node_config, persist_community_node_invite_code,
    persist_community_node_local_consents, record_community_node_local_consents,
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
#[cfg(not(target_family = "wasm"))]
mod legacy_store_retirement;
#[cfg(test)]
pub(crate) use legacy_store_retirement::LegacyStoreProgress;
mod notifications_messages_api;
mod private_channels_game_api;
#[cfg(not(target_family = "wasm"))]
mod protected_migration;
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
    /// 通信状態の差分(#1221 R2-D)。`sync_status` は件数と、変わった topic だけの `topic_diagnostics`。
    /// CN の node は変わった node だけ。抜けた topic と外した node は `removed_*`。
    SyncStatusChanged {
        sync_status: Option<Box<SyncStatus>>,
        removed_topics: Vec<String>,
        community_node_statuses: Vec<CommunityNodeNodeStatus>,
        removed_community_nodes: Vec<String>,
    },
}

pub struct DesktopRuntime {
    pub(crate) app_service: AppService,
    pub(crate) author_keys: Arc<KukuriKeys>,
    pub(crate) db_path: PathBuf,
    pub(crate) identity_mode: IdentityStorageMode,
    pub(crate) store: Arc<dyn kukuri_store::AccountStore>,
    /// 同じ store の SQLite の型。旧 store の退役・保護データの移行・停止の close など、native だけの処理が使う。
    #[cfg(not(target_family = "wasm"))]
    pub(crate) sqlite: Arc<SqliteStore>,
    pub(crate) iroh_stack: SharedIrohStack,
    pub(crate) discovery_config: Arc<Mutex<DiscoveryConfig>>,
    pub(crate) community_node_config: Arc<Mutex<CommunityNodeConfig>>,
    pub(crate) community_node_sessions: Arc<Mutex<crate::community_node::CommunityNodeSessions>>,
    pub(crate) community_node_dome_heartbeats:
        Arc<Mutex<HashMap<String, kukuri_core::SignedDomeHostHeartbeatV1>>>,
    pub(crate) community_node_session_guard: crate::community_node::SessionLocks,
    /// node ごとの relay と seed、適用した和(#1221 R2-B)。適用はこの lock の中で直列にする。
    pub(crate) community_node_connectivity: Mutex<crate::community_node::AppliedConnectivity>,
    pub(crate) community_node_scheduler_task: Mutex<Option<n0_future::task::JoinHandle<()>>>,
    pub(crate) sync_status_observer_task: Mutex<Option<n0_future::task::JoinHandle<()>>>,
    /// 計測用: 差分を作った回数(#1221 R2-D)。
    #[cfg(test)]
    pub(crate) sync_status_delta_reads: std::sync::atomic::AtomicUsize,
    /// #1221 R5-G・R5-I: 旧 `iroh-data` の保護移行と退役の背景 task。backup 前の drain と直列にする。
    #[cfg(not(target_family = "wasm"))]
    pub(crate) legacy_store_task: Mutex<Option<n0_future::task::JoinHandle<()>>>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) protected_migration_guard: Mutex<()>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) private_migration_dirty: Arc<AtomicBool>,
    /// #1221 R5-I: 読むだけに開いた旧 iroh store。退役させたら(または初めから無ければ)`None`。
    #[cfg(not(target_family = "wasm"))]
    pub(crate) legacy_store: Mutex<Option<Arc<kukuri_iroh_node::LegacyStore>>>,
    /// Notification forwarding belongs to this account runtime, including Drop without shutdown.
    notification_event_task: StdMutex<Option<n0_future::task::JoinHandle<()>>>,
    /// #1055: Community Node の content advisory を成人向けゲートへ合成するかどうか。
    /// ADR 0046 §6.4 の利用規約改訂と再同意(C4 = #1056)で既定 ON にした。
    pub(crate) content_advisory_synthesis_enabled: Arc<AtomicBool>,
    /// #1056: 一括照会で使う発行元(manifest `node_id`)の cache。node 設定の保存で破棄する。
    pub(crate) content_advisory_issuer_cache: Arc<Mutex<HashMap<String, String>>>,
    /// #1061: ブロック / ミュート観測の提供状態ファイルの読み書きを直列化する。
    pub(crate) trust_observation_guard: Arc<Mutex<()>>,
    pub(crate) private_index_grant_guard: Mutex<()>,
    /// #1061: 採用 CN から採った評価の cache（key = (base_url, target)）。
    pub(crate) author_trust_gate_cache:
        Arc<Mutex<HashMap<(String, String), crate::community_node::CachedAuthorTrustEvaluation>>>,
    /// cache の世代。設定・同意・認証の変更で進め、古い応答を採らない。
    pub(crate) author_trust_gate_generation: Arc<AtomicU64>,
    event_sender: tokio::sync::broadcast::Sender<RuntimeEvent>,
}

async fn load_private_channel_capabilities(
    db_path: &Path,
    mode: IdentityStorageMode,
) -> Result<Vec<PrivateChannelCapability>> {
    let Some(raw) = load_optional_secret(
        db_path,
        mode,
        PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
        PRIVATE_CHANNEL_CAPABILITIES_KEY,
    )
    .await?
    else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&raw).context("failed to decode private channel capabilities")
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
pub(crate) async fn load_content_display_settings(db_path: &Path) -> ContentDisplaySettingsState {
    let Ok(Some(bytes)) = crate::storage::read_file(&content_display_settings_path(db_path)).await
    else {
        return ContentDisplaySettingsState::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub(crate) async fn save_content_display_settings(
    db_path: &Path,
    state: &ContentDisplaySettingsState,
) -> Result<()> {
    let bytes =
        serde_json::to_vec_pretty(state).context("failed to encode content display settings")?;
    crate::storage::write_file(&content_display_settings_path(db_path), &bytes)
        .await
        .context("failed to write content display settings")
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct GossipSubscriptionState {
    #[serde(default)]
    disabled_topics: Vec<String>,
    #[serde(default)]
    disabled_channels: Vec<String>,
}

async fn load_gossip_subscription_state(
    db_path: &Path,
    mode: IdentityStorageMode,
) -> Result<GossipSubscriptionState> {
    let Some(raw) = load_optional_secret(
        db_path,
        mode,
        GOSSIP_SUBSCRIPTION_STATE_PURPOSE,
        GOSSIP_SUBSCRIPTION_STATE_KEY,
    )
    .await?
    else {
        return Ok(GossipSubscriptionState::default());
    };
    serde_json::from_str(&raw).context("failed to decode gossip subscription state")
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn validate_persisted_runtime_state(
    db_path: &Path,
    mode: IdentityStorageMode,
) -> Result<()> {
    for capability in load_private_channel_capabilities(db_path, mode).await? {
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
    load_gossip_subscription_state(db_path, mode).await?;
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
fn validate_private_channel_namespace_secret(secret: &str) -> Result<()> {
    let secret = secret.trim();
    if secret.len() != 64 || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("private channel namespace secret must be 32-byte hex");
    }
    Ok(())
}

async fn persist_gossip_subscription_state(
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
    .await
}

/// 保存した Community Node の設定。無ければ `initial` を正規化して保存する（初回の起動）。
async fn community_node_config_or_initial(
    db_path: &Path,
    initial: Option<CommunityNodeConfig>,
) -> Result<CommunityNodeConfig> {
    if let Some(config) = load_community_node_config_from_file(db_path).await? {
        return Ok(config);
    }
    let Some(initial) = initial else {
        return Ok(CommunityNodeConfig::default());
    };
    let config = normalize_community_node_config(initial)?;
    save_community_node_config(db_path, &config).await?;
    Ok(config)
}

// runtime の構築（永続の node・SQLite・旧 store）は native だけ。Web の構築は web-runtime と一緒に足す（W1 AC-5）。
impl DesktopRuntime {
    #[cfg(not(target_family = "wasm"))]
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

    #[cfg(not(target_family = "wasm"))]
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

    #[cfg(not(target_family = "wasm"))]
    pub(crate) async fn new_with_config_and_identity_and_discovery(
        db_path: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
        identity_mode: IdentityStorageMode,
        discovery_config: DiscoveryConfig,
        dht_options: DhtDiscoveryOptions,
        initial_community_node_config: Option<CommunityNodeConfig>,
    ) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        let community_node_config =
            community_node_config_or_initial(&db_path, initial_community_node_config).await?;
        // 保存済みの Node relay / seed は、公開 policy とサーバ同意を確認する前の
        // 起動入力へ渡さない。設定と resolved_urls は保持し、node ごとに確かめてから足す。
        // #1221 R5-I: node は新しい store で動く。旧 `iroh-data` の endpoint secret を写して endpoint ID を保つ。
        let docs_root = db_path.with_extension("iroh-store");
        kukuri_iroh_node::adopt_endpoint_secret(&db_path.with_extension("iroh-data"), &docs_root)?;
        let store = Arc::new(SqliteStore::connect_file(&db_path).await?);
        // #1221 R5-I: 旧 store は読むだけの別の instance として開く。中身の無い旧 root は退役の手順へ回す。
        let legacy_root = db_path.with_extension("iroh-data");
        let legacy_store = kukuri_iroh_node::LegacyStore::open(&legacy_root).await?;
        if legacy_store.is_none() && legacy_root.exists() {
            std::fs::rename(&legacy_root, db_path.with_extension("iroh-data.retiring"))?;
        }
        let iroh_stack = SharedIrohStack::new(
            &docs_root,
            network_config.clone(),
            &discovery_config,
            &[],
            dht_options,
            TransportRelayConfig::default(),
            Some(store.clone()),
        )
        .await?;
        let writer_switched_at = store.writer_switched_at().await?;
        Self::assemble(
            db_path,
            identity_mode,
            store.clone(),
            store,
            legacy_store,
            iroh_stack,
            discovery_config,
            community_node_config,
            writer_switched_at,
        )
        .await
    }

    /// Web の runtime（ADR 0056 §2・§8）。保存先は呼び出し元（web-runtime）が開いた account の store。node はメモリの
    /// store で、保存した endpoint の秘密鍵（無ければ作って保存する）と WebRTC の transport を渡して開く。DHT は使えない。
    /// 旧 store は無いので、新形式の writer で始める。
    pub async fn open_in_memory_node<S>(
        db_path: PathBuf,
        store: Arc<S>,
        webrtc: Option<Arc<kukuri_webrtc_transport::WebRtcTransport>>,
        initial_community_node_config: CommunityNodeConfig,
    ) -> Result<Self>
    where
        S: kukuri_store::AccountStore + crate::stack::StackStore + 'static,
    {
        let storage = crate::storage::platform_storage();
        let secret_key = match crate::identity::load_endpoint_secret(storage, &db_path).await? {
            Some(secret) => iroh::SecretKey::from_bytes(&secret),
            None => {
                let secret_key = iroh::SecretKey::generate();
                crate::identity::save_endpoint_secret(storage, &db_path, &secret_key.to_bytes())
                    .await?;
                secret_key
            }
        };
        let community_node_config =
            community_node_config_or_initial(&db_path, Some(initial_community_node_config)).await?;
        let discovery_config = resolve_discovery_config_from_env(&db_path).await?;
        let iroh_stack = SharedIrohStack::open(
            crate::stack::NodeSource::Memory {
                secret_key: Box::new(secret_key),
                webrtc,
            },
            TransportNetworkConfig::default(),
            &discovery_config,
            &[],
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
            store.clone(),
        )
        .await?;
        Self::assemble(
            db_path,
            IdentityStorageMode::from_env(),
            store,
            #[cfg(not(target_family = "wasm"))]
            Arc::new(SqliteStore::connect_memory().await?),
            #[cfg(not(target_family = "wasm"))]
            None,
            iroh_stack,
            discovery_config,
            community_node_config,
            Some(chrono::Utc::now().timestamp_millis()),
        )
        .await
    }

    /// native と Web の起動の共通部分: 鍵、docs author、AppService の再開、通知の event の転送。
    #[cfg_attr(
        not(target_family = "wasm"),
        expect(
            clippy::too_many_arguments,
            reason = "native だけの保存先（SQLite・旧 store）を cfg の引数で渡すため"
        )
    )]
    async fn assemble(
        db_path: PathBuf,
        identity_mode: IdentityStorageMode,
        store: Arc<dyn kukuri_store::AccountStore>,
        #[cfg(not(target_family = "wasm"))] sqlite: Arc<SqliteStore>,
        #[cfg(not(target_family = "wasm"))] legacy_store: Option<kukuri_iroh_node::LegacyStore>,
        iroh_stack: SharedIrohStack,
        discovery_config: DiscoveryConfig,
        community_node_config: CommunityNodeConfig,
        writer_switched_at: Option<i64>,
    ) -> Result<Self> {
        let keys = load_or_create_keys(&db_path, identity_mode).await?;
        // docs へ何かを書く前に、書き込みの名義をアカウントの docs author にする(ADR 0053 §1)。
        iroh_stack
            .use_account_docs_author(keys.derive_docs_author_seed(), keys.public_key_hex())
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
        let status_changes = iroh_stack.status_changes.clone();
        let app_service =
            AppService::from_handles_with_metaverse_budget(services, metaverse_budget)?
                .with_status_changes(status_changes.clone());
        // #1221 R5-H: 保存済みの切替状態は、最初の書込みより前に渡す(再起動で旧 writer へ戻らない)。
        if let Some(switched_at) = writer_switched_at {
            app_service.switch_writer(switched_at);
        }
        // 取り下げの書込みの outbox を、積んだ時の宛先へ再開する。
        if let Err(error) = app_service.resume_withdrawal_writes().await {
            tracing::warn!(%error, "queued withdrawal writes stay pending");
        }
        // 本人の端末間の同期（ADR 0061）。private channel の復元より前に scope の枠を取る。
        app_service.start_account_sync().await?;
        // 表示設定にすぎないので、旧版の file を取り込めなくても起動を止めない（次の起動で取り込み直す）。
        if let Err(error) =
            crate::community_node::import_legacy_trust_display(&db_path, &app_service).await
        {
            tracing::warn!(%error, "legacy trust display exceptions were not imported");
        }
        // ADR 0061 §9: 旧 registry(全件の 1 つの JSON)があれば、1 回だけ参加の行と世代の鍵の行へ移して消す。
        // 行の書き込みは冪等なので、途中で止まっても次の起動でやり直す。
        let legacy_capabilities =
            load_private_channel_capabilities(&db_path, identity_mode).await?;
        if !legacy_capabilities.is_empty() {
            for capability in legacy_capabilities {
                app_service
                    .restore_private_channel_capability(capability)
                    .await?;
            }
            delete_optional_secret(
                &db_path,
                identity_mode,
                PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
                PRIVATE_CHANNEL_CAPABILITIES_KEY,
            )
            .await?;
        }
        app_service.restore_joined_private_channels().await?;
        // #1221 R5-G: 参加状態は索引の順に並ばないため、起動時と変更時に現 epoch の記録の移行を先頭から読み直す。
        #[cfg(not(target_family = "wasm"))]
        let private_migration_dirty = app_service.private_channel_rows_changed();
        let gossip_subscription_state =
            load_gossip_subscription_state(&db_path, identity_mode).await?;
        app_service
            .restore_gossip_disabled_state(
                gossip_subscription_state.disabled_topics,
                gossip_subscription_state.disabled_channels,
            )
            .await;
        // #858: 成人向け表現の表示設定(既定 OFF)を起動時に反映する。設定が読めない
        // 場合も OFF のまま(fail-closed)。
        app_service.set_adult_content_display_enabled(
            load_content_display_settings(&db_path)
                .await
                .adult_content_enabled,
        );
        app_service
            .reconcile_blocked_dome_connections_at_start()
            .await?;
        if let Err(error) = app_service.start_account_receive_offers().await {
            tracing::warn!(%error, "account receive route could not start; legacy receivers remain active");
        }
        app_service.resume_direct_message_state().await?;

        let (event_sender, _) = tokio::sync::broadcast::channel(64);
        let notification_event_task = {
            let notify = app_service.notification_inserted_notify();
            let mut label_evictions = store.subscribe_adult_label_evictions();
            let sender = event_sender.clone();
            n0_future::task::spawn(async move {
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
            #[cfg(not(target_family = "wasm"))]
            sqlite,
            iroh_stack,
            discovery_config: Arc::new(Mutex::new(discovery_config)),
            community_node_config: Arc::new(Mutex::new(community_node_config)),
            community_node_sessions: Arc::new(Mutex::new(
                crate::community_node::CommunityNodeSessions::new(status_changes),
            )),
            community_node_dome_heartbeats: Arc::new(Mutex::new(HashMap::new())),
            community_node_session_guard: Default::default(),
            community_node_connectivity: Mutex::default(),
            community_node_scheduler_task: Mutex::new(None),
            sync_status_observer_task: Mutex::new(None),
            #[cfg(test)]
            sync_status_delta_reads: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(not(target_family = "wasm"))]
            legacy_store_task: Mutex::new(None),
            #[cfg(not(target_family = "wasm"))]
            protected_migration_guard: Mutex::new(()),
            #[cfg(not(target_family = "wasm"))]
            private_migration_dirty,
            #[cfg(not(target_family = "wasm"))]
            legacy_store: Mutex::new(legacy_store.map(Arc::new)),
            notification_event_task: StdMutex::new(Some(notification_event_task)),
            content_advisory_synthesis_enabled: Arc::new(AtomicBool::new(
                CONTENT_ADVISORY_SYNTHESIS_DEFAULT,
            )),
            content_advisory_issuer_cache: Arc::new(Mutex::new(HashMap::new())),
            trust_observation_guard: Arc::new(Mutex::new(())),
            private_index_grant_guard: Mutex::new(()),
            author_trust_gate_cache: Arc::new(Mutex::new(HashMap::new())),
            author_trust_gate_generation: Arc::new(AtomicU64::new(0)),
            event_sender,
        })
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn from_env(
        db_path: impl AsRef<Path>,
        initial_community_node_config: CommunityNodeConfig,
    ) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        let discovery_config = resolve_discovery_config_from_env(&db_path).await?;
        let dht_options = match discovery_config.mode {
            kukuri_transport::DiscoveryMode::SeededDht => DhtDiscoveryOptions::seeded_dht(),
            kukuri_transport::DiscoveryMode::StaticPeer => DhtDiscoveryOptions::disabled(),
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

    pub(crate) fn take_notification_event_task(&self) -> Option<n0_future::task::JoinHandle<()>> {
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
        .await
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
