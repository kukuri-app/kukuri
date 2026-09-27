import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { App } from '@/App';
import { DEVELOPER_MODE_STORAGE_KEY } from '@/lib/developerMode';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import {
  getActiveColumn,
  openControlCenter,
  openSettingsSection,
  renderAtHash,
  selectWorkspace,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
  // test/setup.ts は既存テスト向けに developer mode を有効化するため、既定 OFF の挙動をここで検証する。
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'false');
});

test('developer mode off hides Live/Game tabs and shell status badges by default', async () => {
  const user = userEvent.setup();
  const { container } = render(<App api={createDesktopMockApi()} />);

  const controlCenter = await openControlCenter(user);
  expect(within(controlCenter).getByRole('button', { name: 'Add Timeline Column' })).toBeInTheDocument();
  expect(within(controlCenter).queryByRole('button', { name: 'Add Live Column' })).not.toBeInTheDocument();
  expect(within(controlCenter).queryByRole('button', { name: 'Add Metaverse Column' })).not.toBeInTheDocument();
  expect(within(controlCenter).getByRole('button', { name: 'Add Messages Column' })).toBeInTheDocument();
  expect(within(controlCenter).getByRole('button', { name: 'Add Profile Column' })).toBeInTheDocument();

  expect(container.querySelector('.shell-status-badges')).toBeNull();
});

test('developer mode off falls back to timeline for live and game hash routes', async () => {
  renderAtHash('#/live?topic=kukuri%3Atopic%3Ageneral');

  await waitFor(() => {
    expect(window.location.hash).toMatch(/^#\/timeline/);
  });
  expect(getActiveColumn('Timeline')).toHaveAttribute('aria-current', 'true');
});

test('developer mode toggle reveals WIP tabs and badges and persists across reloads', async () => {
  const user = userEvent.setup();
  const { container, unmount } = render(<App api={createDesktopMockApi()} />);

  const drawer = await openSettingsSection(user, 'developer');
  await user.click(within(drawer).getByRole('checkbox', { name: 'Enable developer mode' }));

  expect(window.localStorage.getItem(DEVELOPER_MODE_STORAGE_KEY)).toBe('true');
  expect(container.querySelector('.shell-status-badges')).toBeNull();

  await user.keyboard('{Escape}');
  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument();
  });
  const controlCenter = await openControlCenter(user);
  expect(within(controlCenter).getByRole('button', { name: 'Add Live Column' })).toBeInTheDocument();
  expect(within(controlCenter).getByRole('button', { name: 'Add Metaverse Column' })).toBeInTheDocument();

  // localStorage に永続化されるため、再マウント後も developer mode は有効のまま。
  unmount();
  render(<App api={createDesktopMockApi()} />);
  const restoredControlCenter = await openControlCenter(user);
  expect(within(restoredControlCenter).getByRole('button', { name: 'Add Live Column' })).toBeInTheDocument();
});

test('disabling developer mode while on Live routes back to timeline', async () => {
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'true');
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await selectWorkspace(user, 'Live');
  expect(window.location.hash).toMatch(/^#\/live/);

  const drawer = await openSettingsSection(user, 'developer');
  await user.click(within(drawer).getByRole('checkbox', { name: 'Enable developer mode' }));

  await waitFor(() => {
    expect(window.location.hash).toMatch(/^#\/timeline/);
    expect(getActiveColumn('Timeline')).toHaveAttribute('aria-current', 'true');
  });
  const controlCenter = await openControlCenter(user);
  expect(within(controlCenter).queryByRole('button', { name: 'Add Live Column' })).not.toBeInTheDocument();
});

test('developer mode off keeps ticket import while hiding connectivity diagnostics', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const drawer = await openSettingsSection(user, 'connectivity');

  expect(within(drawer).getByText('Your Ticket')).toBeInTheDocument();
  expect(within(drawer).getByText('Peer Ticket')).toBeInTheDocument();
  expect(within(drawer).queryByText('Connected peers and assistance candidates')).not.toBeInTheDocument();
  expect(within(drawer).queryByText('Connected Peers')).not.toBeInTheDocument();
});

test('developer settings immediately confirm mode changes and restore the current status', async () => {
  const user = userEvent.setup();
  const { unmount } = render(<App api={createDesktopMockApi()} />);
  const drawer = await openSettingsSection(user, 'developer');
  const toggle = within(drawer).getByRole('checkbox', { name: 'Enable developer mode' });
  expect(within(drawer).getByRole('status')).toHaveTextContent('Developer mode is off.');
  expect(within(drawer).queryByRole('button', { name: 'Connection diagnostics' })).not.toBeInTheDocument();
  await user.click(toggle);
  expect(toggle).toHaveFocus();
  expect(within(drawer).getByRole('status')).toHaveTextContent('Developer mode is on.');
  expect(within(drawer).getByRole('button', { name: 'Connection diagnostics' })).toBeVisible();
  await user.click(toggle);
  expect(within(drawer).getByRole('status')).toHaveTextContent('Developer mode is off.');
  expect(within(drawer).queryByRole('button', { name: 'Connection diagnostics' })).not.toBeInTheDocument();
  await user.click(toggle);
  unmount();
  render(<App api={createDesktopMockApi()} />);
  const restored = await openSettingsSection(user, 'developer');
  expect(within(restored).getByRole('status')).toHaveTextContent('Developer mode is on.');
  expect(within(restored).getByRole('button', { name: 'Connection diagnostics' })).toBeVisible();
});

test.each([
  ['connectivity', 'Connection diagnostics'],
  ['discovery', 'Discovery diagnostics'],
  ['community-node', 'Community node diagnostics'],
])('developer diagnostics navigate directly to %s and keep settings open', async (section, name) => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const drawer = await openSettingsSection(user, 'developer');
  await user.click(within(drawer).getByRole('checkbox', { name: 'Enable developer mode' }));
  await user.click(within(drawer).getByRole('button', { name }));
  expect(drawer).toBeVisible();
  const selected = within(drawer).getByTestId(`settings-section-${section}`);
  expect(selected).toHaveAttribute('aria-current', 'location');
  await waitFor(() => expect(selected).toHaveFocus());
  expect(window.location.hash).toContain(`settings=${section}`);
  await user.click(within(drawer).getByTestId('settings-section-developer'));
  expect(within(drawer).getByRole('status')).toHaveTextContent('Developer mode is on.');
});

test('diagnostic shortcuts preserve unsaved input and show connection errors without mutations', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const status = await api.getSyncStatus();
  vi.spyOn(api, 'getSyncStatus').mockResolvedValue({ ...status, last_error: 'Connection unavailable' });
  const mutations = [
    vi.spyOn(api, 'importPeerTicket'),
    vi.spyOn(api, 'setDiscoverySeeds'),
    vi.spyOn(api, 'setCommunityNodeConfig'),
    vi.spyOn(api, 'authenticateCommunityNode'),
    vi.spyOn(api, 'clearCommunityNodeToken'),
    vi.spyOn(api, 'refreshCommunityNodeMetadata'),
  ];
  render(<App api={api} />);
  const drawer = await openSettingsSection(user, 'connectivity');
  await user.type(within(drawer).getByRole('textbox', { name: 'Peer Ticket' }), 'unsaved ticket');
  await user.click(within(drawer).getByTestId('settings-section-discovery'));
  await user.type(within(drawer).getByRole('textbox', { name: 'Seed Peers' }), 'unsaved seed');
  await user.click(within(drawer).getByTestId('settings-section-developer'));
  await user.click(within(drawer).getByRole('checkbox', { name: 'Enable developer mode' }));
  await user.click(within(drawer).getByRole('button', { name: 'Connection diagnostics' }));
  expect(within(drawer).getByRole('textbox', { name: 'Peer Ticket' })).toHaveValue('unsaved ticket');
  // 詳細は開くまで描かない(#1221 R2-D。peer の一覧を開いたときだけ読む)。
  expect(within(drawer).queryByText(/Connection unavailable/)).toBeNull();
  await user.click(within(drawer).getAllByText('Technical diagnostic details')[0]);
  expect(within(drawer).getByText('Connected peers and assistance candidates')).toBeVisible();
  expect(within(drawer).getAllByText(/Connection unavailable/).length).toBeGreaterThan(0);
  await user.click(within(drawer).getByTestId('settings-section-developer'));
  await user.click(within(drawer).getByRole('button', { name: 'Discovery diagnostics' }));
  expect(within(drawer).getByRole('textbox', { name: 'Seed Peers' })).toHaveValue('unsaved seed');
  await user.click(within(drawer).getByText('Technical diagnostic details'));
  expect(within(drawer).getByText('Local Endpoint ID')).toBeVisible();
  await user.click(within(drawer).getByTestId('settings-section-developer'));
  await user.click(within(drawer).getByRole('button', { name: 'Community node diagnostics' }));
  for (const mutation of mutations) expect(mutation).not.toHaveBeenCalled();
});

// #962: 開発者 section から診断レポート(リリース section の開発者向け診断)へ移動できる。
test('developer settings open the diagnostic report without leaving the drawer', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const drawer = await openSettingsSection(user, 'developer');
  await user.click(within(drawer).getByRole('checkbox', { name: 'Enable developer mode' }));
  expect(within(drawer).getByRole('heading', { name: 'Logs' })).toBeVisible();
  await user.click(within(drawer).getByRole('button', { name: 'Diagnostic report' }));
  expect(drawer).toBeVisible();
  expect(within(drawer).getByTestId('settings-section-release')).toHaveAttribute('aria-current', 'location');
  expect(window.location.hash).toContain('settings=release');
  expect(within(drawer).getByRole('button', { name: 'Copy Report' })).toBeVisible();
});

// #978: OFF のあいだはログ IPC を呼ばず、ON で表示時に 1 回、「更新」で明示的にもう 1 回読む。
test('developer log viewer reads backend logs only while the mode is on', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const readDesktopLogs = vi.spyOn(api, 'readDesktopLogs');
  render(<App api={api} />);

  const drawer = await openSettingsSection(user, 'developer');
  expect(readDesktopLogs).not.toHaveBeenCalled();
  expect(within(drawer).queryByRole('heading', { name: 'Logs' })).not.toBeInTheDocument();

  await user.click(within(drawer).getByRole('checkbox', { name: 'Enable developer mode' }));
  const region = await within(drawer).findByRole('region', { name: 'Recent logs, 12 lines' });
  expect(within(region).getByText(/desktop profile lease acquired/)).toBeVisible();
  expect(readDesktopLogs).toHaveBeenCalledTimes(1);
  expect(readDesktopLogs).toHaveBeenLastCalledWith(null, null);

  await user.click(within(drawer).getByRole('button', { name: 'Refresh logs' }));
  await waitFor(() => expect(readDesktopLogs).toHaveBeenCalledTimes(2));
  expect(readDesktopLogs).toHaveBeenLastCalledWith(12, null);
  expect(within(drawer).getByRole('region', { name: 'Recent logs, 12 lines' })).toBeVisible();

  await user.click(within(drawer).getByRole('checkbox', { name: 'Enable developer mode' }));
  expect(within(drawer).queryByRole('heading', { name: 'Logs' })).not.toBeInTheDocument();
  expect(readDesktopLogs).toHaveBeenCalledTimes(2);
});
