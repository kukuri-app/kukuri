import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import * as identity from '@/lib/api/identity';
import * as session from '@/lib/accountSession';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { requestAccountAdd } from '@/shell/page/accountAddRequest';
import { renderAtHash, setViewportWidth } from './DesktopShellPage.testHelpers';

async function setup() {
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
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Account menu' }));
  const menu = await screen.findByRole('menu', { name: 'Account menu' });
  await within(menu).findByText('Second Account');
  return { user, menu, change, a, b };
}

test('account menu orders profile first and switches a row without extra confirmation', async () => {
  const { user, menu, change, b } = await setup();
  const buttons = within(menu).getAllByRole('menuitem');
  expect(buttons.map((button) => button.textContent)).toEqual(['View profile', 'Add account', 'Manage accounts', 'Log out']);
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

async function completeTransferTo(accountId: string) {
  const { user, menu, change } = await setup();
  vi.spyOn(identity, 'cancelAccountTransfer').mockResolvedValue(undefined);
  vi.spyOn(identity, 'openAccountTransfer').mockResolvedValue(undefined);
  vi.spyOn(identity, 'getAccountTransferStatus').mockResolvedValue({ state: 'completed', role: 'target', account_id: accountId });
  await user.click(within(menu).getByRole('menuitem', { name: 'Add account' }));
  await user.click(within(await screen.findByRole('dialog', { name: 'Add account' })).getByRole('button', { name: 'Move from another device' }));
  const target = await screen.findByRole('dialog', { name: 'Move from another device' });
  await user.type(within(target).getByRole('textbox', { name: /^Transfer link/ }), 'kukuri://transfer#v1.ZXhhbXBsZQ');
  await user.click(within(target).getByRole('button', { name: 'Connect' }));
  expect(await within(target).findByTestId('account-transfer-completed', {}, { timeout: 2000 })).toBeVisible();
  return change;
}

test('a completed transfer switches to the received account', async () => {
  const change = await completeTransferTo('cccccccccccccccc');
  await waitFor(() => expect(change).toHaveBeenCalledExactlyOnceWith('cccccccccccccccc', false));
});

test('a completed transfer into the active account does not switch', async () => {
  const change = await completeTransferTo('aaaaaaaaaaaaaaaa');
  await new Promise((resolve) => setTimeout(resolve, 100));
  expect(change).not.toHaveBeenCalled();
});

// #1217 AC-5: 初回の profile 設定の「以前のアカウントを戻す」は、アカウント追加の dialog（import・移行）を開く。
test('a restore request opens the add account dialog', async () => {
  await setup();
  act(() => requestAccountAdd());
  const dialog = await screen.findByRole('dialog', { name: 'Add account' });
  expect(within(dialog).getByTestId('import-input')).toBeVisible();
  expect(within(dialog).getByRole('button', { name: 'Move from another device' })).toBeVisible();
  expect(screen.queryByRole('menu', { name: 'Account menu' })).not.toBeInTheDocument();
});
