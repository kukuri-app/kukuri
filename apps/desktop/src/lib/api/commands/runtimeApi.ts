import type {
  SessionCandidateView,
  AuthorSocialView,
  BlobMediaPayload,
  BlobMediaFile,
  BookmarkedCustomReactionView,
  BookmarkedPostView,
  BookmarkedPostPageView,
  BookmarkCursor,
  ChannelAccessTokenExport,
  ChannelAccessTokenPreview,
  CommunityNodeConfig,
  CommunityNodeIndexingRequest,
  CommunityNodeIndexingStatusRequest,
  CommunityNodeIndexQueryRequest,
  CommunityIndexPostResolveResponse,
  CommunityNodeManifestFetch,
  CommunityNodeNodeStatus,
  AuthorTrustGate,
  AuthorTrustGateRequest,
  AuthorTrustGateResult,
  CommunityNodeObservationSharingStatus,
  EnableCommunityNodeObservationSharingRequest,
  CommunityNodePoliciesResponse,
  CommunityNodeRelationNeighborsRequest,
  CommunityNodeTesterFeedbackResponse,
  CommunityNodeTesterFeedbackSubmission,
  CommunityNodeUserAdvisoryRequest,
  CustomReactionAssetView,
  DesktopApi,
  DirectMessageConversationView,
  DirectMessageStatusView,
  DirectMessageTimelineView,
  DiscoveryConfig,
  DomeConnectionProposalView,
  DomeConnectionTopologyView,
  DomeConnectionView,
  DomeDirection,
  DomeHostingView,
  DomeLayoutCommitView,
  DomeMoveRecordV1,
  DomePhysicsSnapshotV1,
  DomeSessionInputKindV1,
  FriendOnlyGrantPreview,
  FriendPlusSharePreview,
  GameRoomView,
  JoinedPrivateChannelView,
  IndexQueryResponse,
  IndexingStatusResponse,
  RelationNeighborsResponse,
  RelationOptoutResponse,
  RelationReadResponse,
  LiveSessionView,
  MetaverseAssetRef,
  MetaverseRoomEventView,
  SpatialContextV1,
  NotificationStatusView,
  NotificationPageView,
  NotificationCursor,
  PrivateChannelInvitePreview,
  Profile,
  ReactionStateView,
  RecentReactionView,
  SubmitCommunityNodeReportResult,
  SubmitIndexingRequestResponse,
  SyncStatus,
  TimelineView,
  TrustUserReadResponse,
} from '../types';
// request DTO の生成型(WP-B6)。組み立てた literal を satisfies で拘束し、
// Rust 側 DTO の変更を tsc で検出する。手書き types.ts の同名 shadow を避けるため
// types.generated から直接 import する。
import type {
  AcceptCommunityNodeConsentsRequest,
  AcceptDomeConnectionProposalRequest,
  AuthorRequest,
  BookmarkCustomReactionRequest,
  BookmarkPostRequest,
  BookmarkedPostIdsRequest,
  ListBookmarkedPostsRequest,
  CommunityNodeTargetRequest,
  FetchCommunityNodePoliciesRequest,
  CloseDomeHostingRequest,
  CreateCustomReactionAssetRequest,
  CreateDomeConnectionProposalRequest,
  CreateGameRoomRequest,
  CreateLiveSessionRequest,
  CreateMetaverseRoomRequest,
  ContentDisplaySettings,
  CreatePostRequest,
  CreatePrivateChannelRequest,
  CreateRepostRequest,
  DelegateDomeHostingRequest,
  DeleteDirectMessageMessageRequest,
  DeleteDomeInput,
  DirectMessageRequest,
  ExportChannelAccessTokenRequest,
  ExportFriendOnlyGrantRequest,
  ExportFriendPlusShareRequest,
  ExportPrivateChannelInviteRequest,
  FreezePrivateChannelRequest,
  GetBlobMediaRequest,
  GetBlobPreviewRequest,
  GetDomeHostingRequest,
  ImportChannelAccessTokenRequest,
  ImportFriendOnlyGrantRequest,
  ImportFriendPlusShareRequest,
  ImportMetaverseRoomAssetRequest,
  ImportPeerTicketRequest,
  ImportPrivateChannelInviteRequest,
  LeavePrivateChannelRequest,
  ListDirectMessageMessagesRequest,
  ListDomeConnectionTopologyRequest,
  ListGameRoomsRequest,
  ListJoinedPrivateChannelsRequest,
  ListLiveSessionsRequest,
  ListMetaverseRoomEventsRequest,
  ListProfileTimelineRequest,
  ListRecentReactionsRequest,
  ListSocialConnectionsRequest,
  ListThreadRequest,
  ListTimelineRequest,
  LiveSessionCommandRequest,
  MoveDomeRequest,
  NotificationIdRequest,
  ListNotificationsPageRequest,
  PreviewChannelAccessTokenRequest,
  PublishMetaverseRoomEventRequest,
  RemoveBookmarkedCustomReactionRequest,
  RemoveBookmarkedPostRequest,
  ResolveCommunityIndexPostsRequest,
  RevokeDomeConnectionRequest,
  RotatePrivateChannelRequest,
  SendDirectMessageRequest,
  SetChannelGossipEnabledRequest,
  SetPrivateChannelEntryDomeRequest,
  SetCommunityNodeConfigRequest,
  SetCommunityNodeInviteCodeRequest,
  SetDiscoverySeedsRequest,
  SetTopicGossipEnabledRequest,
  StartOwnerDomeHostingRequest,
  SubmitDomeSessionInputRequest,
  ToggleReactionRequest,
  UnsubscribeTopicRequest,
  UpdateGameRoomRequest,
  UpdateMetaverseRoomRequest,
  WithdrawDomeConnectionProposalRequest,
  WithdrawPostRequest,
} from '../types.generated';

import { invokeDesktop } from '../invoke/desktop';
import { command } from '../invoke/dispatch';
import { commitDomeLayoutRequest, resyncDomeSnapshotsRequest } from './domeHostingRequests';
import { developerLogsApi, displayDemandApi, domeTransitionApi, postReloadApi, socialBlockApi } from './apiModules';

export const runtimeApi: DesktopApi = {
  createPost: command('createPost', async (topic, content, replyTo, attachments = [], channelRef = { kind: 'public' }, contentLabels = []) => {
    return invokeDesktop<string>('create_post', {
      request: {
        topic,
        content,
        reply_to: replyTo,
        channel_ref: channelRef,
        attachments,
        content_labels: contentLabels,
      } satisfies CreatePostRequest,
    });
  }),
  createRepost: command('createRepost', async (topic, sourceTopic, sourceObjectId, commentary) => {
    return invokeDesktop<string>('create_repost', {
      request: {
        topic,
        source_topic: sourceTopic,
        source_object_id: sourceObjectId,
        commentary,
      } satisfies CreateRepostRequest,
    });
  }),
  withdrawPost: command(
    'withdrawPost',
    async (
      topic,
      objectId,
      channelRef = { kind: 'public' },
      replacementObjectId = null,
      reasonVisibility = 'public',
      reason = 'author_request'
    ) => {
      return invokeDesktop<string>('withdraw_post', {
        request: {
          topic,
          object_id: objectId,
          channel_ref: channelRef,
          replacement_object_id: replacementObjectId,
          reason_visibility: reasonVisibility,
          reason,
        } satisfies WithdrawPostRequest,
      });
    }
  ),
  toggleReaction: command('toggleReaction', async (targetTopicId, targetObjectId, reactionKey, channelRef = null) => {
    return invokeDesktop<ReactionStateView>('toggle_reaction', {
      request: {
        target_topic_id: targetTopicId,
        target_object_id: targetObjectId,
        reaction_key:
          reactionKey.kind === 'emoji'
            ? { kind: 'emoji', emoji: reactionKey.emoji }
            : {
                kind: 'custom_asset',
                asset_id: reactionKey.asset.asset_id,
                owner_pubkey: reactionKey.asset.owner_pubkey,
                blob_hash: reactionKey.asset.blob_hash,
                search_key: reactionKey.asset.search_key,
                mime: reactionKey.asset.mime,
                bytes: reactionKey.asset.bytes,
                width: reactionKey.asset.width,
                height: reactionKey.asset.height,
              },
        channel_ref: channelRef,
      } satisfies ToggleReactionRequest,
    });
  }),
  listMyCustomReactionAssets: command('listMyCustomReactionAssets', async () => {
    return invokeDesktop<CustomReactionAssetView[]>('list_my_custom_reaction_assets');
  }),
  listRecentReactions: command('listRecentReactions', async (limit = 8) => {
    return invokeDesktop<RecentReactionView[]>('list_recent_reactions', {
      request: {
        limit,
      } satisfies ListRecentReactionsRequest,
    });
  }),
  createCustomReactionAsset: command('createCustomReactionAsset', async (upload, cropRect, searchKey) => {
    return invokeDesktop<CustomReactionAssetView>('create_custom_reaction_asset', {
      request: {
        upload,
        crop_rect: cropRect,
        search_key: searchKey,
      } satisfies CreateCustomReactionAssetRequest,
    });
  }),
  listBookmarkedCustomReactions: command('listBookmarkedCustomReactions', async () => {
    return invokeDesktop<BookmarkedCustomReactionView[]>('list_bookmarked_custom_reactions');
  }),
  bookmarkCustomReaction: command('bookmarkCustomReaction', async (asset) => {
    return invokeDesktop<BookmarkedCustomReactionView>('bookmark_custom_reaction', {
      request: {
        asset_id: asset.asset_id,
        owner_pubkey: asset.owner_pubkey,
        blob_hash: asset.blob_hash,
        search_key: asset.search_key,
        mime: asset.mime,
        bytes: asset.bytes,
        width: asset.width,
        height: asset.height,
      } satisfies BookmarkCustomReactionRequest,
    });
  }),
  removeBookmarkedCustomReaction: command('removeBookmarkedCustomReaction', async (assetId) => {
    return invokeDesktop<void>('remove_bookmarked_custom_reaction', {
      request: {
        asset_id: assetId,
      } satisfies RemoveBookmarkedCustomReactionRequest,
    });
  }),
  listBookmarkedPostsPage: command('listBookmarkedPostsPage', async (cursor?: BookmarkCursor | null, before = false) => {
    return invokeDesktop<BookmarkedPostPageView>('list_bookmarked_posts_page', {
      request: { cursor: cursor ?? null, before } satisfies ListBookmarkedPostsRequest,
    });
  }),
  bookmarkedPostIds: command('bookmarkedPostIds', async (objectIds: string[]) => {
    return invokeDesktop<string[]>('bookmarked_post_ids', {
      request: { object_ids: objectIds } satisfies BookmarkedPostIdsRequest,
    });
  }),
  bookmarkPost: command('bookmarkPost', async (topic, objectId, channelRef = { kind: 'public' }) => {
    return invokeDesktop<BookmarkedPostView>('bookmark_post', {
      request: { topic, object_id: objectId, channel_ref: channelRef } satisfies BookmarkPostRequest,
    });
  }),
  removeBookmarkedPost: command('removeBookmarkedPost', async (objectId) => {
    return invokeDesktop<void>('remove_bookmarked_post', {
      request: {
        object_id: objectId,
      } satisfies RemoveBookmarkedPostRequest,
    });
  }),
  resolveCommunityIndexPosts: command('resolveCommunityIndexPosts', (entries) =>
    invokeDesktop<CommunityIndexPostResolveResponse>('resolve_community_index_posts', {
      request: { entries } satisfies ResolveCommunityIndexPostsRequest,
    })),
  listTimeline: command('listTimeline', async (topic, cursor, limit, scope = { kind: 'public' }) => {
    return invokeDesktop<TimelineView>('list_timeline', {
      request: {
        topic,
        scope,
        cursor,
        limit,
      } satisfies ListTimelineRequest,
    });
  }),
  listThread: command('listThread', async (topic, threadId, cursor, limit) => {
    return invokeDesktop<TimelineView>('list_thread', {
      request: {
        topic,
        thread_id: threadId,
        cursor,
        limit,
      } satisfies ListThreadRequest,
    });
  }),
  listProfileTimeline: command('listProfileTimeline', async (pubkey, cursor, limit) => {
    return invokeDesktop<TimelineView>('list_profile_timeline', {
      request: {
        pubkey,
        cursor,
        limit,
      } satisfies ListProfileTimelineRequest,
    });
  }),
  getMyProfile: command('getMyProfile', async () => {
    return invokeDesktop<Profile>('get_my_profile');
  }),
  setMyProfile: command('setMyProfile', async (input) => {
    return invokeDesktop<Profile>('set_my_profile', {
      request: input,
    });
  }),
  followAuthor: command('followAuthor', async (pubkey) => {
    return invokeDesktop<AuthorSocialView>('follow_author', {
      request: { pubkey } satisfies AuthorRequest,
    });
  }),
  unfollowAuthor: command('unfollowAuthor', async (pubkey) => {
    return invokeDesktop<AuthorSocialView>('unfollow_author', {
      request: { pubkey } satisfies AuthorRequest,
    });
  }),
  getAuthorSocialView: command('getAuthorSocialView', async (pubkey) => {
    return invokeDesktop<AuthorSocialView>('get_author_social_view', {
      request: { pubkey } satisfies AuthorRequest,
    });
  }),
  muteAuthor: command('muteAuthor', async (pubkey) => {
    return invokeDesktop<AuthorSocialView>('mute_author', {
      request: { pubkey } satisfies AuthorRequest,
    });
  }),
  unmuteAuthor: command('unmuteAuthor', async (pubkey) => {
    return invokeDesktop<AuthorSocialView>('unmute_author', {
      request: { pubkey } satisfies AuthorRequest,
    });
  }),
  listSocialConnections: command('listSocialConnections', async (kind) => {
    return invokeDesktop<AuthorSocialView[]>('list_social_connections', {
      request: { kind } satisfies ListSocialConnectionsRequest,
    });
  }),
  listNotificationsPage: command('listNotificationsPage', async (cursor?: NotificationCursor | null, before = false) => {
    return invokeDesktop<NotificationPageView>('list_notifications_page', {
      request: { cursor: cursor ?? null, before } satisfies ListNotificationsPageRequest,
    });
  }),
  markNotificationRead: command('markNotificationRead', async (notificationId) => {
    return invokeDesktop<NotificationStatusView>('mark_notification_read', {
      request: { notification_id: notificationId } satisfies NotificationIdRequest,
    });
  }),
  markAllNotificationsRead: command('markAllNotificationsRead', async () => {
    return invokeDesktop<NotificationStatusView>('mark_all_notifications_read');
  }),
  getNotificationStatus: command('getNotificationStatus', async () => {
    return invokeDesktop<NotificationStatusView>('get_notification_status');
  }),
  openDirectMessage: command('openDirectMessage', async (pubkey) => {
    return invokeDesktop<DirectMessageConversationView>('open_direct_message', {
      request: { pubkey } satisfies DirectMessageRequest,
    });
  }),
  listDirectMessages: command('listDirectMessages', async () => {
    return invokeDesktop<DirectMessageConversationView[]>('list_direct_messages');
  }),
  listDirectMessageMessages: command('listDirectMessageMessages', async (pubkey, cursor, limit) => {
    return invokeDesktop<DirectMessageTimelineView>('list_direct_message_messages', {
      request: {
        pubkey,
        cursor,
        limit,
      } satisfies ListDirectMessageMessagesRequest,
    });
  }),
  sendDirectMessage: command('sendDirectMessage', async (pubkey, text, attachments = [], replyToMessageId) => {
    return invokeDesktop<string>('send_direct_message', {
      request: {
        pubkey,
        text,
        reply_to_message_id: replyToMessageId,
        attachments,
      } satisfies SendDirectMessageRequest,
    });
  }),
  deleteDirectMessageMessage: command('deleteDirectMessageMessage', async (pubkey, messageId) => {
    return invokeDesktop<void>('delete_direct_message_message', {
      request: {
        pubkey,
        message_id: messageId,
      } satisfies DeleteDirectMessageMessageRequest,
    });
  }),
  clearDirectMessage: command('clearDirectMessage', async (pubkey) => {
    return invokeDesktop<void>('clear_direct_message', {
      request: { pubkey } satisfies DirectMessageRequest,
    });
  }),
  getDirectMessageStatus: command('getDirectMessageStatus', async (pubkey) => {
    return invokeDesktop<DirectMessageStatusView>('get_direct_message_status', {
      request: { pubkey } satisfies DirectMessageRequest,
    });
  }),
  listSessionCandidates: command('listSessionCandidates', async (topic, scope) =>
    invokeDesktop<SessionCandidateView[]>('list_session_candidates', { request: { topic, scope } satisfies ListLiveSessionsRequest })),
  listLiveSessions: command('listLiveSessions', async (topic, scope = { kind: 'public' }) => {
    return invokeDesktop<LiveSessionView[]>('list_live_sessions', {
      request: {
        topic,
        scope,
      } satisfies ListLiveSessionsRequest,
    });
  }),
  createLiveSession: command('createLiveSession', async (topic, title, description, channelRef = { kind: 'public' }) => {
    return invokeDesktop<string>('create_live_session', {
      request: {
        topic,
        channel_ref: channelRef,
        title,
        description,
      } satisfies CreateLiveSessionRequest,
    });
  }),
  endLiveSession: command('endLiveSession', async (topic, sessionId) => {
    return invokeDesktop<void>('end_live_session', {
      request: {
        topic,
        session_id: sessionId,
      } satisfies LiveSessionCommandRequest,
    });
  }),
  joinLiveSession: command('joinLiveSession', async (topic, sessionId) => {
    return invokeDesktop<void>('join_live_session', {
      request: {
        topic,
        session_id: sessionId,
      } satisfies LiveSessionCommandRequest,
    });
  }),
  leaveLiveSession: command('leaveLiveSession', async (topic, sessionId) => {
    return invokeDesktop<void>('leave_live_session', {
      request: {
        topic,
        session_id: sessionId,
      } satisfies LiveSessionCommandRequest,
    });
  }),
  listGameRooms: command('listGameRooms', async (topic, scope = { kind: 'public' }) => {
    return invokeDesktop<GameRoomView[]>('list_game_rooms', {
      request: {
        topic,
        scope,
      } satisfies ListGameRoomsRequest,
    });
  }),
  createGameRoom: command('createGameRoom', async (
    topic,
    title,
    description,
    participants,
    channelRef = { kind: 'public' }
  ) => {
    return invokeDesktop<string>('create_game_room', {
      request: {
        topic,
        channel_ref: channelRef,
        title,
        description,
        participants,
      } satisfies CreateGameRoomRequest,
    });
  }),
  createMetaverseRoom: command('createMetaverseRoom', async (
    topic,
    title,
    description,
    maxPeers = null,
    channelRef = { kind: 'public' }
  ) => {
    return invokeDesktop<string>('create_metaverse_room', {
      request: {
        topic,
        channel_ref: channelRef,
        title,
        description,
        max_peers: maxPeers,
      } satisfies CreateMetaverseRoomRequest,
    });
  }),
  createPrivateChannel: command('createPrivateChannel', async (topic, label, audienceKind = 'invite_only') => {
    return invokeDesktop<JoinedPrivateChannelView>('create_private_channel', {
      request: { topic, label, audience_kind: audienceKind } satisfies CreatePrivateChannelRequest,
    });
  }),
  exportPrivateChannelInvite: command('exportPrivateChannelInvite', async (topic, channelId, expiresAt = null) => {
    return invokeDesktop<string>('export_private_channel_invite', {
      request: {
        topic,
        channel_id: channelId,
        expires_at: expiresAt,
      } satisfies ExportPrivateChannelInviteRequest,
    });
  }),
  importPrivateChannelInvite: command('importPrivateChannelInvite', async (token) => {
    return invokeDesktop<PrivateChannelInvitePreview>('import_private_channel_invite', {
      request: { token } satisfies ImportPrivateChannelInviteRequest,
    });
  }),
  exportChannelAccessToken: command('exportChannelAccessToken', async (topic, channelId, expiresAt = null) => {
    return invokeDesktop<ChannelAccessTokenExport>('export_channel_access_token', {
      request: {
        topic,
        channel_id: channelId,
        expires_at: expiresAt,
      } satisfies ExportChannelAccessTokenRequest,
    });
  }),
  previewChannelAccessToken: command('previewChannelAccessToken', async (token) => {
    return invokeDesktop<ChannelAccessTokenPreview>('preview_channel_access_token', {
      request: {
        token,
      } satisfies PreviewChannelAccessTokenRequest,
    });
  }),
  importChannelAccessToken: command('importChannelAccessToken', async (token) => {
    return invokeDesktop<ChannelAccessTokenPreview>('import_channel_access_token', {
      request: { token } satisfies ImportChannelAccessTokenRequest,
    });
  }),
  exportFriendOnlyGrant: command('exportFriendOnlyGrant', async (topic, channelId, expiresAt = null) => {
    return invokeDesktop<string>('export_friend_only_grant', {
      request: {
        topic,
        channel_id: channelId,
        expires_at: expiresAt,
      } satisfies ExportFriendOnlyGrantRequest,
    });
  }),
  importFriendOnlyGrant: command('importFriendOnlyGrant', async (token) => {
    return invokeDesktop<FriendOnlyGrantPreview>('import_friend_only_grant', {
      request: { token } satisfies ImportFriendOnlyGrantRequest,
    });
  }),
  exportFriendPlusShare: command('exportFriendPlusShare', async (topic, channelId, expiresAt = null) => {
    return invokeDesktop<string>('export_friend_plus_share', {
      request: {
        topic,
        channel_id: channelId,
        expires_at: expiresAt,
      } satisfies ExportFriendPlusShareRequest,
    });
  }),
  importFriendPlusShare: command('importFriendPlusShare', async (token) => {
    return invokeDesktop<FriendPlusSharePreview>('import_friend_plus_share', {
      request: { token } satisfies ImportFriendPlusShareRequest,
    });
  }),
  freezePrivateChannel: command('freezePrivateChannel', async (topic, channelId) => {
    return invokeDesktop<JoinedPrivateChannelView>('freeze_private_channel', {
      request: {
        topic,
        channel_id: channelId,
      } satisfies FreezePrivateChannelRequest,
    });
  }),
  rotatePrivateChannel: command('rotatePrivateChannel', async (topic, channelId) => {
    return invokeDesktop<JoinedPrivateChannelView>('rotate_private_channel', {
      request: {
        topic,
        channel_id: channelId,
      } satisfies RotatePrivateChannelRequest,
    });
  }),
  setPrivateChannelEntryDome: command('setPrivateChannelEntryDome', async (
    topic,
    channelId,
    entryDomeInstanceId
  ) => {
    return invokeDesktop<JoinedPrivateChannelView>('set_private_channel_entry_dome', {
      request: {
        topic,
        channel_id: channelId,
        entry_dome_instance_id: entryDomeInstanceId,
      } satisfies SetPrivateChannelEntryDomeRequest,
    });
  }),
  leavePrivateChannel: command('leavePrivateChannel', async (topic, channelId) => {
    return invokeDesktop<void>('leave_private_channel', {
      request: {
        topic,
        channel_id: channelId,
      } satisfies LeavePrivateChannelRequest,
    });
  }),
  listJoinedPrivateChannels: command('listJoinedPrivateChannels', async (topic) => {
    return invokeDesktop<JoinedPrivateChannelView[]>('list_joined_private_channels', {
      request: { topic } satisfies ListJoinedPrivateChannelsRequest,
    });
  }),
  updateGameRoom: command('updateGameRoom', async (topic, roomId, status, phaseLabel, scores) => {
    return invokeDesktop<void>('update_game_room', {
      request: {
        topic,
        room_id: roomId,
        status,
        phase_label: phaseLabel,
        scores,
      } satisfies UpdateGameRoomRequest,
    });
  }),
  listPendingDomeDeletions: command('listPendingDomeDeletions', async (spatialContext) => invokeDesktop('list_pending_dome_deletions', { spatialContext })),
  deleteDome: command('deleteDome', async (spatialContext, instanceId, generation, operationId) => invokeDesktop<{ deleted: boolean; cleanup_pending: boolean }>('delete_dome', { request: { spatial_context: spatialContext, instance_id: instanceId, expected_generation: generation, operation_id: operationId } satisfies DeleteDomeInput })),
  updateMetaverseRoom: command('updateMetaverseRoom', async (
    topic,
    roomId,
    status,
    customization
  ) => {
    return invokeDesktop<void>('update_metaverse_room', {
      request: {
        topic,
        room_id: roomId,
        status,
        customization,
      } satisfies UpdateMetaverseRoomRequest,
    });
  }),
  getDomeHosting: command('getDomeHosting', async (spatialContext, instanceId) => {
    return invokeDesktop<DomeHostingView>('get_dome_hosting', {
      request: {
        spatial_context: spatialContext,
        instance_id: instanceId,
      } satisfies GetDomeHostingRequest,
    });
  }),
  startOwnerDomeHosting: command('startOwnerDomeHosting', async (
    spatialContext,
    instanceId,
    endpointId,
    leaseDurationMillis,
    expectedGeneration
  ) => {
    return invokeDesktop<DomeHostingView>('start_owner_dome_hosting', {
      request: {
        spatial_context: spatialContext,
        instance_id: instanceId,
        endpoint_id: endpointId,
        lease_duration_millis: leaseDurationMillis,
        expected_generation: expectedGeneration,
      } satisfies StartOwnerDomeHostingRequest,
    });
  }),
  delegateDomeHosting: command('delegateDomeHosting', async (
    spatialContext,
    instanceId,
    nodeId,
    baseUrl,
    leaseDurationMillis,
    expectedGeneration
  ) => {
    return invokeDesktop<DomeHostingView>('delegate_dome_hosting', {
      request: {
        spatial_context: spatialContext,
        instance_id: instanceId,
        node_id: nodeId,
        base_url: baseUrl,
        lease_duration_millis: leaseDurationMillis,
        expected_generation: expectedGeneration,
      } satisfies DelegateDomeHostingRequest,
    });
  }),
  closeDomeHosting: command('closeDomeHosting', async (spatialContext, instanceId, expectedGeneration) => {
    return invokeDesktop<DomeHostingView>('close_dome_hosting', {
      request: {
        spatial_context: spatialContext,
        instance_id: instanceId,
        expected_generation: expectedGeneration,
      } satisfies CloseDomeHostingRequest,
    });
  }),
  submitDomeSessionInput: command('submitDomeSessionInput', async (
    spatialContext,
    instanceId,
    sequence,
    input: DomeSessionInputKindV1,
    expectedGeneration?: number
  ) => {
    return invokeDesktop<DomePhysicsSnapshotV1>('submit_dome_session_input', {
      request: {
        spatial_context: spatialContext,
        instance_id: instanceId,
        sequence,
        input,
        expected_generation: expectedGeneration,
      } satisfies SubmitDomeSessionInputRequest,
    });
  }),
  ...domeTransitionApi,
  ...socialBlockApi,
  ...developerLogsApi,
  ...postReloadApi,
  ...displayDemandApi,
  commitDomeLayout: command('commitDomeLayout', async (
    spatialContext,
    instanceId,
    operationId
  ) => {
    return invokeDesktop<DomeLayoutCommitView>('commit_dome_layout', {
      request: commitDomeLayoutRequest(spatialContext, instanceId, operationId),
    });
  }),
  resyncDomeSnapshots: command('resyncDomeSnapshots', async (
    spatialContext,
    instanceId,
    afterSequence
  ) => {
    return invokeDesktop<DomePhysicsSnapshotV1[]>('resync_dome_snapshots', {
      request: resyncDomeSnapshotsRequest(spatialContext, instanceId, afterSequence),
    });
  }),
  moveDome: command('moveDome', async (
    sourceTopic: string,
    moveId: string,
    sourceInstanceId: string,
    targetContext: SpatialContextV1
  ) => {
    return invokeDesktop<DomeMoveRecordV1>('move_dome', {
      request: {
        source_topic: sourceTopic,
        move_id: moveId,
        source_instance_id: sourceInstanceId,
        target_context: targetContext,
      } satisfies MoveDomeRequest,
    });
  }),
  listDomeConnectionTopology: command('listDomeConnectionTopology', async (
    spatialContext: SpatialContextV1
  ) => {
    return invokeDesktop<DomeConnectionTopologyView>('list_dome_connection_topology', {
      request: {
        spatial_context: spatialContext,
      } satisfies ListDomeConnectionTopologyRequest,
    });
  }),
  createDomeConnectionProposal: command('createDomeConnectionProposal', async (
    proposalId: string,
    spatialContext: SpatialContextV1,
    proposerInstanceId: string,
    receiverInstanceId: string,
    proposerDirection: DomeDirection
  ) => {
    return invokeDesktop<DomeConnectionProposalView>('create_dome_connection_proposal', {
      request: {
        proposal_id: proposalId,
        spatial_context: spatialContext,
        proposer_instance_id: proposerInstanceId,
        receiver_instance_id: receiverInstanceId,
        proposer_direction: proposerDirection,
      } satisfies CreateDomeConnectionProposalRequest,
    });
  }),
  acceptDomeConnectionProposal: command('acceptDomeConnectionProposal', async (
    spatialContext: SpatialContextV1,
    proposalId: string
  ) => {
    return invokeDesktop<DomeConnectionView>('accept_dome_connection_proposal', {
      request: {
        spatial_context: spatialContext,
        proposal_id: proposalId,
      } satisfies AcceptDomeConnectionProposalRequest,
    });
  }),
  withdrawDomeConnectionProposal: command('withdrawDomeConnectionProposal', async (
    spatialContext: SpatialContextV1,
    proposalId: string
  ) => {
    return invokeDesktop<DomeConnectionProposalView>('withdraw_dome_connection_proposal', {
      request: {
        spatial_context: spatialContext,
        proposal_id: proposalId,
      } satisfies WithdrawDomeConnectionProposalRequest,
    });
  }),
  revokeDomeConnection: command('revokeDomeConnection', async (
    spatialContext: SpatialContextV1,
    connectionId: string
  ) => {
    return invokeDesktop<DomeConnectionView>('revoke_dome_connection', {
      request: {
        spatial_context: spatialContext,
        connection_id: connectionId,
      } satisfies RevokeDomeConnectionRequest,
    });
  }),
  publishMetaverseRoomEvent: command('publishMetaverseRoomEvent', async (topic, roomId, peerId, seq, event) => {
    return invokeDesktop<MetaverseRoomEventView>('publish_metaverse_room_event', {
      request: {
        topic,
        room_id: roomId,
        peer_id: peerId,
        seq,
        event,
      } satisfies PublishMetaverseRoomEventRequest,
    });
  }),
  listMetaverseRoomEvents: command('listMetaverseRoomEvents', async (topic, roomId, afterEnvelopeId = null, limit = null) => {
    return invokeDesktop<MetaverseRoomEventView[]>('list_metaverse_room_events', {
      request: {
        topic,
        room_id: roomId,
        after_envelope_id: afterEnvelopeId,
        limit,
      } satisfies ListMetaverseRoomEventsRequest,
    });
  }),
  importMetaverseRoomAsset: command('importMetaverseRoomAsset', async (topic, roomId, kind, mimeType, name, dataBase64) => {
    return invokeDesktop<MetaverseAssetRef>('import_metaverse_room_asset', {
      request: {
        topic,
        room_id: roomId,
        kind,
        mime_type: mimeType,
        name,
        data_base64: dataBase64,
      } satisfies ImportMetaverseRoomAssetRequest,
    });
  }),
  getSyncStatus: command('getSyncStatus', async () => {
    return invokeDesktop<SyncStatus>('get_sync_status');
  }),
  getDiscoveryConfig: command('getDiscoveryConfig', async () => {
    return invokeDesktop<DiscoveryConfig>('get_discovery_config');
  }),
  getCommunityNodeConfig: command('getCommunityNodeConfig', async () => {
    return invokeDesktop<CommunityNodeConfig>('get_community_node_config');
  }),
  getCommunityNodeStatuses: command('getCommunityNodeStatuses', async () => {
    return invokeDesktop<CommunityNodeNodeStatus[]>('get_community_node_statuses');
  }),
  setCommunityNodeConfig: command('setCommunityNodeConfig', async (nodes, trustNodePriority) => {
    return invokeDesktop<CommunityNodeConfig>('set_community_node_config', {
      request: {
        nodes,
        trust_node_priority: trustNodePriority ?? null,
      } satisfies SetCommunityNodeConfigRequest,
    });
  }),
  clearCommunityNodeConfig: command('clearCommunityNodeConfig', async () => {
    return invokeDesktop<void>('clear_community_node_config');
  }),
  authenticateCommunityNode: command('authenticateCommunityNode', async (baseUrl) => {
    return invokeDesktop<CommunityNodeNodeStatus>('authenticate_community_node', {
      request: {
        base_url: baseUrl,
      } satisfies CommunityNodeTargetRequest,
    });
  }),
  setCommunityNodeInviteCode: command(
    'setCommunityNodeInviteCode',
    async (baseUrl, inviteCode) => {
      return invokeDesktop<CommunityNodeNodeStatus>('set_community_node_invite_code', {
        request: {
          base_url: baseUrl,
          invite_code: inviteCode,
        } satisfies SetCommunityNodeInviteCodeRequest,
      });
    }
  ),
  clearCommunityNodeToken: command('clearCommunityNodeToken', async (baseUrl) => {
    return invokeDesktop<CommunityNodeNodeStatus>('clear_community_node_token', {
      request: {
        base_url: baseUrl,
      } satisfies CommunityNodeTargetRequest,
    });
  }),
  fetchCommunityNodePolicies: command('fetchCommunityNodePolicies', async (baseUrl, language) => {
    return invokeDesktop<CommunityNodePoliciesResponse>('fetch_community_node_policies', {
      request: {
        base_url: baseUrl,
        language,
      } satisfies FetchCommunityNodePoliciesRequest,
    });
  }),
  acceptCommunityNodeConsents: command(
    'acceptCommunityNodeConsents',
    async (baseUrl, documents, language) => {
      return invokeDesktop<CommunityNodeNodeStatus>('accept_community_node_consents', {
        request: {
          base_url: baseUrl,
          documents,
          language,
        } satisfies AcceptCommunityNodeConsentsRequest,
      });
    }
  ),
  withdrawCommunityNodeConsents: command('withdrawCommunityNodeConsents', async (baseUrl) => {
    return invokeDesktop<CommunityNodeNodeStatus>('withdraw_community_node_consents', {
      request: {
        base_url: baseUrl,
      } satisfies CommunityNodeTargetRequest,
    });
  }),
  refreshCommunityNodeMetadata: command('refreshCommunityNodeMetadata', async (baseUrl) => {
    return invokeDesktop<CommunityNodeNodeStatus>('refresh_community_node_metadata', {
      request: {
        base_url: baseUrl,
      } satisfies CommunityNodeTargetRequest,
    });
  }),
  fetchCommunityNodeManifest: command('fetchCommunityNodeManifest', async (baseUrl) => {
    return invokeDesktop<CommunityNodeManifestFetch>('fetch_community_node_manifest', {
      request: {
        base_url: baseUrl,
      } satisfies CommunityNodeTargetRequest,
    });
  }),
  lookupCommunityNodeContentAdvisories: command('lookupCommunityNodeContentAdvisories', (request) =>
    invokeDesktop('lookup_community_node_content_advisories', { request })),
  readCommunityNodeTrustUser: command('readCommunityNodeTrustUser', async (request) => {
    return invokeDesktop<TrustUserReadResponse>('read_community_node_trust_user', {
      request: request satisfies CommunityNodeUserAdvisoryRequest,
    });
  }),
  readCommunityNodeRelationUser: command('readCommunityNodeRelationUser', async (request) => {
    return invokeDesktop<RelationReadResponse>('read_community_node_relation_user', {
      request: request satisfies CommunityNodeUserAdvisoryRequest,
    });
  }),
  listCommunityNodeRelationNeighbors: command('listCommunityNodeRelationNeighbors', (request) =>
    invokeDesktop<RelationNeighborsResponse>('list_community_node_relation_neighbors', {
      request: request satisfies CommunityNodeRelationNeighborsRequest,
    })),
  evaluateAuthorTrustGates: command('evaluateAuthorTrustGates', (request) =>
    invokeDesktop<AuthorTrustGateResult>('evaluate_author_trust_gates', {
      request: request satisfies AuthorTrustGateRequest,
    })),
  setAuthorTrustDisplayException: command(
    'setAuthorTrustDisplayException',
    (authorPubkey, alwaysVisible) =>
      invokeDesktop<AuthorTrustGate>('set_author_trust_display_exception', {
        request: { author_pubkey: authorPubkey, always_visible: alwaysVisible },
      })
  ),
  listAuthorTrustDisplayExceptions: command('listAuthorTrustDisplayExceptions', () =>
    invokeDesktop<string[]>('list_author_trust_display_exceptions', {})),
  getCommunityNodeObservationSharing: command('getCommunityNodeObservationSharing', (baseUrl) =>
    invokeDesktop<CommunityNodeObservationSharingStatus>('get_community_node_observation_sharing', {
      request: { base_url: baseUrl } satisfies CommunityNodeTargetRequest,
    })),
  enableCommunityNodeObservationSharing: command('enableCommunityNodeObservationSharing', (request) =>
    invokeDesktop<CommunityNodeObservationSharingStatus>(
      'enable_community_node_observation_sharing',
      { request: request satisfies EnableCommunityNodeObservationSharingRequest }
    )),
  disableCommunityNodeObservationSharing: command('disableCommunityNodeObservationSharing', (baseUrl) =>
    invokeDesktop<CommunityNodeObservationSharingStatus>(
      'disable_community_node_observation_sharing',
      { request: { base_url: baseUrl } satisfies CommunityNodeTargetRequest }
    )),
  getCommunityNodeRelationOptout: command('getCommunityNodeRelationOptout', async (baseUrl) => {
    return invokeDesktop<RelationOptoutResponse>('get_community_node_relation_optout', {
      request: { base_url: baseUrl } satisfies CommunityNodeTargetRequest,
    });
  }),
  setCommunityNodeRelationOptout: command('setCommunityNodeRelationOptout', async (baseUrl) => {
    return invokeDesktop<RelationOptoutResponse>('set_community_node_relation_optout', {
      request: { base_url: baseUrl } satisfies CommunityNodeTargetRequest,
    });
  }),
  clearCommunityNodeRelationOptout: command('clearCommunityNodeRelationOptout', async (baseUrl) => {
    return invokeDesktop<RelationOptoutResponse>('clear_community_node_relation_optout', {
      request: { base_url: baseUrl } satisfies CommunityNodeTargetRequest,
    });
  }),
  searchCommunityNodeIndex: command('searchCommunityNodeIndex', (request) =>
    invokeDesktop<IndexQueryResponse>('search_community_node_index', {
      request: request satisfies CommunityNodeIndexQueryRequest,
    })),
  discoverCommunityNodeIndex: command('discoverCommunityNodeIndex', (request) =>
    invokeDesktop<IndexQueryResponse>('discover_community_node_index', {
      request: request satisfies CommunityNodeIndexQueryRequest,
    })),
  recommendCommunityNodeIndex: command('recommendCommunityNodeIndex', (request) =>
    invokeDesktop<IndexQueryResponse>('recommend_community_node_index', {
      request: request satisfies CommunityNodeIndexQueryRequest,
    })),
  submitCommunityNodeIndexingRequest: command(
    'submitCommunityNodeIndexingRequest',
    async (request) => {
      return invokeDesktop<SubmitIndexingRequestResponse>(
        'submit_community_node_indexing_request',
        {
          request: request satisfies CommunityNodeIndexingRequest,
        }
      );
    }
  ),
  revokeCommunityNodeIndexingRequest: command('revokeCommunityNodeIndexingRequest', (request) =>
    invokeDesktop<void>('revoke_community_node_indexing_request', {
      request: request satisfies CommunityNodeIndexingRequest,
    })),
  readCommunityNodeIndexingStatus: command('readCommunityNodeIndexingStatus', (request) =>
    invokeDesktop<IndexingStatusResponse>('read_community_node_indexing_status', {
      request: request satisfies CommunityNodeIndexingStatusRequest,
    })),
  submitCommunityNodeReport: command('submitCommunityNodeReport', async (request) => {
    return invokeDesktop<SubmitCommunityNodeReportResult>('submit_community_node_report', {
      request,
    });
  }),
  submitCommunityNodeTesterFeedback: command(
    'submitCommunityNodeTesterFeedback',
    async (request) => {
      return invokeDesktop<CommunityNodeTesterFeedbackResponse>(
        'submit_community_node_tester_feedback',
        {
          request: request satisfies CommunityNodeTesterFeedbackSubmission,
        }
      );
    }
  ),
  importPeerTicket: command('importPeerTicket', async (ticket) => {
    return invokeDesktop<void>('import_peer_ticket', {
      request: {
        ticket,
      } satisfies ImportPeerTicketRequest,
    });
  }),
  setDiscoverySeeds: command('setDiscoverySeeds', async (seedEntries) => {
    return invokeDesktop<DiscoveryConfig>('set_discovery_seeds', {
      request: {
        seed_entries: seedEntries,
      } satisfies SetDiscoverySeedsRequest,
    });
  }),
  unsubscribeTopic: command('unsubscribeTopic', async (topic) => {
    return invokeDesktop<void>('unsubscribe_topic', {
      request: {
        topic,
      } satisfies UnsubscribeTopicRequest,
    });
  }),
  setTopicGossipEnabled: command('setTopicGossipEnabled', async (topic, enabled) => {
    return invokeDesktop<void>('set_topic_gossip_enabled', {
      request: {
        topic,
        enabled,
      } satisfies SetTopicGossipEnabledRequest,
    });
  }),
  setChannelGossipEnabled: command('setChannelGossipEnabled', async (topic, channelId, enabled) => {
    return invokeDesktop<void>('set_channel_gossip_enabled', {
      request: {
        topic,
        channel: channelId,
        enabled,
      } satisfies SetChannelGossipEnabledRequest,
    });
  }),
  getLocalPeerTicket: command('getLocalPeerTicket', async () => {
    return invokeDesktop<string | null>('get_local_peer_ticket');
  }),
  getBlobMediaPayload: command('getBlobMediaPayload', async (hash, mime, sourceObjectId) => {
    return invokeDesktop<BlobMediaPayload | null>('get_blob_media_payload', {
      request: {
        hash,
        mime,
        source_object_id: sourceObjectId ?? null,
      } satisfies GetBlobMediaRequest,
    });
  }),
  getBlobMediaFile: command('getBlobMediaFile', async (hash, mime, sourceObjectId, requestId) => {
    return invokeDesktop<BlobMediaFile | null>('get_blob_media_file', {
      request: { hash, mime, source_object_id: sourceObjectId ?? null } satisfies GetBlobMediaRequest,
      requestId,
    });
  }),
  releaseBlobMediaFile: command('releaseBlobMediaFile', async (requestId) => {
    await invokeDesktop<void>('release_blob_media_file', { requestId });
  }),
  getBlobPreviewUrl: command('getBlobPreviewUrl', async (hash, mime, metaverseKind) => {
    return invokeDesktop<string | null>('get_blob_preview_url', {
      request: {
        hash,
        mime,
        metaverse_kind: metaverseKind ?? null,
      } satisfies GetBlobPreviewRequest,
    });
  }),
  // #858: 成人向け表現の表示設定(既定 OFF、canonical は Rust 側ローカル JSON)。
  getContentDisplaySettings: command('getContentDisplaySettings', async () => {
    return invokeDesktop<ContentDisplaySettings>('get_content_display_settings');
  }),
  setAdultContentDisplayEnabled: command('setAdultContentDisplayEnabled', async (enabled) => {
    return invokeDesktop<ContentDisplaySettings>('set_adult_content_display_enabled', {
      enabled,
    });
  }),
};
