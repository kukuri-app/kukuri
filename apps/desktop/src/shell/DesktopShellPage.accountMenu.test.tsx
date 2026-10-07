import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import * as identity from '@/lib/api/identity';
import * as session from '@/lib/accountSession';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { requestAccountAdd } from '@/shell/page/accountAddRequest';
import { renderAtHash, setViewportWidth } from './DesktopShellPage.testHelpers';

async function renderShell() {
  setViewportWidth(1280);
  const api = createDesktopMockApi();
  const profile = await api.getMyProfile();
  const a = { id: 'aaaaaaaaaaaaaaaa', pubkey: profile.pubkey, label: null, created_at: 1, last_used_at: 2 };
  const b = { id: 'bbbbbbbbbbbbbbbb', pubkey: 'b'.repeat(64), label: null, created_at: 1, last_used_at: 1 };
  vi.spyOn(identity, 'listAccounts').mockResolvedValue({ active_account_id: a.id, accounts: [a, b] });
  vi.spyOn(identity, 'getProfileSetupRequired').mockResolvedValue(false);
  vi.spyOn(identity, 'getAccountDisplay').mockResolvedValue([
    { id: a.id, name: profile.name ?? null, display_name: profile.display_name ?? null, picture: null, unavailable: false },
    { id: b.id, name: 'second-user', display_name: 'Second Account', picture: null, unavailable: false },
  ]);
  const change = vi.spyOn(session, 'changeAccountSession').mockResolvedValue(undefined);
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral', api);
  return { change, a, b };
}

async function setup() {
  const { change, a, b } = await renderShell();
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Account menu' }));
  const menu = await screen.findByRole('menu', { name: 'Account menu' });
  await within(menu).findByText('Second Account');
  return { user, menu, change, a, b };
}

test('account menu orders profile first and switches a row without extra confirmation', async () => {
  const { user, menu, change, b } = await setup();
  const buttons = within(menu).getAllByRole('menuitem');
  expect(buttons.map((button) => button.getAttribute('aria-label') ?? button.textContent))
    .toEqual(['View profile', 'Sync profile across devices', 'Add account', 'Manage accounts', 'Log out']);
  await user.click(within(menu).getByRole('menuitemradio', { name: /Second Account.*@second-user/ }));
  expect(change).toHaveBeenCalledExactlyOnceWith(b.id, false);
});

test('logout requires yes and cancel leaves accounts unchanged', async () => {
  const { user, menu, change, a } = await setup();
  await user.click(within(menu).getByRole('menuitem', { name: 'Log out' }));
  const dialog = await screen.findByRole('dialog', { name: 'Log out of this account?' });
  expect(within(dialog).getByText(/Local data stays here/)).toBeVisible();
  await waitFor(() => expect(within(dialog).getByRole('button', { name: 'Cancel' })).toHaveFocus());
  await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  expect(change).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Account menu' }));
  await user.click(within(await screen.findByRole('menu')).getByRole('menuitem', { name: 'Log out' }));
  await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Yes' }));
  expect(change).toHaveBeenCalledExactlyOnceWith(a.id, true);
});

test('view profile focuses an existing own column and does not duplicate it', async () => {
  const { user, menu } = await setup();
  await user.click(within(menu).getByRole('menuitem', { name: 'View profile' }));
  await waitFor(() => expect(document.activeElement).toHaveAttribute('data-column-id'));
  const first = document.activeElement;
  const count = document.querySelectorAll('[data-column-id]').length;
  await user.click(screen.getByRole('button', { name: 'Account menu' }));
  await user.click(within(await screen.findByRole('menu')).getByRole('menuitem', { name: 'View profile' }));
  await waitFor(() => expect(document.activeElement).toBe(first));
  expect(document.querySelectorAll('[data-column-id]')).toHaveLength(count);
});

test('add account dialog offers key import and explicit account creation', async () => {
  const { user, menu, change, a } = await setup();
  await user.click(within(menu).getByRole('menuitem', { name: 'Add account' }));
  const dialog = await screen.findByRole('dialog', { name: 'Add account' });
  expect(within(dialog).getByTestId('import-input')).toBeVisible();
  await user.click(within(dialog).getByRole('button', { name: 'Create a new account' }));
  expect(change).toHaveBeenCalledExactlyOnceWith(a.id, false, expect.stringMatching(/^[a-f0-9-]{36}$/));
  expect(within(dialog).getByRole('button', { name: 'Working…' })).toBeDisabled();
});

test('add account dialog opens both device transfer entries and returns with Back', async () => {
  const { user, menu } = await setup();
  const cancel = vi.spyOn(identity, 'cancelAccountTransfer').mockResolvedValue(undefined);
  vi.spyOn(identity, 'getAccountTransferStatus').mockResolvedValue({ state: 'waiting', expires_at_ms: Date.now() + 300_000 });
  vi.spyOn(identity, 'createAccountTransferInvite').mockResolvedValue({ link: 'kukuri://transfer#v1.ZXhhbXBsZQ', expires_at_ms: Date.now() + 300_000 });
  await user.click(within(menu).getByRole('menuitem', { name: 'Add account' }));
  const dialog = await screen.findByRole('dialog', { name: 'Add account' });
  await user.click(within(dialog).getByRole('button', { name: 'Move to another device' }));
  const source = await screen.findByRole('dialog', { name: 'Move to another device' });
  expect(await within(source).findByRole('img', { name: 'QR code of the transfer link' })).toBeVisible();
  await user.click(within(source).getByRole('button', { name: 'Back' }));
  await waitFor(() => expect(cancel).toHaveBeenCalled());
  await user.click(within(await screen.findByRole('dialog', { name: 'Add account' })).getByRole('button', { name: 'Move from another device' }));
  const target = await screen.findByRole('dialog', { name: 'Move from another device' });
  expect(within(target).getByRole('textbox', { name: /^Transfer link/ })).toHaveValue('');
});

// #1629: 設定の「アカウント」は 3 つの方法の一覧に代えて、アカウントメニューの「別の端末へ移す」の dialog を開くボタンを置く。
test('the account settings open the move to another device dialog', async () => {
  const { user, menu } = await setup();
  vi.spyOn(identity, 'cancelAccountTransfer').mockResolvedValue(undefined);
  vi.spyOn(identity, 'getAccountTransferStatus').mockResolvedValue({ state: 'waiting', expires_at_ms: Date.now() + 300_000 });
  vi.spyOn(identity, 'createAccountTransferInvite').mockResolvedValue({ link: 'kukuri://transfer#v1.ZXhhbXBsZQ', expires_at_ms: Date.now() + 300_000 });
  await user.click(within(menu).getByRole('menuitem', { name: 'Manage accounts' }));
  const settings = await screen.findByRole('dialog', { name: 'Settings' });
  expect(within(settings).queryByText('You can use your account on another device in these ways.')).not.toBeInTheDocument();
  await user.click(await within(settings).findByRole('button', { name: 'Move your account to another device' }));
  const source = await screen.findByRole('dialog', { name: 'Move to another device' });
  expect(await within(source).findByRole('img', { name: 'QR code of the transfer link' })).toBeVisible();
});

async function completeTransferTo(accountId: string) {
  const { user, menu, change } = await setup();
  vi.spyOn(identity, 'cancelAccountTransfer').mockResolvedValue(undefined);
  vi.spyOn(identity, 'openAccountTransfer').mockResolvedValue(undefined);
  vi.spyOn(identity, 'getAccountTransferStatus').mockResolvedValue({ state: 'completed', role: 'target', account_id: accountId, history: null });
  await user.click(within(menu).getByRole('menuitem', { name: 'Add account' }));
  await user.click(within(await screen.findByRole('dialog', { name: 'Add account' })).getByRole('button', { name: 'Move from another device' }));
  const target = await screen.findByRole('dialog', { name: 'Move from another device' });
  await user.type(within(target).getByRole('textbox', { name: /^Transfer link/ }), 'kukuri://transfer#v1.ZXhhbXBsZQ');
  await user.click(within(target).getByRole('button', { name: 'Connect' }));
  expect(await within(target).findByTestId('account-transfer-completed', {}, { timeout: 2000 })).toBeVisible();
  // AC-5: 完了の画面の説明を読めるように、「このアカウントを使う」を押すまでは切り替えない。
  await new Promise((resolve) => setTimeout(resolve, 700));
  expect(change).not.toHaveBeenCalled();
  await user.click(within(target).getByRole('button', { name: 'Use this account' }));
  return change;
}

test('a completed transfer switches to the received account when asked', async () => {
  const change = await completeTransferTo('cccccccccccccccc');
  await waitFor(() => expect(change).toHaveBeenCalledExactlyOnceWith('cccccccccccccccc', false));
});

test('a completed transfer into the active account closes the dialog without switching', async () => {
  const change = await completeTransferTo('aaaaaaaaaaaaaaaa');
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Move from another device' })).not.toBeInTheDocument());
  expect(change).not.toHaveBeenCalled();
});

// #1211 AC-3（監査 B-1、2026-10-04 ユーザー決定）: 移行先は履歴を受けている間は Dialog を閉じられず、「戻る」も出ない。
// 履歴は「やめる」で終え、終えたら完了の画面から受け取ったアカウントへ切り替える。
test('the target cannot close the dialog while receiving the history and switches after stopping it', async () => {
  const { user, menu, change } = await setup();
  const cancel = vi.spyOn(identity, 'cancelAccountTransfer').mockResolvedValue(undefined);
  vi.spyOn(identity, 'openAccountTransfer').mockResolvedValue(undefined);
  const status = vi.spyOn(identity, 'getAccountTransferStatus')
    .mockResolvedValue({ state: 'history', role: 'target', account_id: 'cccccccccccccccc', posts: 3, unavailable: 0 });
  await user.click(within(menu).getByRole('menuitem', { name: 'Add account' }));
  await user.click(within(await screen.findByRole('dialog', { name: 'Add account' })).getByRole('button', { name: 'Move from another device' }));
  const target = await screen.findByRole('dialog', { name: 'Move from another device' });
  expect(within(target).getByRole('button', { name: 'Close dialog' })).toBeVisible();
  await user.type(within(target).getByRole('textbox', { name: /^Transfer link/ }), 'kukuri://transfer#v1.ZXhhbXBsZQ');
  // 履歴の画面が出た時点から「戻る」と閉じるボタンは無い（描いた後の次の更新を待たない）。DOM が変わるたびに記録する。
  const closable: boolean[] = [];
  const observer = new MutationObserver(() => {
    if (!within(target).queryByText('Receiving post history (3)…')) return;
    closable.push(within(target).queryAllByRole('button', { name: /^(Back|Close dialog)$/ }).length > 0);
  });
  observer.observe(target, { childList: true, subtree: true });
  await user.click(within(target).getByRole('button', { name: 'Connect' }));
  expect(await within(target).findByText('Receiving post history (3)…', {}, { timeout: 2000 })).toBeVisible();
  observer.disconnect();
  expect(closable.length).toBeGreaterThan(0);
  expect(closable).not.toContain(true);
  await user.keyboard('{Escape}');
  expect(screen.getByRole('dialog', { name: 'Move from another device' })).toBeVisible();
  expect(cancel).not.toHaveBeenCalled();

  status.mockResolvedValue({ state: 'completed', role: 'target', account_id: 'cccccccccccccccc', history: { posts: 3, unavailable: 0, stopped: 'cancelled' } });
  await user.click(within(target).getByRole('button', { name: 'Stop receiving history' }));
  await user.click(await within(target).findByRole('button', { name: 'Use this account' }, { timeout: 2000 }));
  await waitFor(() => expect(change).toHaveBeenCalledExactlyOnceWith('cccccccccccccccc', false));
});

// #1217 AC-5: 初回の profile 設定の「以前のアカウントを戻す」は、menu を開いていなくても、menu から開くときと同じ
// アカウント追加の dialog（作成・import・移行）を開く。
test('a restore request opens the add account dialog with every action available', async () => {
  await renderShell();
  await screen.findByRole('button', { name: 'Account menu' });
  act(() => requestAccountAdd());
  const dialog = await screen.findByRole('dialog', { name: 'Add account' });
  expect(within(dialog).getByTestId('import-input')).toBeVisible();
  await waitFor(() => expect(within(dialog).getByTestId('create-new-account')).toBeEnabled());
  expect(within(dialog).getByRole('button', { name: 'Move to another device' })).toBeEnabled();
  expect(within(dialog).getByRole('button', { name: 'Move from another device' })).toBeEnabled();
});

// #1650: 同期のボタンは使用中のアカウントの行だけに、チェックの右に置く。押すと同期の dialog が開いて待ち受け、閉じると
// 止める。
test('the current account row syncs the profile across devices', async () => {
  const { user, menu, change } = await setup();
  const cancel = vi.spyOn(identity, 'cancelAccountTransfer').mockResolvedValue(undefined);
  const start = vi.spyOn(identity, 'startAccountUnionSync').mockResolvedValue(undefined);
  vi.spyOn(identity, 'getAccountTransferStatus').mockResolvedValue({ state: 'waiting', expires_at_ms: Date.now() + 300_000 });
  const sync = within(menu).getByRole('menuitem', { name: 'Sync profile across devices' });
  expect(within(menu).getAllByRole('menuitem', { name: 'Sync profile across devices' })).toHaveLength(1);
  expect(within(menu).getByRole('menuitemradio', { checked: true }).nextElementSibling).toBe(sync);
  await user.click(sync);
  const dialog = await screen.findByRole('dialog', { name: 'Sync profile across devices' });
  expect(await within(dialog).findByText(/On the other device, also choose/)).toBeVisible();
  expect(start).toHaveBeenCalledTimes(1);
  expect(change).not.toHaveBeenCalled();
  await user.click(within(dialog).getByRole('button', { name: 'Close dialog' }));
  await waitFor(() => expect(cancel).toHaveBeenCalled());
});

// ADR 0059 §1: 版の更新でレイアウトを外していた間に切替が失敗したら、戻ったレイアウトでメニューを開いて知らせる。
test('a switch that failed while the layout was away opens the menu with the failure', async () => {
  vi.spyOn(session, 'takeUnseenAccountFailure').mockReturnValueOnce(true);
  await renderShell();
  const menu = await screen.findByRole('menu', { name: 'Account menu' });
  expect(await within(menu).findByText('Could not complete the action. Please try again.')).toBeVisible();
  expect(within(menu).getByText('Second Account')).toBeVisible();
});
