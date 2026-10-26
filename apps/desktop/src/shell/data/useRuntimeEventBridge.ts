import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type { LiveOverlayPushView, ReactionReceivedView, RuntimeEvent } from '@/lib/api';
import { isTauriRuntime } from '@/lib/releaseReadiness';
import { IS_WEB_RUNTIME, listenWebRuntimeEvents } from '@/lib/webRuntime';

// 通信状態の差分(#1221 R2-D)。変わった部分だけを持つ。
export type SyncStatusDelta = Extract<RuntimeEvent, { type: 'sync_status_changed' }>;

// #1841: 手元へ反映したリアクションの新しい版。Live の重ねる表示だけが購読し、保存しない。
const reactionListeners = new Set<(reaction: ReactionReceivedView) => void>();
const overlayPushListeners = new Set<(push: LiveOverlayPushView | null) => void>();

export function subscribeLiveOverlayPush(listener: (push: LiveOverlayPushView | null) => void) {
  overlayPushListeners.add(listener);
  return () => void overlayPushListeners.delete(listener);
}

export function announceLiveOverlayPush(push: LiveOverlayPushView | null) {
  overlayPushListeners.forEach((listener) => listener(push));
}

export function subscribeReactionReceived(listener: (reaction: ReactionReceivedView) => void) {
  reactionListeners.add(listener);
  return () => void reactionListeners.delete(listener);
}

export function announceReactionReceived(reaction: ReactionReceivedView) {
  reactionListeners.forEach((listener) => listener(reaction));
}

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
        case 'author_relationship_changed':
          relationshipCallbackRef.current(payload.pubkey ?? null);
          break;
        case 'reaction_received':
          announceReactionReceived(payload.reaction);
          break;
        case 'live_overlay_push_received':
          announceLiveOverlayPush(payload.push ?? null);
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
