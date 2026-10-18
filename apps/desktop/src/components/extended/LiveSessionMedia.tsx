import { type ReactNode, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Volume2 } from 'lucide-react';

import type { DesktopApi, LiveMediaRequest, LiveSessionView } from '@/lib/api';
import { IS_WEB_RUNTIME } from '@/lib/webRuntime';

import { createLivePlayer, type LiveWatchState } from './liveMedia';
import { joinLiveWatch, setLiveWatchVolume } from './liveWatch';

type LiveMediaApi = Pick<DesktopApi, 'watchLiveMedia' | 'nextLiveVideo' | 'stopWatchingLiveMedia' | 'setLiveVolume'>;

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

type MediaProps = { api: LiveMediaApi; request: LiveMediaRequest; overlay?: ReactNode };

/// 映像と音量。`overlay` は映像の上に重ねる DOM で、映像の bytes・署名・時刻には触れない(#1841)。
export function LiveViewer({ api, request, overlay }: MediaProps) {
  const { t } = useTranslation('live');
  const [state, setState] = useState<LiveWatchState | 'connecting'>('connecting');
  const [volume, setVolume] = useState(1);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const { topic, session_id: sessionId } = request;

  // 同じ Live の表示は 1 つの視聴を共有し、この canvas はそこから映像を受けて描く(#1839 AC-4)。
  useEffect(() => {
    const player = createLivePlayer(canvasRef.current!);
    const leave = joinLiveWatch(api, { topic, session_id: sessionId }, {
      onBatch: (batch) => {
        setState('watching');
        player.push(batch);
      },
      onState: (next) => {
        player.reset();
        setState(next);
      },
      onVolume: setVolume,
    });
    return () => {
      leave();
      player.close();
    };
  }, [api, topic, sessionId]);

  return (
    <div className='flex flex-col gap-2'>
      <div className='live-viewer-screen'>
        <canvas
          ref={canvasRef}
          aria-label={t('media.video')}
          hidden={state !== 'watching'}
          className='aspect-video w-full rounded-[var(--radius-input)] bg-[var(--surface-panel-muted)] object-contain'
        />
        {overlay}
        {state === 'watching' ? null : <p role='status'>{t(`media.states.${state}`)}</p>}
      </div>
      <div className='flex items-center gap-2'>
        <Volume2 className='size-4 shrink-0' aria-hidden='true' />
        <input
          type='range'
          min={0}
          max={100}
          value={Math.round(volume * 100)}
          aria-label={t('media.volume')}
          className='w-full'
          onChange={(event) => setLiveWatchVolume(api, request, Number(event.target.value) / 100)}
        />
      </div>
    </div>
  );
}
