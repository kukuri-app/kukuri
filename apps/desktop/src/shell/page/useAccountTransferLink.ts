import { useEffect, useRef } from 'react';

import { isTauriRuntime } from '@/lib/releaseReadiness';

export const TRANSFER_LINK_PREFIX = 'kukuri://transfer#';
// getCurrent() は WebView の再読込・account 切替の後も起動時のリンクを返す。招待の秘密を含むので
// browser の storage へ置かず、この process の中だけで処理済みを覚える。
const consumedInitialLinks = new Set<string>();

/**
 * #1211: OS のリンク起動（`kukuri://transfer#...`）で受けた移行用のリンクを渡す。接続はしない。
 */
export function useAccountTransferLink(onLink: (link: string) => void): void {
  const onLinkRef = useRef(onLink);
  useEffect(() => { onLinkRef.current = onLink; }, [onLink]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    const receive = (urls: string[] | null, initial: boolean) => {
      const link = [...(urls ?? [])].reverse().find((url) => url.startsWith(TRANSFER_LINK_PREFIX));
      if (cancelled || !link) return;
      if (initial) {
        if (consumedInitialLinks.has(link)) return;
        consumedInitialLinks.add(link);
      }
      onLinkRef.current(link);
    };
    void import('@tauri-apps/plugin-deep-link').then(async ({ getCurrent, onOpenUrl }) => {
      if (cancelled) return;
      const dispose = await onOpenUrl((urls) => receive(urls, false));
      if (cancelled) { dispose(); return; }
      unlisten = dispose;
      receive(await getCurrent(), true);
    }).catch(() => undefined);
    return () => { cancelled = true; unlisten?.(); };
  }, []);
}
