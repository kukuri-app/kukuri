import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { DESKTOP_THEME_STORAGE_KEY } from '@/lib/theme';
import { App } from '@/App';
import { appUpdateStore, INITIAL_UPDATE_STATE } from '@/shell/useAppUpdateStore';
import {
  closestSection,
  openChannelManager,
  openControlCenter,
  openPublishDialog,
  openSettingsSection,
  renderAtHash,
  selectTimelineView,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
  // module 単位の store なので、更新状態を test 間へ持ち越さない。
  appUpdateStore.setState({ updateState: INITIAL_UPDATE_STATE, pendingUpdate: null });
});

test('Control Center is the global navigation entry on desktop and narrow viewports', async () => {
  const desktop = render(<App api={createDesktopMockApi()} />);

  expect(await screen.findByTestId('control-center-trigger')).toBeVisible();
  expect(screen.queryByRole('complementary', { name: 'Primary navigation' })).not.toBeInTheDocument();
  expect(screen.queryByTestId('shell-nav-trigger')).not.toBeInTheDocument();

  desktop.unmount();
  setViewportWidth(640);
  render(<App api={createDesktopMockApi()} />);

  expect(await screen.findByTestId('control-center-trigger')).toBeVisible();
  expect(screen.queryByTestId('shell-nav-trigger')).not.toBeInTheDocument();
});

test('tester feedback trigger sits next to the Control Center trigger and opens the dialog', async () => {
  render(<App api={createDesktopMockApi()} />);

  const trigger = await screen.findByTestId('tester-feedback-trigger');
  expect(trigger).toBeVisible();
  expect(trigger.parentElement).toBe(
    screen.getByTestId('control-center-trigger').parentElement
  );

  fireEvent.click(trigger);
  expect(await screen.findByRole('dialog', { name: 'Send feedback' })).toBeInTheDocument();
});

// #1189: 更新があることは Control Center を開かないと分からなかった。更新がある間だけ、
// フィードバックの右から設定の「リリースと更新」へ直接行ける導線を出す。
test('update trigger appears next to the feedback trigger only while an update is available', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const feedback = await screen.findByTestId('tester-feedback-trigger');
  expect(screen.queryByTestId('update-available-trigger')).not.toBeInTheDocument();

  act(() => {
    appUpdateStore.setState({
      updateState: {
        ...INITIAL_UPDATE_STATE,
        status: 'available',
        availableVersion: '0.2.7-preview.1',
      },
    });
  });

  const update = await screen.findByTestId('update-available-trigger');
  expect(update).toBeVisible();
  expect(update).toHaveAccessibleName('Update available');
  expect(update.parentElement).toBe(feedback.parentElement);
  // 「フィードバックを送る」の右、つまり DOM 上で後ろに置く。
  expect(feedback.compareDocumentPosition(update) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

  // Control Center 展開中は cluster ごと隠れる。既存ボタンと同じ扱いにする。
  const controlCenter = await openControlCenter(user);
  expect(screen.queryByTestId('update-available-trigger')).not.toBeInTheDocument();
  fireEvent.keyDown(window, { key: 'Escape' });
  await waitFor(() => {
    expect(controlCenter).not.toBeInTheDocument();
  });

  await user.click(await screen.findByTestId('update-available-trigger'));
  const settings = await screen.findByRole('dialog', { name: 'Settings' });
  expect(within(settings).getByTestId('settings-section-release')).toHaveAttribute(
    'aria-current',
    'location'
  );
  expect(window.location.hash).toContain('settings=release');

  act(() => {
    appUpdateStore.setState({ updateState: INITIAL_UPDATE_STATE });
  });
  await waitFor(() => {
    expect(screen.queryByTestId('update-available-trigger')).not.toBeInTheDocument();
  });
});

test('Control Center exposes Columns, Places, Activity, and System and restores trigger focus', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const trigger = await screen.findByTestId('control-center-trigger');
  expect(trigger).toHaveAccessibleName('Open Control Center · Connected');
  await user.click(trigger);

  const controlCenter = screen.getByRole('complementary', { name: 'Control Center' });
  expect(controlCenter).toBeVisible();
  expect(within(controlCenter).getByRole('heading', { name: 'Columns' })).toBeInTheDocument();
  expect(within(controlCenter).getByRole('heading', { name: 'Places' })).toBeInTheDocument();
  expect(within(controlCenter).getByRole('heading', { name: 'Activity' })).toBeInTheDocument();
  expect(within(controlCenter).getByRole('heading', { name: 'System' })).toBeInTheDocument();
  expect(within(controlCenter).getByRole('button', { name: 'Focus Timeline' })).toHaveAttribute(
    'aria-current',
    'true'
  );
  expect(within(controlCenter).getByRole('button', { name: /^Notifications/ })).toBeInTheDocument();
  expect(within(controlCenter).getByRole('button', { name: 'Messages' })).toBeInTheDocument();
  expect(within(controlCenter).getByRole('button', { name: 'Settings' })).toBeInTheDocument();
  // #964: ショートカットを知らなくても、通常の GUI(コントロールセンター)から案内へ到達できる。
  expect(within(controlCenter).getByRole('button', { name: 'Keyboard' })).toBeInTheDocument();

  fireEvent.keyDown(window, { key: 'Escape' });

  await waitFor(() => {
    expect(screen.queryByRole('complementary', { name: 'Control Center' })).not.toBeInTheDocument();
  });
  expect(trigger).toHaveFocus();
});

test('column actions replace the legacy global floating action button', async () => {
  const user = userEvent.setup();
  const initial = render(<App api={createDesktopMockApi()} />);

  expect(screen.queryByTestId('shell-fab')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: /^Post to / })).toBeInTheDocument();

  initial.unmount();
  const live = renderAtHash('#/live?topic=kukuri%3Atopic%3Ageneral');
  expect(screen.getByRole('button', { name: 'Start Live' })).toBeInTheDocument();

  live.unmount();
  const game = renderAtHash('#/game?topic=kukuri%3Atopic%3Ageneral');
  expect(await screen.findByRole('button', { name: 'Create metaverse room' })).toBeInTheDocument();

  game.unmount();
  const timeline = renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await selectTimelineView(user, 'Bookmarks');
  expect(screen.queryByTestId('shell-fab')).not.toBeInTheDocument();

  timeline.unmount();
  renderAtHash('#/profile?topic=kukuri%3Atopic%3Ageneral');
  expect(screen.queryByTestId('shell-fab')).not.toBeInTheDocument();
});

test('channel manager opens as a modal from Control Center', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const controlCenter = await openControlCenter(user);
  expect(
    within(controlCenter).getByRole('button', { name: 'Create or join a private channel' })
  ).toBeInTheDocument();

  const dialog = await openChannelManager(user);
  expect(dialog).toBeInTheDocument();
  expect(dialog).toHaveAccessibleName('Create / Join Private Channel');
  expect(within(dialog).getByText('Create')).toBeInTheDocument();
  expect(within(dialog).getAllByText('Join').length).toBeGreaterThan(0);
  expect(within(dialog).getByText('Channel name')).toBeInTheDocument();
  expect(within(dialog).getByPlaceholderText('Channel name')).toBeInTheDocument();

  await user.click(within(dialog).getByRole('button', { name: 'Close dialog' }));
  await waitFor(() => {
    expect(
      screen.queryByRole('dialog', { name: 'Create / Join Private Channel' })
    ).not.toBeInTheDocument();
  });
});

test('settings hash route opens the drawer and keeps the selected section in sync', async () => {
  const user = userEvent.setup();
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=discovery');

  const drawer = await screen.findByRole('dialog', { name: 'Settings' });
  await waitFor(() => {
    expect(within(drawer).getByTestId('settings-section-discovery')).toHaveAttribute(
      'aria-current',
      'location'
    );
  });

  await user.click(within(drawer).getByTestId('settings-section-connectivity'));

  await waitFor(() => {
    expect(window.location.hash).toContain('settings=connectivity');
  });
});

test('desktop shell defaults to the dark theme and persists it locally', async () => {
  render(<App api={createDesktopMockApi()} />);

  await waitFor(() => {
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
  });
  expect(window.localStorage.getItem(DESKTOP_THEME_STORAGE_KEY)).toBe('dark');
});

test('desktop shell restores a persisted light theme on boot', async () => {
  window.localStorage.setItem(DESKTOP_THEME_STORAGE_KEY, 'light');

  render(<App api={createDesktopMockApi()} />);

  await waitFor(() => {
    expect(document.documentElement).toHaveAttribute('data-theme', 'light');
  });
});

test('appearance settings deep link updates the document theme and storage immediately', async () => {
  const user = userEvent.setup();
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=appearance');

  const drawer = await screen.findByRole('dialog', { name: 'Settings' });
  await waitFor(() => {
    expect(within(drawer).getByTestId('settings-section-appearance')).toHaveAttribute(
      'aria-current',
      'location'
    );
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
  });

  await user.click(within(drawer).getByRole('radio', { name: /Light/i }));

  await waitFor(() => {
    expect(document.documentElement).toHaveAttribute('data-theme', 'light');
  });
  expect(window.localStorage.getItem(DESKTOP_THEME_STORAGE_KEY)).toBe('light');
});

test('settings drawer removes redundant section copy and duplicate headings', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const drawer = await openSettingsSection(user, 'appearance');

  expect(within(drawer).queryByText('Current section')).not.toBeInTheDocument();
  expect(
    within(drawer).queryByText('Local light, dark, and language selection.')
  ).not.toBeInTheDocument();
  expect(within(drawer).queryByRole('heading', { name: 'Appearance', level: 3 })).not.toBeInTheDocument();
  expect(within(drawer).getByRole('button', { name: 'Close settings' })).toBeInTheDocument();
});

test('language is discoverable in settings and switching preserves the workspace and draft', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  await openPublishDialog(user);
  const composer = await screen.findByPlaceholderText('Write a post');
  await user.type(composer, 'Keep this draft');
  const controlCenter = await openControlCenter(user);
  const settingsButton = within(controlCenter).getByRole('button', { name:'Settings' });
  expect(settingsButton).toHaveAccessibleDescription('Language and theme');
  await user.click(settingsButton);
  const drawer = screen.getByRole('dialog', { name:'Settings' });
  expect(within(drawer).getByRole('button', { name:'Language & theme' })).toHaveAttribute('aria-current', 'location');
  const language = within(drawer).getByRole('combobox', { name:'Language' });
  const theme = within(drawer).getByRole('radiogroup', { name:'Theme mode' });
  expect(language.compareDocumentPosition(theme) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  await user.selectOptions(language, 'ja');
  expect(document.documentElement).toHaveAttribute('lang', 'ja');
  expect(localStorage.getItem('kukuri.desktop.locale')).toBe('ja');
  expect(language).toHaveFocus();
  expect(within(drawer).getByTestId('settings-section-appearance')).toHaveAttribute('aria-current', 'location');
  expect(composer).toHaveValue('Keep this draft');
  expect(composer).toBeInTheDocument();
  await user.click(within(drawer).getByRole('button', { name:'設定を閉じる' }));
  expect(composer).toHaveValue('Keep this draft');
});

test('desktop shell can update discovery seeds', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const setDiscoverySeeds = vi.fn(api.setDiscoverySeeds);
  api.setDiscoverySeeds = setDiscoverySeeds;

  render(<App api={api} />);

  const drawer = await openSettingsSection(user, 'discovery');
  const seedEditor = within(drawer).getByPlaceholderText('node_id or node_id@host:port');
  await user.type(seedEditor, 'seed-peer-1');
  await user.click(within(drawer).getByRole('button', { name: 'Save Seeds' }));

  await waitFor(() => {
    expect(setDiscoverySeeds).toHaveBeenCalledWith(['seed-peer-1']);
  });
  expect(within(drawer).getAllByText('seed-peer-1').length).toBeGreaterThan(0);
});

test('desktop shell surfaces docs-assisted topic recovery in diagnostics', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi({ assistPeerIds: ['relay-peer'] })} />);

  const controlCenter = await openControlCenter(user);
  await user.type(within(controlCenter).getByPlaceholderText('general'), 'kukuri:topic:relay');
  await user.click(within(controlCenter).getByRole('button', { name: 'Add Topic' }));

  const drawer = await openSettingsSection(user, 'discovery');
  await waitFor(() => {
    expect(within(drawer).getByText('Stored-data sync assistance candidates')).toBeInTheDocument();
    expect(within(drawer).getAllByText('relay-peer').length).toBeGreaterThan(0);
  });

  await user.click(within(drawer).getByTestId('settings-section-connectivity'));
  const relayHeading = await within(drawer).findByRole('heading', { name: 'relay' });
  const relaySection = closestSection(relayHeading);
  expect(
    within(relaySection).getByText(
      'Connection details (Diagnostic details: docs-assisted recovery is in progress via 1 peer(s); live topic delivery is unavailable)'
    )
  ).toBeInTheDocument();
});

test('desktop shell renders diagnostics error reasons', async () => {
  const user = userEvent.setup();
  render(
    <App
      api={createDesktopMockApi({
        globalLastError: 'failed to import peer ticket: invalid endpoint id',
        topicLastError: 'timed out waiting for gossip topic join',
      })}
    />
  );

  const drawer = await openSettingsSection(user, 'connectivity');
  await waitFor(() => {
    expect(
      within(drawer).getByText('An error occurred. (Diagnostic details: failed to import peer ticket: invalid endpoint id)')
    ).toBeInTheDocument();
  });

  const topicHeading = await within(drawer).findByRole('heading', { name: 'general' });
  const topicSection = closestSection(topicHeading);
  expect(within(topicSection).getByText('An error occurred. (Diagnostic details: timed out waiting for gossip topic join)')).toBeInTheDocument();
});

test('desktop shell exposes the Timeline Column and settings drawer restores trigger focus on escape', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  expect(screen.queryByRole('tablist', { name: 'Workspaces' })).not.toBeInTheDocument();
  expect(
    screen.getByRole('main', { name: 'Primary workspace' }).querySelector('.shell-workspace-header-card')
  ).toBeNull();
  const timelineColumn = screen.getByRole('region', { name: /Timeline Column/ });
  expect(timelineColumn).toHaveAttribute('aria-current', 'true');
  const columnHeader = timelineColumn.querySelector('.shell-column-header');
  expect(columnHeader).not.toBeNull();
  const timelineViews = within(columnHeader as HTMLElement).getByRole('tablist', {
    name: 'Timeline views',
  });
  const feedTab = within(timelineViews).getByRole('tab', { name: 'Feed' });
  const bookmarksTab = within(timelineViews).getByRole('tab', { name: 'Bookmarks' });
  expect(feedTab).toHaveAttribute('aria-label', 'Feed');
  expect(feedTab).not.toHaveTextContent('Feed');
  expect(feedTab.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
  expect(bookmarksTab).toHaveAttribute('aria-label', 'Bookmarks');
  expect(bookmarksTab).not.toHaveTextContent('Bookmarks');
  expect(bookmarksTab.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
  expect(feedTab).toHaveAttribute('tabindex', '0');
  expect(bookmarksTab).toHaveAttribute('tabindex', '-1');
  feedTab.focus();
  fireEvent.keyDown(feedTab, { key: 'ArrowRight' });
  expect(bookmarksTab).toHaveFocus();
  expect(bookmarksTab).toHaveAttribute('aria-selected', 'true');
  expect(timelineColumn.querySelector('.shell-column-body .shell-workspace-tabs')).toBeNull();

  const settingsTrigger = screen.getByTestId('control-center-trigger');
  const controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Settings' }));
  await screen.findByRole('dialog', { name: 'Settings' });

  fireEvent.keyDown(window, { key: 'Escape' });

  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument();
  });
  await waitFor(() => {
    expect(settingsTrigger).toHaveFocus();
  });
});

// #964: 案内はコントロールセンターと投稿作成の hint から開け、開いても下書きと投稿作成は残る。
test('keyboard guidance opens from the Control Center and from the composer hint without losing the draft', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Keyboard' }));
  const drawer = await screen.findByRole('dialog', { name: 'Settings' });
  expect(within(drawer).getByTestId('settings-section-keyboard')).toHaveAttribute('aria-current', 'location');
  expect(within(drawer).getByRole('heading', { name: 'Keyboard' })).toBeVisible();
  expect(window.location.hash).toContain('settings=keyboard');
  await user.click(within(drawer).getByRole('button', { name: 'Close settings' }));
  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument();
  });

  await user.click(screen.getByRole('button', { name: /^Post to / }));
  const composerInput = screen.getByPlaceholderText('Write a post');
  await user.type(composerInput, 'draft survives help');
  await user.click(screen.getByRole('button', { name: 'Keyboard shortcuts' }));
  const reopened = await screen.findByRole('dialog', { name: 'Settings' });
  expect(within(reopened).getByRole('heading', { name: 'Keyboard' })).toBeVisible();
  await user.keyboard('{Escape}');
  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument();
  });
  expect(screen.getByPlaceholderText('Write a post')).toHaveValue('draft survives help');
});
