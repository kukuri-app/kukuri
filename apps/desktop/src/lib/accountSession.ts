import { switchAccount, logoutAccount, listAccounts, createAccount } from '@/lib/api/identity';
import { COLUMN_DRAFT_STORAGE_KEY, suspendColumnDraftPersistence } from '@/shell/columnDraftPersistence';

const retainedDraftKey = (id: string) => `kukuri:account:${id}:retained-column-drafts`;
const TRANSITION_KEY = 'kukuri:account-transition-drafts';
const creationKey = (id: string) => `kukuri:account:${id}:pending-creation`;

// 切替の途中で database の版を更新する間は、画面がレイアウトを外して待つ（ADR 0059 §1）。画面は切替が終わるまでレイアウトに
// 戻さず、外していた間の失敗は、戻ったレイアウトのアカウントのメニューで知らせる。
let changing = false, layoutRemoved = false, unseenFailure = false;
export const isChangingAccount = () => changing;
export function noteAccountLayoutRemoved() { if (changing) layoutRemoved = true; }
export function takeUnseenAccountFailure() { const failed = unseenFailure; unseenFailure = false; return failed; }

export function accountCreationOperationId(accountId: string): string {
  const existing = localStorage.getItem(creationKey(accountId));
  if (existing) return existing;
  const id = crypto.randomUUID();
  localStorage.setItem(creationKey(accountId), id);
  return id;
}

export async function reconcileAccountDrafts(): Promise<boolean> {
  const encoded = localStorage.getItem(TRANSITION_KEY);
  if (encoded === null) return false;
  const pending: { sourceId: string; draft: string | null; createOperationId?: string } = JSON.parse(encoded);
  if (typeof pending.sourceId !== 'string' || (pending.draft !== null && typeof pending.draft !== 'string')) throw new Error('Invalid account draft transition');
  const snapshot = await listAccounts();
  const nextDraft = snapshot.active_account_id === pending.sourceId ? pending.draft : localStorage.getItem(retainedDraftKey(snapshot.active_account_id));
  if (nextDraft === null) localStorage.removeItem(COLUMN_DRAFT_STORAGE_KEY);
  else localStorage.setItem(COLUMN_DRAFT_STORAGE_KEY, nextDraft);
  if (pending.createOperationId && snapshot.active_account_id !== pending.sourceId) localStorage.removeItem(creationKey(pending.sourceId));
  localStorage.removeItem(TRANSITION_KEY);
  return true;
}

export async function changeAccountSession(accountId: string, logout = false, createOperationId?: string) {
  const before = await listAccounts();
  const resume = suspendColumnDraftPersistence(localStorage);
  let keepSuspended = false;
  try {
  const draft = localStorage.getItem(COLUMN_DRAFT_STORAGE_KEY);
  if (logout) {
    if (draft !== null) localStorage.setItem(retainedDraftKey(accountId), draft);
    else localStorage.removeItem(retainedDraftKey(accountId));
  } else localStorage.removeItem(retainedDraftKey(before.active_account_id));
  localStorage.setItem(TRANSITION_KEY, JSON.stringify({ sourceId: before.active_account_id, draft, createOperationId }));
  localStorage.removeItem(COLUMN_DRAFT_STORAGE_KEY);
  keepSuspended = true;
  changing = true; layoutRemoved = false;
  try {
    await (createOperationId ? createAccount(accountId, createOperationId) : logout ? logoutAccount(accountId) : switchAccount(accountId));
  } catch (error) {
    try {
      const actual = await listAccounts();
      await reconcileAccountDrafts();
      if (actual.active_account_id !== before.active_account_id) window.location.reload();
      else { keepSuspended = false; unseenFailure = layoutRemoved; }
    } catch { window.location.reload(); }
    throw error;
  }
  // On a storage failure the durable marker is reconciled before shell startup.
  try { await reconcileAccountDrafts(); } finally { window.location.reload(); }
  } finally { changing = false; if (!keepSuspended) resume(); }
}
