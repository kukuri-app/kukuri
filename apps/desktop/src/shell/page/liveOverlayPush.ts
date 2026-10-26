import { useEffect, useRef, useState } from 'react';
import type { DesktopApi, LiveOverlayPushView, PostView } from '@/lib/api';
import { isPublicChannelId } from '@/lib/publicChannels';
import { subscribeLiveOverlayPush } from '@/shell/data/useRuntimeEventBridge';
import { isHiddenAuthor } from '@/shell/slices/publicThreads';
import { useDesktopShellStore, useDesktopShellStoreApi } from '@/shell/store';
import type { LiveOverlaySource } from './liveOverlay';

export type OverlayPush = { key: string; event: LiveOverlayPushView; count: number; until: number; post: PostView | null; resolving: boolean };
type Notice = 'merged' | 'replaced' | 'busy' | null;
const EMPTY = { items: [] as OverlayPush[], notice: null as Notice, context: '' };

/** runtimeで検証・重複除去した押下を、6つの表示枠に合流する。履歴や待ちqueueは持たない。 */
export function useLiveOverlayPushes(api: DesktopApi | undefined, source: LiveOverlaySource, active: boolean, hidden?: (post: PostView) => boolean) {
  const [snapshot, setSnapshot] = useState(EMPTY);
  const store = useDesktopShellStoreApi();
  const hiddenRef = useRef(hidden);
  const refresh = useRef(() => {});
  const purge = useRef(() => {});
  const lastSync = useDesktopShellStore((state) => state.syncStatus.last_sync_ts);
  useEffect(() => { hiddenRef.current = hidden; purge.current(); }, [hidden]);
  useEffect(() => { refresh.current(); }, [lastSync]);
  const { topic, channelId } = source.conversation;
  const context = JSON.stringify([source.live.topic_id, source.live.session_id, topic, channelId]);
  useEffect(() => {
    if (!api || !active || !isPublicChannelId(channelId)) return;
    let current = true;
    let frame = 0;
    let timer = 0;
    let notice: Notice = null;
    let noticeUntil = 0;
    const openedAt = Date.now();
    const slots = new Map<string, OverlayPush>();
    const publish = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        if (current) setSnapshot({ items: [...slots.values()].map((slot) => ({ ...slot })), notice, context });
      });
    };
    const expire = () => {
      clearTimeout(timer);
      const now = Date.now();
      for (const [key, slot] of slots) if (slot.until <= now) slots.delete(key);
      if (noticeUntil <= now) notice = null;
      const next = Math.min(...[...slots.values()].map((slot) => slot.until), notice ? noticeUntil : Infinity);
      if (Number.isFinite(next)) timer = window.setTimeout(() => { expire(); publish(); }, Math.max(1, next - now));
    };
    const showNotice = (value: Notice) => { notice = value; noticeUntil = Date.now() + 5000; };
    const resolve = (slot: OverlayPush) => {
      if (slot.resolving) return;
      slot.resolving = true;
      const requestedId = slot.event.event_id;
      void api.resolveLiveOverlayPush(requestedId).then((post) => {
        if (!current || slots.get(slot.key) !== slot) return;
        slot.resolving = false;
        if (!post && requestedId !== slot.event.event_id) { resolve(slot); return; }
        if (!post || post.withdrawal || hiddenRef.current?.(post)) slots.delete(slot.key);
        else slot.post = post;
        expire();
        publish();
      }, () => {
        if (current && slots.get(slot.key) === slot) { slots.delete(slot.key); showNotice('busy'); publish(); expire(); }
      });
    };
    refresh.current = () => { for (const slot of slots.values()) resolve(slot); };
    purge.current = () => {
      let changed = false;
      for (const [key, slot] of slots) if (slot.post && hiddenRef.current?.(slot.post)) { slots.delete(key); changed = true; }
      if (changed) { expire(); publish(); }
    };
    const unsubscribe = subscribeLiveOverlayPush((event) => {
      expire();
      if (!event) { showNotice('busy'); expire(); publish(); return; }
      if (event.target.topic !== topic || event.target.channel_id !== channelId || event.received_at < openedAt || event.sent_at + 10000 <= Date.now()) return;
      const state = store.getState();
      if (isHiddenAuthor(state.knownAuthorsByPubkey[event.sender_pubkey]) || state.authorTrustGates[event.sender_pubkey]?.hidden) return;
      const key = `${event.target.object_id}:${event.target.attachment_index}`;
      const previous = slots.get(key);
      if (previous) {
        previous.count += 1; previous.until = Date.now() + 5000; previous.event = event;
        showNotice('merged');
      } else {
        if (slots.size === 6) {
          const oldest = [...slots.values()].reduce((left, right) => left.until <= right.until ? left : right);
          slots.delete(oldest.key); showNotice('replaced');
        }
        const slot = { key, event, count: 1, until: Date.now() + 5000, post: null, resolving: false };
        slots.set(key, slot); resolve(slot);
      }
      expire(); publish();
    });
    return () => {
      current = false; unsubscribe(); clearTimeout(timer); cancelAnimationFrame(frame);
      slots.clear(); refresh.current = () => {}; purge.current = () => {}; setSnapshot(EMPTY);
    };
  }, [api, active, topic, channelId, context, store]);
  return active && snapshot.context === context ? snapshot : EMPTY;
}
