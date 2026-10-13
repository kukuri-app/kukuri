import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Camera, Mic, MicOff, MonitorUp, Radio, Square } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import type { DesktopApi, LiveMediaRequest, LiveSessionView, LiveVideoSource } from '@/lib/api';
import { IS_WEB_RUNTIME } from '@/lib/webRuntime';

import {
  captureErrorKind,
  captureVideo,
  createLivePlayer,
  encodeVideo,
  parseLiveVideoBatch,
  type LiveWatchState,
} from './liveMedia';

type LiveMediaApi = Pick<
  DesktopApi,
  | 'setLiveBroadcast'
  | 'pushLiveVideo'
  | 'stopLiveBroadcast'
  | 'watchLiveMedia'
  | 'nextLiveVideo'
  | 'stopWatchingLiveMedia'
>;

/// 満員・映像の無い間・止まった後に、視聴を試し直す間隔。
const RETRY_MS = 5_000;

type LiveSessionMediaProps = {
  api: LiveMediaApi;
  topic: string;
  session: LiveSessionView;
  isOwner: boolean;
};

/// 配信中の live session の映像(#1034、ADR 0067 §9)。主催者には配信の操作、参加した視聴者には映像を出す。Web は扱わない。
export function LiveSessionMedia({ api, topic, session, isOwner }: LiveSessionMediaProps) {
  if (IS_WEB_RUNTIME || session.status !== 'Live') return null;
  const request = { topic, session_id: session.session_id };
  if (isOwner) return <LiveBroadcastControls api={api} request={request} />;
  return session.joined_by_me ? <LiveViewer api={api} request={request} /> : null;
}

type MediaProps = { api: LiveMediaApi; request: LiveMediaRequest };

function LiveBroadcastControls({ api, request }: MediaProps) {
  const { t } = useTranslation('live');
  const [source, setSource] = useState<LiveVideoSource>('camera');
  const [microphone, setMicrophone] = useState(true);
  const [phase, setPhase] = useState<'idle' | 'starting' | 'live'>('idle');
  const [notice, setNotice] = useState<string | null>(null);
  const previewRef = useRef<HTMLVideoElement>(null);
  const encodingRef = useRef<{ stop(): void } | null>(null);
  const aliveRef = useRef(true);
  const { topic, session_id: sessionId } = request;

  const stopEncoding = () => {
    encodingRef.current?.stop();
    encodingRef.current = null;
  };
  const stop = async () => {
    stopEncoding();
    setPhase('idle');
    await api.stopLiveBroadcast(request).catch(() => undefined);
  };
  // 画面を離れた後に終わった操作は、配信を止めて捨てる(取り込みも mic も残さない)。
  const abandoned = () => {
    if (aliveRef.current) return false;
    void api.stopLiveBroadcast(request).catch(() => undefined);
    return true;
  };
  // mic は取り込みが始まってから開く(取り込みが拒否されたら mic も開かない)。配信中の source の切替では mic を閉じない。
  const start = async (nextSource: LiveVideoSource, nextMicrophone: boolean) => {
    const keepMicrophone = phase === 'live' && nextMicrophone;
    stopEncoding();
    setPhase('starting');
    setNotice(null);
    try {
      const status = await api.setLiveBroadcast({ ...request, source: nextSource, microphone: keepMicrophone });
      if (abandoned()) return;
      const stream = await captureVideo(nextSource, status.limits);
      if (abandoned()) {
        stream.getTracks().forEach((track) => track.stop());
        return;
      }
      encodingRef.current = encodeVideo({
        stream,
        limits: status.limits,
        preview: previewRef.current!,
        push: (keyframe, ageUs, data) => api.pushLiveVideo(request.session_id, keyframe, ageUs, data),
        onEnded: () => void stop(),
      });
      if (nextMicrophone && !keepMicrophone) {
        const opened = await api.setLiveBroadcast({ ...request, source: nextSource, microphone: true });
        if (abandoned()) return;
        setMicrophone(opened.microphone);
        if (!opened.microphone) setNotice(t('media.microphoneUnavailable'));
      }
      setPhase('live');
    } catch (error) {
      if (abandoned()) return;
      await stop();
      setNotice(t(`media.errors.${captureErrorKind(error, nextSource)}`));
    }
  };
  const toggleMicrophone = async () => {
    const next = !microphone;
    setMicrophone(next);
    if (phase !== 'live') return;
    const status = await api.setLiveBroadcast({ ...request, source, microphone: next });
    if (abandoned()) return;
    setMicrophone(status.microphone);
    setNotice(next && !status.microphone ? t('media.microphoneUnavailable') : null);
  };
  const selectSource = (next: LiveVideoSource) => {
    setSource(next);
    if (phase === 'live') void start(next, microphone);
  };
  // 画面を離れたら取り込みと配信を止める(再起動の後も自動では始めない)。
  useEffect(() => {
    const encoding = encodingRef;
    const alive = aliveRef;
    alive.current = true;
    return () => {
      alive.current = false;
      encoding.current?.stop();
      void api.stopLiveBroadcast({ topic, session_id: sessionId }).catch(() => undefined);
    };
  }, [api, topic, sessionId]);

  const busy = phase === 'starting';
  return (
    <div className='flex flex-col gap-2' aria-busy={busy}>
      <video
        ref={previewRef}
        muted
        playsInline
        hidden={phase === 'idle'}
        aria-label={t('media.preview')}
        className='aspect-video w-full max-w-sm rounded-[var(--radius-input)] bg-[var(--surface-panel-muted)] object-contain'
      />
      <div className='post-actions'>
        <div className='post-actions' role='group' aria-label={t('media.source')}>
          {(['camera', 'screen'] as const).map((each) => (
            <Button
              key={each}
              variant={source === each ? 'primary' : 'secondary'}
              type='button'
              aria-pressed={source === each}
              disabled={busy}
              onClick={() => selectSource(each)}
            >
              {each === 'camera' ? <Camera className='size-4' aria-hidden='true' /> : <MonitorUp className='size-4' aria-hidden='true' />}
              {t(`media.sources.${each}`)}
            </Button>
          ))}
        </div>
        <IconButton
          variant={microphone ? 'primary' : 'ghost'}
          type='button'
          label={t(microphone ? 'media.disableMicrophone' : 'media.enableMicrophone')}
          aria-pressed={microphone}
          disabled={busy}
          onClick={() => void toggleMicrophone()}
        >
          {microphone ? <Mic className='size-4' aria-hidden='true' /> : <MicOff className='size-4' aria-hidden='true' />}
        </IconButton>
        {phase === 'idle' ? (
          <Button variant='primary' type='button' onClick={() => void start(source, microphone)}>
            <Radio className='size-4' aria-hidden='true' />
            {t('media.startBroadcast')}
          </Button>
        ) : (
          <Button variant='secondary' type='button' disabled={busy} onClick={() => void stop()}>
            <Square className='size-4' aria-hidden='true' />
            {t('media.stopBroadcast')}
          </Button>
        )}
      </div>
      {phase === 'live' ? <p role='status'>{t('media.broadcasting')}</p> : null}
      {notice ? <p role='alert' className='error error-inline'>{notice}</p> : null}
    </div>
  );
}

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
