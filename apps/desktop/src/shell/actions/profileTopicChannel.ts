import { setRecordEntry, updateRecordEntry } from '@/shell/stateUpdates';
import type { FormEvent } from 'react';

import type {
  CommunityNodeConsentDocumentRef,
  CommunityNodeNodeStatus,
  JoinedPrivateChannelView,
  ProfileInput,
} from '@/lib/api';
import { fileToCreateAttachment } from '@/lib/attachments';
import { normalizeTopicId } from '@/lib/topicId';
import i18n from '@/i18n';

import {
  DEFAULT_COMMUNITY_NODE_CONFIG,
  PUBLIC_CHANNEL_REF,
  PUBLIC_TIMELINE_SCOPE,
  type DesktopShellStore,
} from '@/shell/store';
import {
  communityNodeDraftNodesToConfigInput,
  communityNodeConsentView,
  communityNodesToDraftNodes,
  joinedChannelFromAccessTokenPreview,
  messageFromError,
  privateComposeTarget,
  privateTimelineScope,
  profileInputFromProfile,
  seedPeersToEditorValue,
  syncCommunityNodeConfigWithStatus,
  upsertCommunityNodeStatus,
  upsertJoinedChannel,
} from '@/shell/presentation';

import type {
  ActionsBaseParams,
  DerivedSetter,
  NullableStringDispatch,
  NumberStateDispatch,
  Setter,
} from './shared';
import { explainScopeLimit } from '@/shell/columnScopeLeases';

type ProfileTopicChannelParams = ActionsBaseParams & {
  refreshProfile: () => Promise<void>;
  getState: () => DesktopShellStore;
  activePrivateChannel: JoinedPrivateChannelView | null;
  activeTopic: string;
  channelAudienceInput: 'invite_only' | 'friend_only' | 'friend_plus';
  channelLabelInput: string;
  communityNodeInput: Array<{ id: string; base_url: string }>;
  discoverySeedInput: string;
  inviteTokenInput: string;
  localProfile: {
    pubkey: string;
    name?: string | null;
    display_name?: string | null;
    about?: string | null;
    picture_asset?: { hash: string; mime: string; bytes: number; role: 'profile_avatar' } | null;
    updated_at: number;
  } | null;
  profileDraft: ProfileInput;
  selectedChannelIdByTopic: Record<string, string | null>;
  selectedThread: string | null;
  topicInput: string;
  trackedTopics: string[];
  clearThreadContext: () => void;
  setProfileAvatarPreviewUrl: NullableStringDispatch;
  setProfileAvatarInputKey: NumberStateDispatch;
  setTrackedTopics: Setter<'trackedTopics'>;
  setActiveTopic: DerivedSetter<string>;
  setTopicInput: Setter<'topicInput'>;
  setTimelineScopeByTopic: Setter<'timelineScopeByTopic'>;
  setComposeChannelByTopic: Setter<'composeChannelByTopic'>;
  setSelectedChannelIdByTopic: DerivedSetter<Record<string, string | null>>;
  setShellChromeState: Setter<'shellChromeState'>;
  setProfileDraft: Setter<'profileDraft'>;
  setProfileDirty: Setter<'profileDirty'>;
  setProfileError: Setter<'profileError'>;
  setProfilePanelState: Setter<'profilePanelState'>;
  setProfileSaving: Setter<'profileSaving'>;
  setLocalProfile: Setter<'localProfile'>;
  setChannelLabelInput: Setter<'channelLabelInput'>;
  setChannelAudienceInput: Setter<'channelAudienceInput'>;
  setInviteTokenInput: Setter<'inviteTokenInput'>;
  setInviteOutput: Setter<'inviteOutput'>;
  setInviteOutputLabel: Setter<'inviteOutputLabel'>;
  setChannelError: Setter<'channelError'>;
  setChannelPanelStateByTopic: Setter<'channelPanelStateByTopic'>;
  setChannelActionPending: Setter<'channelActionPending'>;
  setJoinedChannelsByTopic: Setter<'joinedChannelsByTopic'>;
  setCommunityNodeConfig: Setter<'communityNodeConfig'>;
  setCommunityNodeStatuses: Setter<'communityNodeStatuses'>;
  setCommunityNodePolicies: Setter<'communityNodePolicies'>;
  setCommunityNodeInput: Setter<'communityNodeInput'>;
  setCommunityNodeEditorDirty: Setter<'communityNodeEditorDirty'>;
  setCommunityNodeError: Setter<'communityNodeError'>;
  setDiscoveryConfig: Setter<'discoveryConfig'>;
  setDiscoverySeedInput: Setter<'discoverySeedInput'>;
  setDiscoveryEditorDirty: Setter<'discoveryEditorDirty'>;
  setDiscoveryError: Setter<'discoveryError'>;
};

export function createProfileTopicChannelActions({
  api,
  getState,
  translate,
  loadTopics,
  refreshProfile,
  syncRoute,
  activePrivateChannel,
  activeTopic,
  channelAudienceInput,
  channelLabelInput,
  communityNodeInput,
  discoverySeedInput,
  inviteTokenInput,
  localProfile,
  profileDraft,
  selectedChannelIdByTopic,
  selectedThread,
  topicInput,
  trackedTopics,
  clearThreadContext,
  setProfileAvatarPreviewUrl,
  setProfileAvatarInputKey,
  setTrackedTopics,
  setActiveTopic,
  setTopicInput,
  setTimelineScopeByTopic,
  setComposeChannelByTopic,
  setSelectedChannelIdByTopic,
  setShellChromeState,
  setProfileDraft,
  setProfileDirty,
  setProfileError,
  setProfilePanelState,
  setProfileSaving,
  setLocalProfile,
  setChannelLabelInput,
  setChannelAudienceInput,
  setInviteTokenInput,
  setInviteOutput,
  setInviteOutputLabel,
  setChannelError,
  setChannelPanelStateByTopic,
  setChannelActionPending,
  setJoinedChannelsByTopic,
  setCommunityNodeConfig,
  setCommunityNodeStatuses,
  setCommunityNodePolicies,
  setCommunityNodeInput,
  setCommunityNodeEditorDirty,
  setCommunityNodeError,
  setDiscoveryConfig,
  setDiscoverySeedInput,
  setDiscoveryEditorDirty,
  setDiscoveryError,
}: ProfileTopicChannelParams) {
  async function runCommunityNodeOperation(
    baseUrl: string,
    operation: () => Promise<CommunityNodeNodeStatus>
  ): Promise<CommunityNodeNodeStatus | null> {
    const before = getState();
    if (!before.communityNodeConfig.nodes.some((node) => node.base_url === baseUrl)) return null;
    const baseline = before.communityNodeStatuses.find((node) => node.base_url === baseUrl);
    let response = await operation();
    let current = getState();
    if (current.syncStatus.local_author_pubkey !== before.syncStatus.local_author_pubkey ||
      !current.communityNodeConfig.nodes.some((node) => node.base_url === baseUrl)) return null;
    const latest = current.communityNodeStatuses.find((node) => node.base_url === baseUrl);
    // RPCとevent/pollが競合したら、到着順を鮮度と見なさずmutation後の読取で確定する。
    if (latest !== baseline) {
      const statuses = await api.getCommunityNodeStatuses();
      current = getState();
      if (current.syncStatus.local_author_pubkey !== before.syncStatus.local_author_pubkey ||
        !current.communityNodeConfig.nodes.some((node) => node.base_url === baseUrl)) return null;
      const observed = current.communityNodeStatuses.find((node) => node.base_url === baseUrl);
      if (observed !== latest) return observed ?? null;
      const refreshed = statuses.find((node) => node.base_url === baseUrl);
      if (!refreshed) return null;
      response = refreshed;
    }
    current.patchState({
      communityNodeStatuses: upsertCommunityNodeStatus(current.communityNodeStatuses, response),
      communityNodeConfig: syncCommunityNodeConfigWithStatus(current.communityNodeConfig, response),
    });
    return response;
  }
  function handleProfileFieldChange(field: 'displayName' | 'name' | 'about', value: string) {
    const nextField: keyof ProfileInput = field === 'displayName' ? 'display_name' : field;
    setProfileDraft(setRecordEntry(nextField, value));
    setProfileDirty(true);
  }

  async function handleProfileAvatarFile(file: File) {
    const pictureUpload = await fileToCreateAttachment(file, 'profile_avatar');
    const nextPreviewUrl = URL.createObjectURL(file);
    setProfileAvatarPreviewUrl((current) => {
      if (current) {
        URL.revokeObjectURL(current);
      }
      return nextPreviewUrl;
    });
    setProfileAvatarInputKey((value) => value + 1);
    setProfileDraft((current) => ({
      ...current,
      picture_upload: pictureUpload,
      clear_picture: false,
    }));
    setProfileDirty(true);
    setProfileError(null);
  }

  function handleClearProfileAvatar() {
    setProfileAvatarPreviewUrl((current) => {
      if (current) {
        URL.revokeObjectURL(current);
      }
      return null;
    });
    setProfileAvatarInputKey((value) => value + 1);
    setProfileDraft((current) => ({
      ...current,
      picture_upload: null,
      clear_picture: true,
    }));
    setProfileDirty(true);
    setProfileError(null);
  }

  function resetProfileDraft() {
    if (!localProfile) {
      return;
    }
    setProfileAvatarPreviewUrl((current) => {
      if (current) {
        URL.revokeObjectURL(current);
      }
      return null;
    });
    setProfileAvatarInputKey((value) => value + 1);
    setProfileDraft(profileInputFromProfile(localProfile));
    setProfileDirty(false);
    setProfileError(null);
    setProfilePanelState({
      status: 'ready',
      error: null,
    });
  }

  function handleSelectPrivateChannel(topicId: string, channelId: string) {
    setSelectedChannelIdByTopic(setRecordEntry(topicId, channelId));
    setTimelineScopeByTopic(setRecordEntry(topicId, {
        kind: 'channel',
        channel_id: channelId,
      }));
    setComposeChannelByTopic(setRecordEntry(topicId, {
        kind: 'private_channel',
        channel_id: channelId,
      }));
    setActiveTopic(topicId);
    setShellChromeState((current) => ({
      ...current,
    }));
    syncRoute('replace', {
      activeTopic: topicId,
      primarySection: 'timeline',
      timelineScope: {
        kind: 'channel',
        channel_id: channelId,
      },
      composeTarget: {
        kind: 'private_channel',
        channel_id: channelId,
      },
    });
  }

  async function handleSaveProfile(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    getState().setField('profileSaveRevision', (revision) => revision + 1);
    setProfileSaving(true);
    try {
      const profile = await api.setMyProfile(profileDraft);
      getState().setField('profileSaveRevision', (revision) => revision + 1);
      setProfileAvatarPreviewUrl((current) => {
        if (current) {
          URL.revokeObjectURL(current);
        }
        return null;
      });
      setProfileAvatarInputKey((value) => value + 1);
      setLocalProfile(profile);
      setProfileDraft(profileInputFromProfile(profile));
      setProfileDirty(false);
      setProfileError(null);
      setProfilePanelState({
        status: 'ready',
        error: null,
      });
      setShellChromeState((current) => ({
        ...current,
        profileMode: 'overview',
      }));
      await Promise.all([loadTopics(trackedTopics, activeTopic, selectedThread), refreshProfile()]);
      syncRoute('replace', {
        primarySection: 'profile',
        profileMode: 'overview',
      });
    } catch (saveError) {
      const nextProfileError = messageFromError(
        saveError,
        translate('common:errors.failedToSaveProfile')
      );
      setProfileError(nextProfileError);
      setProfilePanelState({
        status: 'error',
        error: nextProfileError,
      });
    } finally {
      setProfileSaving(false);
    }
  }

  async function handleAddTopic() {
    // 入力は素の名前でも完全な ID でも受け付け、wire 上は常に名前空間付き ID にする。
    const nextTopic = normalizeTopicId(topicInput);
    if (!nextTopic) {
      return;
    }
    const nextTopics = trackedTopics.includes(nextTopic)
      ? trackedTopics
      : [...trackedTopics, nextTopic];
    setTrackedTopics(nextTopics);
    setActiveTopic(nextTopic);
    setTopicInput('');
    setShellChromeState((current) => ({
      ...current,
    }));
    clearThreadContext();
    syncRoute('replace', {
      activeTopic: nextTopic,
      primarySection: 'timeline',
    });
    await loadTopics(nextTopics, nextTopic, null);
  }

  async function handleSelectTopic(topic: string) {
    setActiveTopic(topic);
    setSelectedChannelIdByTopic(setRecordEntry(topic, null));
    setTimelineScopeByTopic(setRecordEntry(topic, PUBLIC_TIMELINE_SCOPE));
    setComposeChannelByTopic(setRecordEntry(topic, PUBLIC_CHANNEL_REF));
    setShellChromeState((current) => ({
      ...current,
    }));
    clearThreadContext();
    syncRoute('replace', {
      activeTopic: topic,
      primarySection: 'timeline',
      timelineScope: PUBLIC_TIMELINE_SCOPE,
      composeTarget: PUBLIC_CHANNEL_REF,
    });
    await loadTopics(trackedTopics, topic, null);
  }

  async function handleOpenOriginalTopic(topicId: string) {
    const nextTopics = trackedTopics.includes(topicId) ? trackedTopics : [...trackedTopics, topicId];
    setTrackedTopics(nextTopics);
    setActiveTopic(topicId);
    setShellChromeState((current) => ({
      ...current,
    }));
    clearThreadContext();
    syncRoute('replace', {
      activeTopic: topicId,
      primarySection: 'timeline',
      timelineScope: privateTimelineScope(selectedChannelIdByTopic[topicId] ?? null),
      composeTarget: privateComposeTarget(selectedChannelIdByTopic[topicId] ?? null),
      selectedAuthorPubkey: null,
      selectedThread: null,
    });
    await loadTopics(nextTopics, topicId, null);
  }

  async function handleRemoveTopic(topic: string) {
    if (trackedTopics.length === 1) {
      return;
    }
    const nextTopics = trackedTopics.filter((value) => value !== topic);
    const nextActiveTopic = activeTopic === topic ? nextTopics[0] : activeTopic;
    await api.unsubscribeTopic(topic);
    setTrackedTopics(nextTopics);
    setActiveTopic(nextActiveTopic);
    setShellChromeState((current) => ({
      ...current,
    }));
    clearThreadContext();
    syncRoute('replace', {
      activeTopic: nextActiveTopic,
    });
    await loadTopics(nextTopics, nextActiveTopic, null);
  }

  async function handleToggleTopicGossip(topic: string, enabled: boolean) {
    await api.setTopicGossipEnabled(topic, enabled);
    await loadTopics(trackedTopics, activeTopic, selectedThread);
  }

  async function handleToggleChannelGossip(
    topic: string,
    channelId: string,
    enabled: boolean
  ) {
    await api.setChannelGossipEnabled(topic, channelId, enabled);
    await loadTopics(trackedTopics, activeTopic, selectedThread);
  }

  async function handleCreatePrivateChannel(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!channelLabelInput.trim()) {
      setChannelError(translate('channels:errors.channelLabelRequired'));
      return;
    }
    setChannelActionPending('create');
    setInviteOutput(null);
    try {
      const channel = await explainScopeLimit(
        api.createPrivateChannel(activeTopic, channelLabelInput.trim(), channelAudienceInput)
      );
      let nextChannelError: string | null = null;
      try {
        const access = await api.exportChannelAccessToken(activeTopic, channel.channel_id, null);
        setInviteOutput(access.token);
        setInviteOutputLabel(access.kind);
      } catch (shareError) {
        nextChannelError = messageFromError(
          shareError,
          translate('channels:errors.failedShareChannel')
        );
      }
      setJoinedChannelsByTopic(updateRecordEntry(activeTopic, (prev) => upsertJoinedChannel(prev ?? [], channel)));
      setChannelPanelStateByTopic(setRecordEntry(activeTopic, {
          status: 'ready',
          error: null,
        }));
      setChannelLabelInput('');
      setChannelAudienceInput('invite_only');
      setChannelError(nextChannelError);
      setTimelineScopeByTopic(setRecordEntry(activeTopic, {
          kind: 'channel',
          channel_id: channel.channel_id,
        }));
      setSelectedChannelIdByTopic(setRecordEntry(activeTopic, channel.channel_id));
      setComposeChannelByTopic(setRecordEntry(activeTopic, {
          kind: 'private_channel',
          channel_id: channel.channel_id,
        }));
      setShellChromeState((current) => ({
        ...current,
      }));
      syncRoute('replace', {
        activeTopic,
        composeTarget: {
          kind: 'private_channel',
          channel_id: channel.channel_id,
        },
        primarySection: 'timeline',
        timelineScope: {
          kind: 'channel',
          channel_id: channel.channel_id,
        },
      });
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (channelCreateError) {
      setChannelError(
        messageFromError(channelCreateError, translate('channels:errors.failedCreateChannel'))
      );
    } finally {
      setChannelActionPending(null);
    }
  }

  async function handleLeavePrivateChannel(topicId: string, channelId: string) {
    setChannelActionPending('leave');
    try {
      await api.leavePrivateChannel(topicId, channelId);
      setJoinedChannelsByTopic(updateRecordEntry(topicId, (prev) => (prev ?? []).filter(
          (channel) => channel.channel_id !== channelId
        )));
      setChannelPanelStateByTopic(setRecordEntry(topicId, {
          status: 'ready',
          error: null,
        }));
      setInviteOutput(null);
      setChannelError(null);
      const leavingSelectedChannel = selectedChannelIdByTopic[topicId] === channelId;
      if (leavingSelectedChannel) {
        setSelectedChannelIdByTopic(setRecordEntry(topicId, null));
        setTimelineScopeByTopic(setRecordEntry(topicId, PUBLIC_TIMELINE_SCOPE));
        setComposeChannelByTopic(setRecordEntry(topicId, PUBLIC_CHANNEL_REF));
        if (topicId === activeTopic) {
          syncRoute('replace', {
            activeTopic: topicId,
            composeTarget: PUBLIC_CHANNEL_REF,
            timelineScope: PUBLIC_TIMELINE_SCOPE,
          });
        }
      }
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (leaveError) {
      setChannelError(messageFromError(leaveError, translate('channels:errors.failedLeaveChannel')));
    } finally {
      setChannelActionPending(null);
    }
  }

  async function handleShareChannelAccess() {
    if (!activePrivateChannel) {
      setChannelError(translate('channels:errors.selectChannelForShare'));
      return;
    }
    setChannelActionPending('share');
    try {
      const access = await api.exportChannelAccessToken(activeTopic, activePrivateChannel.channel_id, null);
      setInviteOutput(access.token);
      setInviteOutputLabel(access.kind);
      setChannelError(null);
    } catch (shareError) {
      setChannelError(
        messageFromError(shareError, translate('channels:errors.failedShareChannel'))
      );
    } finally {
      setChannelActionPending(null);
    }
  }

  async function activateImportedPrivateChannel(
    topicId: string,
    channelId: string,
    placeholderChannel?: JoinedPrivateChannelView
  ) {
    const nextTopics = trackedTopics.includes(topicId) ? trackedTopics : [...trackedTopics, topicId];
    setTrackedTopics(nextTopics);
    setActiveTopic(topicId);
    if (placeholderChannel) {
      setJoinedChannelsByTopic(updateRecordEntry(topicId, (prev) => upsertJoinedChannel(prev ?? [], placeholderChannel)));
      setChannelPanelStateByTopic(setRecordEntry(topicId, {
          status: 'ready',
          error: null,
        }));
    }
    setSelectedChannelIdByTopic(setRecordEntry(topicId, channelId));
    setTimelineScopeByTopic(setRecordEntry(topicId, {
        kind: 'channel',
        channel_id: channelId,
      }));
    setComposeChannelByTopic(setRecordEntry(topicId, {
        kind: 'private_channel',
        channel_id: channelId,
      }));
    setInviteTokenInput('');
    setInviteOutput(null);
    setChannelError(null);
    setShellChromeState((current) => ({
      ...current,
    }));
    clearThreadContext();
    syncRoute('replace', {
      activeTopic: topicId,
      composeTarget: {
        kind: 'private_channel',
        channel_id: channelId,
      },
      primarySection: 'timeline',
      timelineScope: {
        kind: 'channel',
        channel_id: channelId,
      },
    });
    await loadTopics(nextTopics, topicId, null);
  }

  async function handleJoinChannelAccess(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!inviteTokenInput.trim()) {
      setChannelError(translate('channels:errors.inviteTokenRequired'));
      return;
    }
    await handleImportChannelAccessToken(inviteTokenInput.trim());
  }

  async function handleImportChannelAccessToken(token: string) {
    setChannelActionPending('join');
    try {
      const preview = await explainScopeLimit(api.importChannelAccessToken(token.trim()));
      await activateImportedPrivateChannel(
        preview.topic_id,
        preview.channel_id,
        joinedChannelFromAccessTokenPreview(preview)
      );
    } catch (joinError) {
      setChannelError(messageFromError(joinError, translate('channels:errors.failedJoinChannel')));
    } finally {
      setChannelActionPending(null);
    }
  }

  async function handleSaveDiscoverySeeds() {
    try {
      const seedEntries = discoverySeedInput
        .split('\n')
        .map((entry) => entry.trim())
        .filter(Boolean);
      const nextConfig = await api.setDiscoverySeeds(seedEntries);
      setDiscoveryConfig(nextConfig);
      setDiscoverySeedInput(seedPeersToEditorValue(nextConfig));
      setDiscoveryEditorDirty(false);
      setDiscoveryError(null);
      await loadTopics(trackedTopics, activeTopic, selectedThread);
      syncRoute('replace');
    } catch (saveError) {
      setDiscoveryError(
        saveError instanceof Error
          ? saveError.message
          : translate('common:errors.failedToUpdateDiscoverySeeds')
      );
    }
  }

  async function handleSaveCommunityNodes() {
    try {
      const nextConfig = await api.setCommunityNodeConfig(
        communityNodeDraftNodesToConfigInput(communityNodeInput)
      );
      setCommunityNodeConfig(nextConfig);
      setCommunityNodeInput(communityNodesToDraftNodes(nextConfig));
      setCommunityNodeEditorDirty(false);
      setCommunityNodeError(null);
      await loadTopics(trackedTopics, activeTopic, selectedThread);
      syncRoute('replace');
    } catch (saveError) {
      setCommunityNodeError(
        saveError instanceof Error
          ? saveError.message
          : translate('common:errors.failedToUpdateCommunityNodes')
      );
    }
  }

  /// #1061: 信頼値の採用順位だけを保存する（ノード一覧の下書きは送らない）。
  async function handleSetCommunityNodeTrustPriority(priority: string[]) {
    try {
      // 編集中の下書きではなく、保存済みのノード一覧をそのまま送る。
      const saved = getState().communityNodeConfig;
      const nextConfig = await api.setCommunityNodeConfig(
        communityNodeDraftNodesToConfigInput(communityNodesToDraftNodes(saved)),
        priority
      );
      setCommunityNodeConfig(nextConfig);
      // 保存側の正規化（URL 正規化・重複排除）と下書きをずらさない。
      setCommunityNodeInput(communityNodesToDraftNodes(nextConfig));
      setCommunityNodeEditorDirty(false);
      setCommunityNodeError(null);
    } catch (saveError) {
      setCommunityNodeError(
        saveError instanceof Error
          ? saveError.message
          : translate('common:errors.failedToUpdateCommunityNodes')
      );
    }
  }

  async function handleClearCommunityNodes() {
    try {
      await api.clearCommunityNodeConfig();
      setCommunityNodeConfig(DEFAULT_COMMUNITY_NODE_CONFIG);
      setCommunityNodeStatuses([]);
      setCommunityNodeInput([]);
      setCommunityNodeEditorDirty(false);
      setCommunityNodeError(null);
      await loadTopics(trackedTopics, activeTopic, selectedThread);
      syncRoute('replace');
    } catch (clearError) {
      setCommunityNodeError(
        clearError instanceof Error
          ? clearError.message
          : translate('common:errors.failedToClearCommunityNodes')
      );
    }
  }

  async function handleAuthenticateCommunityNode(baseUrl: string) {
    try {
      const nextStatus = await runCommunityNodeOperation(baseUrl, () => api.authenticateCommunityNode(baseUrl));
      if (!nextStatus) return;
      setCommunityNodeError(null);
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (authError) {
      setCommunityNodeError(
        authError instanceof Error
          ? authError.message
          : translate('common:errors.failedToAuthenticateCommunityNode')
      );
    }
  }

  async function handleSetCommunityNodeInviteCode(baseUrl: string, inviteCode: string) {
    try {
      const nextStatus = await runCommunityNodeOperation(baseUrl, () => api.setCommunityNodeInviteCode(baseUrl, inviteCode));
      if (!nextStatus) return;
      setCommunityNodeError(null);
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (inviteError) {
      setCommunityNodeError(
        inviteError instanceof Error
          ? inviteError.message
          : translate('common:errors.failedToAuthenticateCommunityNode')
      );
      throw inviteError;
    }
  }

  async function handleClearCommunityNodeToken(baseUrl: string) {
    try {
      const nextStatus = await runCommunityNodeOperation(baseUrl, () => api.clearCommunityNodeToken(baseUrl));
      if (!nextStatus) return;
      setCommunityNodeError(null);
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (clearError) {
      setCommunityNodeError(
        clearError instanceof Error
          ? clearError.message
          : translate('common:errors.failedToClearCommunityNodeToken')
      );
    }
  }

  async function handleRefreshCommunityNode(baseUrl: string) {
    try {
      const nextStatus = await runCommunityNodeOperation(baseUrl, () => api.refreshCommunityNodeMetadata(baseUrl));
      if (!nextStatus) return;
      setCommunityNodeError(null);
      await loadTopics(trackedTopics, activeTopic, selectedThread);
      const localConsent = nextStatus.local_consent;
      return (
        nextStatus.consent_update_pending === true ||
        localConsent == null ||
        localConsent.records.length === 0 ||
        localConsent.withdrawn_at != null ||
        nextStatus.consent_state?.all_required_accepted === false
      );
    } catch (refreshError) {
      setCommunityNodeError(
        refreshError instanceof Error
          ? refreshError.message
          : translate('common:errors.failedToRefreshCommunityNode')
      );
      return false;
    }
  }

  // #857: 同意提示に必要な情報は認証不要の公開 policy カタログから取得する。
  async function handleFetchCommunityNodeConsents(
    baseUrl: string, language = i18n.resolvedLanguage ?? i18n.language
  ) {
    const pending = { status: 'loading' as const };
    setCommunityNodePolicies(setRecordEntry(baseUrl, pending));
    try {
      const catalog = await api.fetchCommunityNodePolicies(baseUrl, language);
      const entry = { status: 'ok' as const, policies: catalog.policies };
      if (getState().communityNodePolicies[baseUrl] === pending) {
        setCommunityNodePolicies(setRecordEntry(baseUrl, entry));
        setCommunityNodeError(null);
      }
      // callerは共有cacheの後続更新で表示内容が変わらないsnapshotを受け取る。
      return communityNodeConsentView(
        getState().communityNodeStatuses.find((node) => node.base_url === baseUrl), entry
      );
    } catch (consentError) {
      const message =
        consentError instanceof Error
          ? consentError.message
          : translate('common:errors.failedToFetchConsentStatus');
      // 取得失敗(オフライン等)はダイアログ内で再試行できるよう entry に残す。
      if (getState().communityNodePolicies[baseUrl] === pending) {
        setCommunityNodePolicies(setRecordEntry(baseUrl, { status: 'error' as const, error: message }));
        setCommunityNodeError(message);
      }
      throw consentError;
    }
  }

  // #857: 提示された文書と版をそのまま受諾し、ローカル記録 → セッション確立を開始する。
  async function handleAcceptCommunityNodeConsents(
    baseUrl: string,
    documents: CommunityNodeConsentDocumentRef[],
    language = i18n.resolvedLanguage ?? i18n.language
  ) {
    try {
      const nextStatus = await runCommunityNodeOperation(baseUrl, () =>
        api.acceptCommunityNodeConsents(baseUrl, documents, language)
      );
      if (!nextStatus) return;
      setCommunityNodeError(null);
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (consentError) {
      setCommunityNodeError(
        consentError instanceof Error
          ? consentError.message
          : translate('common:errors.failedToAcceptConsents')
      );
      throw consentError;
    }
  }

  // #857: 同意の撤回。記録は履歴として残り、トークン破棄で接続だけが止まる。
  async function handleWithdrawCommunityNodeConsents(baseUrl: string) {
    try {
      const nextStatus = await runCommunityNodeOperation(baseUrl, () => api.withdrawCommunityNodeConsents(baseUrl));
      if (!nextStatus) return;
      setCommunityNodeError(null);
    } catch (consentError) {
      setCommunityNodeError(
        consentError instanceof Error
          ? consentError.message
          : translate('common:errors.failedToWithdrawConsents')
      );
      throw consentError;
    }
  }

  return {
    handleProfileFieldChange,
    handleProfileAvatarFile,
    handleClearProfileAvatar,
    resetProfileDraft,
    handleSelectPrivateChannel,
    handleSaveProfile,
    handleAddTopic,
    handleSelectTopic,
    handleOpenOriginalTopic,
    handleRemoveTopic,
    handleToggleTopicGossip,
    handleToggleChannelGossip,
    handleCreatePrivateChannel,
    handleLeavePrivateChannel,
    handleShareChannelAccess,
    handleJoinChannelAccess,
    handleImportChannelAccessToken,
    handleSaveDiscoverySeeds,
    handleSaveCommunityNodes,
    handleSetCommunityNodeTrustPriority,
    handleClearCommunityNodes,
    handleAuthenticateCommunityNode,
    handleSetCommunityNodeInviteCode,
    handleClearCommunityNodeToken,
    handleRefreshCommunityNode,
    handleFetchCommunityNodeConsents,
    handleAcceptCommunityNodeConsents,
    handleWithdrawCommunityNodeConsents,
  };
}
