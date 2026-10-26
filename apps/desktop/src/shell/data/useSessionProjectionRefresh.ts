import { useEffect, type MutableRefObject } from 'react';
import { showsThreadLives } from '@/shell/slices/publicThreads';
import { primarySectionForColumn } from '@/shell/slices/workspace';
import type { useDesktopShellStoreApi } from '@/shell/store';

type SectionLoader = (topic: string, channel: string | null) => Promise<void>;

/** projectionの変更通知に追従する表示中session一覧。取得そのものをtimerで繰り返さない。 */
export function useSessionProjectionRefresh(
  storeApi: ReturnType<typeof useDesktopShellStoreApi>,
  loadLiveSection: SectionLoader,
  loadGameSection: SectionLoader,
  visibleColumnIdsRef?: MutableRefObject<string[]>,
) {
  useEffect(() => {
    let disposed = false;
    let running = false;
    // 表示中の一覧へ取り直した projection の版(`last_sync_ts`)。変更の知らせの時点で取り直せなくても(live・game の列が
    // 見えていない等)、次の store の更新で取り直す(#1815)。
    let loaded: number | null | undefined;
    const refreshSessions = async () => {
      if (running) return;
      running = true;
      while (!disposed) {
        const state = storeApi.getState();
        const sync = state.syncStatus.last_sync_ts;
        if (sync === loaded) break;
        const visible = new Set(visibleColumnIdsRef?.current ?? [state.workspaceState.activeColumnId]);
        const scopes = new Set<string>();
        const tasks: Promise<void>[] = [];
        for (const column of state.workspaceState.columns) {
          if (!visible.has(column.id) || !column.scope) continue;
          // 公開スレッドの詳細の列は、上部の配信中の Live のためにそのスレッドの Live を読む(#1840 AC-4)。
          const section = showsThreadLives(column)
            ? 'live'
            : primarySectionForColumn(column);
          if (section !== 'live' && section !== 'game') continue;
          const key = JSON.stringify([section, column.scope]);
          if (scopes.has(key)) continue;
          scopes.add(key);
          tasks.push(section === 'live'
            ? loadLiveSection(column.scope.topicId, column.scope.channelId ?? null)
            : loadGameSection(column.scope.topicId, column.scope.channelId ?? null));
        }
        if (tasks.length === 0) break;
        await Promise.allSettled(tasks);
        loaded = sync;
      }
      running = false;
    };
    const unsubscribe = storeApi.subscribe((state) => {
      // projection/candidateの変更でだけ再取得する。manifest取得のtimer retryではない。
      if (state.syncStatus.last_sync_ts !== loaded) void refreshSessions();
    });
    return () => { disposed = true; unsubscribe(); };
  }, [loadGameSection, loadLiveSection, storeApi, visibleColumnIdsRef]);
}
