import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

const { updaterCheck } = vi.hoisted(() => ({ updaterCheck: vi.fn() }));
vi.mock('@/lib/appUpdater', () => ({ check: updaterCheck }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: async () => '0.2.1' }));
vi.mock('@/lib/releaseReadiness', async (original) => ({
  ...(await original<typeof import('@/lib/releaseReadiness')>()),
  isTauriRuntime: () => true,
}));
vi.mock('@/lib/api/osNotificationPermission', () => ({
  getOsNotificationPermission: async () => 'available',
  requestOsNotificationPermission: async () => 'available',
}));

import i18n from '@/i18n';
import type { DesktopDistribution } from '@/lib/distribution';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';
import { appUpdateStore, INITIAL_UPDATE_STATE } from '@/shell/useAppUpdateStore';

import { ReleasePanel } from './ReleasePanel';

const initialStore = appUpdateStore.getState();
const check = vi.fn(async () => undefined);
const restart = vi.fn(async () => undefined);
const download = vi.fn(async () => undefined);

beforeEach(async () => {
  await i18n.changeLanguage('en');
  vi.clearAllMocks();
  updaterCheck.mockReset();
  appUpdateStore.setState({
    ...initialStore,
    updateState: { ...INITIAL_UPDATE_STATE },
    pendingUpdate: null,
    checkForUpdate: check,
    downloadUpdate: download,
    restartAndInstall: restart,
  });
});

afterEach(async () => {
  appUpdateStore.setState(initialStore);
  await i18n.changeLanguage('en');
});

function findEnabledCheckButton() {
  return screen.findByRole(
    'button',
    { name: i18n.t('settings:release.update.check') },
    { timeout: 2000 }
  );
}

function renderPanel(showDiagnostics = false, distribution?: DesktopDistribution) {
  return render(
    <DesktopShellStoreContext.Provider value={createDesktopShellStore()}>
      <ReleasePanel showDiagnostics={showDiagnostics} distribution={distribution} />
    </DesktopShellStoreContext.Provider>
  );
}

// 更新を Store が管理する build（Microsoft Store 版、#1199 AC-3 の Google Play 版）。文言は Store ごと。
const STORE_BUILDS = [
  { distribution: 'microsoft-store', copy: 'store', otherStore: 'Google Play' },
  { distribution: 'google-play', copy: 'play', otherStore: 'Microsoft Store' },
] as const;

test.each(
  STORE_BUILDS.flatMap((build) => ['en', 'ja', 'zh-CN'].map((locale) => ({ ...build, locale })))
)('$distribution builds delegate updates without invoking the GitHub updater in $locale', async ({ distribution, copy, otherStore, locale }) => {
  await i18n.changeLanguage(locale);
  renderPanel(true, distribution);
  expect(screen.getByText(i18n.t(`settings:release.${copy}Summary`))).toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent(
    i18n.t(`settings:release.update.${copy}Managed`)
  );
  expect(screen.getByText(i18n.t(`settings:release.update.${copy}ManagedStatus`))).toBeInTheDocument();
  expect(document.body).not.toHaveTextContent(otherStore);
  expect(screen.queryByRole('button', { name: i18n.t('settings:release.update.check') })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('settings:release.update.install') })).not.toBeInTheDocument();
  expect(screen.queryByText(i18n.t('settings:release.externalTransmission.updateDestination'))).not.toBeInTheDocument();
  expect(check).not.toHaveBeenCalled();
  expect(download).not.toHaveBeenCalled();
  expect(updaterCheck).not.toHaveBeenCalled();
});

test.each(['en', 'ja', 'zh-CN'])('normal mode reports a pending check and its completed result in %s', async (locale) => {
  await i18n.changeLanguage(locale);
  let finish!: (value: null) => void;
  updaterCheck.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  appUpdateStore.setState({ checkForUpdate: initialStore.checkForUpdate });
  const first = renderPanel();
  expect(screen.queryByText(i18n.t('settings:release.update.statuses.up_to_date'))).not.toBeInTheDocument();
  const button = screen.getByRole('button', { name: i18n.t('settings:release.update.check') });
  fireEvent.click(button);
  await waitFor(() => expect(updaterCheck).toHaveBeenCalledTimes(1));
  expect(button).toBeDisabled();
  expect(button).toHaveAttribute('aria-busy', 'true');
  expect(button).toHaveAccessibleName(i18n.t('settings:release.update.statuses.checking'));
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('settings:release.update.statuses.checking'));
  fireEvent.click(button);
  await appUpdateStore.getState().checkForUpdate();
  expect(updaterCheck).toHaveBeenCalledTimes(1);
  first.unmount();
  await act(async () => { finish(null); });
  renderPanel();
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('settings:release.update.statuses.up_to_date'));
  expect(screen.getByRole('button', { name: i18n.t('settings:release.update.check') })).toBeEnabled();
  expect(updaterCheck).toHaveBeenCalledTimes(1);
  expect(download).not.toHaveBeenCalled();
  expect(restart).not.toHaveBeenCalled();
});

test.each([false, true])('failed checks always explain recovery and clear the error on retry (diagnostics: %s)', async (diagnostics) => {
  updaterCheck.mockRejectedValueOnce(new Error('')).mockResolvedValueOnce(null);
  appUpdateStore.setState({ checkForUpdate: initialStore.checkForUpdate });
  renderPanel(diagnostics);
  fireEvent.click(screen.getByRole('button', { name: i18n.t('settings:release.update.check') }));
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent(i18n.t('settings:release.update.errors.unknown'));
  // AC-7: the immediate rejection still keeps the button acknowledged for one second.
  fireEvent.click(await findEnabledCheckButton());
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(i18n.t('settings:release.update.statuses.up_to_date')));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(updaterCheck).toHaveBeenCalledTimes(2);
});

test('available updates announce the version without downloading or exposing diagnostics', async () => {
  updaterCheck.mockResolvedValue({ version: '0.2.2', download, install: restart });
  appUpdateStore.setState({ checkForUpdate: initialStore.checkForUpdate });
  renderPanel();
  fireEvent.click(screen.getByRole('button', { name: i18n.t('settings:release.update.check') }));
  const status = await screen.findByRole('status');
  await waitFor(() => expect(status).toHaveTextContent(i18n.t('settings:release.update.available', { version: '0.2.2' })));
  expect(within(status).queryByText('latest-preview.json')).not.toBeInTheDocument();
  expect(screen.queryByText(i18n.t('settings:release.update.manifest'))).not.toBeInTheDocument();
  expect(download).not.toHaveBeenCalled();
  expect(restart).not.toHaveBeenCalled();
});

test('rechecking identifies the previous version as a previous result', async () => {
  updaterCheck.mockResolvedValueOnce({ version: '0.2.2', download, install: restart });
  appUpdateStore.setState({ checkForUpdate: initialStore.checkForUpdate });
  renderPanel();
  await act(async () => { await appUpdateStore.getState().checkForUpdate(); });
  let finish!: (value: null) => void;
  updaterCheck.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  fireEvent.click(await findEnabledCheckButton());
  await waitFor(() => expect(updaterCheck).toHaveBeenCalledTimes(2));
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('settings:release.update.previouslyAvailable', { version: '0.2.2' }));
  expect(screen.queryByText(i18n.t('settings:release.update.available', { version: '0.2.2' }))).not.toBeInTheDocument();
  await act(async () => { finish(null); });
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('settings:release.update.statuses.up_to_date'));
  expect(screen.getByRole('status')).not.toHaveTextContent('0.2.2');
});

test.each(['en', 'ja', 'zh-CN'])('Deb cancellation and partial failure have distinct recovery guidance in %s', async (locale) => {
  await i18n.changeLanguage(locale);
  for (const [code, key] of [
    ['deb_update_auth_cancelled', 'debCancelled'],
    ['deb_update_auth_unavailable', 'debAuthorization'],
    ['deb_update_install_failed', 'debInstall'],
  ]) {
    appUpdateStore.setState({ updateState: { ...INITIAL_UPDATE_STATE, status: 'failed', lastError: code } });
    const view = renderPanel();
    expect(screen.getByText(i18n.t(`settings:release.update.errors.${key}`))).toBeInTheDocument();
    expect(restart).not.toHaveBeenCalled();
    expect(download).not.toHaveBeenCalled();
    view.unmount();
  }
});

test('a verified update keeps its apply action after deferring and reopening the panel', () => {
  appUpdateStore.setState({
    updateState: { ...INITIAL_UPDATE_STATE, status: 'ready_to_restart', availableVersion: '0.1.9' },
    pendingUpdate: { version: '0.1.9', download, install: vi.fn() },
  });
  const first = renderPanel();
  const checkButton = screen.getByRole('button', { name: i18n.t('settings:release.update.check') });
  expect(checkButton).toBeDisabled();
  fireEvent.click(checkButton);
  fireEvent.click(screen.getByRole('button', { name: i18n.t('settings:release.update.later') }));
  expect(screen.getByRole('button', { name: i18n.t('settings:release.update.restartNow') })).toBeEnabled();
  expect(appUpdateStore.getState().updateState.status).toBe('ready_to_restart');
  expect(check).not.toHaveBeenCalled();
  expect(download).not.toHaveBeenCalled();
  expect(restart).not.toHaveBeenCalled();
  first.unmount();
  renderPanel();
  expect(screen.getByRole('button', { name: i18n.t('settings:release.update.check') })).toBeDisabled();
  fireEvent.click(screen.getAllByRole('button', { name: i18n.t('settings:release.update.restartNow') })[0]);
  expect(restart).toHaveBeenCalledTimes(1);
});

test.each([
  ['en', 'The update file was not found on the server. No update was installed. Try Check again later.'],
  ['ja', '配布先に更新ファイルが見つかりません。更新は適用されていません。時間を置いて「確認」からやり直してください。'],
  ['zh-CN', '更新服务器上未找到更新文件。更新未安装。请稍后重新点击“检查”。'],
])('a missing asset has localized recovery guidance in %s', async (locale, message) => {
  await i18n.changeLanguage(locale);
  appUpdateStore.setState({
    updateState: {
      ...INITIAL_UPDATE_STATE,
      status: 'failed',
      lastError: "'Download request failed with status: 404 Not Found'",
    },
  });
  renderPanel();
  expect(screen.getByText(message)).toBeInTheDocument();
  expect(screen.queryByText(/Download request failed/)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: i18n.t('settings:release.update.check') }));
  expect(check).toHaveBeenCalledTimes(1);
  expect(download).not.toHaveBeenCalled();
  expect(restart).not.toHaveBeenCalled();
});

// #956 Reopen: 高速完了でも「この確認が完了した」ことが分かる（AC-6）。
function expectedClockTime(locale: string, at: number): string {
  return new Intl.DateTimeFormat(locale, { hour: 'numeric', minute: '2-digit', second: '2-digit' }).format(at);
}

test.each(['en', 'ja', 'zh-CN'])('an immediate up-to-date result shows when this check completed in %s', async (locale) => {
  await i18n.changeLanguage(locale);
  vi.useFakeTimers({ toFake: ['Date'] });
  const startup = Date.UTC(2026, 8, 12, 3, 4, 0);
  vi.setSystemTime(startup);
  updaterCheck.mockResolvedValue(null);
  appUpdateStore.setState({ checkForUpdate: initialStore.checkForUpdate });
  renderPanel();
  await act(async () => { await appUpdateStore.getState().checkForUpdate(); });
  const status = screen.getByRole('status');
  expect(status).toHaveTextContent(i18n.t('settings:release.update.statuses.up_to_date'));
  expect(status).toHaveTextContent(
    i18n.t('settings:release.update.checkedAt', { time: expectedClockTime(locale, startup) })
  );
  const before = status.textContent;
  const manual = startup + 5 * 60_000;
  vi.setSystemTime(manual);
  fireEvent.click(await findEnabledCheckButton());
  await waitFor(() => expect(updaterCheck).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(status).toHaveTextContent(expectedClockTime(locale, manual)));
  expect(status).toHaveTextContent(i18n.t('settings:release.update.statuses.up_to_date'));
  expect(status).not.toHaveTextContent(expectedClockTime(locale, startup));
  expect(status.textContent).not.toBe(before);
  expect(download).not.toHaveBeenCalled();
  expect(restart).not.toHaveBeenCalled();
});

test('a failed check also shows when it completed without exposing the raw error', async () => {
  vi.useFakeTimers({ toFake: ['Date'] });
  const at = Date.UTC(2026, 8, 12, 3, 4, 0);
  vi.setSystemTime(at);
  updaterCheck.mockRejectedValueOnce(new Error('network unavailable: fixture.internal'));
  appUpdateStore.setState({ checkForUpdate: initialStore.checkForUpdate });
  renderPanel();
  fireEvent.click(screen.getByRole('button', { name: i18n.t('settings:release.update.check') }));
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent(i18n.t('settings:release.update.errors.network'));
  expect(alert).toHaveTextContent(
    i18n.t('settings:release.update.checkedAt', { time: expectedClockTime('en', at) })
  );
  expect(alert).not.toHaveTextContent('fixture.internal');
});

// #956 Reopen: 確認開始から最低1秒はpendingを保ち、結果の反映は遅らせない（AC-7）。
test('a fast check keeps the check button pending for one second without delaying the result', async () => {
  vi.useFakeTimers();
  updaterCheck.mockImplementation(
    () => new Promise((resolve) => { setTimeout(() => resolve(null), 50); })
  );
  appUpdateStore.setState({ checkForUpdate: initialStore.checkForUpdate });
  renderPanel();
  const button = screen.getByRole('button', { name: i18n.t('settings:release.update.check') });
  fireEvent.click(button);
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('settings:release.update.statuses.up_to_date'));
  expect(button).toBeDisabled();
  expect(button).toHaveAttribute('aria-busy', 'true');
  expect(button).toHaveAccessibleName(i18n.t('settings:release.update.statuses.checking'));
  fireEvent.click(button);
  await act(async () => { await vi.advanceTimersByTimeAsync(899); });
  expect(button).toHaveAttribute('aria-busy', 'true');
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  expect(button).toBeEnabled();
  expect(button).toHaveAccessibleName(i18n.t('settings:release.update.check'));
  expect(updaterCheck).toHaveBeenCalledTimes(1);
  expect(download).not.toHaveBeenCalled();
});

test('a slow check stays pending beyond one second until the store finishes', async () => {
  vi.useFakeTimers();
  let finish!: (value: null) => void;
  updaterCheck.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  appUpdateStore.setState({ checkForUpdate: initialStore.checkForUpdate });
  const view = renderPanel();
  const button = screen.getByRole('button', { name: i18n.t('settings:release.update.check') });
  fireEvent.click(button);
  await act(async () => { await vi.advanceTimersByTimeAsync(1500); });
  expect(button).toHaveAttribute('aria-busy', 'true');
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('settings:release.update.statuses.checking'));
  await act(async () => { finish(null); });
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('settings:release.update.statuses.up_to_date'));
  expect(button).toBeEnabled();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});
