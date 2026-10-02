//! command の dispatch 表と error の封筒（ADR 0056 §6、#1214 W1 AC-5）。
//!
//! `DesktopRuntime` の method へ委譲する command を、名前 → 引数 → method の 1 つの表にする。Tauri の invoke handler
//! と web-runtime の `invoke` がこの表を使う。引数は Tauri と同じ形（top-level の key は camelCase、中身は DTO）。
//! 結果は、呼ぶ前と後で host の世代が同じときだけ返す（旧世代の結果を返さない。ADR 0056 §7）。
//! 起動の状態とアカウントの操作は、platform の状態（[`ClientGate`]）を受ける別の表で、排他と起動の状態の遷移を行う。

use std::future::Future;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::*;

/// command の構造化エラー封筒（WP-C3）。invoke 側には `{"code":"...","message":"..."}` の JSON で届く。
#[derive(Clone, Debug, Serialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

pub const COMMAND_FAILED_CODE: &str = "command_failed";
pub const SCOPE_LIMIT_REACHED_CODE: &str = "SCOPE_LIMIT_REACHED";
pub const PRIVATE_CHANNEL_CONTROLLER_PENDING_CODE: &str = "PRIVATE_CHANNEL_CONTROLLER_PENDING";
/// 呼んでいる間に runtime が差し替わった・止まった（旧世代の結果は返さない）。
pub const STALE_RUNTIME_CODE: &str = "stale_runtime";
/// この platform では使えない command（Web の capability。ADR 0056 §6）。
pub const UNSUPPORTED_PLATFORM_CODE: &str = "unsupported_platform";

impl CommandError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            status: None,
            retry_after_seconds: None,
        }
    }
}

impl From<anyhow::Error> for CommandError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(COMMAND_FAILED_CODE, format!("{error:#}"))
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::new(COMMAND_FAILED_CODE, message)
    }
}

macro_rules! typed_error {
    ($($ty:ty => |$error:ident| $retry:expr),* $(,)?) => {
        $(impl From<$ty> for CommandError {
            fn from($error: $ty) -> Self {
                Self {
                    retry_after_seconds: $retry,
                    code: $error.code,
                    message: $error.message,
                    status: $error.status,
                }
            }
        })*
    };
}

typed_error! {
    CommunityNodeIndexQueryError => |error| error.retry_after_seconds,
    CommunityNodeIndexingRequestError => |error| error.retry_after_seconds,
    CommunityNodeTesterFeedbackError => |error| error.retry_after_seconds,
    CommunityNodeTrustRelationError => |error| None,
    CommunityNodeContentAdvisoryLookupError => |error| None,
    CommunityNodeReportError => |error| None,
}

/// runtime の error を、画面が判別する code へ写す。
pub fn map_error(error: anyhow::Error) -> CommandError {
    if let Some(rejection) = error.downcast_ref::<kukuri_core::MetaverseResourceRejection>() {
        return CommandError {
            code: rejection.code(),
            message: rejection.to_string(),
            status: Some(
                if rejection.reason == kukuri_core::MetaverseResourceRejectionReason::RateExceeded {
                    429
                } else {
                    422
                },
            ),
            retry_after_seconds: None,
        };
    }
    // 購読する scope の上限(#1221 R2-C)。画面は code で判別し、操作を取り消して説明する。
    if error.downcast_ref::<ScopeLimitReached>().is_some() {
        return CommandError::new(SCOPE_LIMIT_REACHED_CODE, format!("{error:#}"));
    }
    // 鍵更新の担当端末でないための保留(#1219 W6)。画面は code で判別してダイアログを出す(W8)。
    if error
        .downcast_ref::<PrivateChannelControllerPending>()
        .is_some()
    {
        return CommandError::new(
            PRIVATE_CHANNEL_CONTROLLER_PENDING_CODE,
            format!("{error:#}"),
        );
    }
    if let Some(request_error) = error.downcast_ref::<DomeHostingRequestError>() {
        return CommandError {
            code: request_error.code.clone(),
            message: request_error.message.clone(),
            status: Some(request_error.status),
            retry_after_seconds: None,
        };
    }
    CommandError::from(error)
}

/// dispatch の platform の値（Tauri は実行中のアプリの版を渡す）。
#[derive(Clone, Debug, Default)]
pub struct DispatchContext {
    pub app_version: String,
}

fn invalid_args(command: &str, error: serde_json::Error) -> CommandError {
    CommandError::new(
        COMMAND_FAILED_CODE,
        format!("invalid args for command {command}: {error}"),
    )
}

/// 同期の method を、native では blocking の thread で呼ぶ（keyring の読取り）。
async fn export_account_key(
    runtime: &Arc<DesktopRuntime>,
    request: ExportAccountKeyRequest,
) -> Result<AccountKeyExport, CommandError> {
    #[cfg(not(target_family = "wasm"))]
    {
        let runtime = runtime.clone();
        tokio::task::spawn_blocking(move || runtime.export_account_key(request))
            .await
            .map_err(|error| CommandError::from(format!("export task failed: {error}")))?
            .map_err(map_error)
    }
    #[cfg(target_family = "wasm")]
    runtime.export_account_key(request).map_err(map_error)
}

/// 呼んだ後の投稿と、表示の再試行の時刻（投稿の再試行の view）。
#[derive(Serialize)]
pub struct PostRetryView {
    #[serde(flatten)]
    post: kukuri_app_api::PostView,
    display_retry_next_at_ms: Option<i64>,
}

async fn retry_post_elements(
    runtime: &Arc<DesktopRuntime>,
    request: RetryPostElementsRequest,
) -> anyhow::Result<Option<PostRetryView>> {
    let Some(post) = runtime.retry_post_elements(request).await? else {
        return Ok(None);
    };
    let display_retry_next_at_ms = runtime.post_display_retry_at(&post).await?;
    Ok(Some(PostRetryView {
        post,
        display_retry_next_at_ms,
    }))
}

/// アカウントの操作の調停が使う platform の状態（ADR 0056 §7）。Tauri は app の state、Web は web-runtime が持つ。
pub trait ClientGate: Send + Sync {
    /// 公開済みの host（起動の前は無い）。
    fn host(&self) -> Option<Arc<ClientHost>>;
    fn startup(&self) -> &ClientStartupState;
    /// アカウントの操作・device backup・終了の処理が互いに排他にする lock。
    fn operation_lock(&self) -> &tokio::sync::Mutex<()>;
    /// 終了の要求の後は断る。排他を待つ間に終了が始まることがあるので、排他を取った後に呼ぶ。
    fn require_running(&self) -> Result<(), CommandError>;
    /// アカウントが替わった後の platform の後始末（Tauri は OS 通知の既読の位置）。
    fn account_switched(&self) {}
}

fn ready_host(gate: &dyn ClientGate) -> Result<Arc<ClientHost>, CommandError> {
    gate.host()
        .ok_or_else(|| CommandError::from("the runtime is not ready".to_string()))
}

/// アカウントの操作を 1 つずつ行う。排他を取り、終了中でなく Ready であることを確かめてから呼ぶ。
async fn exclusive<T, F, Fut>(gate: &dyn ClientGate, operation: F) -> Result<T, CommandError>
where
    F: FnOnce(Arc<ClientHost>) -> Fut,
    Fut: Future<Output = Result<T, CommandError>>,
{
    let _guard = gate.operation_lock().lock().await;
    gate.require_running()?;
    require_runtime_operation_ready(&gate.startup().status()).map_err(CommandError::from)?;
    operation(ready_host(gate)?).await
}

/// runtime を差し替える操作（排他の中で呼ぶ）。間は Initializing にし、後は Ready（host が止まったら再起動を求める
/// 失敗）にする。
async fn transition(
    gate: &dyn ClientGate,
    host: &ClientHost,
    operation: impl Future<Output = anyhow::Result<AccountRecord>>,
) -> Result<AccountRecord, CommandError> {
    gate.startup().set_status(ClientStartupStatus::Initializing);
    let result = operation.await.map_err(map_error);
    if result.is_ok() {
        gate.account_switched();
    }
    gate.startup().set_status(if host.is_stopped() {
        failed_startup_status(
            ClientStartupError::unknown("Account transition requires restart".to_string()),
            None,
        )
    } else {
        ClientStartupStatus::Ready
    });
    result
}

/// アクティブなアカウントを切り替える。新しい runtime を組み立ててから registry と host を更新し、失敗したら旧 runtime
/// をそのまま使う（アプリの再起動は要らない）。
async fn switch_account(
    gate: &dyn ClientGate,
    request: SwitchAccountRequest,
) -> Result<AccountRecord, CommandError> {
    exclusive(gate, |host| async move {
        let snapshot = list_accounts(host.app_data_dir())
            .await
            .map_err(map_error)?;
        let record = snapshot
            .accounts
            .iter()
            .find(|record| record.id == request.account_id)
            .cloned()
            .ok_or_else(|| {
                CommandError::from(format!("unknown account `{}`", request.account_id))
            })?;
        if snapshot.active_account_id == request.account_id {
            return Ok(record);
        }
        transition(gate, &host, host.switch_account(&request.account_id)).await
    })
    .await
}

macro_rules! dispatch_table {
    ($list:ident, $dispatch:ident ($receiver:ident : $receiver_ty:ty, $ctx:ident); $( $(#[$meta:meta])* $name:ident ( $( $arg:ident : $ty:ty ),* ) => $call:expr ; )*) => {
        /// 表にある command の名前。
        #[expect(
            clippy::vec_init_then_push,
            reason = "表の項目ごとに cfg（native だけの command）を付けるため"
        )]
        fn $list() -> Vec<&'static str> {
            let mut commands = Vec::new();
            $( $(#[$meta])* commands.push(stringify!($name)); )*
            commands
        }

        /// 表に無い command は、引数を `Err` で返す。
        async fn $dispatch(
            $receiver: $receiver_ty,
            $ctx: &DispatchContext,
            command: &str,
            args: Value,
        ) -> Result<Result<Value, CommandError>, Value> {
            match command {
                $(
                    $(#[$meta])*
                    stringify!($name) => {
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct Args { $( $arg: $ty ),* }
                        Ok(async {
                            let Args { $( $arg ),* } =
                                serde_json::from_value(args).map_err(|error| invalid_args(command, error))?;
                            let value = $call?;
                            serde_json::to_value(value)
                                .map_err(|error| CommandError::from(anyhow::Error::from(error)))
                        }.await)
                    }
                )*
                _ => Err(args),
            }
        }
    };
}

/// 表にある command の名前（起動の状態・アカウントの操作と、runtime へ委譲するもの）。
pub fn dispatched_commands() -> Vec<&'static str> {
    let mut commands = gate_commands();
    commands.extend(runtime_commands());
    commands
}

/// command を表で呼ぶ。表に無ければ `unsupported_platform`。runtime へ委譲する command は、呼ぶ前と後で host の世代が
/// 違えば、結果を返さず `stale_runtime` にする。
pub async fn dispatch_command(
    gate: &dyn ClientGate,
    ctx: &DispatchContext,
    command: &str,
    args: Value,
) -> Result<Value, CommandError> {
    let args = match dispatch_gate(gate, ctx, command, args).await {
        Ok(result) => return result,
        Err(args) => args,
    };
    let host = ready_host(gate)?;
    let generation = host.generation();
    let result = dispatch_runtime(&host.runtime(), ctx, command, args)
        .await
        .map_err(|_| {
            CommandError::new(
                UNSUPPORTED_PLATFORM_CODE,
                format!("{command} is not available on this platform"),
            )
        })?;
    if host.generation() != generation {
        return Err(CommandError::new(
            STALE_RUNTIME_CODE,
            format!("the runtime changed while {command} was running"),
        ));
    }
    result
}

dispatch_table! { gate_commands, dispatch_gate(gate: &dyn ClientGate, _ctx);
    get_desktop_startup_status() => Ok::<_, CommandError>(gate.startup().status());
    create_account(request: CreateAccountRequest) =>
        exclusive(gate, |host| async move { transition(gate, &host, host.create_account(request)).await }).await;
    logout_account(request: SwitchAccountRequest) =>
        exclusive(gate, |host| async move { transition(gate, &host, host.logout_account(&request.account_id)).await }).await;
    switch_account(request: SwitchAccountRequest) => switch_account(gate, request).await;
    import_account_key(request: ImportAccountKeyRequest) => exclusive(gate, |host| async move {
        host.import_account_key(request.export, request.passphrase, request.label).await.map_err(map_error)
    }).await;
    get_profile_setup_required(request: SwitchAccountRequest) => exclusive(gate, |host| async move {
        host.profile_setup_required(&request.account_id).await.map_err(map_error)
    }).await;
    save_initial_profile(request: InitialProfileRequest) => exclusive(gate, |host| async move {
        host.save_initial_profile(request).await.map_err(map_error)
    }).await;
    preview_account_key_import(request: PreviewAccountKeyImportRequest) =>
        preview_account_key_import(ready_host(gate)?.app_data_dir(), &request.export).await.map_err(map_error);
    list_accounts() => list_accounts(ready_host(gate)?.app_data_dir()).await.map_err(map_error);
}

dispatch_table! { runtime_commands, dispatch_runtime(runtime: &Arc<DesktopRuntime>, ctx);
    abort_dome_transition(request: AbortDomeTransitionRequest) => runtime.abort_dome_transition(request).await.map_err(map_error);
    accept_dome_connection_proposal(request: AcceptDomeConnectionProposalRequest) => runtime.accept_dome_connection_proposal(request).await.map_err(map_error);
    authenticate_community_node(request: CommunityNodeTargetRequest) => runtime.authenticate_community_node(request).await.map_err(map_error);
    block_author(request: AuthorRequest) => runtime.block_author(request).await.map_err(map_error);
    bookmark_custom_reaction(request: BookmarkCustomReactionRequest) => runtime.bookmark_custom_reaction(request).await.map_err(map_error);
    bookmark_post(request: BookmarkPostRequest) => runtime.bookmark_post(request).await.map_err(map_error);
    bookmarked_post_ids(request: BookmarkedPostIdsRequest) => runtime.bookmarked_post_ids(request).await.map_err(map_error);
    cancel_account_transfer() => runtime.cancel_account_transfer().await.map_err(map_error);
    clear_community_node_config() => runtime.clear_community_node_config().await.map_err(map_error);
    clear_community_node_relation_optout(request: CommunityNodeTargetRequest) => runtime.clear_community_node_relation_optout(request).await.map_err(CommandError::from);
    clear_community_node_token(request: CommunityNodeTargetRequest) => runtime.clear_community_node_token(request).await.map_err(map_error);
    clear_direct_message(request: DirectMessageRequest) => runtime.clear_direct_message(request).await.map_err(map_error);
    close_dome_hosting(request: CloseDomeHostingRequest) => runtime.close_dome_hosting(request).await.map_err(map_error);
    commit_dome_layout(request: CommitDomeLayoutRequest) => runtime.commit_dome_layout(request).await.map_err(map_error);
    commit_dome_transition(request: CommitDomeTransitionRequest) => runtime.commit_dome_transition(request).await.map_err(map_error);
    create_account_transfer_invite() => runtime.create_account_transfer_invite().await.map_err(map_error);
    create_custom_reaction_asset(request: CreateCustomReactionAssetRequest) => runtime.create_custom_reaction_asset(request).await.map_err(map_error);
    create_dome_connection_proposal(request: CreateDomeConnectionProposalRequest) => runtime.create_dome_connection_proposal(request).await.map_err(map_error);
    create_game_room(request: CreateGameRoomRequest) => runtime.create_game_room(request).await.map_err(map_error);
    create_live_session(request: CreateLiveSessionRequest) => runtime.create_live_session(request).await.map_err(map_error);
    create_metaverse_room(request: CreateMetaverseRoomRequest) => runtime.create_metaverse_room(request).await.map_err(map_error);
    create_post(request: CreatePostRequest) => runtime.create_post(request).await.map_err(map_error);
    create_private_channel(request: CreatePrivateChannelRequest) => runtime.create_private_channel(request).await.map_err(map_error);
    create_repost(request: CreateRepostRequest) => runtime.create_repost(request).await.map_err(map_error);
    decide_account_transfer(request: DecideAccountTransferRequest) => runtime.decide_account_transfer(request).await.map_err(map_error);
    delegate_dome_hosting(request: DelegateDomeHostingRequest) => runtime.delegate_dome_hosting(request).await.map_err(map_error);
    delete_direct_message_message(request: DeleteDirectMessageMessageRequest) => runtime.delete_direct_message_message(request).await.map_err(map_error);
    delete_dome(request: crate::DeleteDomeRequest) => runtime.delete_dome(request).await.map_err(map_error);
    disable_community_node_observation_sharing(request: CommunityNodeTargetRequest) => runtime.disable_community_node_observation_sharing(request).await.map_err(map_error);
    discover_community_node_index(request: CommunityNodeIndexQueryRequest) => runtime.discover_community_node_index(request).await.map_err(CommandError::from);
    end_live_session(request: LiveSessionCommandRequest) => runtime.end_live_session(request).await.map_err(map_error);
    evaluate_author_trust_gates(request: AuthorTrustGateRequest) => runtime.evaluate_author_trust_gates(request).await.map_err(map_error);
    export_channel_access_token(request: ExportChannelAccessTokenRequest) => runtime.export_channel_access_token(request).await.map_err(map_error);
    export_friend_only_grant(request: ExportFriendOnlyGrantRequest) => runtime.export_friend_only_grant(request).await.map_err(map_error);
    export_friend_plus_share(request: ExportFriendPlusShareRequest) => runtime.export_friend_plus_share(request).await.map_err(map_error);
    export_private_channel_invite(request: ExportPrivateChannelInviteRequest) => runtime.export_private_channel_invite(request).await.map_err(map_error);
    fetch_community_node_manifest(request: CommunityNodeTargetRequest) => runtime.fetch_community_node_manifest(request).await.map_err(map_error);
    fetch_community_node_policies(request: FetchCommunityNodePoliciesRequest) => runtime.fetch_community_node_policies(request).await.map_err(map_error);
    follow_author(request: AuthorRequest) => runtime.follow_author(request).await.map_err(map_error);
    freeze_private_channel(request: FreezePrivateChannelRequest) => runtime.freeze_private_channel(request).await.map_err(map_error);
    get_account_transfer_status() => runtime.account_transfer_status().await.map_err(map_error);
    get_author_social_view(request: AuthorRequest) => runtime.get_author_social_view(request).await.map_err(map_error);
    get_blob_preview_url(request: GetBlobPreviewRequest) => runtime.get_blob_preview_url(request).await.map_err(map_error);
    get_community_node_config() => runtime.get_community_node_config().await.map_err(map_error);
    get_community_node_observation_sharing(request: CommunityNodeTargetRequest) => runtime.get_community_node_observation_sharing(request).await.map_err(map_error);
    get_community_node_relation_optout(request: CommunityNodeTargetRequest) => runtime.get_community_node_relation_optout(request).await.map_err(CommandError::from);
    get_community_node_statuses() => runtime.get_community_node_statuses().await.map_err(map_error);
    get_direct_message_status(request: DirectMessageRequest) => runtime.get_direct_message_status(request).await.map_err(map_error);
    get_discovery_config() => runtime.get_discovery_config().await.map_err(map_error);
    get_dome_hosting(request: GetDomeHostingRequest) => runtime.get_dome_hosting(request).await.map_err(map_error);
    get_local_peer_ticket() => runtime.local_peer_ticket().await.map_err(map_error);
    get_my_profile() => runtime.get_my_profile().await.map_err(map_error);
    get_notification_status() => runtime.get_notification_status().await.map_err(map_error);
    get_sync_status() => runtime.get_sync_status().await.map_err(map_error);
    import_channel_access_token(request: ImportChannelAccessTokenRequest) => runtime.import_channel_access_token(request).await.map_err(map_error);
    import_friend_only_grant(request: ImportFriendOnlyGrantRequest) => runtime.import_friend_only_grant(request).await.map_err(map_error);
    import_friend_plus_share(request: ImportFriendPlusShareRequest) => runtime.import_friend_plus_share(request).await.map_err(map_error);
    import_metaverse_room_asset(request: ImportMetaverseRoomAssetRequest) => runtime.import_metaverse_room_asset(request).await.map_err(map_error);
    import_peer_ticket(request: ImportPeerTicketRequest) => runtime.import_peer_ticket(request).await.map_err(map_error);
    import_private_channel_invite(request: ImportPrivateChannelInviteRequest) => runtime.import_private_channel_invite(request).await.map_err(map_error);
    join_live_session(request: LiveSessionCommandRequest) => runtime.join_live_session(request).await.map_err(map_error);
    leave_live_session(request: LiveSessionCommandRequest) => runtime.leave_live_session(request).await.map_err(map_error);
    leave_private_channel(request: LeavePrivateChannelRequest) => runtime.leave_private_channel(request).await.map_err(map_error);
    list_author_trust_display_exceptions() => runtime.list_author_trust_display_exceptions().await.map_err(map_error);
    list_bookmarked_custom_reactions() => runtime.list_bookmarked_custom_reactions().await.map_err(map_error);
    list_bookmarked_posts_page(request: ListBookmarkedPostsRequest) => runtime.list_bookmarked_posts_page(request).await.map_err(map_error);
    list_community_node_relation_neighbors(request: CommunityNodeRelationNeighborsRequest) => runtime.list_community_node_relation_neighbors(request).await.map_err(CommandError::from);
    list_connectivity_peers(request: crate::ConnectivityPeersRequest) => runtime.list_connectivity_peers(request).await.map_err(map_error);
    list_direct_message_messages(request: ListDirectMessageMessagesRequest) => runtime.list_direct_message_messages(request).await.map_err(map_error);
    list_direct_messages() => runtime.list_direct_messages().await.map_err(map_error);
    list_dome_connection_topology(request: ListDomeConnectionTopologyRequest) => runtime.list_dome_connection_topology(request).await.map_err(map_error);
    list_game_rooms(request: ListGameRoomsRequest) => runtime.list_game_rooms(request).await.map_err(map_error);
    list_joined_private_channels(request: ListJoinedPrivateChannelsRequest) => runtime.list_joined_private_channels(request).await.map_err(map_error);
    list_live_sessions(request: ListLiveSessionsRequest) => runtime.list_live_sessions(request).await.map_err(map_error);
    list_metaverse_room_events(request: ListMetaverseRoomEventsRequest) => runtime.list_metaverse_room_events(request).await.map_err(map_error);
    list_my_custom_reaction_assets() => runtime.list_my_custom_reaction_assets().await.map_err(map_error);
    list_notifications_page(request: ListNotificationsPageRequest) => runtime.list_notifications_page(request).await.map_err(map_error);
    list_pending_dome_deletions(spatial_context: kukuri_core::SpatialContextV1) => runtime.list_pending_dome_deletions(spatial_context).await.map_err(map_error);
    list_profile_timeline(request: ListProfileTimelineRequest) => runtime.list_profile_timeline(request).await.map_err(map_error);
    list_recent_reactions(request: ListRecentReactionsRequest) => runtime.list_recent_reactions(request).await.map_err(map_error);
    list_session_candidates(request: ListLiveSessionsRequest) => runtime.list_session_candidates(request).await.map_err(map_error);
    list_social_connections(request: ListSocialConnectionsRequest) => runtime.list_social_connections(request).await.map_err(map_error);
    list_thread(request: ListThreadRequest) => runtime.list_thread(request).await.map_err(map_error);
    list_timeline(request: ListTimelineRequest) => runtime.list_timeline(request).await.map_err(map_error);
    lookup_community_node_content_advisories(request: CommunityNodeContentAdvisoryLookupRequest) => runtime.lookup_community_node_content_advisories(request).await.map_err(CommandError::from);
    mark_all_notifications_read() => runtime.mark_all_notifications_read().await.map_err(map_error);
    mark_notification_read(request: NotificationIdRequest) => runtime.mark_notification_read(request).await.map_err(map_error);
    move_dome(request: MoveDomeRequest) => runtime.move_dome(request).await.map_err(map_error);
    mute_author(request: AuthorRequest) => runtime.mute_author(request).await.map_err(map_error);
    open_account_transfer(request: OpenAccountTransferRequest) => runtime.open_account_transfer(request).await.map_err(map_error);
    open_direct_message(request: DirectMessageRequest) => runtime.open_direct_message(request).await.map_err(map_error);
    prepare_dome_transition(request: PrepareDomeTransitionRequest) => runtime.prepare_dome_transition(request).await.map_err(map_error);
    preview_channel_access_token(request: PreviewChannelAccessTokenRequest) => runtime.preview_channel_access_token(request).await.map_err(map_error);
    preview_dome_transition_access(request: PrepareDomeTransitionRequest) => runtime.preview_dome_transition_access(request).await.map_err(map_error);
    publish_metaverse_room_event(request: PublishMetaverseRoomEventRequest) => runtime.publish_metaverse_room_event(request).await.map_err(map_error);
    read_community_node_indexing_status(request: CommunityNodeIndexingStatusRequest) => runtime.read_community_node_indexing_status(request).await.map_err(CommandError::from);
    read_community_node_relation_user(request: CommunityNodeUserAdvisoryRequest) => runtime.read_community_node_relation_user(request).await.map_err(CommandError::from);
    read_community_node_trust_user(request: CommunityNodeUserAdvisoryRequest) => runtime.read_community_node_trust_user(request).await.map_err(CommandError::from);
    recommend_community_node_index(request: CommunityNodeIndexQueryRequest) => runtime.recommend_community_node_index(request).await.map_err(CommandError::from);
    refresh_community_node_metadata(request: CommunityNodeTargetRequest) => runtime.refresh_community_node_metadata(request).await.map_err(map_error);
    remove_bookmarked_custom_reaction(request: RemoveBookmarkedCustomReactionRequest) => runtime.remove_bookmarked_custom_reaction(request).await.map_err(map_error);
    remove_bookmarked_post(request: RemoveBookmarkedPostRequest) => runtime.remove_bookmarked_post(request).await.map_err(map_error);
    resolve_community_index_posts(request: ResolveCommunityIndexPostsRequest) => runtime.resolve_community_index_posts(request).await.map_err(map_error);
    resync_dome_snapshots(request: ResyncDomeSnapshotsRequest) => runtime.resync_dome_snapshots(request).await.map_err(map_error);
    revoke_community_node_indexing_request(request: CommunityNodeIndexingRequest) => runtime.revoke_community_node_indexing_request(request).await.map_err(CommandError::from);
    revoke_dome_connection(request: RevokeDomeConnectionRequest) => runtime.revoke_dome_connection(request).await.map_err(map_error);
    rotate_private_channel(request: RotatePrivateChannelRequest) => runtime.rotate_private_channel(request).await.map_err(map_error);
    search_community_node_index(request: CommunityNodeIndexQueryRequest) => runtime.search_community_node_index(request).await.map_err(CommandError::from);
    send_direct_message(request: SendDirectMessageRequest) => runtime.send_direct_message(request).await.map_err(map_error);
    set_author_trust_display_exception(request: SetAuthorTrustDisplayExceptionRequest) => runtime.set_author_trust_display_exception(request).await.map_err(map_error);
    set_channel_gossip_enabled(request: SetChannelGossipEnabledRequest) => runtime.set_channel_gossip_enabled(request).await.map_err(map_error);
    set_community_node_config(request: SetCommunityNodeConfigRequest) => runtime.set_community_node_config(request).await.map_err(map_error);
    set_community_node_invite_code(request: SetCommunityNodeInviteCodeRequest) => runtime.set_community_node_invite_code(request).await.map_err(map_error);
    set_community_node_relation_optout(request: CommunityNodeTargetRequest) => runtime.set_community_node_relation_optout(request).await.map_err(CommandError::from);
    set_discovery_seeds(request: SetDiscoverySeedsRequest) => runtime.set_discovery_seeds(request).await.map_err(map_error);
    set_my_profile(request: SetMyProfileRequest) => runtime.set_my_profile(request).await.map_err(map_error);
    set_private_channel_entry_dome(request: SetPrivateChannelEntryDomeRequest) => runtime.set_private_channel_entry_dome(request).await.map_err(map_error);
    set_scope_display(request: crate::ScopeDisplayRequest) => runtime.set_scope_display(request).await.map_err(map_error);
    set_session_display(request: kukuri_app_api::SessionDisplayRequest) => runtime.set_session_display(request).await.map_err(map_error);
    set_topic_gossip_enabled(request: SetTopicGossipEnabledRequest) => runtime.set_topic_gossip_enabled(request).await.map_err(map_error);
    start_owner_dome_hosting(request: StartOwnerDomeHostingRequest) => runtime.start_owner_dome_hosting(request).await.map_err(map_error);
    submit_community_node_indexing_request(request: CommunityNodeIndexingRequest) => runtime.submit_community_node_indexing_request(request).await.map_err(CommandError::from);
    submit_community_node_report(request: SubmitCommunityNodeReportRequest) => runtime.submit_community_node_report(request).await.map_err(CommandError::from);
    submit_community_node_tester_feedback(request: CommunityNodeTesterFeedbackSubmission) => runtime.submit_community_node_tester_feedback(request).await.map_err(CommandError::from);
    submit_dome_session_input(request: SubmitDomeSessionInputRequest) => runtime.submit_dome_session_input(request).await.map_err(map_error);
    toggle_reaction(request: ToggleReactionRequest) => runtime.toggle_reaction(request).await.map_err(map_error);
    unblock_author(request: AuthorRequest) => runtime.unblock_author(request).await.map_err(map_error);
    unfollow_author(request: AuthorRequest) => runtime.unfollow_author(request).await.map_err(map_error);
    unmute_author(request: AuthorRequest) => runtime.unmute_author(request).await.map_err(map_error);
    unsubscribe_topic(request: UnsubscribeTopicRequest) => runtime.unsubscribe_topic(request).await.map_err(map_error);
    update_game_room(request: UpdateGameRoomRequest) => runtime.update_game_room(request).await.map_err(map_error);
    update_metaverse_room(request: UpdateMetaverseRoomRequest) => runtime.update_metaverse_room(request).await.map_err(map_error);
    withdraw_community_node_consents(request: CommunityNodeTargetRequest) => runtime.withdraw_community_node_consents(request).await.map_err(map_error);
    withdraw_dome_connection_proposal(request: WithdrawDomeConnectionProposalRequest) => runtime.withdraw_dome_connection_proposal(request).await.map_err(map_error);
    withdraw_post(request: WithdrawPostRequest) => runtime.withdraw_post(request).await.map_err(map_error);
    // 委譲に前後の処理を足していたもの（W1 AC-5 で表へ移した）。
    accept_community_node_consents(request: AcceptCommunityNodeConsentsRequest) =>
        runtime.accept_community_node_consents(request, ctx.app_version.as_str()).await.map_err(map_error);
    enable_community_node_observation_sharing(request: EnableCommunityNodeObservationSharingRequest) =>
        runtime.enable_community_node_observation_sharing(request, ctx.app_version.as_str()).await.map_err(map_error);
    export_account_key(request: ExportAccountKeyRequest) => export_account_key(runtime, request).await;
    get_blob_media_payload(request: GetBlobMediaRequest) => runtime.get_blob_media_payload(request).await.map_err(map_error);
    get_content_display_settings() => Ok::<_, CommandError>(runtime.get_content_display_settings());
    retry_post_elements(request: RetryPostElementsRequest) => retry_post_elements(runtime, request).await.map_err(map_error);
    set_adult_content_display_enabled(enabled: bool) => runtime.set_adult_content_display_enabled(enabled).await.map_err(map_error);
}

#[cfg(test)]
mod tests {
    use super::*;

    // IPC エラー封筒の wire 形状(WP-C3)。TS 側 normalizeInvokeError と対になる
    // 同一バイナリ内契約 — 形状を変える場合は両側同時に変更する。
    #[test]
    fn command_error_serializes_to_code_message_envelope() {
        let error = map_error(anyhow::anyhow!("outer").context("inner"));
        let json = serde_json::to_string(&error).expect("serialize command error");
        assert_eq!(
            json,
            r#"{"code":"command_failed","message":"inner: outer"}"#
        );
    }

    #[test]
    fn scope_limit_is_reported_by_its_code_through_context() {
        let error = map_error(anyhow::Error::from(crate::ScopeLimitReached).context("join"));
        assert_eq!(error.code, SCOPE_LIMIT_REACHED_CODE);
    }

    #[test]
    fn controller_pending_is_reported_by_its_code() {
        let error = map_error(anyhow::Error::from(crate::PrivateChannelControllerPending));
        assert_eq!(error.code, PRIVATE_CHANNEL_CONTROLLER_PENDING_CODE);
    }

    #[test]
    fn command_error_from_string_preserves_message() {
        let error = CommandError::from("failed to resolve app data dir: boom".to_string());
        assert_eq!(error.code, COMMAND_FAILED_CODE);
        assert_eq!(error.message, "failed to resolve app data dir: boom");
    }

    #[test]
    fn community_index_error_serializes_status_and_retry_after() {
        let error = CommandError::from(crate::CommunityNodeIndexQueryError {
            code: "RATE_LIMITED".to_string(),
            message: "try again later".to_string(),
            status: Some(429),
            retry_after_seconds: Some(17),
        });
        let json = serde_json::to_string(&error).expect("serialize index error");
        assert_eq!(
            json,
            r#"{"code":"RATE_LIMITED","message":"try again later","status":429,"retry_after_seconds":17}"#
        );
    }

    #[test]
    fn community_indexing_request_error_preserves_stable_conflict_code() {
        let error = CommandError::from(crate::CommunityNodeIndexingRequestError {
            code: "CHANNEL_SECRET_CONFLICT".to_string(),
            message: "channel capability conflicts with the existing registration".to_string(),
            status: Some(409),
            retry_after_seconds: None,
        });
        let json = serde_json::to_string(&error).expect("serialize indexing request error");
        assert_eq!(
            json,
            r#"{"code":"CHANNEL_SECRET_CONFLICT","message":"channel capability conflicts with the existing registration","status":409}"#
        );
    }

    #[test]
    fn trust_relation_error_preserves_stable_unavailable_code() {
        let error = CommandError::from(crate::CommunityNodeTrustRelationError {
            code: "RELATION_NOT_FOUND".to_string(),
            message: "no relation observed for this pair".to_string(),
            status: Some(404),
        });
        let json = serde_json::to_string(&error).expect("serialize trust relation error");
        assert_eq!(
            json,
            r#"{"code":"RELATION_NOT_FOUND","message":"no relation observed for this pair","status":404}"#
        );
    }
}
