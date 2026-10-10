import { renderHook } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';
import type { NotificationView } from '@/lib/api';

const native = vi.hoisted(() => ({ listen: vi.fn(), onOpenUrl: vi.fn(), getCurrent: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));
vi.mock('@tauri-apps/plugin-deep-link', () => native);
import { useOsNotificationActivation } from './useOsNotificationActivation';

const target = { notification_id: 'received-0', kind: 'reply', actor_pubkey: 'actor',
  created_at: 0, received_at: 0 } as NotificationView;
let onUrl: (urls: string[]) => void;
let onEvent: (event: { payload: { notification_id: string } }) => void;
const dispose = vi.fn();
const api = { getNotification: vi.fn() };

beforeEach(() => {
  sessionStorage.clear();
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  dispose.mockReset();
  native.listen.mockReset().mockImplementation(async (_, callback) => { onEvent = callback; return dispose; });
  native.onOpenUrl.mockReset().mockImplementation(async (callback) => { onUrl = callback; return dispose; });
  native.getCurrent.mockReset().mockResolvedValue(null);
  api.getNotification.mockReset().mockResolvedValue(target);
});

test.each(['kukuri://notification?id=', 'kukuri://notification/?id='])('cold/warm tap resolves one ID outside the latest inbox page: %s', async (prefix) => {
  native.getCurrent.mockResolvedValue([prefix + target.notification_id]);
  const activate = vi.fn();
  renderHook(() => useOsNotificationActivation(api, 'account', activate));
  await vi.waitFor(() => expect(activate).toHaveBeenCalledExactlyOnceWith(target));
  expect(api.getNotification).toHaveBeenCalledExactlyOnceWith(target.notification_id);
  onUrl([prefix + target.notification_id]);
  await vi.waitFor(() => expect(activate).toHaveBeenCalledTimes(2));
});

test('native activation uses the same single-ID lookup', async () => {
  const activate = vi.fn();
  renderHook(() => useOsNotificationActivation(api, 'account', activate));
  await vi.waitFor(() => expect(native.listen).toHaveBeenCalled());
  onEvent({ payload: { notification_id: target.notification_id } });
  await vi.waitFor(() => expect(activate).toHaveBeenCalledExactlyOnceWith(target));
});

test('new live URI supersedes an older launch URI and does not replay on remount', async () => {
  let initial!: (urls: string[]) => void;
  native.getCurrent.mockReturnValue(new Promise((resolve) => { initial = resolve; }));
  const activate = vi.fn();
  const first = renderHook(() => useOsNotificationActivation(api, 'account', activate));
  await vi.waitFor(() => expect(native.getCurrent).toHaveBeenCalled());
  onUrl(['kukuri://notification?id=received-0']);
  initial(['kukuri://notification?id=older']);
  await vi.waitFor(() => expect(api.getNotification).toHaveBeenCalledExactlyOnceWith('received-0'));
  first.unmount();
  native.getCurrent.mockResolvedValue(['kukuri://notification?id=received-0']);
  renderHook(() => useOsNotificationActivation(api, 'account', activate));
  await vi.waitFor(() => expect(native.getCurrent).toHaveBeenCalledTimes(2));
  expect(api.getNotification).toHaveBeenCalledTimes(1);
});

test.each(['null', 'error'])('missing/ineligible notification or lookup %s does not navigate', async (result) => {
  if (result === 'null') api.getNotification.mockResolvedValue(null);
  else api.getNotification.mockRejectedValue(new Error('unavailable'));
  const activate = vi.fn();
  renderHook(() => useOsNotificationActivation(api, 'account', activate));
  await vi.waitFor(() => expect(native.onOpenUrl).toHaveBeenCalled());
  onUrl(['kukuri://notification?id=unknown']);
  await vi.waitFor(() => expect(api.getNotification).toHaveBeenCalledTimes(1));
  await Promise.resolve();
  expect(activate).not.toHaveBeenCalled();
});

test('new native activation supersedes an older launch URI', async () => {
  api.getNotification.mockImplementation(async (id: string) => ({ ...target, notification_id: id }));
  let initial!: (urls: string[]) => void;
  native.getCurrent.mockReturnValue(new Promise((resolve) => { initial = resolve; }));
  const activate = vi.fn();
  renderHook(() => useOsNotificationActivation(api, 'account', activate));
  await vi.waitFor(() => expect(native.getCurrent).toHaveBeenCalled());
  onEvent({ payload: { notification_id: 'received-0' } });
  initial(['kukuri://notification?id=older']);
  await vi.waitFor(() => expect(activate).toHaveBeenCalledExactlyOnceWith(target));
  expect(api.getNotification).toHaveBeenCalledExactlyOnceWith('received-0');
});

test('malformed link and non-ready/non-native context do not look up a notification', async () => {
  const first = renderHook(() => useOsNotificationActivation(api, null, vi.fn()));
  expect(native.listen).not.toHaveBeenCalled();
  first.unmount();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  const second = renderHook(() => useOsNotificationActivation(api, 'account', vi.fn()));
  expect(native.listen).not.toHaveBeenCalled();
  second.unmount();
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  renderHook(() => useOsNotificationActivation(api, 'account', vi.fn()));
  await vi.waitFor(() => expect(native.onOpenUrl).toHaveBeenCalled());
  onUrl(['kukuri://notification?id=received-0&profile=other']);
  expect(api.getNotification).not.toHaveBeenCalled();
});

test('latest click wins and uses the current navigation callback', async () => {
  let older!: (value: NotificationView) => void;
  api.getNotification.mockImplementationOnce(() => new Promise((resolve) => { older = resolve; }));
  const previous = vi.fn(), latest = vi.fn();
  const hook = renderHook(({ callback }) => useOsNotificationActivation(api, 'account', callback),
    { initialProps: { callback: previous } });
  await vi.waitFor(() => expect(native.onOpenUrl).toHaveBeenCalled());
  onUrl(['kukuri://notification?id=older']);
  hook.rerender({ callback: latest });
  onUrl(['kukuri://notification?id=received-0']);
  await vi.waitFor(() => expect(latest).toHaveBeenCalledExactlyOnceWith(target));
  older({ ...target, notification_id: 'older' });
  await Promise.resolve();
  expect(previous).not.toHaveBeenCalled();
  expect(latest).toHaveBeenCalledTimes(1);
});

test.each(['switch', 'unmount'])('lookup finishing after %s does not navigate', async (end) => {
  let resolve!: (value: NotificationView) => void;
  api.getNotification.mockReturnValue(new Promise((done) => { resolve = done; }));
  const activate = vi.fn();
  const hook = renderHook(({ account }) => useOsNotificationActivation(api, account, activate),
    { initialProps: { account: 'old' } });
  await vi.waitFor(() => expect(native.onOpenUrl).toHaveBeenCalled());
  const oldUrl = onUrl;
  oldUrl(['kukuri://notification?id=received-0']);
  if (end === 'switch') hook.rerender({ account: 'new' }); else hook.unmount();
  resolve(target);
  oldUrl(['kukuri://notification?id=received-0']);
  await Promise.resolve();
  expect(activate).not.toHaveBeenCalled();
  expect(api.getNotification).toHaveBeenCalledTimes(1);
});

test('listener registration completed after unmount is disposed', async () => {
  let finish!: (callback: () => void) => void;
  native.onOpenUrl.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const hook = renderHook(() => useOsNotificationActivation(api, 'account', vi.fn()));
  await vi.waitFor(() => expect(native.onOpenUrl).toHaveBeenCalled());
  hook.unmount();
  finish(dispose);
  await vi.waitFor(() => expect(dispose).toHaveBeenCalledTimes(2));
  expect(native.getCurrent).not.toHaveBeenCalled();
});
