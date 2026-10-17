import type { DesktopApi, LiveMediaRequest } from '@/lib/api';

import { parseLiveVideoBatch, type LiveVideoBatch, type LiveWatchState } from './liveMedia';

type LiveWatchApi = Pick<DesktopApi, 'watchLiveMedia' | 'nextLiveVideo' | 'stopWatchingLiveMedia' | 'setLiveVolume'>;

/// 1 つの表示(canvas)が受けるもの。
export type LiveWatchListener = {
  onBatch: (batch: LiveVideoBatch) => void;
  onState: (state: LiveWatchState) => void;
  onVolume: (volume: number) => void;
};

/// 空きが見つからない・映像の無い間・止まった後に、視聴を試し直す間隔(視聴中に親を失ったときは runtime が参加し直す)。
const RETRY_MS = 5_000;
/// 端末に残す Live ごとの音量の数(古いものから消す)。
export const LIVE_VOLUME_LIMIT = 64;
const VOLUME_KEY = 'kukuri.desktop.live-volume';

type Watching = { listeners: Set<LiveWatchListener>; volume: number; stop: () => void };
const watchingByApi = new WeakMap<object, Map<string, Watching>>();

/// #1839 AC-4: 同じ Live の表示は 1 つの視聴(購読・音声の再生・映像の受け取り)を共有する。最初の表示で始め、最後の表示が
/// 抜けたら止める。映像は表示ごとの canvas へ同じものを配る。
export function joinLiveWatch(api: LiveWatchApi, request: LiveMediaRequest, listener: LiveWatchListener) {
  let byKey = watchingByApi.get(api);
  if (!byKey) watchingByApi.set(api, (byKey = new Map()));
  const key = `${request.topic}\n${request.session_id}`;
  const watching = byKey.get(key) ?? startWatching(api, request);
  byKey.set(key, watching);
  watching.listeners.add(listener);
  listener.onVolume(watching.volume);
  return () => {
    watching.listeners.delete(listener);
    if (watching.listeners.size > 0) return;
    byKey.delete(key);
    watching.stop();
  };
}

/// この Live の音量を変える(端末に残し、視聴中なら runtime へ渡し、同じ Live の表示へ知らせる)。
export function setLiveWatchVolume(api: LiveWatchApi, request: LiveMediaRequest, volume: number) {
  saveVolume(request.session_id, volume);
  const watching = watchingByApi.get(api)?.get(`${request.topic}\n${request.session_id}`);
  if (!watching) return;
  watching.volume = volume;
  watching.listeners.forEach((listener) => listener.onVolume(volume));
  void api.setLiveVolume({ session_id: request.session_id, volume }).catch(() => undefined);
}

function startWatching(api: LiveWatchApi, request: LiveMediaRequest): Watching {
  let stopped = false;
  let watchingCall: Promise<void> = Promise.resolve();
  const watching: Watching = {
    listeners: new Set(),
    volume: loadVolumes()[request.session_id] ?? 1,
    stop: () => {
      stopped = true;
      // 視聴の開始の途中なら、その後で止める(先に止めると開始が残り、音声が鳴り続ける)。
      void watchingCall
        .catch(() => undefined)
        .then(() => api.stopWatchingLiveMedia(request))
        .catch(() => undefined);
    },
  };
  void (async () => {
    while (!stopped) {
      let ended: LiveWatchState = 'ended';
      try {
        watchingCall = api.watchLiveMedia(request);
        await watchingCall;
        await api.setLiveVolume({ session_id: request.session_id, volume: watching.volume }).catch(() => undefined);
        for (;;) {
          const batch = parseLiveVideoBatch(await api.nextLiveVideo(request));
          if (stopped) return;
          if (batch.state !== 'watching') {
            ended = batch.state;
            break;
          }
          watching.listeners.forEach((listener) => listener.onBatch(batch));
        }
      } catch {
        // 届かなくなった。少し待って試し直す。
      }
      if (stopped) return;
      watching.listeners.forEach((listener) => listener.onState(ended));
      await new Promise((resolve) => window.setTimeout(resolve, RETRY_MS));
    }
  })();
  return watching;
}

function loadVolumes(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(VOLUME_KEY) ?? '{}') as Record<string, number>;
  } catch {
    return {};
  }
}

/// 最近変えた順に `LIVE_VOLUME_LIMIT` 件だけ残す。
function saveVolume(sessionId: string, volume: number) {
  const volumes = loadVolumes();
  delete volumes[sessionId];
  volumes[sessionId] = volume;
  const kept = Object.entries(volumes).slice(-LIVE_VOLUME_LIMIT);
  try {
    localStorage.setItem(VOLUME_KEY, JSON.stringify(Object.fromEntries(kept)));
  } catch {
    // 保存できなくても、今の視聴の音量は変わる。
  }
}
