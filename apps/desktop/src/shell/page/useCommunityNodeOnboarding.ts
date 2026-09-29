import { useEffect, useEffectEvent, useRef, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { firstUnconsentedCommunityNode, reconsentPendingCommunityNodes } from '@/lib/api/communityNodeAvailability';
import { useDesktopShellStore, useDesktopShellStoreApi } from '@/shell/store';

// 対象を1回だけ開く。別のmodalが操作中なら終了まで待つ。pollや追加の外部I/Oはしない。
function useOpenWhenNoDialog(target: string | null, open: (target: string) => void) {
  const openTarget = useEffectEvent(open);
  useEffect(() => {
    if (!target) return;
    let opened = false;
    const tryOpen = () => {
      const modalOpen = [...document.querySelectorAll('[role="dialog"], [role="alertdialog"]')]
        .some((element) => !element.closest('[aria-hidden="true"], [hidden], [data-state="closed"], [data-open="false"]'));
      if (opened || modalOpen) return;
      opened = true;
      openTarget(target);
    };
    tryOpen();
    const observer = new MutationObserver(tryOpen);
    observer.observe(document.body, { childList: true, subtree: true, attributes: true,
      attributeFilter: ['data-state', 'aria-hidden', 'hidden', 'data-open'] });
    return () => observer.disconnect();
  }, [target]);
}

export function useCommunityNodeOnboarding() {
  const store = useDesktopShellStoreApi();
  const state = useDesktopShellStore(useShallow((s) => ({
    config: s.communityNodeConfig, statuses: s.communityNodeStatuses,
    loaded: s.communityNodeConfigLoaded && s.communityNodeStatusesLoaded &&
      !s.communityNodeConfigError && !s.communityNodeStatusError,
    author: s.syncStatus.local_author_pubkey,
    shownFor: s.communityNodeOnboardingShownFor,
    settingsOpen: s.shellChromeState.settingsOpen,
  })));
  const candidate = firstUnconsentedCommunityNode(state.config, state.statuses, state.loaded);
  const [open, setOpen] = useState(false);
  const returnFocus = useRef<HTMLElement | null>(null);
  const handingOff = useRef(false);
  const shown = state.shownFor.includes(state.author);

  useOpenWhenNoDialog(candidate && state.author && !shown && !state.settingsOpen ? candidate : null, () => {
    returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    handingOff.current = false;
    store.getState().setField('communityNodeOnboardingShownFor', (current) => [...current, state.author]);
    setOpen(true);
  });

  useEffect(() => { if (!candidate) setOpen(false); }, [candidate]);

  return {
    baseUrl: open ? candidate : null,
    dismiss: () => setOpen(false),
    resume: () => setOpen(true),
    handOff: () => {
      handingOff.current = true;
      setOpen(false);
      return returnFocus.current;
    },
    restoreFocus: (event: Event) => {
      event.preventDefault();
      if (!handingOff.current && returnFocus.current?.isConnected) returnFocus.current.focus();
    },
  };
}

// #1420: 同意済みNodeの規約更新を検知したら、そのNodeの同意モーダルを開く。一度開いた更新は、
// 閉じても受諾しても、pendingが解消するまで再び開かない。記録は起動中のメモリだけに置く。
// 開いたモーダルはportalのmount前にDOMへ現れないため、表示中は`busy`で次のNodeを待たせる。
export function useCommunityNodeReconsent(open: (baseUrl: string) => void, busy: boolean) {
  const state = useDesktopShellStore(useShallow((s) => ({
    config: s.communityNodeConfig, statuses: s.communityNodeStatuses,
    author: s.syncStatus.local_author_pubkey,
    settingsOpen: s.shellChromeState.settingsOpen,
  })));
  const pending = reconsentPendingCommunityNodes(state.config, state.statuses);
  const [prompted, setPrompted] = useState({ author: state.author, baseUrls: [] as string[] });
  const current = prompted.author === state.author ? prompted.baseUrls : [];
  if (prompted.author !== state.author || current.some((baseUrl) => !pending.includes(baseUrl))) {
    setPrompted({ author: state.author, baseUrls: current.filter((baseUrl) => pending.includes(baseUrl)) });
  }
  const candidate = pending.find((baseUrl) => !current.includes(baseUrl)) ?? null;

  useOpenWhenNoDialog(candidate && state.author && !state.settingsOpen && !busy ? candidate : null, (baseUrl) => {
    setPrompted((value) => ({ ...value, baseUrls: [...value.baseUrls, baseUrl] }));
    open(baseUrl);
  });
}
