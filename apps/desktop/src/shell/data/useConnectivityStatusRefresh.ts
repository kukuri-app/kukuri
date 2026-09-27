import { startTransition, useCallback, useRef } from 'react';

import type { CommunityNodeNodeStatus, DesktopApi } from '@/lib/api';
import { mergeCommunityNodeStatus } from '@/shell/presentation';
import { mergePulledSyncStatus } from '@/shell/slices/connectivity';
import { useDesktopShellStoreApi, type DesktopShellState, type DesktopShellStateValue } from '@/shell/store';

type Setter<K extends keyof DesktopShellState> = (
  value: DesktopShellStateValue<K>
) => void;

export function useConnectivityStatusRefresh(
  api: DesktopApi,
  setSyncStatus: Setter<'syncStatus'>,
  setCommunityNodeStatuses: Setter<'communityNodeStatuses'>
): () => Promise<CommunityNodeNodeStatus[] | null> {
  const store = useDesktopShellStoreApi();
  const inFlight = useRef<Promise<CommunityNodeNodeStatus[] | null> | null>(null);
  return useCallback(() => {
    if (inFlight.current) return inFlight.current;
    const request = (async () => {
      const baseline = store.getState().communityNodeStatuses;
      const syncBaseline = store.getState().syncStatus;
      store.getState().patchState({ syncStatusRead: {
        ...store.getState().syncStatusRead, refreshing: true,
      } });
      const [syncStatusResult, communityNodeStatusesResult] = await Promise.allSettled([
        api.getSyncStatus(),
        api.getCommunityNodeStatuses(),
      ]);
      startTransition(() => {
        // 読む間に届いた差分の event(#1221 R2-D)は、読み直した状態へ重ねて残す。
        const current = store.getState().syncStatus;
        const unchanged = current === syncBaseline;
        if (syncStatusResult.status === 'fulfilled') {
          setSyncStatus(mergePulledSyncStatus(syncStatusResult.value, syncBaseline, current));
        }
        const read = store.getState().syncStatusRead;
        store.getState().patchState({ syncStatusRead: {
          loaded: read.loaded || !unchanged || syncStatusResult.status === 'fulfilled',
          refreshing: false,
          error: unchanged && syncStatusResult.status === 'rejected',
        } });
        if (communityNodeStatusesResult.status === 'fulfilled') {
          setCommunityNodeStatuses((current) => {
            const baselineByUrl = new Map(baseline.map((status) => [status.base_url, status]));
            const currentByUrl = new Map(current.map((status) => [status.base_url, status]));
            const incoming = communityNodeStatusesResult.value;
            const next = incoming.map((status) => {
              const latest = currentByUrl.get(status.base_url);
              // この読込中に受諾/撤回/eventで更新されたNodeを古いsnapshotで巻き戻さない。
              return latest && latest !== baselineByUrl.get(status.base_url)
                ? latest : mergeCommunityNodeStatus(latest, status);
            });
            return [...next, ...current.filter((status) =>
              status !== baselineByUrl.get(status.base_url) &&
              !incoming.some((item) => item.base_url === status.base_url))];
          });
          store.getState().patchState({
            communityNodeStatusesLoaded: true, communityNodeStatusError: null,
          });
        } else if (store.getState().communityNodeStatuses === baseline) {
          store.getState().patchState({ communityNodeStatusError: 'status_unavailable' });
        }
      });
      return communityNodeStatusesResult.status === 'fulfilled'
        ? communityNodeStatusesResult.value
        : null;
    })();
    inFlight.current = request;
    void request.finally(() => { inFlight.current = null; });
    return request;
  }, [api, setCommunityNodeStatuses, setSyncStatus, store]);
}
