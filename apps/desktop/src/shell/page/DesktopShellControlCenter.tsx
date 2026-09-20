import {
  useCallback,
  useEffect,
  useRef,
  type ComponentProps,
  type RefObject,
} from 'react';
import { AccountMenu } from './AccountMenu';
import { useTranslation } from 'react-i18next';
import {
  Bell,
  BookPlus,
  Columns3,
  Compass,
  DatabaseBackup,
  Download,
  GitBranchPlus,
  Info,
  Keyboard,
  Menu,
  MessageCircle,
  MessageSquarePlus,
  Pin,
  PinOff,
  Plus,
  Radio,
  Settings,
  SlidersHorizontal,
  X,
} from 'lucide-react';
import { useShallow } from 'zustand/react/shallow';

import { FilterableTopicNavList } from '@/components/core/FilterableTopicNavList';
import type { TopicDiagnosticSummary } from '@/components/core/types';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Input } from '@/components/ui/input';
import { topicDisplayName } from '@/lib/topicId';
import type { SettingsSection } from '@/components/shell/types';
import {
  activateColumn,
  closeColumn,
  columnIdentityId,
  activeWorkspaceScope,
  defaultColumnSpan,
  openPinnedColumn,
  setColumnPinned,
  type ColumnKind,
  type ColumnState,
} from '@/shell/slices/workspace';
import { formatCount } from '@/shell/presentation';
import { connectivityGuidance } from '@/shell/connectivityGuidance';
import { useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';
import { SavedWorkspaceLayouts } from '@/components/shell/SavedWorkspaceLayouts';
import { applySavedWorkspaceLayout } from '@/shell/savedWorkspaceLayouts';

export const CONTROL_CENTER_ID = 'shell-control-center';

type TopicListProps = Omit<
  ComponentProps<typeof FilterableTopicNavList>,
  'alwaysShowControls' | 'items'
>;

type DesktopShellControlCenterProps = TopicListProps & {
  triggerRef: RefObject<HTMLButtonElement | null>;
  topicItems: TopicDiagnosticSummary[];
  topicInput: string;
  titles: Record<ColumnKind, string>;
  updateAvailable: boolean;
  onTopicInputChange: (value: string) => void;
  onAddTopic: () => void | Promise<void>;
  onOpenChannelManager: () => void;
  // 「場所」の未参加 topic から作成・参加 Dialog を開く。topic を選択してから開く(Issue #966)。
  onOpenChannelManagerForTopic: (topic: string) => void;
  onActivateColumn: (column: ColumnState) => void | Promise<void>;
  onOpenSettings: (section: SettingsSection) => void;
  onOpenProfile: () => void;
  onOpenTesterFeedback: () => void;
};

const ADDABLE_COLUMN_KINDS: ColumnKind[] = [
  'timeline',
  'explore',
  'notifications',
  'messages',
  'profile',
  'stream',
  'metaverse',
];

export function DesktopShellControlCenter({
  triggerRef,
  topicItems,
  topicInput,
  titles,
  updateAvailable,
  onTopicInputChange,
  onAddTopic,
  onOpenChannelManager,
  onOpenChannelManagerForTopic,
  onActivateColumn,
  onOpenSettings,
  onOpenProfile,
  onOpenTesterFeedback,
  onSelectTopic,
  onSelectChannel,
  onOpenChannelSettings,
  onLeaveChannel,
  onRemoveTopic,
  onCopyTopicLink,
  onRequestTopicIndexing,
  onToggleTopicGossip,
  onToggleChannelGossip,
}: DesktopShellControlCenterProps) {
  const { t } = useTranslation(['shell', 'common', 'settings', 'channels']);
  const closeButtonRef = useRef<HTMLButtonElement | null>(null);
  const {
    communityNodeStatuses,
    developerModeEnabled,
    joinedChannelsByTopic,
    notificationStatus,
    syncStatus,
    syncStatusRead,
    workspaceState,
  } = useDesktopShellStore(
    useShallow((state) => ({
      communityNodeStatuses: state.communityNodeStatuses,
      developerModeEnabled: state.developerModeEnabled,
      joinedChannelsByTopic: state.joinedChannelsByTopic,
      notificationStatus: state.notificationStatus,
      syncStatus: state.syncStatus,
      syncStatusRead: state.syncStatusRead,
      workspaceState: state.workspaceState,
    }))
  );
  const activeScope = activeWorkspaceScope(workspaceState);
  const activeTopic = activeScope.topicId;
  const selectedChannelId = activeScope.channelId;
  const setWorkspaceState = useDesktopShellFieldSetter('workspaceState');
  const communityNodeNeedsAttention = communityNodeStatuses.some((status) => status.last_error);
  const guidance = connectivityGuidance(syncStatus, syncStatusRead, t);
  const connectionNeedsAttention = guidance.tone !== 'accent';
  const statusKey = communityNodeNeedsAttention
    ? 'communityNodeAttention'
    : connectionNeedsAttention
      ? 'connectionAttention'
      : 'connected';
  const statusLabel = t(`shell:controlCenter.status.${statusKey}`);
  const triggerLabel = t(
    workspaceState.controlCenterOpen
      ? 'shell:controlCenter.closeWithStatus'
      : 'shell:controlCenter.openWithStatus',
    { status: statusLabel }
  );
  const addableColumnKinds = developerModeEnabled
    ? ADDABLE_COLUMN_KINDS
    : ADDABLE_COLUMN_KINDS.filter(
        (kind) => kind !== 'stream' && kind !== 'metaverse'
      );

  const restoreTriggerFocus = useRef(false);
  useEffect(() => {
    if (!workspaceState.controlCenterOpen && restoreTriggerFocus.current) {
      restoreTriggerFocus.current = false;
      triggerRef.current?.focus();
    }
  }, [workspaceState.controlCenterOpen, triggerRef]);

  const setOpen = useCallback(
    (open: boolean, restoreFocus = false) => {
      setWorkspaceState((current) => ({ ...current, controlCenterOpen: open }));
      restoreTriggerFocus.current = !open && restoreFocus;
    },
    [setWorkspaceState]
  );

  useEffect(() => {
    if (!workspaceState.controlCenterOpen) return;
    closeButtonRef.current?.focus();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setOpen(false, true);
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [setOpen, workspaceState.controlCenterOpen]);

  const scopeLabel = (column: ColumnState) => {
    if (!column.scope) return null;
    const channel = column.scope.channelId
      ? joinedChannelsByTopic[column.scope.topicId]?.find(
          (candidate) => candidate.channel_id === column.scope?.channelId
        )?.label ?? column.scope.channelId
      : t('common:audience.public');
    return `${topicDisplayName(column.scope.topicId)} · ${channel}`;
  };

  const focusColumn = (column: ColumnState) => {
    setWorkspaceState((current) => ({
      ...activateColumn(current, column.id),
      controlCenterOpen: false,
    }));
    void onActivateColumn(column);
  };

  const addColumn = (kind: ColumnKind) => {
    const scope = activeScope;
    const column: ColumnState = {
      id: columnIdentityId(kind, scope),
      kind,
      scope,
      pinned: true,
      preferredDesktopSpan: defaultColumnSpan(kind),
    };
    setWorkspaceState((current) => ({
      ...openPinnedColumn(current, column),
      controlCenterOpen: false,
    }));
    void onActivateColumn(column);
  };

  const closeManagedColumn = (columnId: string) => {
    const next = closeColumn(workspaceState, columnId);
    if (next === workspaceState) return;
    setWorkspaceState(next);
    if (next.activeColumnId !== workspaceState.activeColumnId) {
      const nextActive = next.columns.find((column) => column.id === next.activeColumnId);
      if (nextActive) void onActivateColumn(nextActive);
    }
  };

  const openSettings = (section: SettingsSection) => {
    setOpen(false);
    onOpenSettings(section);
  };

  const selectTopic = (topic: string) => {
    setOpen(false);
    onSelectTopic(topic);
  };

  const selectChannel = (topic: string, channelId: string) => {
    setOpen(false);
    onSelectChannel(topic, channelId);
  };


  return (
    <>
      <div className='shell-control-cluster' data-control-center-open={workspaceState.controlCenterOpen}>
        <AccountMenu onOpen={() => setOpen(false)} onManage={() => openSettings('account')} onProfile={onOpenProfile} />
        <Button
          ref={triggerRef}
          className='shell-control-center-trigger'
          variant='secondary'
          type='button'
          aria-label={triggerLabel}
          aria-controls={CONTROL_CENTER_ID}
          aria-expanded={workspaceState.controlCenterOpen}
          data-status={statusKey}
          data-testid='control-center-trigger'
          onClick={() => setOpen(!workspaceState.controlCenterOpen)}
        >
          <Menu className='size-5' aria-hidden='true' />
          <span>{t('shell:controlCenter.title')}</span>
          <span className='shell-control-center-status-dot' aria-hidden='true' />
          {notificationStatus.unread_count > 0 ? (
            <Badge className='shell-control-center-trigger-badge' tone='accent'>
              {notificationStatus.unread_count > 99
                ? '99+'
                : formatCount(notificationStatus.unread_count)}
            </Badge>
          ) : null}
        </Button>
        {workspaceState.controlCenterOpen ? null : (
          <>
            <Button
              className='shell-tester-feedback-trigger'
              variant='secondary'
              type='button'
              aria-label={t('shell:testerFeedback.openButton')}
              data-testid='tester-feedback-trigger'
              onClick={onOpenTesterFeedback}
            >
              <MessageSquarePlus className='size-5' aria-hidden='true' />
              <span>{t('shell:testerFeedback.openButton')}</span>
            </Button>
            {/* #1189: 更新がある間だけ、Control Center を開かずに「リリースと更新」へ行ける。 */}
            {updateAvailable ? (
              <Button
                className='shell-update-available-trigger'
                variant='primary'
                type='button'
                aria-label={t('shell:navigation.updateAvailable')}
                data-testid='update-available-trigger'
                onClick={() => openSettings('release')}
              >
                <Download className='size-5' aria-hidden='true' />
                <span>{t('shell:navigation.updateAvailable')}</span>
              </Button>
            ) : null}
          </>
        )}
      </div>

      {workspaceState.controlCenterOpen ? (
        <aside
          id={CONTROL_CENTER_ID}
          className='shell-control-center'
          role='complementary'
          aria-label={t('shell:controlCenter.title')}
        >
          <header className='shell-control-center-header'>
            <div>
              <h2>{t('shell:controlCenter.title')}</h2>
              <p className='shell-control-center-summary'>{statusLabel}</p>
            </div>
            <IconButton
              ref={closeButtonRef}
              variant='ghost'
              type='button'
              label={t('shell:controlCenter.close')}
              onClick={() => setOpen(false, true)}
            >
              <X className='size-5' aria-hidden='true' />
            </IconButton>
          </header>

          <div className='shell-control-center-grid'>
            <section className='shell-control-center-section' aria-labelledby='control-center-columns'>
              <div className='shell-control-center-section-heading'>
                <Columns3 className='size-5' aria-hidden='true' />
                <h3 id='control-center-columns'>{t('shell:controlCenter.sections.columns')}</h3>
              </div>
              <div className='shell-control-center-add-grid'>
                {addableColumnKinds.map((kind) => (
                  <Button
                    key={kind}
                    variant='secondary'
                    size='sm'
                    type='button'
                    aria-label={t('shell:controlCenter.addColumn', { title: titles[kind] })}
                    onClick={() => addColumn(kind)}
                  >
                    <Plus className='size-4' aria-hidden='true' />
                    {titles[kind]}
                  </Button>
                ))}
              </div>
              <SavedWorkspaceLayouts
                onActivateLayout={(layout) => {
                  const next = {
                    ...applySavedWorkspaceLayout(workspaceState, layout),
                    controlCenterOpen: false,
                  };
                  setWorkspaceState(next);
                  const activeColumn = next.columns.find(
                    (column) => column.id === next.activeColumnId
                  );
                  if (activeColumn) void onActivateColumn(activeColumn);
                }}
              />
              <ul className='shell-control-center-column-list'>
                {workspaceState.columns.map((column, index) => {
                  const active = workspaceState.activeColumnId === column.id;
                  return (
                    <li key={column.id} className='shell-control-center-column-row'>
                      <button
                        className='shell-control-center-column-focus'
                        type='button'
                        aria-label={t('shell:controlCenter.focusColumn', {
                          title: titles[column.kind],
                        })}
                        aria-current={active ? 'true' : undefined}
                        onClick={() => focusColumn(column)}
                      >
                        <span>
                          {index + 1}. {titles[column.kind]}
                        </span>
                        {scopeLabel(column) ? <small>{scopeLabel(column)}</small> : null}
                      </button>
                      <IconButton
                        variant='ghost'
                        className='min-h-11 min-w-11'
                        type='button'
                        label={t(
                          column.pinned
                            ? 'shell:controlCenter.unpinColumn'
                            : 'shell:controlCenter.pinColumn',
                          { title: titles[column.kind] }
                        )}
                        aria-pressed={column.pinned}
                        onClick={() =>
                          setWorkspaceState((current) =>
                            setColumnPinned(current, column.id, !column.pinned)
                          )
                        }
                      >
                        {column.pinned ? (
                          <PinOff className='size-4' aria-hidden='true' />
                        ) : (
                          <Pin className='size-4' aria-hidden='true' />
                        )}
                      </IconButton>
                      <IconButton
                        variant='ghost'
                        className='min-h-11 min-w-11'
                        type='button'
                        disabled={workspaceState.columns.length <= 1}
                        label={t('shell:controlCenter.closeColumn', {
                          title: titles[column.kind],
                        })}
                        onClick={() => closeManagedColumn(column.id)}
                      >
                        <X className='size-4' aria-hidden='true' />
                      </IconButton>
                    </li>
                  );
                })}
              </ul>
            </section>

            <section className='shell-control-center-section' aria-labelledby='control-center-places'>
              <div className='shell-control-center-section-heading'>
                <Compass className='size-5' aria-hidden='true' />
                <h3 id='control-center-places'>{t('shell:controlCenter.sections.places')}</h3>
              </div>
              <div className='shell-control-center-topic-entry'>
                <Input
                  value={topicInput}
                  onChange={(event) => onTopicInputChange(event.target.value)}
                  placeholder={t('shell:navigation.placeholder')}
                  aria-label={t('shell:navigation.addTopic')}
                />
                <IconButton
                  variant='secondary'
                  type='button'
                  label={t('common:actions.add')}
                  onClick={() => void onAddTopic()}
                >
                  <BookPlus className='size-4' aria-hidden='true' />
                </IconButton>
              </div>
              <div className='shell-control-center-place-actions'>
                <Button
                  variant='secondary'
                  type='button'
                  onClick={() => {
                    setOpen(false);
                    onOpenChannelManager();
                  }}
                >
                  <GitBranchPlus className='size-4' aria-hidden='true' />
                  {t('shell:controlCenter.createJoinChannel')}
                </Button>
                <Button
                  variant='ghost'
                  type='button'
                  disabled={!selectedChannelId || !onOpenChannelSettings}
                  aria-describedby={selectedChannelId ? undefined : 'control-center-share-channel-hint'}
                  onClick={() => {
                    if (selectedChannelId) {
                      setOpen(false);
                      onOpenChannelSettings?.(activeTopic, selectedChannelId);
                    }
                  }}
                >
                  <Radio className='size-4' aria-hidden='true' />
                  {t('shell:controlCenter.shareChannel')}
                </Button>
                {selectedChannelId ? null : (
                  <p
                    id='control-center-share-channel-hint'
                    className='shell-control-center-place-hint'
                  >
                    {t('shell:controlCenter.shareChannelHint')}
                  </p>
                )}
              </div>
              <div className='shell-control-center-place-list'>
                <FilterableTopicNavList
                  alwaysShowControls
                  showAllScopes
                  items={topicItems}
                  onSelectTopic={selectTopic}
                  onSelectChannel={selectChannel}
                  onOpenChannelSettings={onOpenChannelSettings}
                  onOpenChannelManager={(topic) => {
                    setOpen(false);
                    onOpenChannelManagerForTopic(topic);
                  }}
                  onLeaveChannel={onLeaveChannel}
                  onRemoveTopic={onRemoveTopic}
                  onCopyTopicLink={onCopyTopicLink}
                  onRequestTopicIndexing={onRequestTopicIndexing}
                  onToggleTopicGossip={onToggleTopicGossip}
                  onToggleChannelGossip={onToggleChannelGossip}
                />
              </div>
            </section>

            <section className='shell-control-center-section' aria-labelledby='control-center-activity'>
              <div className='shell-control-center-section-heading'>
                <Bell className='size-5' aria-hidden='true' />
                <h3 id='control-center-activity'>{t('shell:controlCenter.sections.activity')}</h3>
              </div>
              <div className='shell-control-center-action-list'>
                <Button variant='secondary' type='button' onClick={() => addColumn('notifications')}>
                  <Bell className='size-4' aria-hidden='true' />
                  {t('shell:primarySections.notifications')}
                  <Badge tone={notificationStatus.unread_count > 0 ? 'accent' : 'neutral'}>
                    {notificationStatus.unread_count > 99
                      ? '99+'
                      : formatCount(notificationStatus.unread_count)}
                  </Badge>
                </Button>
                <Button variant='ghost' type='button' onClick={() => addColumn('messages')}>
                  <MessageCircle className='size-4' aria-hidden='true' />
                  {t('shell:primarySections.messages')}
                </Button>
              </div>
            </section>

            <section className='shell-control-center-section' aria-labelledby='control-center-system'>
              <div className='shell-control-center-section-heading'>
                <SlidersHorizontal className='size-5' aria-hidden='true' />
                <h3 id='control-center-system'>{t('shell:controlCenter.sections.system')}</h3>
              </div>
              <div className='shell-control-center-action-list'>
                <Button variant='secondary' type='button' onClick={() => openSettings('connectivity')}>
                  <Radio className='size-4' aria-hidden='true' />
                  <span className='min-w-0 text-left [overflow-wrap:anywhere]'>{guidance.label}
                    <span className='block text-xs text-muted-foreground'>{t('shell:settingsSections.connectivity.label')}</span>
                  </span>
                </Button>
                <Button variant='ghost' type='button' onClick={() => openSettings('release')}>
                  <Download className='size-4' aria-hidden='true' />
                  {updateAvailable
                    ? t('shell:navigation.updateAvailable')
                    : t('shell:controlCenter.release')}
                </Button>
                <Button
                  variant='ghost'
                  type='button'
                  aria-label={t('shell:workspace.communityNodeUnavailableAction')}
                  onClick={() => openSettings('community-node')}
                >
                  <Radio className='size-4' aria-hidden='true' />
                  {t('shell:settingsSections.community-node.label')}
                </Button>
                <Button variant='ghost' type='button' onClick={() => openSettings('appearance')}
                  aria-label={t('shell:controlCenter.settings')} aria-describedby='control-center-appearance-hint'>
                  <Settings className='size-4' aria-hidden='true' />
                  <span className='text-left'>
                    <span className='block'>{t('shell:controlCenter.settings')}</span>
                    <span id='control-center-appearance-hint' className='block text-xs text-muted-foreground'>
                      {t('shell:controlCenter.languageSettingsHint')}
                    </span>
                  </span>
                </Button>
                <Button variant='ghost' type='button' onClick={() => openSettings('keyboard')}>
                  <Keyboard className='size-4' aria-hidden='true' />
                  {t('shell:settingsSections.keyboard.label')}
                </Button>
                {/* #967: 端末移行・故障への備えの入口。設定の backup section を開くだけで、backup は開始しない。 */}
                <Button variant='ghost' type='button' onClick={() => openSettings('backup')}>
                  <DatabaseBackup className='size-4' aria-hidden='true' />
                  {t('shell:settingsSections.backup.label')}
                </Button>
                <Button variant='ghost' type='button' onClick={() => openSettings('about')}>
                  <Info className='size-4' aria-hidden='true' />
                  {t('shell:settingsSections.about.label')}
                </Button>
                {developerModeEnabled ? (
                  <Button variant='ghost' type='button' onClick={() => openSettings('developer')}>
                    <SlidersHorizontal className='size-4' aria-hidden='true' />
                    {t('shell:settingsSections.developer.label')}
                  </Button>
                ) : null}
              </div>
            </section>
          </div>
        </aside>
      ) : null}
    </>
  );
}
