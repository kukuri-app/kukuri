import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type { DesktopApi, NotificationView } from '@/lib/api';
import { isTauriRuntime } from '@/lib/releaseReadiness';
import { parseOsNotificationActivationLink } from '@/lib/osNotificationActivationLink';

const CONSUMED_INITIAL_URI_KEY = 'kukuri.os-notification.consumed-initial-uri';

function consumeInitialUri(url: string, initial: boolean): boolean {
  try {
    // getCurrent() retains the last URL across WebView reloads/account switches.
    // Live events always remain actionable, including a repeated click.
    if (initial && sessionStorage.getItem(CONSUMED_INITIAL_URI_KEY) === url) return false;
    sessionStorage.setItem(CONSUMED_INITIAL_URI_KEY, url);
  } catch {
    // Storage failure must not break the active notification click path.
  }
  return true;
}

type ActivationPayload = {
  notification_id: string;
};

/**
 * Resolves native events and protocol activations by ID in the current account,
 * independently of the displayed inbox page, through the existing handler.
 *
 * Rust or the single-instance plugin already focuses the window; resolve the
 * id through the runtime's recipient/access checks and reuse `handleOpenNotification`.
 */
export function useOsNotificationActivation(
  api: Pick<DesktopApi, 'getNotification'>,
  account: string | null,
  onActivate: (notification: NotificationView) => void
): void {
  const onActivateRef = useRef(onActivate);
  useEffect(() => { onActivateRef.current = onActivate; }, [onActivate]);

  useEffect(() => {
    if (!isTauriRuntime() || !account) {
      return;
    }

    let unlisten: UnlistenFn | undefined;
    let unlistenUrl: UnlistenFn | undefined;
    let cancelled = false;
    let generation = 0;

    const activate = (notificationId: string | null) => {
      if (cancelled || !notificationId) return;
      const current = ++generation;
      void api.getNotification(notificationId).then((notification) => {
        if (!cancelled && current === generation && notification) onActivateRef.current(notification);
      }).catch(() => undefined);
    };
    const activateUrl = (urls: string[], initial: boolean) => {
      if (cancelled) return;
      // The most recent recognized notification URL wins, just like native clicks.
      for (const url of [...urls].reverse()) {
        const id = parseOsNotificationActivationLink(url);
        if (id !== null) {
          if (consumeInitialUri(url, initial)) activate(id);
          return;
        }
      }
    };

    void (async () => {
      const dispose = await listen<ActivationPayload>('os-notification://activated', (event) => {
        activate(event.payload?.notification_id ?? null);
      });
      if (cancelled) {
        dispose();
        return;
      }
      unlisten = dispose;
    })();

    void import('@tauri-apps/plugin-deep-link').then(async ({ getCurrent, onOpenUrl }) => {
      if (cancelled) return;
      let liveUrlReceived = false;
      const dispose = await onOpenUrl((urls) => {
        liveUrlReceived = true;
        activateUrl(urls, false);
      });
      if (cancelled) {
        dispose();
        return;
      }
      unlistenUrl = dispose;
      // Subscribe before reading the launch URL; a newer live event supersedes it.
      const urls = await getCurrent();
      if (!liveUrlReceived && generation === 0) activateUrl(urls ?? [], true);
    }).catch(() => undefined);

    return () => {
      cancelled = true;
      unlisten?.();
      unlistenUrl?.();
    };
  }, [api, account]);
}
