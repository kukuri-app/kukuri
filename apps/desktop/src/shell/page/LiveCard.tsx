import { useContext } from 'react';
import { Camera, Link2, Maximize2, MonitorUp, Repeat2, Reply } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { SmartReferenceText } from '@/components/core/SmartReferenceText';
import { LiveSessionMedia } from '@/components/extended/LiveSessionMedia';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { formatLocalizedTime } from '@/i18n/format';
import type { LiveSessionView } from '@/lib/api';
import { buildLiveLink } from '@/lib/internalLinks';
import { formatCount, localizeAudienceLabel, translateLiveStatus } from '@/shell/presentation';

import { LiveActionsContext } from './liveActions';


type LiveCardProps = {
  topic: string;
  session: LiveSessionView;
  // 投稿の中の表示では、その投稿の id。開始 post そのものなら「開始 post を開く」を出さない。
  postId?: string;
  targeted?: boolean;
  // #1841: 映像を出すか(ライブの列の左側に大きく出している Live のカードでは出さない)と、「大きく表示」の操作。
  media?: boolean;
  onStage?: () => void;
};

/** Live の一覧と、投稿の中の Live の表示が共用するカード(#1839)。 */
export function LiveCard({ topic, session, postId, targeted = false, media = true, onStage }: LiveCardProps) {
  const { t } = useTranslation(['common', 'live']);
  const actions = useContext(LiveActionsContext);
  if (!actions) return null;
  const id = session.session_id;
  const channelId = session.channel_id ?? null;
  const pending = actions.pendingBySessionId[id] === true;
  const isOwner = session.host_pubkey === actions.localPubkey;
  const ended = session.status === 'Ended';
  const startPostId = session.start_post_id ?? null;
  return (
    <article
      className={`post-card${targeted ? ' post-card-targeted' : ''}`}
      aria-busy={pending}
      data-live-session-id={id}
      tabIndex={targeted ? -1 : undefined}
    >
      <div className='post-meta'>
        <span>{session.title}</span>
        <span>{translateLiveStatus(session.status)}</span>
        <span className='reply-chip'>{localizeAudienceLabel(session.audience_label)}</span>
      </div>
      <div className='post-body'>
        <strong className='post-title post-copy-wrap'>
          <SmartReferenceText
            text={session.description || t('common:fallbacks.noDescription')}
            className='post-copy-wrap'
            onActivateReference={actions.activateReference}
          />
        </strong>
      </div>
      <div className='topic-diagnostic topic-diagnostic-secondary'>
        <span>{t('common:labels.viewers')}: {formatCount(session.viewer_count)}</span>
        <span>
          {t('common:labels.started')}: {formatLocalizedTime(session.started_at, actions.locale)}
        </span>
      </div>
      {session.ended_at ? (
        <div className='topic-diagnostic topic-diagnostic-secondary'>
          <span>
            {t('common:labels.ended')}: {formatLocalizedTime(session.ended_at, actions.locale)}
          </span>
        </div>
      ) : null}
      {media ? <LiveSessionMedia api={actions.api} topic={topic} session={session} isOwner={isOwner} /> : null}
      <div className='post-actions'>
        {isOwner && !ended && actions.broadcastingSessionId === null
          ? (['camera', 'screen'] as const).map((source) => (
              <Button
                key={source}
                variant='secondary'
                type='button'
                onClick={() => actions.startBroadcast({ topic, sessionId: id, title: session.title }, source)}
              >
                {source === 'camera' ? <Camera className='size-4' aria-hidden='true' /> : <MonitorUp className='size-4' aria-hidden='true' />}
                {t(`live:media.start.${source}`)}
              </Button>
            ))
          : null}
        {session.joined_by_me ? (
          <Button variant='secondary' type='button' disabled={pending} onClick={() => void actions.leave(topic, id)}>
            {t('common:actions.leave')}
          </Button>
        ) : (
          <Button
            variant='secondary'
            type='button'
            disabled={pending || ended}
            onClick={() => void actions.join(topic, id)}
          >
            {t('common:actions.join')}
          </Button>
        )}
        {isOwner ? (
          <Button
            variant='secondary'
            type='button'
            disabled={pending || ended}
            onClick={() => void actions.end(topic, id)}
          >
            {t('common:actions.end')}
          </Button>
        ) : null}
        {onStage && !ended ? (
          <Button variant='secondary' type='button' onClick={onStage}>
            <Maximize2 className='size-4' aria-hidden='true' />
            {t('live:actions.stage')}
          </Button>
        ) : null}
        <Button
          variant='secondary'
          type='button'
          onClick={() => actions.introduce({ session_id: id, topic_id: topic, channel_id: channelId }, session.title)}
        >
          <Repeat2 className='size-4' aria-hidden='true' />
          {t('live:actions.introduce')}
        </Button>
        {startPostId && startPostId !== postId ? (
          <Button variant='secondary' type='button' onClick={() => actions.openPost(topic, startPostId)}>
            <Reply className='size-4' aria-hidden='true' />
            {t('live:actions.openStartPost')}
          </Button>
        ) : null}
        <IconButton
          variant='secondary'
          className='post-action-button'
          type='button'
          label={t('common:actions.copyLink')}
          onClick={() => actions.copyLink(buildLiveLink(topic, id, channelId))}
        >
          <Link2 className='size-4' aria-hidden='true' />
        </IconButton>
      </div>
    </article>
  );
}
