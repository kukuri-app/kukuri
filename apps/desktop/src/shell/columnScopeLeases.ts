import { useEffect, useRef, useState } from 'react';

import type { DesktopApi, ScopeDisplayTarget } from '@/lib/api';
import { normalizeInvokeError } from '@/lib/api/invoke/error';
import type { ColumnState } from '@/shell/slices/workspace';

// #1221 R2-C: 購読する scope は account 全体で 64 件まで。超えた操作は backend がこの code で拒否する。
export const SCOPE_LIMIT_REACHED = 'SCOPE_LIMIT_REACHED';

// 列の追加を取り消したか、参加(live・Dome hosting・private channel)を拒否したか。
export type ScopeLimitKind = 'column' | 'participation';

const listeners = new Set<(kind: ScopeLimitKind) => void>();

export function isScopeLimitError(error: unknown): boolean {
  return normalizeInvokeError(error).code === SCOPE_LIMIT_REACHED;
}

export function announceScopeLimit(kind: ScopeLimitKind) {
  for (const listener of listeners) listener(kind);
}

// 参加の操作が上限で失敗したら、列の上限と同じ画面で説明する。エラーはそのまま呼び出し元へ返す。
export async function explainScopeLimit<T>(operation: Promise<T>): Promise<T> {
  try {
    return await operation;
  } catch (error) {
    if (isScopeLimitError(error)) announceScopeLimit('participation');
    throw error;
  }
}

export function useScopeLimitNotice() {
  const [kind, setKind] = useState<ScopeLimitKind | null>(null);
  useEffect(() => {
    listeners.add(setKind);
    return () => {
      listeners.delete(setKind);
    };
  }, []);
  return [kind, setKind] as const;
}

// 列が購読する対象。timeline 系は列の scope、profile は表示中の author、conversation は DM の相手。
export function columnScopeTarget(column: ColumnState): ScopeDisplayTarget | null {
  switch (column.kind) {
    case 'timeline':
    case 'thread':
    case 'stream':
    case 'game':
    case 'metaverse':
      return column.scope
        ? {
            kind: 'timeline',
            topic: column.scope.topicId,
            scope: column.scope.channelId
              ? { kind: 'channel', channel_id: column.scope.channelId }
              : { kind: 'public' },
          }
        : null;
    case 'profile':
    case 'conversation':
      return column.entityId ? { kind: 'author', pubkey: column.entityId } : null;
    default:
      return null;
  }
}

// 開いている列の集合の差分だけを登録・解除する(observer は列 id)。ウィンドウの可視性では解除しない。
// 上限で登録できなかった列は `onLimitReached` へ渡す。
export function useColumnScopeLeases(
  api: DesktopApi,
  columns: ColumnState[],
  onLimitReached: (columnId: string) => void
) {
  const registered = useRef(new Map<string, string>());
  const queue = useRef<Promise<void>>(Promise.resolve());
  const limitReached = useRef(onLimitReached);
  useEffect(() => {
    limitReached.current = onLimitReached;
  });
  const targets = JSON.stringify(
    columns.flatMap((column) => {
      const target = columnScopeTarget(column);
      return target ? [[column.id, target] as const] : [];
    })
  );
  useEffect(() => {
    const next = new Map(
      (JSON.parse(targets) as [string, ScopeDisplayTarget][]).map(([id, target]) => [
        id,
        JSON.stringify(target),
      ])
    );
    // 同じ observer の登録と解除の順序を保つ。
    const send = (observer: string, target: string, visible: boolean) => {
      queue.current = queue.current.then(async () => {
        try {
          await api.setScopeDisplay({
            observer,
            target: JSON.parse(target) as ScopeDisplayTarget,
            visible,
          });
        } catch (error) {
          if (visible && isScopeLimitError(error) && registered.current.get(observer) === target) {
            // 置き換えの拒否では前の key が残るので、列の holder を外してから列を取り消す。
            registered.current.delete(observer);
            send(observer, target, false);
            limitReached.current(observer);
          }
        }
      });
    };
    for (const [observer, target] of registered.current) {
      if (!next.has(observer)) {
        registered.current.delete(observer);
        send(observer, target, false);
      }
    }
    for (const [observer, target] of next) {
      if (registered.current.get(observer) === target) continue;
      registered.current.set(observer, target);
      send(observer, target, true);
    }
  }, [api, targets]);
  useEffect(
    () => () => {
      for (const [observer, target] of registered.current) {
        queue.current = queue.current.then(() =>
          api
            .setScopeDisplay({
              observer,
              target: JSON.parse(target) as ScopeDisplayTarget,
              visible: false,
            })
            .catch(() => undefined)
        );
      }
      registered.current.clear();
    },
    [api]
  );
}
