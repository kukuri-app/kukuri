import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { App } from '@/App';
import { DEVELOPER_MODE_STORAGE_KEY } from '@/lib/developerMode';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import {
  openControlCenter,
  openSettingsDrawer,
  renderAtHash,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

// #967: バックアップ／復元は「Backup & restore」section が正本で、開発者モード OFF の通常設定と
// Control Center から到達できる。鍵だけの移行(Account)とは section を分け、相互に案内する。
// 入口の表示・移動は section と URL だけを変え、backup / 鍵 export などの sink を呼ばない。
const backupApi = vi.hoisted(() => ({
  cancelDeviceBackup: vi.fn(),
  chooseDeviceBackupDestination: vi.fn(),
  chooseDeviceBackupSource: vi.fn(),
  createDeviceBackup: vi.fn(),
  previewDeviceBackup: vi.fn(),
  restoreDeviceBackup: vi.fn(),
}));
const identityApi = vi.hoisted(() => ({
  exportAccountKey: vi.fn(),
  importAccountKey: vi.fn(),
  previewAccountKeyImport: vi.fn(),
  switchAccount: vi.fn(),
}));

vi.mock('@/lib/api/deviceBackup', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/api/deviceBackup')>()),
  ...backupApi,
}));
vi.mock('@/lib/api/identity', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/api/identity')>()),
  ...identityApi,
}));

const SINKS = [...Object.values(backupApi), ...Object.values(identityApi)];

function expectNoSinkCalls() {
  for (const sink of SINKS) {
    expect(sink).not.toHaveBeenCalled();
  }
}

beforeEach(() => {
  vi.clearAllMocks();
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'false');
});

test('settings expose a backup section with the create and restore actions without developer mode', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const drawer = await openSettingsDrawer(user);
  const nav = within(drawer).getByTestId('settings-section-backup');
  expect(nav).toHaveTextContent('Backup & restore');
  await user.click(nav);
  expect(within(drawer).getByRole('heading', { name: 'Create backup' })).toBeVisible();
  expect(within(drawer).getByRole('heading', { name: 'Restore from backup' })).toBeVisible();
  expect(within(drawer).getByRole('button', { name: 'Choose destination and create' })).toBeDisabled();
  expect(within(drawer).getByRole('button', { name: 'Choose backup file' })).toBeEnabled();
  expect(window.location.hash).toContain('settings=backup');
  // 鍵だけの export / import は Account section に残り、backup section には出ない。
  expect(within(drawer).queryByRole('heading', { name: 'Export account key' })).not.toBeInTheDocument();
  expectNoSinkCalls();
});

test('backup and account deep links open their sections instead of the default section', async () => {
  const { unmount } = renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=backup');
  const drawer = await screen.findByRole('dialog', { name: 'Settings' });
  await waitFor(() =>
    expect(within(drawer).getByTestId('settings-section-backup')).toHaveAttribute('aria-current', 'location')
  );
  expect(within(drawer).getByRole('heading', { name: 'Create backup' })).toBeVisible();
  unmount();

  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=account');
  const accountDrawer = await screen.findByRole('dialog', { name: 'Settings' });
  await waitFor(() =>
    expect(within(accountDrawer).getByTestId('settings-section-account')).toHaveAttribute(
      'aria-current',
      'location'
    )
  );
  expect(within(accountDrawer).getByRole('heading', { name: 'Export account key' })).toBeVisible();
  expectNoSinkCalls();
});

test('the Control Center system section opens backup & restore directly', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Backup & restore' }));
  const drawer = await screen.findByRole('dialog', { name: 'Settings' });
  expect(within(drawer).getByTestId('settings-section-backup')).toHaveAttribute('aria-current', 'location');
  expect(window.location.hash).toContain('settings=backup');
  expect(within(drawer).getByRole('heading', { name: 'Create backup' })).toBeVisible();
  expectNoSinkCalls();
});

// #1629: 対象の差は backup の側だけが示す（設定の「アカウント」の 3 つの方法の一覧は削除した）。
test('backup and account sections link to each other and backup explains the scope difference', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const drawer = await openSettingsDrawer(user);

  await user.click(within(drawer).getByTestId('settings-section-account'));
  await user.click(within(drawer).getByRole('button', { name: 'Open backup & restore' }));
  expect(within(drawer).getByTestId('settings-section-backup')).toHaveAttribute('aria-current', 'location');
  expect(window.location.hash).toContain('settings=backup');
  expect(within(drawer).getByTestId('settings-section-backup')).toHaveFocus();

  expect(within(drawer).getByText(/only your account key/i)).toBeVisible();
  await user.click(within(drawer).getByRole('button', { name: 'Open account key settings' }));
  expect(within(drawer).getByTestId('settings-section-account')).toHaveAttribute('aria-current', 'location');
  expect(window.location.hash).toContain('settings=account');
  expect(within(drawer).getByTestId('settings-section-account')).toHaveFocus();
  expectNoSinkCalls();
});

test('the backup section keeps the existing risk acknowledgement before any backup action', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const drawer = await openSettingsDrawer(user);
  await user.click(within(drawer).getByTestId('settings-section-backup'));
  expect(within(drawer).getByTestId('device-backup-passphrase')).toBeDisabled();
  await user.click(within(drawer).getByTestId('device-backup-acknowledge'));
  expect(within(drawer).getByTestId('device-backup-passphrase')).toBeEnabled();
  expect(within(drawer).getByRole('button', { name: 'Choose destination and create' })).toBeDisabled();
  expectNoSinkCalls();
});
