import { useContext, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { PushLiveOverlayRequest } from '@/lib/api';
import { Button } from '@/components/ui/button';
import { isHiddenAuthor } from '@/shell/slices/publicThreads';
import { timelineStorageKeyForChannel, useDesktopShellStore } from '@/shell/store';
import { LiveActionsContext, type LiveActions } from './liveActions';

export function LiveOverlayPushButton({ target }: { target: PushLiveOverlayRequest }) {
  const actions = useContext(LiveActionsContext);
  return actions ? <PushButton target={target} actions={actions} /> : null;
}

function PushButton({ target, actions }: { target: PushLiveOverlayRequest; actions: LiveActions }) {
  const { t } = useTranslation('live');
  const key = timelineStorageKeyForChannel(target.topic, target.channel_id);
  const active = useDesktopShellStore((state) => {
    const lives = state.liveSessionsByScopeKey[key];
    if (!lives) return undefined;
    return lives.some((live) => live.status === 'Live' && !isHiddenAuthor(state.knownAuthorsByPubkey[live.host_pubkey]) && !state.authorTrustGates[live.host_pubkey]?.hidden);
  });
  const [result, setResult] = useState<'sending' | 'accepted' | 'failed' | null>(null);
  const description = useId();
  return <span className='media-push-control'>
    <Button type='button' variant='secondary' size='sm' disabled={!active}
      aria-label={t('push.target', { index: target.attachment_index + 1 })}
      aria-describedby={!active ? description : undefined}
      title={active ? t('push.delivery') : t(active === undefined ? 'push.loading' : 'push.noLive')}
      onClick={(event) => {
        event.stopPropagation();
        setResult('sending');
        void actions.api.pushLiveOverlay(target).then(() => setResult('accepted'), () => setResult('failed'));
      }}>Push</Button>
    {!active ? <span id={description}>{t(active === undefined ? 'push.loading' : 'push.noLive')}</span> : null}
    {result ? <span role='status'>{t(`push.${result}`)}</span> : null}
  </span>;
}
