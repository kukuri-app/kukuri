import { invoke } from '@tauri-apps/api/core';

import { IS_WEB_RUNTIME, nextWebLiveVideo, pushWebLiveVideo } from '../../webRuntime';
import type { DesktopApi, LiveBroadcastStatus } from '../types';
import { invokeDesktop } from '../invoke/desktop';
import { command } from '../invoke/dispatch';
import { normalizeInvokeError } from '../invoke/error';

// #1034: live の配信と視聴(ADR 0067 §9)。映像の chunk は raw の body と応答で渡す(Web は web-runtime の関数。#1879)。
// runtimeApi.ts の大型ファイル ratchet を増やさないため別 module に置く。
export const liveMediaApi: Pick<
  DesktopApi,
  | 'setLiveBroadcast'
  | 'pushLiveVideo'
  | 'stopLiveBroadcast'
  | 'watchLiveMedia'
  | 'nextLiveVideo'
  | 'stopWatchingLiveMedia'
  | 'setLiveVolume'
> = {
  setLiveBroadcast: command('setLiveBroadcast', (request) =>
    invokeDesktop<LiveBroadcastStatus>('set_live_broadcast', { request })),
  pushLiveVideo: command('pushLiveVideo', async (sessionId, keyframe, ageMicros, data) => {
    try {
      if (IS_WEB_RUNTIME) return await pushWebLiveVideo(sessionId, keyframe, Math.max(0, ageMicros), data);
      await invoke('push_live_video', data, {
        headers: {
          'x-session-id': sessionId,
          'x-keyframe': keyframe ? '1' : '0',
          'x-age-us': String(Math.max(0, Math.round(ageMicros))),
        },
      });
    } catch (error) {
      throw normalizeInvokeError(error);
    }
  }),
  stopLiveBroadcast: command('stopLiveBroadcast', (request) =>
    invokeDesktop<void>('stop_live_broadcast', { request })),
  watchLiveMedia: command('watchLiveMedia', (request) =>
    invokeDesktop<void>('watch_live_media', { request })),
  nextLiveVideo: command('nextLiveVideo', (request) =>
    IS_WEB_RUNTIME ? nextWebLiveVideo(request) : invokeDesktop<ArrayBuffer>('next_live_video', { request })),
  stopWatchingLiveMedia: command('stopWatchingLiveMedia', (request) =>
    invokeDesktop<void>('stop_watching_live_media', { request })),
  setLiveVolume: command('setLiveVolume', (request) =>
    invokeDesktop<void>('set_live_volume', { request })),
};
