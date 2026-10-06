// #1217 AC-5: 初回の profile 設定の「以前のアカウントを戻す」から、アカウントメニューの「アカウント追加」の dialog を開く。
// #1629: 設定の「アカウント」からは、同じ menu の「別の端末へ移す」の dialog を開く。
export type AccountDialogRequest = 'import' | 'transfer-source';

const listeners = new Set<(dialog: AccountDialogRequest) => void>();

export function requestAccountAdd(dialog: AccountDialogRequest = 'import'): void {
  for (const open of listeners) open(dialog);
}

export function onAccountAddRequest(open: (dialog: AccountDialogRequest) => void): () => void {
  listeners.add(open);
  return () => {
    listeners.delete(open);
  };
}
