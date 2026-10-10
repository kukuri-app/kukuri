import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

// Android の Tauri の build（#1198 AC-1、#1193 D1）: Tauri CLI が渡す platform を android にして、live・game・metaverse・Dome と
// ウィンドウ・タスクトレイの設定を出さないこと、戻る（#1198 AC-2）が上に重なった層・画面の履歴の順に閉じることを確かめる。
vi.hoisted(() => {
  vi.stubEnv('TAURI_ENV_PLATFORM', 'android');
});

import { App } from '@/App';
import { handleBackButton } from '@/lib/androidBackButton';
import type { PostView } from '@/lib/api';
import { DEVELOPER_MODE_STORAGE_KEY } from '@/lib/developerMode';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import {
  getActiveColumn,
  openChannelManager,
  openControlCenter,
  openSettingsDrawer,
  renderAtHash,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

const threadRoot: PostView = {
  object_id: 'post-android-back',
  envelope_id: 'envelope-android-back',
  author_pubkey: 'b'.repeat(64),
  author_name: 'bob',
  author_display_name: null,
  following: false,
  followed_by: true,
  mutual: false,
  friend_of_friend: false,
  object_kind: 'post',
  is_threadable: true,
  content: 'android back root post',
  content_status: 'Available',
  attachments: [],
  created_at: 1,
  reply_to: null,
  root_id: 'post-android-back',
  channel_id: null,
  audience_label: 'Public',
};

const renderWithThreadRoot = (hash = '#/timeline?topic=kukuri%3Atopic%3Ageneral') =>
  renderAtHash(hash, createDesktopMockApi({ seedPosts: { 'kukuri:topic:general': [threadRoot] } }));

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
  // desktop では開発者モードで live・game・metaverse が出る。Android ではそれでも出さない。
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'true');
});

afterEach(() => {
  delete window.kukuriAndroid;
  vi.restoreAllMocks();
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

test('back on Android closes the top layer without leaving the screen and keeps the draft', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const historyBack = vi.spyOn(window.history, 'back');

  await user.click(await screen.findByRole('button', { name: /^Post to / }));
  await user.type(screen.getByPlaceholderText('Write a post'), 'draft survives back');
  act(() => handleBackButton(true));
  expect(screen.queryByPlaceholderText('Write a post')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: /^Post to / }));
  expect(screen.getByPlaceholderText('Write a post')).toHaveValue('draft survives back');

  await user.click(screen.getByRole('button', { name: 'Account menu' }));
  await screen.findByRole('menu', { name: 'Account menu' });
  act(() => handleBackButton(true));
  await waitFor(() => {
    expect(screen.queryByRole('menu', { name: 'Account menu' })).not.toBeInTheDocument();
  });

  await openChannelManager(user);
  act(() => handleBackButton(true));
  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Create / Join Private Channel' })).not.toBeInTheDocument();
  });

  await openControlCenter(user);
  act(() => handleBackButton(true));
  await waitFor(() => {
    expect(screen.queryByRole('complementary', { name: 'Control Center' })).not.toBeInTheDocument();
  });
  expect(historyBack).not.toHaveBeenCalled();
});

test('back closes an open composer without focus and keeps its draft', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const historyBack = vi.spyOn(window.history, 'back');
  await user.click(await screen.findByRole('button', { name: /^Post to / }));
  await user.type(screen.getByPlaceholderText('Write a post'), 'unfocused draft survives');
  (document.activeElement as HTMLElement).blur();

  act(() => handleBackButton(true));
  expect(screen.queryByPlaceholderText('Write a post')).not.toBeInTheDocument();
  expect(historyBack).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: /^Post to / }));
  expect(screen.getByPlaceholderText('Write a post')).toHaveValue('unfocused draft survives');
});

test('back on Android closes settings and then the thread through the history', async () => {
  const user = userEvent.setup();
  renderWithThreadRoot();
  await user.click(await screen.findByText('android back root post'));
  await waitFor(() => {
    expect(window.location.hash).toContain('context=thread');
  });
  const controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Keyboard' }));
  await screen.findByRole('dialog', { name: 'Settings' });

  act(() => handleBackButton(true));
  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument();
  });
  expect(window.location.hash).toContain('context=thread');

  act(() => handleBackButton(true));
  await waitFor(() => {
    expect(window.location.hash).not.toContain('context=thread');
  });
});

test('back on Android at the first screen closes a restored thread and then moves the app to the background', async () => {
  const moveTaskToBack = vi.fn();
  window.kukuriAndroid = { moveTaskToBack };
  renderWithThreadRoot('#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-android-back');
  await screen.findByRole('region', { name: /^Thread Column,/ });

  act(() => handleBackButton(false));
  await waitFor(() => {
    expect(window.location.hash).not.toContain('context=thread');
  });
  expect(moveTaskToBack).not.toHaveBeenCalled();

  act(() => handleBackButton(false));
  expect(moveTaskToBack).toHaveBeenCalledOnce();
});
