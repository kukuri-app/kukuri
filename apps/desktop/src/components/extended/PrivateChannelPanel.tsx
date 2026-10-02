import { useState, type FormEventHandler } from 'react';
import { useTranslation } from 'react-i18next';
import { Copy, Settings } from 'lucide-react';

import { buildChannelAccessPreviewDeepLink } from '@/lib/internalLinks';
import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { IconButton } from '@/components/ui/icon-button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Notice } from '@/components/ui/notice';
import { Select } from '@/components/ui/select';
import { Textarea } from '@/components/ui/textarea';
import type { JoinedPrivateChannelView } from '@/lib/api';

import {
  type ChannelAudienceOption,
  type ExtendedPanelStatus,
  type InviteOutputLabel,
  type PrivateChannelListItemView,
  type PrivateChannelPendingAction,
} from './types';

function audienceSummaryLabel(
  label: InviteOutputLabel,
  t: ReturnType<typeof useTranslation<'channels'>>['t']
): string {
  if (label === 'grant' || label === 'share') {
    return t('latestShare');
  }
  return t('latestInvite');
}

function policyDescription(
  audienceKind: PrivateChannelListItemView['channel']['audience_kind'],
  t: ReturnType<typeof useTranslation<'channels'>>['t']
) {
  if (audienceKind === 'friend_only') {
    return t('policies.friend_only');
  }
  if (audienceKind === 'friend_plus') {
    return t('policies.friend_plus');
  }
  return t('policies.invite_only');
}

type PrivateChannelPanelProps = {
  status: ExtendedPanelStatus;
  error: string | null;
  pendingAction: PrivateChannelPendingAction;
  channelLabel: string;
  channelAudience: ChannelAudienceOption['value'];
  channelAudienceOptions: ChannelAudienceOption[];
  inviteTokenInput: string;
  inviteOutput?: string | null;
  inviteOutputLabel?: InviteOutputLabel;
  // 同じトピックで参加済みのチャンネル(Issue #966)。未参加なら空配列または省略。
  joinedChannels?: JoinedPrivateChannelView[];
  // 一覧の続きがあるときの「さらに表示」(#1218 AC-4d)。失敗は reject で受け取る。
  onLoadMoreJoinedChannels?: () => Promise<void>;
  onChannelLabelChange: (value: string) => void;
  onChannelAudienceChange: (value: ChannelAudienceOption['value']) => void;
  onInviteTokenChange: (value: string) => void;
  onCreateChannel: FormEventHandler<HTMLFormElement>;
  onJoin: FormEventHandler<HTMLFormElement>;
  onCopyInviteOutput?: (token: string) => void;
  onSelectJoinedChannel?: (channelId: string) => void;
  onOpenJoinedChannelSettings?: (channelId: string) => void;
};

type PrivateChannelSettingsPanelProps = {
  error: string | null;
  pendingAction: PrivateChannelPendingAction;
  channel: PrivateChannelListItemView['channel'];
  inviteOutput: string | null;
  inviteOutputLabel: InviteOutputLabel;
  onShare: () => void;
  onRequestIndexing?: () => void;
  onCopyInviteOutput?: (token: string) => void;
};

export function PrivateChannelPanel({
  status,
  error,
  pendingAction,
  channelLabel,
  channelAudience,
  channelAudienceOptions,
  inviteTokenInput,
  inviteOutput = null,
  inviteOutputLabel = 'invite',
  joinedChannels = [],
  onLoadMoreJoinedChannels,
  onChannelLabelChange,
  onChannelAudienceChange,
  onInviteTokenChange,
  onCreateChannel,
  onJoin,
  onCopyInviteOutput,
  onSelectJoinedChannel,
  onOpenJoinedChannelSettings,
}: PrivateChannelPanelProps) {
  const { t } = useTranslation(['channels', 'common', 'shell']);
  const channelActionDisabled = pendingAction !== null;
  const channelAccessDeepLink = inviteOutput
    ? buildChannelAccessPreviewDeepLink(inviteOutput)
    : null;
  const audienceDescription = policyDescription(channelAudience, t);
  const [moreState, setMoreState] = useState<'idle' | 'loading' | 'error'>('idle');
  const loadMore = () => {
    setMoreState('loading');
    onLoadMoreJoinedChannels?.().then(() => setMoreState('idle'), () => setMoreState('error'));
  };

  return (
    <div className='extended-module-stack'>
      <p className='private-channel-intro'>{t('channels:intro')}</p>
      {status === 'loading' ? <Notice>{t('channels:loading')}</Notice> : null}
      {status === 'error' && error ? <Notice tone='destructive'>{error}</Notice> : null}

      {joinedChannels.length > 0 ? (
        <Card className='panel-subsection private-channel-editor-block'>
          <CardHeader>
            <h3>{t('channels:joinedList.title')}</h3>
          </CardHeader>
          <ul className='private-channel-joined-list'>
            {joinedChannels.map((channel) => (
              <li key={channel.channel_id} className='private-channel-joined-row'>
                <button
                  type='button'
                  className='private-channel-joined-open'
                  aria-label={t('channels:joinedList.open', { channel: channel.label })}
                  disabled={!onSelectJoinedChannel}
                  onClick={() => onSelectJoinedChannel?.(channel.channel_id)}
                >
                  <span className='shell-topic-link-label'>{channel.label}</span>
                  <small>{t(`channels:audienceOptions.${channel.audience_kind}`)}</small>
                </button>
                {onOpenJoinedChannelSettings ? (
                  <Button
                    variant='ghost'
                    size='sm'
                    type='button'
                    aria-label={t('channels:joinedList.settings', { channel: channel.label })}
                    onClick={() => onOpenJoinedChannelSettings(channel.channel_id)}
                  >
                    <Settings className='size-4' aria-hidden='true' />
                    {t('shell:workspace.channelSettingsEntry')}
                  </Button>
                ) : null}
              </li>
            ))}
          </ul>
          {onLoadMoreJoinedChannels ? (
            <div className='private-channel-joined-more'>
              <Button
                variant='secondary'
                size='sm'
                type='button'
                aria-busy={moreState === 'loading'}
                disabled={moreState === 'loading'}
                onClick={loadMore}
              >
                {moreState === 'loading'
                  ? t('channels:joinedList.loadingMore')
                  : t('channels:joinedList.showMore')}
              </Button>
              {moreState === 'error' ? (
                <p className='error error-inline'>{t('common:errors.failedToLoadPrivateChannels')}</p>
              ) : null}
            </div>
          ) : null}
        </Card>
      ) : null}

      <div className='private-channel-editor-grid'>
        <Card className='panel-subsection private-channel-editor-block'>
          <CardHeader>
            <h3>{t('channels:editor.createBlockTitle')}</h3>
          </CardHeader>
          <p className='private-channel-help'>{t('channels:editor.createHint')}</p>
          <form className='composer composer-compact' onSubmit={onCreateChannel}>
            <Label>
              <span>{t('channels:editor.channelName')}</span>
              <Input
                value={channelLabel}
                onChange={(event) => onChannelLabelChange(event.target.value)}
                placeholder={t('channels:editor.placeholders.channelLabel')}
                disabled={channelActionDisabled}
              />
            </Label>
            <Label>
              <span>{t('channels:editor.audience')}</span>
              <Select
                aria-label={t('channels:editor.audience')}
                aria-describedby='private-channel-audience-description'
                value={channelAudience}
                onChange={(event) =>
                  onChannelAudienceChange(event.target.value as ChannelAudienceOption['value'])
                }
                disabled={channelActionDisabled}
              >
                {channelAudienceOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </Select>
            </Label>
            <p id='private-channel-audience-description' className='private-channel-help'>
              {audienceDescription}
            </p>
            <Button variant='secondary' type='submit' disabled={channelActionDisabled}>
              {t('channels:actions.createChannel')}
            </Button>
          </form>
        </Card>

        <Card className='panel-subsection private-channel-editor-block'>
          <CardHeader>
            <h3>{t('channels:editor.joinBlockTitle')}</h3>
          </CardHeader>
          <p className='private-channel-help'>{t('channels:editor.joinHint')}</p>
          <form className='composer composer-compact' onSubmit={onJoin}>
            <Label>
              <span>{t('channels:editor.join')}</span>
              <Textarea
                value={inviteTokenInput}
                onChange={(event) => onInviteTokenChange(event.target.value)}
                placeholder={t('channels:editor.placeholders.inviteToken')}
                disabled={channelActionDisabled}
              />
            </Label>
            <Button variant='secondary' type='submit' disabled={channelActionDisabled}>
              {t('channels:actions.join')}
            </Button>
          </form>
        </Card>
      </div>

      {inviteOutput && channelAccessDeepLink ? (
        <Notice tone='accent'>
          <div className='shell-inline-actions'>
            <strong>{t('channels:copyShareLink')}</strong>
            {onCopyInviteOutput ? (
              <IconButton
                variant='secondary'
                className='post-action-button'
                type='button'
                label={t('common:actions.copyLink')}
                onClick={() => onCopyInviteOutput(channelAccessDeepLink)}
              >
                <Copy className='size-4' aria-hidden='true' />
              </IconButton>
            ) : null}
          </div>
          <span className='sr-only'>{audienceSummaryLabel(inviteOutputLabel, t)}</span>
        </Notice>
      ) : null}

      {status !== 'error' && error ? <p className='error error-inline'>{error}</p> : null}
    </div>
  );
}

export function PrivateChannelSettingsPanel({
  error,
  pendingAction,
  channel,
  inviteOutput,
  inviteOutputLabel,
  onShare,
  onRequestIndexing,
  onCopyInviteOutput,
}: PrivateChannelSettingsPanelProps) {
  const { t } = useTranslation(['channels', 'common', 'shell']);
  const channelActionDisabled = pendingAction !== null;
  const policyLabel = policyDescription(channel.audience_kind, t);
  const channelAccessDeepLink = inviteOutput
    ? buildChannelAccessPreviewDeepLink(inviteOutput)
    : null;
  // 相互フォロー限定の共有リンク(grant)は owner だけが発行できる(app-api の guard と同じ条件)。
  // client 側の無効化は理由の提示であり、権限判定の正本は runtime のまま。
  const ownerOnlyShareBlocked = channel.audience_kind === 'friend_only' && !channel.is_owner;
  const shareDisabled = channelActionDisabled || ownerOnlyShareBlocked;

  return (
    <Card tone='accent' className='panel-subsection extended-channel-detail'>
      <CardHeader>
        <h3>{t('channels:settings.channelName', { channel: channel.label })}</h3>
      </CardHeader>
      <p className='private-channel-help'>{t('channels:settings.policy', { policy: policyLabel })}</p>

      <div className='extended-module-stack'>
        {(channel.audience_kind === 'friend_only' || channel.audience_kind === 'friend_plus') ? (
          <div className='topic-diagnostic topic-diagnostic-secondary'>
            {/* 参加者数は参加・退出 record が届く owner の端末だけが持つ(#1221 R5-H)。 */}
            {channel.participant_count != null ? (
              <span>{t('common:labels.participants')}: {channel.participant_count}</span>
            ) : null}
            <span>{t('common:labels.stale')}: {channel.stale_participant_count}</span>
            <span>
              {t('common:labels.owner')}: {channel.is_owner ? t('common:states.yes') : t('common:states.no')}
            </span>
          </div>
        ) : null}
        {channel.audience_kind === 'friend_only' && channel.rotation_required ? (
          <div className='topic-diagnostic topic-diagnostic-error'>
            <span>{t('channels:rotationRequired')}</span>
            <span>{t('channels:settings.rotationNextStep')}</span>
          </div>
        ) : null}
        {ownerOnlyShareBlocked ? (
          <Notice id='private-channel-share-reason'>{t('channels:settings.ownerOnlyShare')}</Notice>
        ) : channelActionDisabled ? (
          <Notice id='private-channel-share-reason' role='status'>
            {t('channels:settings.pending')}
          </Notice>
        ) : null}

        <div className='discovery-actions'>
          <Button
            aria-label={t('channels:actions.createShareLink')}
            aria-describedby={shareDisabled ? 'private-channel-share-reason' : undefined}
            className='w-full'
            variant='secondary'
            type='button'
            disabled={shareDisabled}
            onClick={onShare}
          >
            {t('channels:actions.createShareLink')}
          </Button>
          {onRequestIndexing ? (
            <Button
              className='w-full'
              variant='secondary'
              type='button'
              disabled={channelActionDisabled}
              onClick={onRequestIndexing}
            >
              {t('shell:indexingRequest.openPrivate')}
            </Button>
          ) : null}
        </div>
        <p className='private-channel-help'>{t('channels:settings.shareHint')}</p>

        {inviteOutput && channelAccessDeepLink ? (
          <Notice tone='accent'>
            <div className='shell-inline-actions'>
              <strong>{t('channels:copyShareLink')}</strong>
              {onCopyInviteOutput ? (
                <IconButton
                  variant='secondary'
                  className='post-action-button'
                  type='button'
                  label={t('common:actions.copyLink')}
                  onClick={() => onCopyInviteOutput(channelAccessDeepLink)}
                >
                  <Copy className='size-4' aria-hidden='true' />
                </IconButton>
              ) : null}
            </div>
            <span className='sr-only'>{audienceSummaryLabel(inviteOutputLabel, t)}</span>
          </Notice>
        ) : null}

        {error ? <p className='error error-inline'>{error}</p> : null}
      </div>
    </Card>
  );
}
