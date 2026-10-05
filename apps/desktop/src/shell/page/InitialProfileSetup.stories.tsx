import { useMemo } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { InitialProfileSetup } from './InitialProfileSetup';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';

function SetupStory({ failure = false, nodeFailed = false, restore = false }: { failure?: boolean; nodeFailed?: boolean; restore?: boolean }) {
  const pubkey = 'a'.repeat(64);
  const store = useMemo(() => {
    const result = createDesktopShellStore();
    result.getState().patchState({ syncStatus: { ...result.getState().syncStatus, local_author_pubkey: pubkey } });
    return result;
  }, [pubkey]);
  const access = useMemo(() => ({
    listAccounts: async () => ({ active_account_id: 'account-a', accounts: [{ id: 'account-a', pubkey, label: null, created_at: 1, last_used_at: 1 }] }),
    getProfileSetupRequired: async () => true,
    saveInitialProfile: async () => {
      if (failure) throw new Error('save unavailable');
      return { pubkey, name: 'new-user', display_name: 'New User', about: null, picture_asset: null, updated_at: 1 };
    },
  }), [failure, pubkey]);
  return <DesktopShellStoreContext.Provider value={store}><InitialProfileSetup ready={!nodeFailed} nodeFailed={nodeFailed} onSkipNode={() => {}} accountAccess={access} onRestore={restore ? () => {} : undefined} /></DesktopShellStoreContext.Provider>;
}

const meta = { title: 'Shell/InitialProfileSetup', component: SetupStory, parameters: { layout: 'fullscreen' } } satisfies Meta<typeof SetupStory>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Ready: Story = {};
export const SaveFailure: Story = { args: { failure: true } };
export const NodeUnavailable: Story = { args: { nodeFailed: true } };
// Web の build（#1217 AC-5）: 以前のアカウントを戻す案内と入口。
export const WebRestore: Story = { args: { restore: true } };
