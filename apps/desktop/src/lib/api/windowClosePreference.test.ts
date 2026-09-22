import { beforeEach, expect, test, vi } from 'vitest';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

import {
  getPendingWindowCloseRequest,
  getWindowClosePreference,
  respondWindowCloseRequest,
  setWindowClosePreference,
} from './windowClosePreference';

beforeEach(() => {
  delete window.__KUKURI_DESKTOP__;
  invokeMock.mockReset();
});

test('uses the registered preference and pending-request command contracts', async () => {
  invokeMock.mockResolvedValueOnce({ behavior: null });
  await expect(getWindowClosePreference()).resolves.toEqual({ behavior: null });
  expect(invokeMock).toHaveBeenLastCalledWith('get_window_close_preference', undefined);

  invokeMock.mockResolvedValueOnce({ behavior: 'tray' });
  await expect(setWindowClosePreference({ behavior: 'tray' })).resolves.toEqual({
    behavior: 'tray',
  });
  expect(invokeMock).toHaveBeenLastCalledWith('set_window_close_preference', {
    preference: { behavior: 'tray' },
  });

  invokeMock.mockResolvedValueOnce({ request_id: 12 });
  await expect(getPendingWindowCloseRequest()).resolves.toEqual({ request_id: 12 });
  expect(invokeMock).toHaveBeenLastCalledWith('get_pending_window_close_request', undefined);

  invokeMock.mockResolvedValueOnce(undefined);
  await respondWindowCloseRequest(12, 'quit', true);
  expect(invokeMock).toHaveBeenLastCalledWith('respond_window_close_request', {
    requestId: 12,
    behavior: 'quit',
    remember: true,
  });
});
