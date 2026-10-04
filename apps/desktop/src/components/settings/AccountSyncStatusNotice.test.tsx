import { render, screen } from '@testing-library/react';
import { afterEach, expect, test } from 'vitest';

import i18n from '@/i18n';
import { AccountSyncStatusNotice } from './AccountSyncStatusNotice';

const SYNCED = { no_peers: false, fetch_failed: false, behind: false, pending_writes: false, rebuilding: false };

afterEach(() => i18n.changeLanguage('en'));

test.each([
  ['synced', SYNCED],
  ['pendingWrites', { ...SYNCED, pending_writes: true }],
  ['behind', { ...SYNCED, behind: true, pending_writes: true }],
  ['noPeers', { ...SYNCED, no_peers: true, behind: true, pending_writes: true }],
  ['fetchFailed', { ...SYNCED, fetch_failed: true, no_peers: true }],
  ['rebuilding', { ...SYNCED, rebuilding: true, fetch_failed: true }],
] as const)('several marks show the one that matters first: %s', async (state, status) => {
  for (const locale of ['en', 'ja', 'zh-CN']) {
    await i18n.changeLanguage(locale);
    const { unmount } = render(<AccountSyncStatusNotice status={status} />);
    expect(screen.getByTestId('account-sync-status')).toHaveAttribute('data-state', state);
    expect(screen.getByRole('status')).toHaveTextContent(i18n.t(`settings:accountKey.accountSync.${state}`));
    unmount();
  }
});
