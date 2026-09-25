import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ChangeEvent,
  type FormEvent,
  type ReactNode,
} from 'react';
import { useTranslation } from 'react-i18next';
import { Lock, RefreshCw, Trash2 } from 'lucide-react';

import { ColumnComposerFooter } from '@/components/shell/ColumnComposerFooter';
import { ColumnDomainActionFooter } from '@/components/shell/ColumnDomainActionFooter';
import { ColumnCanvas } from '@/components/shell/ColumnCanvas';
import { AuthorIdentityButton } from '@/components/core/AuthorIdentityButton';
import {
  ColumnContextSelect,
  type ColumnContextSelectOption,
} from '@/components/shell/ColumnContextSelect';
import { ColumnSurface } from '@/components/shell/ColumnSurface';
import { ProfileRefreshButton } from '@/components/shell/ProfileRefreshButton';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import {
  TimelineViewIconTabs,
  type TimelineViewId,
} from '@/components/shell/TimelineViewIconTabs';
import type { MentionCandidate } from '@/components/core/types';
import {
  communityIndexNodeLabel,
  eligibleCommunityIndexNodes,
  resolveCommunityIndexNodePreference,
} from '@/lib/api/communityIndex';
import { topicDisplayName } from '@/lib/topicId';
import {
  authorDisplayLabel,
  localizeAudienceLabel,
  resolveProfilePictureSrc,
} from '@/shell/presentation';
import type { ColumnDraftTarget } from '@/shell/slices/columnDrafts';
import {
  activateColumn,
  closeColumn,
  columnSpanPolicy,
  moveColumn,
  setColumnPinned,
  setColumnSpan,
  type ColumnKind,
  type ColumnState,
} from '@/shell/slices/workspace';
import { useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';
import { ColumnRuntimeProvider } from '@/shell/ColumnRuntimeContext';
import { useThreadFocusScroll } from '@/shell/page/useThreadFocusScroll';
import {
  projectColumnRuntime,
  releaseColumnAudioFocus,
  requestColumnAudioFocus,
} from '@/shell/columnRuntime';

type DesktopShellColumnWorkspaceProps = {
  locale: string;
  /// 表示中 Column id の変化を親へ通知する(背景 refresh 用、Issue #765)。
  onVisibleColumnsChange?: (columnIds: string[]) => void;
  mentionCandidates: MentionCandidate[];
  onColumnAttachmentSelection: (
    target: ColumnDraftTarget,
    event: ChangeEvent<HTMLInputElement>
  ) => Promise<void>;
  onColumnAttachmentPaste: (target: ColumnDraftTarget, files: File[]) => Promise<void>;
  onRemoveColumnAttachment: (target: ColumnDraftTarget, itemId: string) => void;
  onSubmitColumnDraft: (
    target: ColumnDraftTarget,
    event: FormEvent<HTMLFormElement>
  ) => Promise<void>;
  onEndLiveSession: (sessionId: string, topic: string) => Promise<void>;
  onJoinLiveSession: (sessionId: string, topic: string) => Promise<void>;
  onLeaveLiveSession: (sessionId: string, topic: string) => Promise<void>;
  onOpenGameCreate: () => void;
  onOpenKeyboardHelp: () => void;
  onOpenLiveCreate: () => void;
  renderConversationSurface: (column: ColumnState) => ReactNode;
  renderMessagesSurface: (column: ColumnState) => ReactNode;
  renderNotificationsSurface: (column: ColumnState) => ReactNode;
  onRefreshNotifications: () => void;
  onRefreshProfile: () => Promise<void>;
  onRefreshConversation: (peerPubkey: string) => void;
  onClearConversation: (peerPubkey: string) => void;
  onOpenConversationAuthor: (peerPubkey: string, parentColumnId: string) => void;
  onActivateColumn: (column: ColumnState, preserveAuthorPane?: boolean) => void;
  // Column 内の操作(ボタン / プルダウン)で active 化したとき、route だけを同期する(Issue #1053)。
  onSyncColumnRoute: (column: ColumnState) => void;
  onSelectTimelineTopic: (column: ColumnState, topicId: string) => void;
  onSelectTimelineView: (column: ColumnState, view: TimelineViewId) => void;
  // Timeline Column header のプライベートチャンネル入口(Issue #966)。公開 scope では
  // 作成・参加 Dialog、channel scope ではそのチャンネルの設定・共有 Dialog を開く。
  onOpenChannelManager: (column: ColumnState) => void;
  onOpenChannelSettings: (column: ColumnState, topicId: string, channelId: string) => void;
  renderProfileSurface: (column: ColumnState) => ReactNode;
  renderPrimarySurface: (column: ColumnState) => ReactNode;
  scopeLabel: string;
  renderThreadSurface: (column: ColumnState) => ReactNode;
  timelineViewItems: Array<{ id: TimelineViewId; label: string }>;
  titles: Record<ColumnKind, string>;
};

export function DesktopShellColumnWorkspace({
  locale,
  onVisibleColumnsChange,
  mentionCandidates,
  onColumnAttachmentSelection,
  onColumnAttachmentPaste,
  onRemoveColumnAttachment,
  onSubmitColumnDraft,
  onEndLiveSession,
  onJoinLiveSession,
  onLeaveLiveSession,
  onOpenGameCreate,
  onOpenKeyboardHelp,
  onOpenLiveCreate,
  renderConversationSurface,
  renderMessagesSurface,
  renderNotificationsSurface,
  onRefreshNotifications,
  onRefreshProfile,
  onRefreshConversation,
  onClearConversation,
  onOpenConversationAuthor,
  onActivateColumn,
  onSyncColumnRoute,
  onSelectTimelineTopic,
  onSelectTimelineView,
  onOpenChannelManager,
  onOpenChannelSettings,
  renderProfileSurface,
  renderPrimarySurface,
  scopeLabel,
  renderThreadSurface,
  timelineViewItems,
  titles,
}: DesktopShellColumnWorkspaceProps) {
  useThreadFocusScroll();
  const { t } = useTranslation('shell');
  const workspaceState = useDesktopShellStore((state) => state.workspaceState);
  const trackedTopics = useDesktopShellStore((state) => state.trackedTopics);
  const joinedChannelsByTopic = useDesktopShellStore((state) => state.joinedChannelsByTopic);
  const directMessages = useDesktopShellStore((state) => state.directMessages);
  const directMessageStatusByPeer = useDesktopShellStore(
    (state) => state.directMessageStatusByPeer
  );
  const directMessageTimelineByPeer = useDesktopShellStore(
    (state) => state.directMessageTimelineByPeer
  );
  const notifications = useDesktopShellStore((state) => state.notifications);
  const hasMoreNotifications = useDesktopShellStore((state) => Boolean(state.notificationsOlderCursor));
  const profileRefreshing = useDesktopShellStore((state) => state.profileRefreshing);
  const profileSaving = useDesktopShellStore((state) => state.profileSaving);
  const notificationStatus = useDesktopShellStore((state) => state.notificationStatus);
  const knownAuthorsByPubkey = useDesktopShellStore((state) => state.knownAuthorsByPubkey);
  const mediaObjectUrls = useDesktopShellStore((state) => state.mediaObjectUrls);
  const communityNodeConfig = useDesktopShellStore((state) => state.communityNodeConfig);
  const communityNodeStatuses = useDesktopShellStore((state) => state.communityNodeStatuses);
  const communityNodeManifests = useDesktopShellStore((state) => state.communityNodeManifests);
  const communityIndexNodePreference = useDesktopShellStore(
    (state) => state.communityIndexNodePreference
  );
  const patchState = useDesktopShellStore((state) => state.patchState);
  const setWorkspaceState = useDesktopShellFieldSetter('workspaceState');
  const [visibleColumnIds, setVisibleColumnIds] = useState<string[]>([
    workspaceState.activeColumnId,
  ]);
  const [audioFocusedColumnId, setAudioFocusedColumnId] = useState<string | null>(null);

  const updateVisibleColumnIds = useCallback(
    (columnIds: string[]) => {
      onVisibleColumnsChange?.(columnIds);
      setVisibleColumnIds((current) =>
        current.length === columnIds.length && current.every((id, index) => id === columnIds[index])
          ? current
          : columnIds
      );
    },
    [onVisibleColumnsChange]
  );

  useEffect(() => {
    const ids = new Set(workspaceState.columns.map((column) => column.id));
    setVisibleColumnIds((current) => current.filter((id) => ids.has(id)));
    setAudioFocusedColumnId((current) => current && ids.has(current) ? current : null);
  }, [workspaceState.columns]);

  const visibleColumnIdSet = useMemo(() => new Set(visibleColumnIds), [visibleColumnIds]);
  const eligibleIndexNodeBaseUrls = useMemo(
    () =>
      eligibleCommunityIndexNodes(
        communityNodeConfig,
        communityNodeStatuses,
        communityNodeManifests
      ),
    [communityNodeConfig, communityNodeManifests, communityNodeStatuses]
  );
  const communityIndexNodeOptions = useMemo(() => {
    const labels = eligibleIndexNodeBaseUrls.map((baseUrl) => ({
      baseUrl,
      label: communityIndexNodeLabel(baseUrl, communityNodeManifests[baseUrl]),
    }));
    const labelCounts = new Map<string, number>();
    labels.forEach(({ label }) => labelCounts.set(label, (labelCounts.get(label) ?? 0) + 1));
    const options: ColumnContextSelectOption[] = [
      { value: 'automatic', label: t('communityIndex.automaticNode') },
      ...labels.map(({ baseUrl, label }) => ({
        value: baseUrl,
        label: labelCounts.get(label) === 1 ? label : `${label} · ${new URL(baseUrl).host}`,
      })),
    ];
    if (
      communityIndexNodePreference.mode === 'manual' &&
      !eligibleIndexNodeBaseUrls.includes(communityIndexNodePreference.baseUrl)
    ) {
      options.push({
        value: communityIndexNodePreference.baseUrl,
        label: t('communityIndex.unavailableNodeOption', {
          baseUrl: communityIndexNodePreference.baseUrl,
        }),
        disabled: true,
      });
    }
    return options;
  }, [
    communityNodeManifests,
    communityIndexNodePreference,
    eligibleIndexNodeBaseUrls,
    t,
  ]);

  const renderScopeControl = (column: ColumnState) => {
    if (column.kind === 'conversation' && column.entityId) {
      const author = knownAuthorsByPubkey[column.entityId] ?? null;
      return (
        <AuthorIdentityButton
          label={conversationLabel(column.entityId)}
          picture={resolveProfilePictureSrc(author, mediaObjectUrls)}
          buttonClassName='shell-column-context-author'
          onClick={() => onOpenConversationAuthor(column.entityId!, column.id)}
        />
      );
    }
    if (column.kind === 'timeline' && column.scope?.channelId === null) {
      const topics = trackedTopics.includes(column.scope.topicId)
        ? trackedTopics
        : [column.scope.topicId, ...trackedTopics];
      return (
        <ColumnContextSelect
          label={t('workspace.timelineTopicSwitch')}
          value={column.scope.topicId}
          title={column.scope.topicId}
          options={topics.map((topicId) => ({
            value: topicId,
            label: topicDisplayName(topicId),
          }))}
          onChange={(topicId) => onSelectTimelineTopic(column, topicId)}
        />
      );
    }
    if (column.kind === 'explore') {
      const selectedValue =
        communityIndexNodePreference.mode === 'auto'
          ? 'automatic'
          : communityIndexNodePreference.baseUrl;
      return (
        <ColumnContextSelect
          label={t('communityIndex.nodeSwitch')}
          value={selectedValue}
          title={
            communityIndexNodePreference.mode === 'auto'
              ? t('communityIndex.automaticNode')
              : communityIndexNodePreference.baseUrl
          }
          options={communityIndexNodeOptions}
          onChange={(value) => {
            const preference =
              value === 'automatic'
                ? ({ mode: 'auto' } as const)
                : ({ mode: 'manual', baseUrl: value } as const);
            const resolution = resolveCommunityIndexNodePreference(
              preference,
              communityNodeConfig.nodes.map((node) => node.base_url),
              eligibleIndexNodeBaseUrls
            );
            patchState({
              communityIndexNodePreference: resolution.preference,
              communityIndexNodeBaseUrl: resolution.selectedBaseUrl,
            });
          }}
        />
      );
    }
    return undefined;
  };

  const renderScopeActions = (column: ColumnState) => {
    if (column.kind !== 'timeline' || !column.scope) return undefined;
    const scope = column.scope;
    if (scope.channelId === null) {
      return (
        <div className='shell-column-scope-actions'>
          <Button
            variant='ghost'
            size='sm'
            type='button'
            className='shell-column-scope-action'
            aria-label={t('workspace.privateChannelEntryLabel')}
            onClick={() => onOpenChannelManager(column)}
          >
            <Lock className='size-4' aria-hidden='true' />
            {t('workspace.privateChannelEntry')}
          </Button>
        </div>
      );
    }
    const channelId = scope.channelId;
    const channelLabel =
      joinedChannelsByTopic[scope.topicId]?.find(
        (candidate) => candidate.channel_id === channelId
      )?.label ?? channelId;
    return (
      <div className='shell-column-scope-actions'>
        <Button
          variant='ghost'
          size='sm'
          type='button'
          className='shell-column-scope-action'
          aria-label={t('workspace.channelSettingsEntryLabel', { channel: channelLabel })}
          onClick={() => onOpenChannelSettings(column, scope.topicId, channelId)}
        >
          <Lock className='size-4' aria-hidden='true' />
          {t('workspace.channelSettingsEntry')}
        </Button>
      </div>
    );
  };

  const activate = (columnId: string, loadContext: boolean) => {
    const column = workspaceState.columns.find((candidate) => candidate.id === columnId);
    if (!column) return;
    setWorkspaceState((current) => activateColumn(current, columnId));
    // Column 内の操作では文脈の読み込みを押された handler に任せ、route だけを揃える。
    // route を旧 Column に残すと、route 同期が scope のずれから Timeline を開き直す(Issue #1053)。
    if (loadContext) onActivateColumn(column);
    else onSyncColumnRoute(column);
  };
  const close = (columnId: string) => {
    const next = closeColumn(workspaceState, columnId);
    if (next === workspaceState) return;
    setWorkspaceState(next);
    const activeColumn = next.columns.find((column) => column.id === next.activeColumnId);
    if (activeColumn) onActivateColumn(activeColumn, false);
  };
  const renderBody = (column: ColumnState) => {
    if (column.kind === 'thread') return renderThreadSurface(column);
    if (column.kind === 'profile' && column.entityId) return renderProfileSurface(column);
    if (column.kind === 'conversation') return renderConversationSurface(column);
    if (column.kind === 'messages') return renderMessagesSurface(column);
    if (column.kind === 'notifications') return renderNotificationsSurface(column);
    return renderPrimarySurface(column);
  };
  const scopeDestinationLabel = (column: ColumnState) => {
    if (!column.scope) return scopeLabel;
    const topicLabel = column.scope.topicId.split(':').filter(Boolean).at(-1) ?? column.scope.topicId;
    const channel = column.scope.channelId
      ? joinedChannelsByTopic[column.scope.topicId]?.find(
          (candidate) => candidate.channel_id === column.scope?.channelId
        )?.label ?? column.scope.channelId
      : localizeAudienceLabel('Public');
    return `${channel} · ${topicLabel}`;
  };
  function conversationLabel(peerPubkey: string) {
    const conversation = directMessages.find((item) => item.peer_pubkey === peerPubkey);
    const author = knownAuthorsByPubkey[peerPubkey];
    return authorDisplayLabel(
      peerPubkey,
      author?.display_name ?? conversation?.peer_display_name,
      author?.name ?? conversation?.peer_name
    );
  };
  const columnScopeLabel = (column: ColumnState) => {
    if (column.kind === 'conversation' && column.entityId) {
      return conversationLabel(column.entityId);
    }
    if (column.kind === 'profile' && column.entityId) {
      const author = knownAuthorsByPubkey[column.entityId];
      return authorDisplayLabel(column.entityId, author?.display_name, author?.name);
    }
    if (column.kind === 'thread') return `${titles.thread} · ${scopeDestinationLabel(column)}`;
    if (column.entityId) return titles[column.kind];
    return scopeDestinationLabel(column);
  };
  const renderFooter = (column: ColumnState, active: boolean) => {
    const common = {
      active,
      locale,
      mentionCandidates,
      onActivate: () => activate(column.id, true),
      onOpenKeyboardHelp,
      onAttachmentSelection: onColumnAttachmentSelection,
      onAttachmentPaste: onColumnAttachmentPaste,
      onRemoveAttachment: onRemoveColumnAttachment,
      onSubmit: onSubmitColumnDraft,
    };
    if (column.kind === 'timeline' && column.scope) {
      return (
        <ColumnComposerFooter
          {...common}
          destinationLabel={scopeDestinationLabel(column)}
          target={{ columnId: column.id, action: 'post', scope: column.scope }}
        />
      );
    }
    if (column.kind === 'thread' && column.scope && column.entityId) {
      return (
        <ColumnComposerFooter
          {...common}
          destinationLabel={`${titles.thread} · ${scopeDestinationLabel(column)}`}
          target={{
            columnId: column.id,
            action: 'reply',
            scope: column.scope,
            threadId: column.entityId,
          }}
        />
      );
    }
    if (column.kind === 'conversation' && column.entityId) {
      return (
        <ColumnComposerFooter
          {...common}
          destinationLabel={conversationLabel(column.entityId)}
          target={{ columnId: column.id, action: 'message', peerPubkey: column.entityId }}
        />
      );
    }
    if (column.kind === 'stream' || column.kind === 'game' || column.kind === 'metaverse') {
      return (
        <ColumnDomainActionFooter
          active={active}
          column={column}
          onActivate={() => activate(column.id, true)}
          onEndLiveSession={onEndLiveSession}
          onJoinLiveSession={onJoinLiveSession}
          onLeaveLiveSession={onLeaveLiveSession}
          onOpenGameCreate={onOpenGameCreate}
          onOpenLiveCreate={onOpenLiveCreate}
        />
      );
    }
    return undefined;
  }
  const renderHeaderActions = (column: ColumnState) => {
    if (column.kind === 'profile' && !column.entityId) {
      return (
        <div className='shell-column-context-actions'>
          <ProfileRefreshButton refreshing={profileRefreshing} saving={profileSaving} onRefresh={onRefreshProfile} />
        </div>
      );
    }
    if (column.kind === 'timeline') {
      return (
        <TimelineViewIconTabs
          // 表示・切替の正本は Column 単位の timelineView(Issue #765)。
          activeView={column.timelineView ?? 'feed'}
          items={timelineViewItems}
          onSelect={(view) => onSelectTimelineView(column, view)}
        />
      );
    }
    if (column.kind === 'notifications') {
      return (
        <div className='shell-column-context-actions'>
          <span className='shell-column-header-summary'>
            {t('notifications.summary', {
              count: notifications.length,
              visibleCount: `${notifications.length}${hasMoreNotifications ? '+' : ''}`,
              unread: notificationStatus.unread_count,
            })}
          </span>
          <IconButton
            variant='ghost'
            type='button'
            label={t('common:actions.refresh')}
            onClick={onRefreshNotifications}
          >
            <RefreshCw className='size-4' aria-hidden='true' />
          </IconButton>
        </div>
      );
    }
    if (column.kind === 'conversation' && column.entityId) {
      const peerPubkey = column.entityId;
      const conversation = directMessages.find((item) => item.peer_pubkey === peerPubkey);
      const status = directMessageStatusByPeer[peerPubkey] ?? conversation?.status ?? null;
      const timeline = directMessageTimelineByPeer[peerPubkey] ?? [];
      return (
        <div className='shell-column-context-actions'>
          {status ? (
            <span className='shell-column-header-summary'>
              {status.send_enabled ? t('messages.sendEnabled') : t('messages.sendDisabled')}
            </span>
          ) : null}
          <IconButton
            variant='ghost'
            type='button'
            label={t('common:actions.refresh')}
            onClick={() => onRefreshConversation(peerPubkey)}
          >
            <RefreshCw className='size-4' aria-hidden='true' />
          </IconButton>
          <IconButton
            variant='ghost'
            type='button'
            label={t('common:actions.clear')}
            disabled={timeline.length === 0}
            onClick={() => onClearConversation(peerPubkey)}
          >
            <Trash2 className='size-4' aria-hidden='true' />
          </IconButton>
        </div>
      );
    }
    return undefined;
  };

  return (
    <ColumnCanvas
      activeColumnId={workspaceState.activeColumnId}
      columnIds={workspaceState.columns.map((column) => column.id)}
      onActivateColumn={activate}
      onVisibleColumnIdsChange={updateVisibleColumnIds}
      onMoveColumn={(columnId, targetIndex) =>
        setWorkspaceState((current) => moveColumn(current, columnId, targetIndex))
      }
    >
      {workspaceState.columns.map((column, index) => {
        const runtime = projectColumnRuntime({
          kind: column.kind,
          columnId: column.id,
          activeColumnId: workspaceState.activeColumnId,
          visibleColumnIds: visibleColumnIdSet,
          audioFocusedColumnId,
        });
        return (
          <ColumnRuntimeProvider
            key={column.id}
            value={{
              ...runtime,
              requestAudioFocus: () =>
                setAudioFocusedColumnId((current) =>
                  requestColumnAudioFocus(current, column.id)
                ),
              releaseAudioFocus: () =>
                setAudioFocusedColumnId((current) =>
                  releaseColumnAudioFocus(current, column.id)
                ),
            }}
          >
            <ColumnSurface
              columnId={column.id}
              title={titles[column.kind]}
              scopeLabel={columnScopeLabel(column)}
              position={index + 1}
              total={workspaceState.columns.length}
              span={column.preferredDesktopSpan}
              spanOptions={(() => {
                const policy = columnSpanPolicy(column.kind);
                return Array.from(
                  { length: policy.max - policy.min + 1 },
                  (_, optionIndex) => (policy.min + optionIndex) as 1 | 2 | 3 | 4
                );
              })()}
              active={runtime.active}
              pinned={column.pinned}
              fullscreenable={column.kind === 'stream' || column.kind === 'metaverse'}
              resourceManaged={column.kind === 'stream' || column.kind === 'metaverse'}
              footer={renderFooter(column, runtime.active)}
              scopeControl={renderScopeControl(column)}
              scopeActions={renderScopeActions(column)}
              headerActions={renderHeaderActions(column)}
              onPinnedChange={(pinned) =>
                setWorkspaceState((current) => setColumnPinned(current, column.id, pinned))
              }
              onMoveLeft={
                index > 0
                  ? () =>
                      setWorkspaceState((current) => moveColumn(current, column.id, index - 1))
                  : undefined
              }
              onMoveRight={
                index < workspaceState.columns.length - 1
                  ? () =>
                      setWorkspaceState((current) => moveColumn(current, column.id, index + 1))
                  : undefined
              }
              onSpanChange={(span) =>
                setWorkspaceState((current) => setColumnSpan(current, column.id, span))
              }
              onClose={workspaceState.columns.length > 1 ? () => close(column.id) : undefined}
            >
              {renderBody(column)}
            </ColumnSurface>
          </ColumnRuntimeProvider>
        );
      })}
    </ColumnCanvas>
  );
}
