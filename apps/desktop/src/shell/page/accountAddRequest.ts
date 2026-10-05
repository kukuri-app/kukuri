// #1217 AC-5: 初回の profile 設定の「以前のアカウントを戻す」から、アカウントメニューの「アカウント追加」の dialog を開く。
const listeners = new Set<() => void>();

export function requestAccountAdd(): void {
  for (const open of listeners) open();
}

export function onAccountAddRequest(open: () => void): () => void {
  listeners.add(open);
  return () => {
    listeners.delete(open);
  };
}
