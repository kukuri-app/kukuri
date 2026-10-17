import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Camera, Mic, MicOff, MonitorUp, Radio, Square, X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import type { DesktopApi, LiveVideoSource } from '@/lib/api';

import { captureErrorKind, captureVideo, encodeVideo } from './liveMedia';

type LiveBroadcastApi = Pick<DesktopApi, 'setLiveBroadcast' | 'pushLiveVideo' | 'stopLiveBroadcast'>;

type LiveBroadcastBarProps = {
  api: LiveBroadcastApi;
  topic: string;
  sessionId: string;
  title: string;
  onClose: () => void;
};

/**
 * 配信者の映像・mic の配信の操作(#1034、ADR 0067 §9)。shell の上部に 1 つだけ置き、取り込み・圧縮・送信を持つ
 * (#1839 AC-3)。カードや列を閉じても止めず、停止・閉じる・Live の終了・shell を離れるときだけ止める。
 */
export function LiveBroadcastBar({ api, topic, sessionId, title, onClose }: LiveBroadcastBarProps) {
  const { t } = useTranslation(['common', 'live']);
  const [source, setSource] = useState<LiveVideoSource>('camera');
  const [microphone, setMicrophone] = useState(true);
  const [phase, setPhase] = useState<'idle' | 'starting' | 'live'>('idle');
  const [notice, setNotice] = useState<string | null>(null);
  const previewRef = useRef<HTMLVideoElement>(null);
  const encodingRef = useRef<{ stop(): void } | null>(null);
  const aliveRef = useRef(true);
  const request = { topic, session_id: sessionId };

  const stopEncoding = () => {
    encodingRef.current?.stop();
    encodingRef.current = null;
  };
  const stop = async () => {
    stopEncoding();
    setPhase('idle');
    await api.stopLiveBroadcast(request).catch(() => undefined);
  };
  // bar を閉じた後に終わった操作は、配信を止めて捨てる(取り込みも mic も残さない)。
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
        push: (keyframe, ageUs, data) => api.pushLiveVideo(sessionId, keyframe, ageUs, data),
        onEnded: () => void stop(),
      });
      if (nextMicrophone && !keepMicrophone) {
        const opened = await api.setLiveBroadcast({ ...request, source: nextSource, microphone: true });
        if (abandoned()) return;
        setMicrophone(opened.microphone);
        if (!opened.microphone) setNotice(t('live:media.microphoneUnavailable'));
      }
      setPhase('live');
    } catch (error) {
      if (abandoned()) return;
      await stop();
      setNotice(t(`live:media.errors.${captureErrorKind(error, nextSource)}`));
    }
  };
  const toggleMicrophone = async () => {
    const next = !microphone;
    setMicrophone(next);
    if (phase !== 'live') return;
    const status = await api.setLiveBroadcast({ ...request, source, microphone: next });
    if (abandoned()) return;
    setMicrophone(status.microphone);
    setNotice(next && !status.microphone ? t('live:media.microphoneUnavailable') : null);
  };
  const selectSource = (next: LiveVideoSource) => {
    setSource(next);
    if (phase === 'live') void start(next, microphone);
  };
  // bar を閉じたら取り込みと配信を止める(再起動の後も自動では始めない)。
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
    <section
      className='shell-live-broadcast mx-[var(--space-lg)] mt-[var(--space-sm)] box-border flex h-[calc(var(--live-broadcast-reserve)-var(--space-sm))] items-center gap-2 overflow-x-auto rounded-[var(--radius-panel)] border border-[var(--border-subtle)] bg-[var(--surface-panel-solid)] px-4'
      aria-label={t('live:media.controls', { title })}
      aria-busy={busy}
    >
      <video
        ref={previewRef}
        muted
        playsInline
        hidden={phase === 'idle'}
        aria-label={t('live:media.preview')}
        className='aspect-video h-10 shrink-0 rounded-[var(--radius-input)] bg-[var(--surface-panel-muted)] object-contain'
      />
      <strong className='min-w-16 max-w-48 shrink-0 truncate'>{title}</strong>
      {phase === 'live' ? <span role='status' className='shrink-0'>{t('live:media.broadcasting')}</span> : null}
      {notice ? <span role='alert' className='error error-inline min-w-0 truncate'>{notice}</span> : null}
      <div className='ml-auto flex shrink-0 items-center gap-2' role='group' aria-label={t('live:media.source')}>
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
            {t(`live:media.sources.${each}`)}
          </Button>
        ))}
      </div>
      <IconButton
        variant={microphone ? 'primary' : 'ghost'}
        type='button'
        label={t(microphone ? 'live:media.disableMicrophone' : 'live:media.enableMicrophone')}
        aria-pressed={microphone}
        disabled={busy}
        onClick={() => void toggleMicrophone()}
      >
        {microphone ? <Mic className='size-4' aria-hidden='true' /> : <MicOff className='size-4' aria-hidden='true' />}
      </IconButton>
      {phase === 'idle' ? (
        <Button variant='primary' type='button' className='shrink-0' onClick={() => void start(source, microphone)}>
          <Radio className='size-4' aria-hidden='true' />
          {t('live:media.startBroadcast')}
        </Button>
      ) : (
        <Button variant='secondary' type='button' className='shrink-0' disabled={busy} onClick={() => void stop()}>
          <Square className='size-4' aria-hidden='true' />
          {t('live:media.stopBroadcast')}
        </Button>
      )}
      <IconButton variant='ghost' type='button' label={t('common:actions.close')} onClick={onClose}>
        <X className='size-4' aria-hidden='true' />
      </IconButton>
    </section>
  );
}
