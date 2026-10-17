import { type RefObject, useEffect, useId, useRef, useState } from 'react';
import type { DesktopApi, TimelineScope } from '@/lib/api';
export type SessionDisplayContext = { api: DesktopApi; topic: string; scope: TimelineScope };

/** `onDisplayed` は表示の要求が読み終えた(手元の行が新しくなりえた)ときに呼ぶ。 */
export function useSessionDisplay<T extends HTMLElement>({ context, sessionId, kind, replicaId = '', target, onDisplayed }: {
  context?: SessionDisplayContext; sessionId: string; kind: string; replicaId?: string; target?: RefObject<T | null>;
  onDisplayed?: () => void;
}) {
  const ownElement = useRef<T>(null);
  const displayed = useRef(onDisplayed);
  useEffect(() => { displayed.current = onDisplayed; });
  const element = target ?? ownElement;
  const observerId = useId();
  const generation = useRef(0);
  const [failed, setFailed] = useState(false);
  const retry = useRef<() => void>(() => {});
  const api = context?.api;
  const topic = context?.topic;
  const scopeJson = JSON.stringify(context?.scope);
  useEffect(() => {
    if (!api || !topic || !element.current || typeof IntersectionObserver === 'undefined') return;
    const scope = JSON.parse(scopeJson) as TimelineScope;
    const observerKey = `${observerId}:${++generation.current}`;
    let intersecting = false;
    let visible = false;
    let disposed = false;
    // 同一observerの登録と解除は順序を保つ。遅い登録がunmount後に残ることを防ぐ。
    let running = false;
    let queued: { visible: boolean; retry: boolean } | null = null;
    const send = (next: boolean, manualRetry = false) => {
      queued = { visible: next, retry: manualRetry };
      if (running) return;
      running = true;
      void (async () => {
        while (queued) {
          const request = queued;
          queued = null;
          try {
            await api.setSessionDisplay({ topic, scope, replica_id: replicaId, session_id: sessionId,
              kind, observer: observerKey, visible: request.visible && !disposed, retry: request.retry });
            if (request.visible && !disposed) displayed.current?.();
          } catch { if (!disposed) setFailed(true); }
        }
        running = false;
      })();
    };
    const update = () => {
      const next = intersecting && document.visibilityState !== 'hidden';
      if (next === visible) return;
      visible = next;
      send(next);
    };
    retry.current = () => { if (visible) { setFailed(false); send(true, true); } };
    const observer = new IntersectionObserver(([entry]) => {
      intersecting = entry.isIntersecting && entry.intersectionRatio > 0;
      update();
    }, { threshold: 0 });
    observer.observe(element.current);
    document.addEventListener('visibilitychange', update);
    return () => {
      disposed = true;
      observer.disconnect();
      document.removeEventListener('visibilitychange', update);
      send(false);
    };
  }, [api, topic, scopeJson, sessionId, kind, replicaId, observerId, element]);
  return { element, failed, retry };
}
