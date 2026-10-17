import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { DesktopApi, LiveMediaRequest, LiveSessionView } from '@/lib/api';
import { IS_WEB_RUNTIME } from '@/lib/webRuntime';

import { createLivePlayer, parseLiveVideoBatch, type LiveWatchState } from './liveMedia';

type LiveMediaApi = Pick<DesktopApi, 'watchLiveMedia' | 'nextLiveVideo' | 'stopWatchingLiveMedia'>;

/// 空きが見つからない・映像の無い間・止まった後に、視聴を試し直す間隔(視聴中に親を失ったときは runtime が参加し直す)。
const RETRY_MS = 5_000;

type LiveSessionMediaProps = {
  api: LiveMediaApi;
  topic: string;
  session: LiveSessionView;
  isOwner: boolean;
};

/// 配信中の live session の映像(#1034、ADR 0067 §9)。参加した視聴者に映像を出す。配信の操作は shell の上部の
/// `LiveBroadcastBar`(#1839)。Web は扱わない。
export function LiveSessionMedia({ api, topic, session, isOwner }: LiveSessionMediaProps) {
  if (IS_WEB_RUNTIME || session.status !== 'Live' || isOwner || !session.joined_by_me) return null;
  return <LiveViewer api={api} request={{ topic, session_id: session.session_id }} />;
}

type MediaProps = { api: LiveMediaApi; request: LiveMediaRequest };

function LiveViewer({ api, request }: MediaProps) {
  const { t } = useTranslation('live');
  const [state, setState] = useState<LiveWatchState | 'connecting'>('connecting');
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const { topic, session_id: sessionId } = request;

  useEffect(() => {
    const target = { topic, session_id: sessionId };
    const player = createLivePlayer(canvasRef.current!);
    let stopped = false;
    let watching: Promise<void> = Promise.resolve();
    void (async () => {
      while (!stopped) {
        let ended: LiveWatchState = 'ended';
        try {
          watching = api.watchLiveMedia(target);
          await watching;
          for (;;) {
            const batch = parseLiveVideoBatch(await api.nextLiveVideo(target));
            if (stopped) return;
            if (batch.state !== 'watching') {
              ended = batch.state;
              break;
            }
            setState('watching');
            player.push(batch);
          }
        } catch {
          // 届かなくなった。少し待って試し直す。
        }
        if (stopped) return;
        player.reset();
        setState(ended);
        await new Promise((resolve) => window.setTimeout(resolve, RETRY_MS));
      }
    })();
    return () => {
      stopped = true;
      player.close();
      // 視聴の開始の途中なら、その後で止める(先に止めると開始が残り、音声が鳴り続ける)。
      void watching
        .catch(() => undefined)
        .then(() => api.stopWatchingLiveMedia(target))
        .catch(() => undefined);
    };
  }, [api, topic, sessionId]);

  return (
    <div className='flex flex-col gap-2'>
      <canvas
        ref={canvasRef}
        aria-label={t('media.video')}
        hidden={state !== 'watching'}
        className='aspect-video w-full rounded-[var(--radius-input)] bg-[var(--surface-panel-muted)] object-contain'
      />
      {state === 'watching' ? null : <p role='status'>{t(`media.states.${state}`)}</p>}
    </div>
  );
}
