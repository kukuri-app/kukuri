// IPC の view / DTO 型は Rust(crates/app-api/src/views.rs ほか)から生成する
// (WP-H7 PR3)。定義元は types.generated.ts。ここでは front 専用型・入力型・
// DesktopApi interface と、生成型に front 専用フィールドを交差させる PostView を扱う。
export * from './types.generated';
import type {
  AuthorSocialView,
  BlobMediaPayload,
  ContentDisplaySettings,
  BookmarkedPostView,
  ChannelAccessTokenExport,
  ChannelAccessTokenPreview,
  ChannelAudienceKind,
  ChannelRef,
  CommunityNodeConfig,
  CommunityNodeConsentDocumentRef,
  CommunityNodeContentAdvisoryLookupRequest,
  CommunityNodeContentAdvisoryLookupResult,
  CommunityNodeIndexingRequest,
  CommunityNodeIndexingStatusRequest,
  CommunityNodeIndexQueryRequest,
  CommunityIndexPostResolveInput,
  CommunityIndexPostResolveResponse,
  CommunityNodeManifestFetch,
  CommunityNodeNodeStatus,
  CommunityNodePoliciesResponse,
  CommunityNodeRelationNeighborsRequest,
  CommunityNodeTesterFeedbackResponse,
  CommunityNodeTesterFeedbackSubmission,
  CommunityNodeUserAdvisoryRequest,
  CustomReactionAssetView,
  DirectMessageConversationView,
  DirectMessageStatusView,
  DirectMessageTimelineView,
  DiscoveryConfig,
  DomeConnectionProposalView,
  DomeConnectionTopologyView,
  DomeConnectionView,
  DomeCustomizationV1,
  DomeDirection,
  DomeHostingView,
  DomeLayoutCommitView,
  DomeMoveRecordV1,
  DomePhysicsSnapshotV1,
  DomeSessionInputKindV1,
  DomeTransitionAdmissionRequestV1,
  DomeTransitionAdmissionTicketV1,
  DomeTransitionAccessDecisionV1,
  FriendOnlyGrantPreview,
  FriendPlusSharePreview,
  GameRoomStatus,
  GameRoomView,
  GameScoreView,
  JoinedPrivateChannelView,
  LiveSessionView,
  MetaverseAssetKind,
  MetaverseAssetRef,
  MetaverseRoomEventV1,
  SpatialContextV1,
  MetaverseRoomEventView,
  AuthorTrustGate,
  AuthorTrustGateRequest,
  AuthorTrustGateResult,
  CommunityNodeObservationSharingStatus,
  EnableCommunityNodeObservationSharingRequest,
  NotificationStatusView,
  NotificationView,
  PostView as WirePostView,
  PostWithdrawalReasonRequest,
  PrivateChannelInvitePreview,
  Profile,
  ReactionStateView,
  RecentReactionView,
  SocialConnectionKind,
  SubmitCommunityNodeReportRequest,
  SubmitCommunityNodeReportResult,
  SubmitIndexingRequestResponse,
  IndexingStatusResponse,
  IndexQueryResponse,
  RelationNeighborsResponse,
  RelationOptoutResponse,
  RelationReadResponse,
  SyncStatus,
  TimelineCursor,
  TimelineScope,
  TimelineView,
  TrustUserReadResponse,
  WithdrawalReasonVisibilityRequest,
} from './types.generated';

// PostView は wire 型に front 専用のローカル下書き状態を交差させる。
export type PostView = WirePostView & {
  local_id?: string | null;
  local_state?: 'pending' | 'syncing' | 'failed' | null;
  local_error?: string | null;
  server_object_id?: string | null;
  local_draft?: LocalPostDraft | null;
  local_draft_media_items?: LocalDraftMediaItem[] | null;
};

// Tauri コマンドの構造化エラー封筒(src-tauri の CommandError と対、WP-C3)。
// invoke reject の payload 形状であり views fixture(S4 codegen)の対象外。
// message は従来の平文エラー文言と同一、code は機械判定用(既知値:
// 'command_failed'。'bridge_unavailable' は TS 側合成 — invoke/error.ts 参照)。
export type CommandError = {
  code: string;
  message: string;
  status?: number | null;
  retry_after_seconds?: number | null;
};

export type DesktopStartupErrorKind =
  | 'database_open'
  | 'database_migration'
  | 'profile_in_use'
  | 'profile_invalid'
  | 'subscription_state'
  | 'unknown';

export type DesktopStartupErrorView = {
  kind: DesktopStartupErrorKind;
  message: string;
  detail: string;
  db_path?: string | null;
};

export type DesktopStartupStatus =
  | { status: 'initializing' }
  | { status: 'ready' }
  | {
      status: 'consent_required';
      documents: AppConsentDocumentStatus[];
      age_attestation: AgeAttestationStatus;
    }
  | { status: 'failed'; error: DesktopStartupErrorView };

// #858: 18歳以上の自己申告の状態。文書同意とは別レコード(ADR 0046)。
export type AgeAttestationStatus = {
  currentVersion: number;
  attestedVersion: number | null;
  attestedAt: number | null;
};

// #857: アプリ同意は bundle 単一フラグではなく文書単位で記録・判定する。
export type AppConsentDocumentStatus = {
  slug: string;
  currentVersion: number;
  effectiveDate: string;
  authoritativeLanguage: string;
  materialChange: boolean;
  controllerName: string;
  contact: string;
  acceptedVersion: number | null;
  acceptedAt: number | null;
  acceptedLanguage: string | null;
  acceptedAppVersion: string | null;
};

export type AppConsentStatus = {
  documents: AppConsentDocumentStatus[];
  ageAttestation: AgeAttestationStatus;
  satisfied: boolean;
};

export type LocalPostDraft = {
  kind: 'post' | 'repost';
  topic: string;
  content: string;
  reply_to?: string | null;
  source_topic?: string | null;
  source_object_id?: string | null;
  channel_ref?: ChannelRef | null;
  attachments?: CreateAttachmentInput[];
  // #858: 投稿者自己申告のラベル(既知値は 'adult' のみ)。
  content_labels?: string[];
};

export type LocalDraftMediaItem = {
  id: string;
  source_name: string;
  preview_url: string;
  attachments: CreateAttachmentInput[];
};

export type BookmarkedCustomReactionView = CustomReactionAssetView;

export type ReactionKeyInput =
  | { kind: 'emoji'; emoji: string }
  | { kind: 'custom_asset'; asset: CustomReactionAssetView };

export type CustomReactionCropRect = {
  x: number;
  y: number;
  size: number;
};

export type ProfileInput = {
  name?: string | null;
  display_name?: string | null;
  about?: string | null;
  picture_upload?: CreateAttachmentInput | null;
  clear_picture?: boolean;
};

export type CreateAttachmentInput = {
  file_name?: string | null;
  mime: string;
  byte_size: number;
  data_base64: string;
  role?: string | null;
};

export type CreateRepostInput = {
  topic: string;
  source_topic: string;
  source_object_id: string;
  commentary?: string | null;
};

export type CommunityNodeConfigInput = {
  base_url: string;
  /// #1056: この node の content advisory(成人向け表現の推定)を採用するか。未指定は保存済みの値を維持する。
  content_advisory_enabled?: boolean | null;
};

// community node manifest (#355/#356) の client 側表現。public manifest endpoint から取得し、
// dependency 表示 (#357) に使う。snake_case は Rust 由来の JSON 形状に合わせる。
// manifest fetch 状態。'ok' は取得成功、'absent' は node が未公開 (404)。
// 'error' は fetch 失敗、'loading' は取得中（client 側で付与）。
// 分散通報ルーティング (#310) の送信リクエスト。通報先は client が provenance + manifest
// から解決し、その report_endpoint を載せて渡す。snake_case は Rust 由来の JSON 形状。
/// #978: Tauri backend の in-memory tracing ring buffer の 1 行。`seq` はプロセス内で単調増加する。
/// #1206: 直前と同じ level・target・message の event は行を増やさず、同じ行が新しい `seq` で
/// 再発行される。`first_seq` / `first_timestamp_ms` は最初の発生、`seq` / `timestamp_ms` は最新の発生。
export type DesktopLogEntry = {
  seq: number;
  timestamp_ms: number;
  level: string;
  target: string;
  message: string;
  repeat_count: number;
  first_seq: number;
  first_timestamp_ms: number;
};

/// `read_desktop_logs` の応答。`oldest_seq` は buffer に残る最古の行(の `first_seq`)、`next_seq` は次に採番される値。
export type DesktopLogSnapshot = {
  entries: DesktopLogEntry[];
  oldest_seq: number | null;
  next_seq: number;
  max_entries: number;
  max_bytes: number;
};

export interface DesktopApi {
  createPost(
    topic: string,
    content: string,
    replyTo?: string | null,
    attachments?: CreateAttachmentInput[],
    channelRef?: ChannelRef,
    contentLabels?: string[]
  ): Promise<string>;
  createRepost(
    topic: string,
    sourceTopic: string,
    sourceObjectId: string,
    commentary?: string | null
  ): Promise<string>;
  withdrawPost(
    topic: string,
    objectId: string,
    channelRef?: ChannelRef,
    replacementObjectId?: string | null,
    reasonVisibility?: WithdrawalReasonVisibilityRequest,
    reason?: PostWithdrawalReasonRequest | null
  ): Promise<string>;
  toggleReaction(
    targetTopicId: string,
    targetObjectId: string,
    reactionKey: ReactionKeyInput,
    channelRef?: ChannelRef | null
  ): Promise<ReactionStateView>;
  listMyCustomReactionAssets(): Promise<CustomReactionAssetView[]>;
  listRecentReactions(limit?: number): Promise<RecentReactionView[]>;
  createCustomReactionAsset(
    upload: CreateAttachmentInput,
    cropRect: CustomReactionCropRect,
    searchKey: string
  ): Promise<CustomReactionAssetView>;
  listBookmarkedCustomReactions(): Promise<BookmarkedCustomReactionView[]>;
  bookmarkCustomReaction(asset: CustomReactionAssetView): Promise<BookmarkedCustomReactionView>;
  removeBookmarkedCustomReaction(assetId: string): Promise<void>;
  listBookmarkedPosts(): Promise<BookmarkedPostView[]>;
  bookmarkPost(
    topic: string,
    objectId: string,
    channelRef?: ChannelRef
  ): Promise<BookmarkedPostView>;
  removeBookmarkedPost(objectId: string): Promise<void>;
  resolveCommunityIndexPosts(
    entries: CommunityIndexPostResolveInput[]
  ): Promise<CommunityIndexPostResolveResponse>;
  listTimeline(
    topic: string,
    cursor?: TimelineCursor | null,
    limit?: number,
    scope?: TimelineScope
  ): Promise<TimelineView>;
  listThread(
    topic: string,
    threadId: string,
    cursor?: TimelineCursor | null,
    limit?: number
  ): Promise<TimelineView>;
  listProfileTimeline(
    pubkey: string,
    cursor?: TimelineCursor | null,
    limit?: number
  ): Promise<TimelineView>;
  getMyProfile(): Promise<Profile>;
  setMyProfile(input: ProfileInput): Promise<Profile>;
  followAuthor(pubkey: string): Promise<AuthorSocialView>;
  unfollowAuthor(pubkey: string): Promise<AuthorSocialView>;
  getAuthorSocialView(pubkey: string): Promise<AuthorSocialView>;
  muteAuthor(pubkey: string): Promise<AuthorSocialView>;
  unmuteAuthor(pubkey: string): Promise<AuthorSocialView>;
  blockAuthor(pubkey: string): Promise<AuthorSocialView>;
  unblockAuthor(pubkey: string): Promise<AuthorSocialView>;
  listSocialConnections(kind: SocialConnectionKind): Promise<AuthorSocialView[]>;
  listNotifications(): Promise<NotificationView[]>;
  markNotificationRead(notificationId: string): Promise<NotificationStatusView>;
  markAllNotificationsRead(): Promise<NotificationStatusView>;
  getNotificationStatus(): Promise<NotificationStatusView>;
  openDirectMessage(pubkey: string): Promise<DirectMessageConversationView>;
  listDirectMessages(): Promise<DirectMessageConversationView[]>;
  listDirectMessageMessages(
    pubkey: string,
    cursor?: TimelineCursor | null,
    limit?: number
  ): Promise<DirectMessageTimelineView>;
  sendDirectMessage(
    pubkey: string,
    text?: string | null,
    attachments?: CreateAttachmentInput[],
    replyToMessageId?: string | null
  ): Promise<string>;
  deleteDirectMessageMessage(pubkey: string, messageId: string): Promise<void>;
  clearDirectMessage(pubkey: string): Promise<void>;
  getDirectMessageStatus(pubkey: string): Promise<DirectMessageStatusView>;
  listLiveSessions(topic: string, scope?: TimelineScope): Promise<LiveSessionView[]>;
  createLiveSession(
    topic: string,
    title: string,
    description: string,
    channelRef?: ChannelRef
  ): Promise<string>;
  endLiveSession(topic: string, sessionId: string): Promise<void>;
  joinLiveSession(topic: string, sessionId: string): Promise<void>;
  leaveLiveSession(topic: string, sessionId: string): Promise<void>;
  listGameRooms(topic: string, scope?: TimelineScope): Promise<GameRoomView[]>;
  createGameRoom(
    topic: string,
    title: string,
    description: string,
    participants: string[],
    channelRef?: ChannelRef
  ): Promise<string>;
  createMetaverseRoom(
    topic: string,
    title: string,
    description: string,
    maxPeers?: number | null,
    channelRef?: ChannelRef
  ): Promise<string>;
  createPrivateChannel(
    topic: string,
    label: string,
    audienceKind?: ChannelAudienceKind
  ): Promise<JoinedPrivateChannelView>;
  exportPrivateChannelInvite(
    topic: string,
    channelId: string,
    expiresAt?: number | null
  ): Promise<string>;
  importPrivateChannelInvite(token: string): Promise<PrivateChannelInvitePreview>;
  exportChannelAccessToken(
    topic: string,
    channelId: string,
    expiresAt?: number | null
  ): Promise<ChannelAccessTokenExport>;
  previewChannelAccessToken(token: string): Promise<ChannelAccessTokenPreview>;
  importChannelAccessToken(token: string): Promise<ChannelAccessTokenPreview>;
  exportFriendOnlyGrant(
    topic: string,
    channelId: string,
    expiresAt?: number | null
  ): Promise<string>;
  importFriendOnlyGrant(token: string): Promise<FriendOnlyGrantPreview>;
  exportFriendPlusShare(
    topic: string,
    channelId: string,
    expiresAt?: number | null
  ): Promise<string>;
  importFriendPlusShare(token: string): Promise<FriendPlusSharePreview>;
  freezePrivateChannel(topic: string, channelId: string): Promise<JoinedPrivateChannelView>;
  rotatePrivateChannel(topic: string, channelId: string): Promise<JoinedPrivateChannelView>;
  setPrivateChannelEntryDome(
    topic: string,
    channelId: string,
    entryDomeInstanceId: string | null
  ): Promise<JoinedPrivateChannelView>;
  leavePrivateChannel(topic: string, channelId: string): Promise<void>;
  listJoinedPrivateChannels(topic: string): Promise<JoinedPrivateChannelView[]>;
  updateGameRoom(
    topic: string,
    roomId: string,
    status: GameRoomStatus,
    phaseLabel: string | null,
    scores: GameScoreView[]
  ): Promise<void>;
  listPendingDomeDeletions(context: SpatialContextV1): Promise<Array<{ request: { spatial_context: SpatialContextV1; instance_id: string; expected_generation: number; operation_id: string }; title: string; deleted: boolean }>>;
  deleteDome(context: SpatialContextV1, instanceId: string, generation: number, operationId: string): Promise<{ deleted: boolean; cleanup_pending: boolean }>;
  updateMetaverseRoom(
    topic: string,
    roomId: string,
    status: GameRoomStatus,
    customization: DomeCustomizationV1
  ): Promise<void>;
  getDomeHosting(spatialContext: SpatialContextV1, instanceId: string): Promise<DomeHostingView>;
  startOwnerDomeHosting(
    spatialContext: SpatialContextV1,
    instanceId: string,
    endpointId: string,
    leaseDurationMillis: number,
    expectedGeneration?: number
  ): Promise<DomeHostingView>;
  delegateDomeHosting(
    spatialContext: SpatialContextV1,
    instanceId: string,
    nodeId: string,
    baseUrl: string,
    leaseDurationMillis: number,
    expectedGeneration?: number
  ): Promise<DomeHostingView>;
  closeDomeHosting(spatialContext: SpatialContextV1, instanceId: string, expectedGeneration?: number): Promise<DomeHostingView>;
  submitDomeSessionInput(
    spatialContext: SpatialContextV1,
    instanceId: string,
    sequence: number,
    input: DomeSessionInputKindV1,
    expectedGeneration?: number
  ): Promise<DomePhysicsSnapshotV1>;
  prepareDomeTransition(
    request: DomeTransitionAdmissionRequestV1
  ): Promise<DomeTransitionAdmissionTicketV1>;
  previewDomeTransitionAccess(
    request: DomeTransitionAdmissionRequestV1
  ): Promise<DomeTransitionAccessDecisionV1>;
  commitDomeTransition(
    ticket: DomeTransitionAdmissionTicketV1,
    position: [number, number, number],
    rotation: [number, number, number]
  ): Promise<void>;
  abortDomeTransition(ticket: DomeTransitionAdmissionTicketV1): Promise<void>;
  commitDomeLayout(
    spatialContext: SpatialContextV1,
    instanceId: string,
    operationId: string
  ): Promise<DomeLayoutCommitView>;
  resyncDomeSnapshots(
    spatialContext: SpatialContextV1,
    instanceId: string,
    afterSequence: number
  ): Promise<DomePhysicsSnapshotV1[]>;
  moveDome(
    sourceTopic: string,
    moveId: string,
    sourceInstanceId: string,
    targetContext: SpatialContextV1
  ): Promise<DomeMoveRecordV1>;
  listDomeConnectionTopology(
    spatialContext: SpatialContextV1
  ): Promise<DomeConnectionTopologyView>;
  createDomeConnectionProposal(
    proposalId: string,
    spatialContext: SpatialContextV1,
    proposerInstanceId: string,
    receiverInstanceId: string,
    proposerDirection: DomeDirection
  ): Promise<DomeConnectionProposalView>;
  acceptDomeConnectionProposal(
    spatialContext: SpatialContextV1,
    proposalId: string
  ): Promise<DomeConnectionView>;
  withdrawDomeConnectionProposal(
    spatialContext: SpatialContextV1,
    proposalId: string
  ): Promise<DomeConnectionProposalView>;
  revokeDomeConnection(
    spatialContext: SpatialContextV1,
    connectionId: string
  ): Promise<DomeConnectionView>;
  publishMetaverseRoomEvent(
    topic: string,
    roomId: string,
    peerId: string,
    seq: number,
    event: MetaverseRoomEventV1
  ): Promise<MetaverseRoomEventView>;
  listMetaverseRoomEvents(
    topic: string,
    roomId: string,
    afterEnvelopeId?: string | null,
    limit?: number | null
  ): Promise<MetaverseRoomEventView[]>;
  importMetaverseRoomAsset(
    topic: string,
    roomId: string,
    kind: MetaverseAssetKind,
    mimeType: string,
    name: string | null,
    dataBase64: string
  ): Promise<MetaverseAssetRef>;
  getSyncStatus(): Promise<SyncStatus>;
  getDiscoveryConfig(): Promise<DiscoveryConfig>;
  getCommunityNodeConfig(): Promise<CommunityNodeConfig>;
  getCommunityNodeStatuses(): Promise<CommunityNodeNodeStatus[]>;
  // #1061: `trustNodePriority` を渡すと信頼値の採用順位も更新する（未指定なら保存済みを維持）。
  setCommunityNodeConfig(
    nodes: CommunityNodeConfigInput[],
    trustNodePriority?: string[]
  ): Promise<CommunityNodeConfig>;
  clearCommunityNodeConfig(): Promise<void>;
  authenticateCommunityNode(baseUrl: string): Promise<CommunityNodeNodeStatus>;
  setCommunityNodeInviteCode(
    baseUrl: string,
    inviteCode: string | null
  ): Promise<CommunityNodeNodeStatus>;
  clearCommunityNodeToken(baseUrl: string): Promise<CommunityNodeNodeStatus>;
  // #857: 認証不要の公開 policy カタログ。同意モーダルの提示内容を組み立てる。
  fetchCommunityNodePolicies(
    baseUrl: string,
    language?: string | null
  ): Promise<CommunityNodePoliciesResponse>;
  // #857: 提示済み文書への同意をローカル記録し、セッション確立(認証・同期)を開始する。
  acceptCommunityNodeConsents(
    baseUrl: string,
    documents: CommunityNodeConsentDocumentRef[],
    language: string
  ): Promise<CommunityNodeNodeStatus>;
  // #857: Node 同意の撤回。記録は履歴として残し、トークンを破棄して接続を停止する。
  withdrawCommunityNodeConsents(baseUrl: string): Promise<CommunityNodeNodeStatus>;
  refreshCommunityNodeMetadata(baseUrl: string): Promise<CommunityNodeNodeStatus>;
  fetchCommunityNodeManifest(baseUrl: string): Promise<CommunityNodeManifestFetch>;
  // #1056: 可視 post id / blob hash の content advisory を、採用 ON の設定済み node へ一括照会する。
  lookupCommunityNodeContentAdvisories(
    request: CommunityNodeContentAdvisoryLookupRequest
  ): Promise<CommunityNodeContentAdvisoryLookupResult>;
  readCommunityNodeTrustUser(
    request: CommunityNodeUserAdvisoryRequest
  ): Promise<TrustUserReadResponse>;
  readCommunityNodeRelationUser(
    request: CommunityNodeUserAdvisoryRequest
  ): Promise<RelationReadResponse>;
  listCommunityNodeRelationNeighbors(
    request: CommunityNodeRelationNeighborsRequest
  ): Promise<RelationNeighborsResponse>;
  // #1061: 採用 CN の信頼値による著者の表示判断。
  evaluateAuthorTrustGates(request: AuthorTrustGateRequest): Promise<AuthorTrustGateResult>;
  setAuthorTrustDisplayException(
    authorPubkey: string,
    alwaysVisible: boolean
  ): Promise<AuthorTrustGate>;
  listAuthorTrustDisplayExceptions(): Promise<string[]>;
  // #1061: ブロック / ミュート観測の提供（CN の任意文書への同意）。
  getCommunityNodeObservationSharing(
    baseUrl: string
  ): Promise<CommunityNodeObservationSharingStatus>;
  enableCommunityNodeObservationSharing(
    request: EnableCommunityNodeObservationSharingRequest
  ): Promise<CommunityNodeObservationSharingStatus>;
  disableCommunityNodeObservationSharing(
    baseUrl: string
  ): Promise<CommunityNodeObservationSharingStatus>;
  getCommunityNodeRelationOptout(baseUrl: string): Promise<RelationOptoutResponse>;
  setCommunityNodeRelationOptout(baseUrl: string): Promise<RelationOptoutResponse>;
  clearCommunityNodeRelationOptout(baseUrl: string): Promise<RelationOptoutResponse>;
  searchCommunityNodeIndex(
    request: CommunityNodeIndexQueryRequest
  ): Promise<IndexQueryResponse>;
  discoverCommunityNodeIndex(
    request: CommunityNodeIndexQueryRequest
  ): Promise<IndexQueryResponse>;
  recommendCommunityNodeIndex(
    request: CommunityNodeIndexQueryRequest
  ): Promise<IndexQueryResponse>;
  submitCommunityNodeIndexingRequest(
    request: CommunityNodeIndexingRequest
  ): Promise<SubmitIndexingRequestResponse>;
  readCommunityNodeIndexingStatus(
    request: CommunityNodeIndexingStatusRequest
  ): Promise<IndexingStatusResponse>;
  /// #978: 開発者モードON時だけ backend が応答する。OFF 時は呼ばない(backend も拒否する)。
  readDesktopLogs(afterSeq?: number | null, limit?: number | null): Promise<DesktopLogSnapshot>;
  submitCommunityNodeReport(
    request: SubmitCommunityNodeReportRequest
  ): Promise<SubmitCommunityNodeReportResult>;
  submitCommunityNodeTesterFeedback(
    request: CommunityNodeTesterFeedbackSubmission
  ): Promise<CommunityNodeTesterFeedbackResponse>;
  importPeerTicket(ticket: string): Promise<void>;
  setDiscoverySeeds(seedEntries: string[]): Promise<DiscoveryConfig>;
  unsubscribeTopic(topic: string): Promise<void>;
  setTopicGossipEnabled(topic: string, enabled: boolean): Promise<void>;
  setChannelGossipEnabled(topic: string, channelId: string, enabled: boolean): Promise<void>;
  getLocalPeerTicket(): Promise<string | null>;
  getBlobMediaPayload(hash: string, mime: string): Promise<BlobMediaPayload | null>;
  // #858: 成人向け表現の表示設定(既定 OFF)。
  getContentDisplaySettings(): Promise<ContentDisplaySettings>;
  setAdultContentDisplayEnabled(enabled: boolean): Promise<ContentDisplaySettings>;
  getBlobPreviewUrl(
    hash: string,
    mime: string,
    metaverseKind?: MetaverseAssetKind | null
  ): Promise<string | null>;
}

declare global {
  interface Window {
    __KUKURI_DESKTOP__?: DesktopApi;
  }
}
