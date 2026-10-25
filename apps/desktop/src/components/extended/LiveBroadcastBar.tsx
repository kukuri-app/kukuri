import { useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { Camera, Mic, MicOff, MonitorUp, Square, X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Select } from '@/components/ui/select';

import type { LiveBroadcastController } from './liveBroadcast';

/**
 * 配信中の操作(#1034、#1839 AC-3)。shell の上部に、配信中(と開始の失敗の理由を示す間)だけ出す。取り込み・圧縮・送信は
 * `LiveBroadcastController` が持ち、この bar は状態を示して操作するだけ。
 */
export function LiveBroadcastBar({ controller }: { controller: LiveBroadcastController }) {
  const { t } = useTranslation(['common', 'live']);
  const state = useSyncExternalStore(controller.subscribe, controller.getState);
  if (!state) return null;
  const busy = state.phase === 'starting';
  const failed = state.phase === 'failed';
  return (
    <section
      className='shell-live-broadcast mx-[var(--space-lg)] mt-[var(--space-sm)] box-border flex h-[calc(var(--live-broadcast-reserve)-var(--space-sm))] items-center gap-2 overflow-x-auto rounded-[var(--radius-panel)] border border-[var(--border-subtle)] bg-[var(--surface-panel-solid)] px-4'
      aria-label={t('live:media.controls', { title: state.title })}
      aria-busy={busy}
    >
          {!failed && state.source === 'camera' && state.cameras.length > 0 ? (
            <Select className='min-w-32 max-w-48 shrink-0' aria-label={t('live:media.camera')} value={state.cameraId} disabled={busy}
              onChange={(event) => controller.setCamera(event.target.value)}>
              {!state.cameraId ? <option value=''>{t('live:media.defaultCamera')}</option> : null}
              {state.cameras.map((camera, index) => <option key={camera.deviceId} value={camera.deviceId}>
                {camera.label || t('live:media.cameraNumber', { number: index + 1 })}
              </option>)}
            </Select>
          ) : null}
      <div
        hidden={failed}
        aria-label={t('live:media.preview')}
        className='aspect-video h-10 shrink-0 overflow-hidden rounded-[var(--radius-input)] bg-[var(--surface-panel-muted)] [&>video]:size-full [&>video]:object-contain'
        ref={(node) => {
          if (node && controller.preview.parentElement !== node) node.append(controller.preview);
        }}
      />
      <strong className='min-w-16 max-w-48 shrink-0 truncate'>{state.title}</strong>
      {state.phase === 'live' ? <span role='status' className='shrink-0'>{t('live:media.broadcasting')}</span> : null}
      {state.notice ? <span role='alert' className='error error-inline min-w-0 truncate'>{t(`live:${state.notice}`)}</span> : null}
      {failed ? null : (
        <>
          <div className='ml-auto flex shrink-0 items-center gap-2' role='group' aria-label={t('live:media.source')}>
            {(['camera', 'screen'] as const).map((each) => (
              <Button
                key={each}
                variant={state.source === each ? 'primary' : 'secondary'}
                type='button'
                aria-pressed={state.source === each}
                disabled={busy}
                onClick={() => controller.setSource(each)}
              >
                {each === 'camera' ? <Camera className='size-4' aria-hidden='true' /> : <MonitorUp className='size-4' aria-hidden='true' />}
                {t(`live:media.sources.${each}`)}
              </Button>
            ))}
          </div>
          <IconButton
            variant={state.microphone ? 'primary' : 'ghost'}
            type='button'
            label={t(state.microphone ? 'live:media.disableMicrophone' : 'live:media.enableMicrophone')}
            aria-pressed={state.microphone}
            disabled={busy}
            onClick={() => void controller.toggleMicrophone()}
          >
            {state.microphone ? <Mic className='size-4' aria-hidden='true' /> : <MicOff className='size-4' aria-hidden='true' />}
          </IconButton>
        </>
      )}
      {failed ? (
        <IconButton variant='ghost' type='button' className='ml-auto' label={t('common:actions.close')} onClick={() => controller.stop()}>
          <X className='size-4' aria-hidden='true' />
        </IconButton>
      ) : (
        <Button variant='secondary' type='button' className='shrink-0' onClick={() => controller.stop()}>
          <Square className='size-4' aria-hidden='true' />
          {t('live:media.stopBroadcast')}
        </Button>
      )}
    </section>
  );
}
