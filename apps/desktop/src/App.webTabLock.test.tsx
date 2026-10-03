import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import type { RuntimeEvent } from '@/lib/api';

// Web の build（#1217 W4 AC-4、ADR 0059 §4）: command と event を web-runtime の代わりに試験の mock へ向ける。
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

// web-runtime は起動の状態の読取りと引継ぎだけに答える。shell の command は返らない（mock の API が居る間は、そちらが受ける）。
function webRuntime(status: () => Promise<unknown>, takeOver: () => Promise<unknown>) {
  invokeMock.mockImplementation((command: string) => {
    if (command === 'get_desktop_startup_status') return status();
    if (command === 'take_over_runtime') return takeOver();
    return new Promise(() => {});
  });
}

beforeEach(() => {
  invokeMock.mockReset();
  listeners.clear();
  delete window.__KUKURI_DESKTOP__;
  window.history.replaceState(null, '', '/');
});

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
