import {
  PrivateChannelPanel,
  PrivateChannelSettingsPanel,
} from '@/components/extended/PrivateChannelPanel';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { ImageCropDialog } from '@/components/ui/ImageCropDialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Notice } from '@/components/ui/notice';
import { Textarea } from '@/components/ui/textarea';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';

import { authorDisplayLabel } from '@/shell/presentation';
import { useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';
import { activeWorkspaceScope } from '@/shell/slices/workspace';
import type { Translate } from '@/shell/actions/shared';
import type { useShellDialogs } from '@/shell/page/useShellDialogs';
import type { useSharePreview } from '@/shell/page/useSharePreview';
import type { useDesktopShellActions } from '@/shell/useDesktopShellActions';
import { useDesktopShellViewModels } from '@/shell/useDesktopShellViewModels';
import { useCallback, useRef } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { topicDisplayName } from '@/lib/topicId';
import type { CommunityIndexingTarget } from '@/components/core/CommunityIndexingRequestDialog';

type ViewModels = ReturnType<typeof useDesktopShellViewModels>;
type OverlayActions = Pick<
  ReturnType<typeof useDesktopShellActions>,
  | 'handleCreateGameRoom'
  | 'handleCreateLiveSession'
  | 'handleCreatePrivateChannel'
  | 'handleJoinChannelAccess'
  | 'handleSelectPrivateChannel'
  | 'handleLeavePrivateChannel'
  | 'handleProfileAvatarFile'
  | 'handleShareChannelAccess'
>;
type ShellDialogs = Pick<
  ReturnType<typeof useShellDialogs>,
  | 'channelDialogOpen'
  | 'channelSettingsDialogOpen'
  | 'confirmLeaveChannel'
  | 'gameCreateDialogOpen'
  | 'leaveChannelDialogOpen'
  | 'liveCreateDialogOpen'
  | 'profileAvatarCropFile'
  | 'profileAvatarCropOpen'
  | 'setChannelDialogOpen'
  | 'setChannelSettingsDialogOpen'
  | 'setGameCreateDialogOpen'
  | 'setLeaveChannelDialogOpen'
  | 'setLiveCreateDialogOpen'
  | 'setProfileAvatarCropFile'
  | 'setProfileAvatarCropOpen'
>;
type SharePreview = Pick<
  ReturnType<typeof useSharePreview>,
  | 'confirmImport'
  | 'data'
  | 'error'
  | 'handleOpenChange'
  | 'importPending'
  | 'loading'
  | 'open'
  | 'token'
>;

type DesktopShellOverlaysProps = {
  actions: OverlayActions;
  dialogs: ShellDialogs;
  t: Translate;
  viewModels: Pick<
    ViewModels,
    | 'activeComposeAudienceLabel'
    | 'activeChannelPanelState'
    | 'channelAudienceOptions'
    | 'activePrivateChannel'
  >;
  handleCopyInternalLink: (link: string) => void;
  sharePreview: SharePreview;
  clipboardToastId: number;
  onRequestPrivateIndexing: (target: CommunityIndexingTarget) => void;
  // 作成・参加 Dialog の参加済み一覧から設定・共有 Dialog へ進む(Issue #966)。
  onOpenChannelSettings: (topicId: string, channelId: string) => void;
  onLoadMoreJoinedChannels: (topicId: string) => Promise<void>;
};

// Issue #966: Radix の既定の focus 復元はこの shell では body へ落ちるため、
// TesterFeedbackDialog と同じく開いた要素を記録して閉じたときに戻す。
// Control Center から開いた場合は trigger が閉じて外れているため、そのときは既定に任せる。
function useDialogReturnFocus() {
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const onOpenAutoFocus = useCallback(() => {
    returnFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }, []);
  const onCloseAutoFocus = useCallback((event: Event) => {
    const target = returnFocusRef.current;
    returnFocusRef.current = null;
    if (target?.isConnected) {
      event.preventDefault();
      target.focus();
    }
  }, []);
  return { onOpenAutoFocus, onCloseAutoFocus };
}

function AccessPreviewItem({
  label,
  value,
  tooltip,
}: {
  label: string;
  value: string | null;
  tooltip: string;
}) {
  return (
    <TooltipProvider delayDuration={180}>
      <Tooltip>
        <TooltipTrigger asChild>
          <div>
            <dt>{label}</dt>
            <dd>{value ?? '-'}</dd>
          </div>
        </TooltipTrigger>
        <TooltipContent>{tooltip}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

export function DesktopShellOverlays({
  actions,
  dialogs,
  t,
  viewModels,
  handleCopyInternalLink,
  sharePreview,
  clipboardToastId,
  onRequestPrivateIndexing,
  onOpenChannelSettings,
  onLoadMoreJoinedChannels,
}: DesktopShellOverlaysProps) {
  const {
    handleCreateGameRoom,
    handleCreateLiveSession,
    handleCreatePrivateChannel,
    handleJoinChannelAccess,
    handleSelectPrivateChannel,
    handleProfileAvatarFile,
    handleShareChannelAccess,
  } = actions;
  const {
    channelDialogOpen,
    channelSettingsDialogOpen,
    gameCreateDialogOpen,
    leaveChannelDialogOpen,
    liveCreateDialogOpen,
    profileAvatarCropFile,
    profileAvatarCropOpen,
    setChannelDialogOpen,
    setChannelSettingsDialogOpen,
    setGameCreateDialogOpen,
    setLeaveChannelDialogOpen,
    setLiveCreateDialogOpen,
    setProfileAvatarCropFile,
    setProfileAvatarCropOpen,
  } = dialogs;
  const {
    confirmImport: handleConfirmShareImport,
    data: sharePreviewData,
    error: sharePreviewError,
    handleOpenChange: handleSharePreviewOpenChange,
    importPending: shareImportPending,
    loading: sharePreviewLoading,
    open: sharePreviewOpen,
    token: sharePreviewToken,
  } = sharePreview;
  const {
    activeComposeAudienceLabel,
    activeChannelPanelState,
    channelAudienceOptions,
    activePrivateChannel,
  } = viewModels;
  const {
    activeTopic,
    channelActionPending,
    channelAudienceInput,
    channelError,
    channelLabelInput,
    gameCreatePending,
    gameDescription,
    gameError,
    gameParticipantsInput,
    gameTitle,
    inviteOutput,
    inviteOutputLabel,
    inviteTokenInput,
    joinedChannelsByTopic,
    joinedChannelsNextCursorByTopic,
    knownAuthorsByPubkey,
    localProfile,
    liveCreatePending,
    liveDescription,
    liveError,
    liveTitle,
    syncStatus,
  } = useDesktopShellStore(
    useShallow((s) => ({
      activeTopic: activeWorkspaceScope(s.workspaceState).topicId,
      channelActionPending: s.channelActionPending,
      channelAudienceInput: s.channelAudienceInput,
      channelError: s.channelError,
      channelLabelInput: s.channelLabelInput,
      gameCreatePending: s.gameCreatePending,
      gameDescription: s.gameDescription,
      gameError: s.gameError,
      gameParticipantsInput: s.gameParticipantsInput,
      gameTitle: s.gameTitle,
      inviteOutput: s.inviteOutput,
      inviteOutputLabel: s.inviteOutputLabel,
      inviteTokenInput: s.inviteTokenInput,
      joinedChannelsByTopic: s.joinedChannelsByTopic,
      joinedChannelsNextCursorByTopic: s.joinedChannelsNextCursorByTopic,
      knownAuthorsByPubkey: s.knownAuthorsByPubkey,
      localProfile: s.localProfile,
      liveCreatePending: s.liveCreatePending,
      liveDescription: s.liveDescription,
      liveError: s.liveError,
      liveTitle: s.liveTitle,
      syncStatus: s.syncStatus,
    }))
  );
  const channelDialogFocus = useDialogReturnFocus();
  const channelSettingsFocus = useDialogReturnFocus();
  const setChannelLabelInput = useDesktopShellFieldSetter('channelLabelInput');
  const setChannelAudienceInput = useDesktopShellFieldSetter('channelAudienceInput');
  const setInviteTokenInput = useDesktopShellFieldSetter('inviteTokenInput');
  const setLiveTitle = useDesktopShellFieldSetter('liveTitle');
  const setLiveDescription = useDesktopShellFieldSetter('liveDescription');
  const setGameTitle = useDesktopShellFieldSetter('gameTitle');
  const setGameDescription = useDesktopShellFieldSetter('gameDescription');
  const setGameParticipantsInput = useDesktopShellFieldSetter('gameParticipantsInput');
  const previewOwnerProfile =
    sharePreviewData?.owner_pubkey === syncStatus.local_author_pubkey
      ? localProfile
      : sharePreviewData
        ? knownAuthorsByPubkey[sharePreviewData.owner_pubkey] ?? null
        : null;
  const previewOwnerLabel = sharePreviewData
    ? authorDisplayLabel(
        sharePreviewData.owner_pubkey,
        previewOwnerProfile?.display_name,
        previewOwnerProfile?.name
      )
    : null;
  const previewAudienceLabel = sharePreviewData
    ? t(
        sharePreviewData.kind === 'invite'
          ? 'channels:audienceOptions.invite_only'
          : sharePreviewData.kind === 'grant'
            ? 'channels:audienceOptions.friend_only'
            : 'channels:audienceOptions.friend_plus'
      )
    : null;

  return (
    <>
      <ImageCropDialog
        open={profileAvatarCropOpen}
        file={profileAvatarCropFile}
        title={t('profile:editor.picture')}
        description={t('profile:editor.pictureCropDescription', {
          defaultValue: 'Drag and zoom to choose the visible square for your avatar.',
        })}
        confirmLabel={t('common:actions.save')}
        onOpenChange={(open) => {
          setProfileAvatarCropOpen(open);
          if (!open) {
            setProfileAvatarCropFile(null);
          }
        }}
        onConfirm={async ({ croppedFile }) => {
          await handleProfileAvatarFile(croppedFile);
          setProfileAvatarCropOpen(false);
          setProfileAvatarCropFile(null);
        }}
      />

      <Dialog open={channelDialogOpen} onOpenChange={setChannelDialogOpen}>
        <DialogContent
          onOpenAutoFocus={channelDialogFocus.onOpenAutoFocus}
          onCloseAutoFocus={channelDialogFocus.onCloseAutoFocus}
        >
          <DialogHeader>
            <DialogTitle>{t('channels:createDialogTitle')}</DialogTitle>
            <DialogDescription>{topicDisplayName(activeTopic)}</DialogDescription>
          </DialogHeader>
          <DialogBody>
            <PrivateChannelPanel
              status={activeChannelPanelState.status}
              error={channelError ?? activeChannelPanelState.error}
              pendingAction={channelActionPending}
              channelLabel={channelLabelInput}
              channelAudience={channelAudienceInput}
              channelAudienceOptions={channelAudienceOptions}
              inviteTokenInput={inviteTokenInput}
              inviteOutput={inviteOutput}
              inviteOutputLabel={inviteOutputLabel}
              onChannelLabelChange={setChannelLabelInput}
              onChannelAudienceChange={setChannelAudienceInput}
              onInviteTokenChange={setInviteTokenInput}
              joinedChannels={joinedChannelsByTopic[activeTopic] ?? []}
              onLoadMoreJoinedChannels={joinedChannelsNextCursorByTopic[activeTopic]
                ? () => onLoadMoreJoinedChannels(activeTopic) : undefined}
              onCreateChannel={(event) => void handleCreatePrivateChannel(event)}
              onJoin={(event) => void handleJoinChannelAccess(event)}
              onCopyInviteOutput={handleCopyInternalLink}
              onSelectJoinedChannel={(channelId) => {
                setChannelDialogOpen(false);
                handleSelectPrivateChannel(activeTopic, channelId);
              }}
              onOpenJoinedChannelSettings={(channelId) => {
                setChannelDialogOpen(false);
                onOpenChannelSettings(activeTopic, channelId);
              }}
            />
          </DialogBody>
        </DialogContent>
      </Dialog>

      <Dialog open={channelSettingsDialogOpen} onOpenChange={setChannelSettingsDialogOpen}>
        <DialogContent
          onOpenAutoFocus={channelSettingsFocus.onOpenAutoFocus}
          onCloseAutoFocus={channelSettingsFocus.onCloseAutoFocus}
        >
          <DialogHeader>
            <DialogTitle>{t('channels:settings.title')}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            {activePrivateChannel ? (
              <PrivateChannelSettingsPanel
                error={channelError ?? activeChannelPanelState.error}
                pendingAction={channelActionPending}
                channel={activePrivateChannel}
                inviteOutput={inviteOutput}
                inviteOutputLabel={inviteOutputLabel}
                onShare={() => void handleShareChannelAccess()}
                onRequestIndexing={() =>
                  onRequestPrivateIndexing({
                    kind: 'private_channel',
                    topicId: activeTopic,
                    channelId: activePrivateChannel.channel_id,
                    channelLabel: activePrivateChannel.label,
                  })
                }
                onCopyInviteOutput={handleCopyInternalLink}
              />
            ) : (
              <Notice>{t('channels:selectChannelNotice')}</Notice>
            )}
          </DialogBody>
        </DialogContent>
      </Dialog>

      <Dialog open={leaveChannelDialogOpen} onOpenChange={setLeaveChannelDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('channels:leaveDialog.title')}</DialogTitle>
            <DialogDescription>{t('channels:leaveDialog.description')}</DialogDescription>
          </DialogHeader>
          <DialogBody>
            <div className='ui-dialog-footer'>
              <Button
                variant='secondary'
                type='button'
                onClick={() => setLeaveChannelDialogOpen(false)}
              >
                {t('channels:leaveDialog.no')}
              </Button>
              <Button
                type='button'
                disabled={channelActionPending === 'leave'}
                onClick={() =>
                  void dialogs.confirmLeaveChannel(actions.handleLeavePrivateChannel)
                }
              >
                {t('channels:leaveDialog.yes')}
              </Button>
            </div>
          </DialogBody>
        </DialogContent>
      </Dialog>

      <Dialog
        open={sharePreviewOpen}
        onOpenChange={handleSharePreviewOpenChange}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('channels:previewDialog.title')}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            {sharePreviewLoading ? <Notice>{t('channels:loading')}</Notice> : null}
            {sharePreviewError ? <Notice tone='destructive'>{sharePreviewError}</Notice> : null}
            {sharePreviewData ? (
              <dl className='access-preview-list'>
                <AccessPreviewItem
                  label={t('common:labels.owner')}
                  value={previewOwnerLabel}
                  tooltip={sharePreviewData.owner_pubkey}
                />
                <AccessPreviewItem
                  label={t('common:labels.sourceTopic')}
                  value={sharePreviewData.topic_id}
                  tooltip={sharePreviewData.topic_id}
                />
                <AccessPreviewItem
                  label={t('channels:previewDialog.channel')}
                  value={sharePreviewData.channel_label}
                  tooltip={sharePreviewData.channel_id}
                />
                <AccessPreviewItem
                  label={t('common:labels.audience')}
                  value={previewAudienceLabel}
                  tooltip={`${sharePreviewData.kind} / ${sharePreviewData.epoch_id}`}
                />
              </dl>
            ) : null}
            <div className='ui-dialog-footer'>
              {sharePreviewToken ? (
                <Button
                  variant='secondary'
                  type='button'
                  onClick={() => handleCopyInternalLink(sharePreviewToken)}
                >
                  {t('channels:previewDialog.copyToken')}
                </Button>
              ) : null}
              <Button
                variant='secondary'
                type='button'
                onClick={() => handleSharePreviewOpenChange(false)}
              >
                {t('common:actions.cancel')}
              </Button>
              <Button
                type='button'
                disabled={sharePreviewLoading || shareImportPending || !sharePreviewData}
                onClick={() => void handleConfirmShareImport()}
              >
                {shareImportPending ? t('common:actions.join') : t('channels:previewDialog.import')}
              </Button>
            </div>
          </DialogBody>
        </DialogContent>
      </Dialog>

      <Dialog open={liveCreateDialogOpen} onOpenChange={setLiveCreateDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('live:actions.start')}</DialogTitle>
            <DialogDescription>
              {t('common:labels.audience')}: {activeComposeAudienceLabel}
            </DialogDescription>
          </DialogHeader>
          <DialogBody>
            <form
              className='composer composer-compact'
              onSubmit={(event) => void handleCreateLiveSession(event)}
              aria-busy={liveCreatePending}
            >
              <Label>
                <span>{t('live:fields.title')}</span>
                <Input
                  value={liveTitle}
                  onChange={(event) => setLiveTitle(event.target.value)}
                  placeholder={t('live:fields.placeholders.title')}
                  disabled={liveCreatePending}
                />
              </Label>
              <Label>
                <span>{t('live:fields.description')}</span>
                <Textarea
                  value={liveDescription}
                  onChange={(event) => setLiveDescription(event.target.value)}
                  placeholder={t('live:fields.placeholders.description')}
                  disabled={liveCreatePending}
                />
              </Label>
              {liveError ? <p className='error error-inline'>{liveError}</p> : null}
              <Button type='submit' disabled={liveCreatePending}>
                {t('live:actions.start')}
              </Button>
            </form>
          </DialogBody>
        </DialogContent>
      </Dialog>

      <Dialog open={gameCreateDialogOpen} onOpenChange={setGameCreateDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('game:actions.createRoom')}</DialogTitle>
            <DialogDescription>
              {t('common:labels.audience')}: {activeComposeAudienceLabel}
            </DialogDescription>
          </DialogHeader>
          <DialogBody>
            <form
              className='composer composer-compact'
              onSubmit={(event) => void handleCreateGameRoom(event)}
              aria-busy={gameCreatePending}
            >
              <Label>
                <span>{t('game:fields.title')}</span>
                <Input
                  value={gameTitle}
                  onChange={(event) => setGameTitle(event.target.value)}
                  placeholder={t('game:fields.placeholders.title')}
                  disabled={gameCreatePending}
                />
              </Label>
              <Label>
                <span>{t('game:fields.description')}</span>
                <Textarea
                  value={gameDescription}
                  onChange={(event) => setGameDescription(event.target.value)}
                  placeholder={t('game:fields.placeholders.description')}
                  disabled={gameCreatePending}
                />
              </Label>
              <Label>
                <span>{t('game:fields.participants')}</span>
                <Input
                  value={gameParticipantsInput}
                  onChange={(event) => setGameParticipantsInput(event.target.value)}
                  placeholder={t('game:fields.placeholders.participants')}
                  disabled={gameCreatePending}
                />
              </Label>
              {gameError ? <p className='error error-inline'>{gameError}</p> : null}
              <Button type='submit' disabled={gameCreatePending}>
                {t('game:actions.createRoom')}
              </Button>
            </form>
          </DialogBody>
        </DialogContent>
      </Dialog>

      {clipboardToastId > 0 ? (
        <div className='pointer-events-none fixed right-4 bottom-4 z-[90] w-[calc(100vw-2rem)] max-w-xs'>
          <Notice
            key={clipboardToastId}
            role='status'
            aria-live='polite'
            aria-atomic='true'
            tone='accent'
            className='pointer-events-auto'
          >
            {t('common:feedback.copiedToClipboard')}
          </Notice>
        </div>
      ) : null}
    </>
  );
}
