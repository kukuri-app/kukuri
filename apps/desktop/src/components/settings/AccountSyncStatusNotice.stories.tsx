import type { Meta, StoryObj } from '@storybook/react-vite';

import { AccountSyncStatusNotice } from './AccountSyncStatusNotice';

const SYNCED = { no_peers: false, fetch_failed: false, behind: false, pending_writes: false, rebuilding: false };

const meta = {
  title: 'Settings/AccountSyncStatusNotice',
  component: AccountSyncStatusNotice,
  args: { status: SYNCED },
} satisfies Meta<typeof AccountSyncStatusNotice>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Synced: Story = {};
export const NoPeers: Story = { args: { status: { ...SYNCED, no_peers: true, pending_writes: true } } };
export const Behind: Story = { args: { status: { ...SYNCED, behind: true } } };
export const PendingWrites: Story = { args: { status: { ...SYNCED, pending_writes: true } } };
export const FetchFailed: Story = { args: { status: { ...SYNCED, fetch_failed: true, behind: true } } };
export const Rebuilding: Story = { args: { status: { ...SYNCED, rebuilding: true, behind: true } } };
