import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type { RuntimeEvent } from '@/lib/api';
import { isTauriRuntime } from '@/lib/releaseReadiness';
import { IS_WEB_RUNTIME, listenWebRuntimeEvents } from '@/lib/webRuntime';

// 通信状態の差分(#1221 R2-D)。変わった部分だけを持つ。
export type SyncStatusDelta = Extract<RuntimeEvent, { type: 'sync_status_changed' }>;

export function useRuntimeEventBridge(
  onNotificationStatusChanged: () => void,
  onSyncStatusChanged: (delta: SyncStatusDelta) => void,
  onAdultMediaLabelEvicted: (hash: string | null) => void
): void {
  const notificationCallbackRef = useRef(onNotificationStatusChanged);
  const syncStatusCallbackRef = useRef(onSyncStatusChanged);
  const adultLabelCallbackRef = useRef(onAdultMediaLabelEvicted);

  useEffect(() => {
    notificationCallbackRef.current = onNotificationStatusChanged;
  }, [onNotificationStatusChanged]);

  useEffect(() => {
    syncStatusCallbackRef.current = onSyncStatusChanged;
  }, [onSyncStatusChanged]);

  useEffect(() => {
    adultLabelCallbackRef.current = onAdultMediaLabelEvicted;
  }, [onAdultMediaLabelEvicted]);

  useEffect(() => {
    const onEvent = (payload: RuntimeEvent | undefined) => {
      switch (payload?.type) {
        case 'notification_status_changed':
          notificationCallbackRef.current();
          break;
        case 'sync_status_changed':
          syncStatusCallbackRef.current(payload);
          break;
        case 'adult_media_label_evicted':
          adultLabelCallbackRef.current(payload.hash ?? null);
          break;
      }
    };
    if (IS_WEB_RUNTIME) {
      return listenWebRuntimeEvents(onEvent);
    }
    if (!isTauriRuntime()) {
      return;
    }

    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    void (async () => {
      const dispose = await listen<RuntimeEvent>('kukuri://runtime-event', (event) =>
        onEvent(event.payload)
      );
      if (cancelled) {
        dispose();
        return;
      }
      unlisten = dispose;
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}
