import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type { RuntimeEvent } from '@/lib/api';
import { isTauriRuntime } from '@/lib/releaseReadiness';

// 通信状態の差分(#1221 R2-D)。変わった部分だけを持つ。
export type SyncStatusDelta = Extract<RuntimeEvent, { type: 'sync_status_changed' }>;

export function useRuntimeEventBridge(
  onNotificationStatusChanged: () => void,
  onSyncStatusChanged: (delta: SyncStatusDelta) => void,
  onAdultMediaLabelEvicted: (hash: string | null) => void,
  // #1521 AC-1b: 自分を指す相手の follow の edge が届いた。`null` は知らせが溢れたとき。
  onAuthorRelationshipChanged: (pubkey: string | null) => void
): void {
  const notificationCallbackRef = useRef(onNotificationStatusChanged);
  const syncStatusCallbackRef = useRef(onSyncStatusChanged);
  const adultLabelCallbackRef = useRef(onAdultMediaLabelEvicted);
  const relationshipCallbackRef = useRef(onAuthorRelationshipChanged);

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
    relationshipCallbackRef.current = onAuthorRelationshipChanged;
  }, [onAuthorRelationshipChanged]);

  useEffect(() => {
    if (!isTauriRuntime()) {
      return;
    }

    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    void (async () => {
      const dispose = await listen<RuntimeEvent>(
        'kukuri://runtime-event',
        (event) => {
          switch (event.payload?.type) {
            case 'notification_status_changed':
              notificationCallbackRef.current();
              break;
            case 'sync_status_changed':
              syncStatusCallbackRef.current(event.payload);
              break;
            case 'adult_media_label_evicted':
              adultLabelCallbackRef.current(event.payload.hash ?? null);
              break;
            case 'author_relationship_changed':
              relationshipCallbackRef.current(event.payload.pubkey ?? null);
              break;
          }
        }
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
