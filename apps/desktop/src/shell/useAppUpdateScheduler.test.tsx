import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';

import { useAppUpdateScheduler } from './useAppUpdateScheduler';

afterEach(() => {
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
  vi.useRealTimers();
});

test.each(['microsoft-store', 'google-play'] as const)('%s builds never schedule or invoke the self-managed updater', async (distribution) => {
  vi.useFakeTimers();
  Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} });
  const check = vi.fn(async () => undefined);
  const view = renderHook(() => useAppUpdateScheduler(check, distribution));
  await act(async () => { await vi.advanceTimersByTimeAsync(60 * 60 * 1000); });
  expect(check).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
  view.unmount();
});

test('direct builds check immediately, repeat every 30 minutes, and clean up', async () => {
  vi.useFakeTimers();
  Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} });
  const check = vi.fn(async () => undefined);
  const view = renderHook(() => useAppUpdateScheduler(check, 'direct'));
  expect(check).toHaveBeenCalledTimes(1);
  await act(async () => { await vi.advanceTimersByTimeAsync(60 * 60 * 1000); });
  expect(check).toHaveBeenCalledTimes(3);
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});
