import { useContext, useEffect, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { Camera, Link2, Maximize2, MonitorUp, Repeat2, Reply } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { PostGatedContent } from '@/components/core/PostAdvisoryNotice';
import { SmartReferenceText } from '@/components/core/SmartReferenceText';
import { LiveSessionMedia } from '@/components/extended/LiveSessionMedia';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Notice } from '@/components/ui/notice';
import { formatLocalizedTime } from '@/i18n/format';
import type { LiveSessionView, PostView } from '@/lib/api';
import { isPublicChannelId } from '@/lib/publicChannels';
import { buildLiveLink } from '@/lib/internalLinks';
import { formatCount, localizeAudienceLabel, translateLiveStatus } from '@/shell/presentation';

import { Dialog, DialogBody, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { authorDisplayLabel } from '@/shell/presentation';
import { useDesktopShellStore } from '@/shell/store';
import { isAdultLabeledPost } from '@/shell/media';
import { resolvePostAdvisory } from '@/shell/contentAdvisories';
import { LiveThumbnailField, LiveThumbnailImage } from './LiveThumbnailField';
import { useLiveThumbnail } from './useLiveThumbnail';
import { PublicThreadActionsContext } from './publicThreadActions';
import { LiveActionsContext } from './liveActions';


type LiveCardProps = {
  topic: string;
  session: LiveSessionView;
  // 投稿の中の表示では、その投稿の id。開始 post そのものなら「開始 post を開く」を出さない。
  postId?: string;
  post?: PostView;
  onStage?: () => void;
  onCloseDetails?: () => void;
};

/** 一覧は画像を主にし、参加・配信操作は詳細を開いてから行う。 */
export function LiveCard(props: LiveCardProps) {
  const { t } = useTranslation('live');
  const actions = useContext(LiveActionsContext);
  const threads = useContext(PublicThreadActionsContext);
  const api = actions?.api ?? threads?.api;
  const { session, topic } = props;
  const host = useDesktopShellStore((state) => session.host_pubkey === state.localProfile?.pubkey
    ? state.localProfile : state.knownAuthorsByPubkey[session.host_pubkey]);
  const hostLabel = authorDisplayLabel(session.host_pubkey, host?.display_name, host?.name);
  const { element: thumbnailRef, image: thumbnail, refresh: refreshThumbnail } = useLiveThumbnail(api, topic, session.session_id, session.status === 'Live');
  const [open, setOpen] = useState(false);
  const [thumbnailError, setThumbnailError] = useState(false);
  const [savingThumbnail, setSavingThumbnail] = useState(false);
  return <article className='live-thumbnail-card' data-live-session-id={session.session_id}>
    <div ref={thumbnailRef}>
      <button type='button' className='live-thumbnail-open' onClick={() => setOpen(true)}
        aria-label={`${translateLiveStatus(session.status)} ${session.title} ${hostLabel}`}>
        <LiveThumbnailImage url={thumbnail?.data_url} />
        <span className='live-thumbnail-status'>{translateLiveStatus(session.status)}</span>
        <span className='live-thumbnail-caption'><strong>{session.title}</strong><span>{hostLabel}</span></span>
      </button>
    </div>
    {open ? <Dialog open onOpenChange={setOpen}><DialogContent className='live-details-dialog'>
      <DialogHeader><DialogTitle>{session.title}</DialogTitle><DialogDescription>{hostLabel}</DialogDescription></DialogHeader>
      <DialogBody>
        <LivePostDetails topic={topic} session={session} post={props.post} />
        {actions?.localPubkey === session.host_pubkey && session.status === 'Live' ? <LiveThumbnailField disabled={savingThumbnail} value={thumbnail?.custom ? thumbnail.data_url : null}
          onChange={(dataUrl) => {
            setSavingThumbnail(true);
            setThumbnailError(false);
            void actions.api.setLiveThumbnail({ topic, session_id: session.session_id, data_url: dataUrl, custom: true })
              .then(refreshThumbnail).catch(() => setThumbnailError(true)).finally(() => setSavingThumbnail(false));
          }} /> : null}
        {thumbnailError ? <p role='alert'>{t('thumbnail.failed')}</p> : null}
        <LiveCardDetails {...props} onCloseDetails={() => setOpen(false)} onStage={props.onStage ? () => { setOpen(false); props.onStage?.(); } : undefined} />
      </DialogBody>
    </DialogContent></Dialog> : null}
  </article>;
}

function LivePostDetails({ topic, session, post: provided }: { topic: string; session: LiveSessionView; post?: PostView }) {
  const { t } = useTranslation(['live', 'common']);
  const actions = useContext(LiveActionsContext);
  const { enabled, advisories, lookup } = useDesktopShellStore(useShallow((state) => ({
    enabled: state.adultContentEnabled,
    advisories: state.timelineContentAdvisories,
    lookup: state.timelineAdvisoryLookup,
  })));
  const [post, setPost] = useState<PostView | null | undefined>(provided);
  const [hostLabel, setHostLabel] = useState<string | null>(null);
  const selfLabeled = post && isAdultLabeledPost(post);
  const gated = post && !enabled && (selfLabeled || resolvePostAdvisory(post, advisories, lookup).advisory !== null);
  useEffect(() => {
    const api = actions?.api;
    if (!api) return;
    let current = true;
    if (api.getAuthorSocialView) void api.getAuthorSocialView(session.host_pubkey).then((author) => {
      if (current) setHostLabel(authorDisplayLabel(session.host_pubkey, author.display_name, author.name));
    }).catch(() => undefined);
    const loadPost = async () => {
      let found = provided;
      if (!found && session.start_post_id && api.listThread) {
        const page = await api.listThread(topic, session.start_post_id, null, 1);
        found = page.items.find((item) => item.object_id === session.start_post_id);
      }
      if (!found?.content?.trim() && isPublicChannelId(session.channel_id) && api.getPublicChannel) {
        const thread = await api.getPublicChannel(topic, session.channel_id!);
        found = thread?.first_post ?? found;
      }
      if (current) setPost(found ?? null);
    };
    void loadPost().catch(() => { if (current) setPost(null); });
    return () => { current = false; };
  }, [actions?.api, provided, session.host_pubkey, session.start_post_id, session.channel_id, topic]);
  return <div className='live-post-details'>
    {hostLabel ? <p>{t('details.broadcaster')}: {hostLabel}</p> : null}
    <h3>{t('details.post')}</h3>
    {gated ? <PostGatedContent objectId={post!.object_id} gatedBy={selfLabeled ? 'self_label' : 'advisory'} /> :
      <p className='post-copy-wrap'>{post === undefined && session.start_post_id ? t('live:loading') : post?.content || t('live:details.noPost')}</p>}
  </div>;
}

/** Live の一覧と、投稿の中の Live の表示が共用するカード(#1839)。 */
function LiveCardDetails({ topic, session, postId, onStage, onCloseDetails }: LiveCardProps) {
  const { t } = useTranslation(['common', 'live']);
  const actions = useContext(LiveActionsContext);
  const liveError = useDesktopShellStore((state) => state.liveError);
  if (!actions) return null;
  const id = session.session_id;
  const channelId = session.channel_id ?? null;
  const pending = actions.pendingBySessionId[id] === true;
  const isOwner = session.host_pubkey === actions.localPubkey;
  const ended = session.status === 'Ended';
  const startPostId = session.start_post_id ?? null;
  return (
    <article
      className='post-card'
      aria-busy={pending}
      data-live-session-id={id}
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
      {liveError ? <Notice tone='destructive'>{liveError}</Notice> : null}
      <LiveSessionMedia api={actions.api} topic={topic} session={session} isOwner={isOwner} />
      <div className='post-actions'>
        <LiveBroadcastStartButtons topic={topic} session={session} />
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
          onClick={() => { onCloseDetails?.(); actions.introduce({ session_id: id, topic_id: topic, channel_id: channelId }, session.title); }}
        >
          <Repeat2 className='size-4' aria-hidden='true' />
          {t('live:actions.introduce')}
        </Button>
        {startPostId && startPostId !== postId ? (
          <Button variant='secondary' type='button' onClick={() => { onCloseDetails?.(); actions.openPost(topic, startPostId); }}>
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

/** 投稿カードと拡大表示で共用する、自分の映像の配信開始。 */
export function LiveBroadcastStartButtons({ topic, session }: { topic: string; session: LiveSessionView }) {
  const { t } = useTranslation('live');
  const actions = useContext(LiveActionsContext);
  if (!actions || session.host_pubkey !== actions.localPubkey || session.status !== 'Live' || actions.broadcastingSessionId !== null) return null;
  return <>{(['camera', 'screen'] as const).map((source) => (
    <Button key={source} variant='secondary' type='button'
      onClick={() => actions.startBroadcast({ topic, sessionId: session.session_id, title: session.title }, source)}>
      {source === 'camera' ? <Camera className='size-4' aria-hidden='true' /> : <MonitorUp className='size-4' aria-hidden='true' />}
      {t(`media.start.${source}`)}
    </Button>
  ))}</>;
}
