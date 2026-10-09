import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

// Android の Tauri の build（#1198 AC-1、#1193 D1）: Tauri CLI が渡す platform を android にして、live・game・metaverse・Dome と
// ウィンドウ・タスクトレイの設定を出さないことを確かめる。
vi.hoisted(() => {
  vi.stubEnv('TAURI_ENV_PLATFORM', 'android');
});

import { App } from '@/App';
import { DEVELOPER_MODE_STORAGE_KEY } from '@/lib/developerMode';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import {
  getActiveColumn,
  openControlCenter,
  openSettingsDrawer,
  renderAtHash,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
  // desktop では開発者モードで live・game・metaverse が出る。Android ではそれでも出さない。
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'true');
});

test('developer mode on Android does not offer live or metaverse columns', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const controlCenter = await openControlCenter(user);
  expect(within(controlCenter).getByRole('button', { name: 'Add Timeline Column' })).toBeInTheDocument();
  expect(within(controlCenter).queryByRole('button', { name: 'Add Live Column' })).not.toBeInTheDocument();
  expect(within(controlCenter).queryByRole('button', { name: 'Add Metaverse Column' })).not.toBeInTheDocument();
});

test.each(['#/live', '#/game'])('%s on Android falls back to the topic timeline in developer mode', async (route) => {
  renderAtHash(`${route}?topic=kukuri%3Atopic%3Ageneral`);

  await waitFor(() => {
    expect(window.location.hash).toMatch(/^#\/timeline/);
  });
  expect(getActiveColumn('Timeline')).toHaveAttribute('aria-current', 'true');
});

test('settings on Android leave out the window section and say which features are unavailable', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const drawer = await openSettingsDrawer(user);
  expect(within(drawer).queryByTestId('settings-section-system')).not.toBeInTheDocument();
  expect(within(drawer).getByTestId('settings-section-notifications')).toBeInTheDocument();
  expect(within(drawer).getByTestId('settings-section-backup')).toBeInTheDocument();

  await user.click(within(drawer).getByTestId('settings-section-about'));
  expect(
    await screen.findByText('The Android version does not support live / game / metaverse / Dome.')
  ).toBeVisible();
});
