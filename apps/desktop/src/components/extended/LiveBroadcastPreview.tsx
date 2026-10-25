import { type ReactNode, useEffect, useRef, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';

import type { LiveBroadcastController } from './liveBroadcast';

/** 同じ取り込みを表示するだけで、カメラや配信の寿命はcontrollerが持つ。 */
export function LiveBroadcastPreview({ controller, sessionId, overlay }: {
  controller: LiveBroadcastController;
  sessionId: string;
  overlay?: ReactNode;
}) {
  const { t } = useTranslation('live');
  const state = useSyncExternalStore(controller.subscribe, controller.getState);
  const video = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const element = video.current!;
    element.srcObject = state?.sessionId === sessionId ? controller.preview.srcObject : null;
    if (element.srcObject) void element.play().catch(() => undefined);
    return () => { element.srcObject = null; };
  }, [controller, state, sessionId]);
  const broadcasting = state?.sessionId === sessionId && state.phase === 'live';
  return <div className='live-viewer-screen live-stage-placeholder'>
    <video ref={video} muted autoPlay playsInline aria-label={t('media.preview')} />
    {overlay}
    {broadcasting ? null : <p role='status'>{t('stage.broadcasting')}</p>}
  </div>;
}
