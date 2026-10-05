import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import { InitialProfileSetup } from './InitialProfileSetup';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';
import type { Profile } from '@/lib/api';

test('a delayed save response cannot overwrite the newly active account profile', async () => {
  const store = createDesktopShellStore();
  const a: Profile = { pubkey: 'a'.repeat(64), name: 'a', display_name: 'Account A', about: null, picture_asset: null, updated_at: 0 };
  const b: Profile = { ...a, pubkey: 'b'.repeat(64), name: 'b', display_name: 'Account B' };
  store.getState().patchState({ syncStatus: { ...store.getState().syncStatus, local_author_pubkey: a.pubkey }, localProfile: a });
  let resolve!: (profile: Profile) => void;
  const save = vi.fn(() => new Promise<Profile>((done) => { resolve = done; }));
  const access = {
    listAccounts: async () => {
      const pubkey = store.getState().syncStatus.local_author_pubkey;
      return { active_account_id: pubkey, accounts: [{ id: pubkey, pubkey, label: null, created_at: 1, last_used_at: 1 }] };
    },
    getProfileSetupRequired: async (id: string) => id === a.pubkey,
    saveInitialProfile: save,
  };
  const view = (key: string) => <DesktopShellStoreContext.Provider value={store}><InitialProfileSetup key={key} ready nodeFailed={false} onSkipNode={() => {}} accountAccess={access} /></DesktopShellStoreContext.Provider>;
  const rendered = render(view(a.pubkey));
  const user = userEvent.setup();
  await user.click(within(await screen.findByRole('dialog', { name: 'Set up your profile' })).getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  act(() => { store.getState().patchState({ syncStatus: { ...store.getState().syncStatus, local_author_pubkey: b.pubkey }, localProfile: b }); });
  rendered.rerender(view(b.pubkey));
  await act(async () => { resolve({ ...a, updated_at: 5 }); });
  expect(store.getState().localProfile).toEqual(b);
  expect(save).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ account_id: a.pubkey }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

// #1217 AC-5: Web では、以前のアカウントを戻す案内と入口を出す。押すと dialog を閉じて入口を開く。Tauri（`onRestore` なし）では出さない。
test('the web build offers to restore a previous account from the initial profile setup', async () => {
  const store = createDesktopShellStore();
  const pubkey = 'a'.repeat(64);
  store.getState().patchState({ syncStatus: { ...store.getState().syncStatus, local_author_pubkey: pubkey } });
  const access = {
    listAccounts: async () => ({ active_account_id: 'account-a', accounts: [{ id: 'account-a', pubkey, label: null, created_at: 1, last_used_at: 1 }] }),
    getProfileSetupRequired: async () => true,
    saveInitialProfile: vi.fn(),
  };
  const view = (onRestore?: () => void) => <DesktopShellStoreContext.Provider value={store}><InitialProfileSetup ready nodeFailed={false} onSkipNode={() => {}} accountAccess={access} onRestore={onRestore} /></DesktopShellStoreContext.Provider>;
  const desktop = render(view());
  await screen.findByRole('dialog', { name: 'Set up your profile' });
  expect(screen.queryByRole('button', { name: 'Restore a previous account' })).not.toBeInTheDocument();
  desktop.unmount();

  const restore = vi.fn();
  render(view(restore));
  const dialog = await screen.findByRole('dialog', { name: 'Set up your profile' });
  expect(dialog).toHaveTextContent('importing an exported account key or by moving it from another device');
  await userEvent.setup().click(within(dialog).getByRole('button', { name: 'Restore a previous account' }));
  expect(restore).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
});
