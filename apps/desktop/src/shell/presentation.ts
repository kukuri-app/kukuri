import {
  type AuthorSocialView,
  type ChannelAccessTokenPreview,
  type ChannelRef,
  type CommunityNodeConfig,
  type CommunityNodeConfigInput,
  type CommunityNodeNodeStatus,
  type DirectMessageConversationView,
  type DiscoveryConfig,
  type GameRoomView,
  type JoinedPrivateChannelView,
  type LiveSessionView,
  type PostView,
  type Profile,
  type ProfileInput,
  type ReactionStateView,
  type SyncStatus,
  type TimelineScope,
  type TopicSyncStatus,
} from '@/lib/api';
import { TRUST_OBSERVATION_SHARING_POLICY_SLUG } from '@/lib/api/observationSharing';
import { consentPolicyOrder, isConsentDialogHiddenPolicy } from '@/lib/api/policyKind';
import type {
  CommunityNodeConsentPolicyView,
  CommunityNodeConsentView,
} from '@/components/settings/types';
import i18n from '@/i18n';
import {
  formatLocalizedBytes,
  formatLocalizedDateTime,
  formatLocalizedNumber,
  formatLocalizedTime,
} from '@/i18n/format';

import {
  type CommunityNodeDraftNode,
  type CommunityNodePoliciesEntry,
  type GameEditorDraft,
  type KnownAuthorsByPubkey,
  PUBLIC_CHANNEL_REF,
  PUBLIC_TIMELINE_SCOPE,
} from './store';

function translate(key: string, options?: Record<string, unknown>): string {
  return i18n.t(key, options) as string;
}

export function formatBytes(bytes: number, locale?: string | null): string {
  return formatLocalizedBytes(bytes, locale);
}

export function shortPubkey(pubkey: string): string {
  return pubkey.slice(0, 12);
}

export function isHex64(value: string): boolean {
  return value.length === 64 && [...value].every((character) => character.match(/[0-9a-f]/i));
}

export function messageFromError(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

export function profileInputFromProfile(profile: Profile): ProfileInput {
  return {
    name: profile.name ?? '',
    display_name: profile.display_name ?? '',
    about: profile.about ?? '',
    picture_upload: null,
    clear_picture: false,
  };
}

export function resolveProfilePictureSrc(
  profile:
    | Pick<Profile, 'picture_asset'>
    | Pick<AuthorSocialView, 'picture_asset'>
    | null
    | undefined,
  mediaObjectUrls: Record<string, string | null>
): string | null {
  const pictureAssetHash = profile?.picture_asset?.hash;
  if (pictureAssetHash && typeof mediaObjectUrls[pictureAssetHash] === 'string') {
    return mediaObjectUrls[pictureAssetHash];
  }
  return null;
}

export function authorDisplayLabel(
  _authorPubkey: string,
  displayName?: string | null,
  name?: string | null
): string {
  return displayName?.trim() || name?.trim() || translate('common:fallbacks.unknownAuthor');
}

export function publishedTopicIdForPost(
  post: Pick<PostView, 'published_topic_id' | 'origin_topic_id'>
): string | null {
  return post.published_topic_id?.trim() || post.origin_topic_id?.trim() || null;
}

export function patchReactionStateIntoPosts(
  posts: PostView[],
  reactionState: ReactionStateView
): PostView[] {
  return posts.map((post) =>
    post.object_id === reactionState.target_object_id
      ? {
          ...post,
          reaction_summary: reactionState.reaction_summary,
          my_reactions: reactionState.my_reactions,
        }
      : post
  );
}

export function canCreateRepostFromPost(post: PostView): boolean {
  return (post.object_kind === 'post' || post.object_kind === 'comment') && !post.channel_id;
}

export function isQuoteRepost(
  post: Pick<PostView, 'object_kind' | 'repost_commentary'>
): boolean {
  return post.object_kind === 'repost' && Boolean(post.repost_commentary?.trim());
}

export function formatListLabel(values: string[]): string {
  return values.length > 0 ? values.join(', ') : translate('common:fallbacks.none');
}

export function formatLastReceivedLabel(
  timestamp?: number | null,
  locale?: string | null
): string {
  return timestamp
    ? formatLocalizedTime(timestamp, locale)
    : translate('common:fallbacks.noEvents');
}

export function strongestRelationshipLabel(relationship: {
  mutual: boolean;
  following: boolean;
  followed_by: boolean;
  friend_of_friend: boolean;
}): string | null {
  if (relationship.mutual) {
    return 'mutual';
  }
  if (relationship.following) {
    return 'following';
  }
  if (relationship.followed_by) {
    return 'follows you';
  }
  if (relationship.friend_of_friend) {
    return 'friend of friend';
  }
  return null;
}

export function mergeAuthorView(
  current: AuthorSocialView | null | undefined,
  incoming: Partial<AuthorSocialView> & { author_pubkey: string }
): AuthorSocialView {
  return {
    author_pubkey: incoming.author_pubkey,
    name: incoming.name ?? current?.name ?? null,
    display_name: incoming.display_name ?? current?.display_name ?? null,
    about: incoming.about ?? current?.about ?? null,
    picture_asset: incoming.picture_asset ?? current?.picture_asset ?? null,
    updated_at: incoming.updated_at ?? current?.updated_at ?? null,
    following: incoming.following ?? current?.following ?? false,
    followed_by: incoming.followed_by ?? current?.followed_by ?? false,
    mutual: incoming.mutual ?? current?.mutual ?? false,
    friend_of_friend: incoming.friend_of_friend ?? current?.friend_of_friend ?? false,
    friend_of_friend_via_pubkeys:
      incoming.friend_of_friend_via_pubkeys ?? current?.friend_of_friend_via_pubkeys ?? [],
    muted: incoming.muted ?? current?.muted ?? false,
    blocking: incoming.blocking ?? current?.blocking ?? false,
    blocked_by: incoming.blocked_by ?? current?.blocked_by ?? false,
  };
}

export function mergeKnownAuthors(
  current: KnownAuthorsByPubkey,
  incoming: Array<(Partial<AuthorSocialView> & { author_pubkey: string }) | null | undefined>
): KnownAuthorsByPubkey {
  let next = current;
  for (const view of incoming) {
    if (!view) {
      continue;
    }
    const merged = mergeAuthorView(next[view.author_pubkey], view);
    if (next === current) {
      next = { ...current };
    }
    next[view.author_pubkey] = merged;
  }
  return next;
}

export function authorViewFromDirectMessageConversation(
  conversation: DirectMessageConversationView
): AuthorSocialView {
  return {
    author_pubkey: conversation.peer_pubkey,
    name: conversation.peer_name ?? null,
    display_name: conversation.peer_display_name ?? null,
    about: null,
    picture_asset: conversation.peer_picture_asset ?? null,
    updated_at: null,
    following: false,
    followed_by: false,
    mutual: conversation.status.mutual,
    friend_of_friend: false,
    friend_of_friend_via_pubkeys: [],
    muted: false,
    blocking: false,
    blocked_by: false,
  };
}

export function privateTimelineScope(channelId: string | null): TimelineScope {
  return channelId
    ? {
        kind: 'channel',
        channel_id: channelId,
      }
    : PUBLIC_TIMELINE_SCOPE;
}

export function privateComposeTarget(channelId: string | null): ChannelRef {
  return channelId
    ? {
        kind: 'private_channel',
        channel_id: channelId,
      }
    : PUBLIC_CHANNEL_REF;
}

export function audienceLabelForChannelRef(
  channelRef: ChannelRef,
  joinedChannels: JoinedPrivateChannelView[]
): string {
  if (channelRef.kind === 'public') {
    return translate('common:audience.public');
  }
  return (
    joinedChannels.find((channel) => channel.channel_id === channelRef.channel_id)?.label ??
    translate('common:audience.privateChannel')
  );
}

export function audienceLabelForTimelineScope(
  scope: TimelineScope,
  joinedChannels: JoinedPrivateChannelView[]
): string {
  if (scope.kind === 'all_joined') {
    return translate('common:audience.allJoined');
  }
  if (scope.kind === 'channel') {
    return (
      joinedChannels.find((channel) => channel.channel_id === scope.channel_id)?.label ??
      translate('common:audience.privateChannel')
    );
  }
  return translate('common:audience.public');
}

export function formatSeedPeer(peer: DiscoveryConfig['seed_peers'][number]): string {
  return peer.addr_hint ? `${peer.endpoint_id}@${peer.addr_hint}` : peer.endpoint_id;
}

export function seedPeersToEditorValue(config: DiscoveryConfig): string {
  return config.seed_peers.map((peer) => formatSeedPeer(peer)).join('\n');
}

export function communityNodesToDraftNodes(config: CommunityNodeConfig): CommunityNodeDraftNode[] {
  return config.nodes.map((node, index) => ({
    id: `community-node-${index}-${node.base_url}`,
    base_url: node.base_url,
    content_advisory_enabled: node.content_advisory_enabled !== false,
  }));
}

export function communityNodeDraftNodesToConfigInput(
  draftNodes: CommunityNodeDraftNode[]
): CommunityNodeConfigInput[] {
  return draftNodes
    .map((node) => ({
      base_url: node.base_url.trim(),
      content_advisory_enabled: node.content_advisory_enabled,
    }))
    .filter((node) => node.base_url.length > 0);
}

export function syncStatusBadgeTone(
  syncStatus: SyncStatus
): 'accent' | 'destructive' | 'warning' {
  if (syncStatus.last_error) {
    return 'destructive';
  }
  return syncStatus.delivery_state === 'Live' ? 'accent' : 'warning';
}

export function syncStatusBadgeLabel(syncStatus: SyncStatus): string {
  switch (syncStatus.delivery_state) {
    case 'Live':
      return translate('common:states.connected');
    case 'DurableReady':
      return translate('settings:connectionGuidance.states.durable');
    case 'DurableRecovering':
      return translate('settings:connectionGuidance.states.recovering');
    case 'Offline':
    default:
      return translate('common:states.waiting');
  }
}

export function topicConnectionLabel(diagnostic?: TopicSyncStatus): string {
  if (!diagnostic) {
    return 'idle';
  }
  if (diagnostic.peer_count > 0 && diagnostic.active_path === 'relay_fallback') {
    return 'relay fallback';
  }
  if (diagnostic.peer_count > 0 && diagnostic.active_path === 'relay_supported_p2p') {
    return 'relay-supported P2P';
  }
  switch (diagnostic.delivery_state) {
    case 'Live':
      return 'joined';
    case 'DurableReady':
      return translate('settings:connectionGuidance.states.durable');
    case 'DurableRecovering':
      return translate('settings:connectionGuidance.states.recovering');
    case 'Offline':
    default:
      return diagnostic.joined ? 'joined' : 'idle';
  }
}

export function communityNodeConnectivityUrlsLabel(
  status?: CommunityNodeNodeStatus
): string {
  if (status?.resolved_urls?.connectivity_urls?.length) {
    return status.resolved_urls.connectivity_urls.join(', ');
  }
  if (status?.consent_state && !status.consent_state.all_required_accepted) {
    return translate('settings:communityNode.values.pendingConsentAcceptance');
  }
  if (status?.auth_state.authenticated) {
    return translate('settings:communityNode.values.notResolvedYet');
  }
  return translate('settings:communityNode.values.notResolved');
}

export function communityNodeSessionPhaseLabel(status?: CommunityNodeNodeStatus): string {
  if (!status?.session_phase) {
    return translate('common:states.unknown');
  }
  return translate(`settings:communityNode.sessionPhases.${status.session_phase}`);
}

export function communityNodeRetryAfterLabel(status?: CommunityNodeNodeStatus): string {
  if (!status?.retry_after) {
    return translate('common:fallbacks.none');
  }
  return formatLocalizedTime(status.retry_after);
}

export function communityNodeNextStepLabel(status?: CommunityNodeNodeStatus): string {
  if (!status) {
    return translate('settings:communityNode.values.saveNodesToBegin');
  }
  // 参加拒否は認証状態の表示に関わらず利用者の操作待ちなので最優先で案内する(#708)。
  if (status.admission_rejection) {
    return translate(
      `settings:communityNode.admission.nextSteps.${status.admission_rejection.code}`
    );
  }
  // #857: 同意はローカル記録が SSoT。未同意/撤回/再同意待ちなら認証より先に同意を促す。
  if (
    !status.local_consent?.records.length ||
    status.local_consent.withdrawn_at != null ||
    status.consent_update_pending
  ) {
    return translate('settings:communityNode.values.acceptPolicies');
  }
  if (!status.auth_state.authenticated) {
    return translate('settings:communityNode.values.authenticateThisNode');
  }
  if (status.consent_state && !status.consent_state.all_required_accepted) {
    return translate('settings:communityNode.values.acceptPolicies');
  }
  if (status.restart_required) {
    return translate('settings:communityNode.values.restartUnexpected');
  }
  if (!status.resolved_urls) {
    return translate('settings:communityNode.values.refreshMetadata');
  }
  return translate('settings:communityNode.values.connectivityUrlsActiveOnCurrentSession');
}

export function communityNodeSessionActivationLabel(
  status?: CommunityNodeNodeStatus
): string {
  if (!status) {
    return translate('common:states.unknown');
  }
  if (status.restart_required) {
    return translate('settings:communityNode.values.restartRequiredUnexpected');
  }
  if (status.resolved_urls?.connectivity_urls?.length) {
    return translate('settings:communityNode.values.activeOnCurrentSession');
  }
  if (status.consent_state && !status.consent_state.all_required_accepted) {
    return translate('settings:communityNode.values.waitingForConsent');
  }
  if (status.auth_state.authenticated) {
    return translate('settings:communityNode.values.awaitingConnectivityMetadata');
  }
  return translate('settings:communityNode.values.notAuthenticated');
}

export function communityNodeAuthLabel(status?: CommunityNodeNodeStatus): string {
  return status?.auth_state.authenticated
    ? `${translate('common:states.yes')} (${status.auth_state.expires_at ?? translate('common:states.unknown')})`
    : translate('common:states.no');
}

// #857: 同意状態はローカル同意記録(local_consent)を SSoT として表示する。
export function communityNodeConsentLabel(status?: CommunityNodeNodeStatus): string {
  const localConsent = status?.local_consent;
  if (!localConsent || localConsent.records.length === 0) {
    return translate('common:states.required');
  }
  if (localConsent.withdrawn_at != null) {
    return translate('settings:communityNode.values.consentWithdrawn');
  }
  if (status?.consent_update_pending) {
    return translate('common:states.required');
  }
  return translate('common:states.accepted');
}

function formatConsentAcceptedAt(value: number | null | undefined): string | null {
  if (!value) {
    return null;
  }
  return formatLocalizedDateTime(value * 1000);
}

// per-node consent ダイアログ（#384 / #857）用の view を組み立てる。
// 提示内容は認証不要の公開 policy カタログ、受諾状態はローカル同意記録から導く。
export function communityNodeConsentView(
  status: CommunityNodeNodeStatus | undefined,
  policiesEntry: CommunityNodePoliciesEntry | undefined
): CommunityNodeConsentView {
  const localConsent = status?.local_consent ?? { records: [], withdrawn_at: null };
  const withdrawn = localConsent.withdrawn_at != null;
  // #1061: 観測提供の任意文書は、CN 設定の専用トグルでだけ同意する。一括受諾の一覧には出さない。
  // #1192: 権利侵害申出ポリシーは権利侵害申請モーダルで提示するため同様に外す。
  // 並びは 利用規約 → プライバシーポリシー → 残り（既存の slug 昇順）で安定させる。
  const catalog = (policiesEntry?.status === 'ok' ? policiesEntry.policies : [])
    .filter(
      (policy) =>
        policy.policy_slug !== TRUST_OBSERVATION_SHARING_POLICY_SLUG &&
        !isConsentDialogHiddenPolicy(policy.policy_kind, policy.required)
    )
    .map((policy, index) => ({ policy, index }))
    .sort((left, right) =>
      consentPolicyOrder(left.policy.policy_kind) - consentPolicyOrder(right.policy.policy_kind) ||
      left.index - right.index
    )
    .map((entry) => entry.policy);
  const policies: CommunityNodeConsentPolicyView[] = catalog.map((policy) => {
    const slugRecords = localConsent.records.filter(
      (record) => record.policy_slug === policy.policy_slug
    );
    const matchingRecords = slugRecords.filter(
      (record) =>
        policy.policy_snapshot_revision == null ||
        record.policy_snapshot_revision === policy.policy_snapshot_revision
    );
    const recordVersions = matchingRecords.map((record) => record.policy_version);
    const highestAccepted = recordVersions.length ? Math.max(...recordVersions) : null;
    const accepted = !withdrawn && highestAccepted != null && highestAccepted >= policy.policy_version;
    const acceptedRecord = matchingRecords.find(
      (record) =>
        record.policy_slug === policy.policy_slug && record.policy_version === highestAccepted
    );
    const previousVersions = slugRecords.map((record) => record.policy_version);
    const previousVersion = previousVersions.length ? Math.max(...previousVersions) : null;
    const previouslyAcceptedVersion = !accepted ? previousVersion : null;
    return {
      policySlug: policy.policy_slug,
      title: policy.title,
      body: policy.body_markdown,
      policyVersion: policy.policy_version,
      effectiveDate: policy.effective_date ?? null,
      language: policy.language ?? null,
      policySnapshotRevision: policy.policy_snapshot_revision ?? null,
      authoritativeLanguage: policy.authoritative_language ?? null,
      referenceTranslation: policy.reference_translation ?? false,
      fallback: policy.fallback ?? false,
      required: policy.required,
      policyKind: policy.policy_kind ?? null,
      acceptedAtLabel: accepted ? formatConsentAcceptedAt(acceptedRecord?.accepted_at) : null,
      // 旧版または旧 snapshot だけ同意済み = 再同意が必要な「更新」。
      updated: !accepted && previouslyAcceptedVersion != null,
      previouslyAcceptedVersion,
    };
  });
  const requiredPolicies = policies.filter((policy) => policy.required);
  return {
    loaded: policiesEntry?.status === 'ok',
    loading: policiesEntry?.status === 'loading',
    loadError: policiesEntry?.status === 'error' ? policiesEntry.error : null,
    withdrawn,
    allRequiredAccepted:
      !withdrawn &&
      policiesEntry?.status === 'ok' &&
      requiredPolicies.every((policy) => policy.acceptedAtLabel != null),
    hasPendingUpdate: requiredPolicies.some((policy) => policy.updated),
    hasLocalConsent: localConsent.records.length > 0 && !withdrawn,
    policies,
  };
}

export function translateTopicConnectionText(label: string): string {
  if (label === 'joined') {
    return translate('common:states.joined');
  }
  if (label === 'durable') {
    return 'durable';
  }
  if (label === 'recovering') {
    return 'recovering';
  }
  if (label === 'idle') {
    return translate('common:states.idle');
  }
  return label;
}

export function translateLiveStatus(status: LiveSessionView['status']): string {
  return translate(`live:statuses.${status}`);
}

export function formatCount(value: number): string {
  return formatLocalizedNumber(value);
}

export function localizeAudienceLabel(label: string): string {
  if (label === 'Public') {
    return translate('common:audience.public');
  }
  if (label === 'All joined') {
    return translate('common:audience.allJoined');
  }
  if (label === 'Private channel') {
    return translate('common:audience.privateChannel');
  }
  return label;
}

export function mergeCommunityNodeStatus(
  previous: CommunityNodeNodeStatus | undefined,
  next: CommunityNodeNodeStatus
): CommunityNodeNodeStatus {
  return {
    ...next,
    consent_state: next.auth_state.authenticated
      ? next.consent_state ?? previous?.consent_state ?? null
      : next.consent_state ?? null,
    resolved_urls: next.resolved_urls ?? previous?.resolved_urls ?? null,
    last_error: next.last_error ?? null,
    session_phase: next.session_phase ?? previous?.session_phase ?? 'idle',
    retry_after: next.retry_after ?? previous?.retry_after ?? null,
  };
}

export function mergeCommunityNodeStatuses(
  previous: CommunityNodeNodeStatus[],
  next: CommunityNodeNodeStatus[]
): CommunityNodeNodeStatus[] {
  const previousByBaseUrl = Object.fromEntries(
    previous.map((status) => [status.base_url, status])
  ) as Record<string, CommunityNodeNodeStatus>;
  return next.map((status) =>
    mergeCommunityNodeStatus(previousByBaseUrl[status.base_url], status)
  );
}

export function upsertCommunityNodeStatus(
  current: CommunityNodeNodeStatus[],
  next: CommunityNodeNodeStatus
): CommunityNodeNodeStatus[] {
  const previous = current.find((status) => status.base_url === next.base_url);
  const merged = mergeCommunityNodeStatus(previous, next);
  const remaining = current.filter((status) => status.base_url !== next.base_url);
  return [...remaining, merged].sort((left, right) =>
    left.base_url.localeCompare(right.base_url)
  );
}

export function syncCommunityNodeConfigWithStatus(
  current: CommunityNodeConfig,
  status: CommunityNodeNodeStatus
): CommunityNodeConfig {
  return {
    nodes: current.nodes.map((node) =>
      node.base_url === status.base_url
        ? {
            ...node,
            resolved_urls: status.resolved_urls ?? node.resolved_urls ?? null,
          }
        : node
    ),
  };
}

export function createGameEditorDraft(room: GameRoomView): GameEditorDraft {
  return {
    status: room.status,
    phase_label: room.phase_label ?? '',
    scores: Object.fromEntries(
      room.scores.map((score) => [score.participant_id, String(score.score)])
    ),
  };
}

export function upsertJoinedChannel(
  channels: JoinedPrivateChannelView[],
  nextChannel: JoinedPrivateChannelView
): JoinedPrivateChannelView[] {
  const remaining = channels.filter((channel) => channel.channel_id !== nextChannel.channel_id);
  return [...remaining, nextChannel];
}

export function joinedChannelFromAccessTokenPreview(
  preview: ChannelAccessTokenPreview
): JoinedPrivateChannelView {
  if (preview.kind === 'grant') {
    return {
      topic_id: preview.topic_id,
      channel_id: preview.channel_id,
      label: preview.channel_label,
      creator_pubkey: preview.owner_pubkey,
      owner_pubkey: preview.owner_pubkey,
      joined_via_pubkey: preview.sponsor_pubkey ?? null,
      audience_kind: 'friend_only',
      is_owner: false,
      current_epoch_id: preview.epoch_id,
      archived_epoch_ids: [],
      sharing_state: 'open',
      rotation_required: false,
      participant_count: 1,
      stale_participant_count: 0,
    };
  }
  if (preview.kind === 'share') {
    return {
      topic_id: preview.topic_id,
      channel_id: preview.channel_id,
      label: preview.channel_label,
      creator_pubkey: preview.owner_pubkey,
      owner_pubkey: preview.owner_pubkey,
      joined_via_pubkey: preview.sponsor_pubkey ?? null,
      audience_kind: 'friend_plus',
      is_owner: false,
      current_epoch_id: preview.epoch_id,
      archived_epoch_ids: [],
      sharing_state: 'open',
      rotation_required: false,
      participant_count: 2,
      stale_participant_count: 0,
    };
  }
  return {
    topic_id: preview.topic_id,
    channel_id: preview.channel_id,
    label: preview.channel_label,
    creator_pubkey: preview.inviter_pubkey ?? preview.owner_pubkey,
    owner_pubkey: preview.owner_pubkey,
    joined_via_pubkey: preview.inviter_pubkey ?? null,
    audience_kind: 'invite_only',
    is_owner: false,
    current_epoch_id: preview.epoch_id,
    archived_epoch_ids: [],
    sharing_state: 'open',
    rotation_required: false,
    participant_count: 1,
    stale_participant_count: 0,
  };
}
