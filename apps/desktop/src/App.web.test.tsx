import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import type { RuntimeEvent } from '@/lib/api';

// Web の build（#1217 W4、ADR 0059）: command と event を web-runtime の代わりに試験の mock へ向ける。
const { invokeMock, listeners } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listeners: new Set<(event: RuntimeEvent) => void>(),
}));
vi.mock('@/lib/webRuntime', () => ({
  IS_WEB_RUNTIME: true,
  invokeWebRuntime: invokeMock,
  listenWebRuntimeEvents: (listener: (event: RuntimeEvent) => void) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
}));

import { App } from '@/App';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { columnIdentityId } from '@/shell/slices/workspace';
import { WORKSPACE_LAYOUT_STORAGE_KEY } from '@/shell/workspacePersistence';

const SCOPE = { topicId: 'kukuri:topic:general', channelId: null };

// runtime を動かす tab は、shell の command を mock の API で受ける（起動の状態は mock の間 ready）。
function runInThisTab() {
  window.__KUKURI_DESKTOP__ = createDesktopMockApi();
  return Promise.resolve({ status: 'ready' });
}

const inUseElsewhere = () => Promise.resolve({ status: 'in_use_elsewhere' });

// 規約の改定の後の再同意（年齢の申告は済み）。
const renewedConsentRequired = {
  status: 'consent_required',
  documents: ['terms', 'privacy'].map((slug) => ({
    slug, currentVersion: 9, effectiveDate: '2026-10-07', authoritativeLanguage: 'ja', materialChange: true,
    controllerName: 'Preview Distributor', contact: 'privacy@example.test',
    acceptedVersion: 8, acceptedAt: 1_700_000_000, acceptedLanguage: 'en', acceptedAppVersion: '0.4.0',
  })),
  age_attestation: { currentVersion: 1, attestedVersion: 1, attestedAt: 1_700_000_000 },
};

// web-runtime は起動の状態の読取りと引継ぎだけに答える。shell の command は返らない（mock の API が居る間は、そちらが受ける）。
function webRuntime(status: () => Promise<unknown>, takeOver: () => Promise<unknown>) {
  invokeMock.mockImplementation((command: string) => {
    if (command === 'get_desktop_startup_status') return status();
    if (command === 'take_over_runtime') return takeOver();
    return new Promise(() => {});
  });
}

// ブラウザの保存の永続化（ADR 0059 §6）。既定は許可済み。
const storage = { persisted: vi.fn<() => Promise<boolean>>(), persist: vi.fn<() => Promise<boolean>>() };

beforeEach(() => {
  invokeMock.mockReset();
  listeners.clear();
  delete window.__KUKURI_DESKTOP__;
  window.history.replaceState(null, '', '/');
  storage.persisted.mockReset().mockResolvedValue(true);
  storage.persist.mockReset().mockResolvedValue(true);
  Object.defineProperty(navigator, 'storage', { configurable: true, value: storage });
});

// #1217 AC-5: runtime が使える状態になるたびに、まだ許可されていなければ保存の永続化を求める。許可済みなら求めない。
test.each([[false, 1], [true, 0]])('a ready runtime asks the browser to keep the site data unless it already does (persisted: %s)', async (persisted, requests) => {
  storage.persisted.mockResolvedValue(persisted);
  webRuntime(runInThisTab, runInThisTab);
  render(<App />);
  expect(await screen.findByTestId('control-center-trigger')).toBeInTheDocument();
  await waitFor(() => expect(storage.persisted).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(storage.persist).toHaveBeenCalledTimes(requests));
}, 20000);

test('a tab opened while another tab runs kukuri takes it over only on request', async () => {
  const user = userEvent.setup();
  const takeOver = vi.fn<() => Promise<unknown>>().mockRejectedValueOnce(new Error('lock request failed'));
  webRuntime(inUseElsewhere, takeOver);
  render(<App />);
  expect(await screen.findByRole('heading', { name: 'kukuri is open in another tab' })).toBeVisible();
  expect(takeOver).not.toHaveBeenCalled();

  // 引き継げなければ、同じ画面のまま、もう一度試せる。
  await user.click(screen.getByRole('button', { name: 'Use in this tab' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Could not switch to this tab. Please try again.');
  expect(screen.getByRole('button', { name: 'Use in this tab' })).toBeEnabled();

  // 引継ぎの間は、処理中であることを示して重ねて押せない。
  let finish!: () => void;
  takeOver.mockImplementation(() => new Promise((resolve) => { finish = () => resolve(runInThisTab()); }));
  await user.click(screen.getByRole('button', { name: 'Use in this tab' }));
  const busy = await screen.findByRole('button', { name: 'Switching to this tab…' });
  expect(busy).toBeDisabled();
  expect(busy).toHaveAttribute('aria-busy', 'true');
  finish();
  expect(await screen.findByTestId('control-center-trigger')).toBeInTheDocument();
  expect(takeOver).toHaveBeenCalledTimes(2);
});

test('a tab whose runtime is taken over moves to the in-use screen and continues from the layout the other tab left', async () => {
  const user = userEvent.setup();
  webRuntime(runInThisTab, runInThisTab);
  render(<App />);
  expect(await screen.findByTestId('control-center-trigger')).toBeInTheDocument();

  // 別の tab が引き継ぐと、この tab の runtime は止まり、web-runtime が起動の状態の変化を知らせる。
  delete window.__KUKURI_DESKTOP__;
  webRuntime(inUseElsewhere, runInThisTab);
  act(() => listeners.forEach((listener) => listener({ type: 'startup_status_changed' })));
  expect(await screen.findByRole('heading', { name: 'kukuri is open in another tab' })).toBeVisible();
  expect(screen.queryByTestId('control-center-trigger')).not.toBeInTheDocument();

  // 別の tab が使う間に Column を並べ替えて保存した。
  const columns = ['timeline', 'notifications'] as const;
  window.localStorage.setItem(WORKSPACE_LAYOUT_STORAGE_KEY, JSON.stringify({
    version: 1,
    activeColumnId: columnIdentityId('timeline', SCOPE),
    columns: columns.map((kind) => ({
      id: columnIdentityId(kind, SCOPE), kind, scope: SCOPE, pinned: true, preferredDesktopSpan: 1,
    })),
  }));

  await user.click(screen.getByRole('button', { name: 'Use in this tab' }));
  const shown = await screen.findAllByRole('region', { name: / Column,/ });
  expect(shown.map((column) => column.getAttribute('data-column-id'))).toEqual(
    columns.map((kind) => columnIdentityId(kind, SCOPE))
  );
}, 20000);

// ADR 0059 §1: 同意・引継ぎの途中で保存先の版を更新する間も、その画面のままにせず、起動の画面で「データの移行中です」を
// 出してからレイアウトを出す（web-runtime は版の更新の始まりと終わりに起動の状態の変化を知らせる）。
test.each([
  ['the renewed consent', renewedConsentRequired, 'accept_app_consents', 'Accept and continue'],
  ['a take-over', { status: 'in_use_elsewhere' }, 'take_over_runtime', 'Use in this tab'],
] as const)('a storage upgrade during %s shows the migration screen before the layout', async (_, initial, operation, button) => {
  const user = userEvent.setup();
  let status: unknown = initial;
  let finish!: () => void;
  invokeMock.mockImplementation((command: string) => {
    if (command === 'get_desktop_startup_status') return Promise.resolve(status);
    if (command !== operation) return new Promise(() => {});
    status = { status: 'migrating' };
    listeners.forEach((listener) => listener({ type: 'startup_status_changed' }));
    return new Promise((resolve) => { finish = () => resolve(runInThisTab()); });
  });
  render(<App />);
  await user.click(await screen.findByRole('button', { name: button }));
  expect(await screen.findByText('Migrating your data…')).toBeVisible();
  expect(screen.queryByRole('button', { name: button })).not.toBeInTheDocument();
  expect(screen.queryByTestId('control-center-trigger')).not.toBeInTheDocument();
  finish();
  expect(await screen.findByTestId('control-center-trigger')).toBeInTheDocument();
}, 20000);
