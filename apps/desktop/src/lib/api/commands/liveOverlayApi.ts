import type { DesktopApi, PostView, ResolveLiveOverlayPushRequest } from '../types';
import { invokeDesktop } from '../invoke/desktop';
import { command } from '../invoke/dispatch';

export const liveOverlayApi: Pick<DesktopApi, 'pushLiveOverlay' | 'resolveLiveOverlayPush'> = {
  pushLiveOverlay: command('pushLiveOverlay', (request) => invokeDesktop<void>('push_live_overlay', { request })),
  resolveLiveOverlayPush: command('resolveLiveOverlayPush', (eventId) => invokeDesktop<PostView | null>('resolve_live_overlay_push', {
    request: { event_id: eventId } satisfies ResolveLiveOverlayPushRequest,
  })),
};
