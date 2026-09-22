import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

const { listenMock, pendingMock, respondMock } = vi.hoisted(() => ({
  listenMock: vi.fn(),
  pendingMock: vi.fn(),
  respondMock: vi.fn(),
}));

let closeListener: ((event: { payload: { request_id: number } }) => void) | undefined;

vi.mock('@tauri-apps/api/event', () => ({
  listen: listenMock,
}));
vi.mock('@/lib/releaseReadiness', () => ({
  isTauriRuntime: () => true,
}));
vi.mock('@/lib/api/windowClosePreference', () => ({
  getPendingWindowCloseRequest: pendingMock,
  respondWindowCloseRequest: respondMock,
}));

import { WindowClosePrompt } from './WindowClosePrompt';

beforeEach(() => {
  closeListener = undefined;
  listenMock.mockReset();
  pendingMock.mockReset();
  respondMock.mockReset();
  pendingMock.mockResolvedValue(null);
  listenMock.mockImplementation(async (_name, listener) => {
    closeListener = listener;
    return vi.fn();
  });
});

test('applies a remembered explicit close choice and keeps the dialog open on failure', async () => {
  const user = userEvent.setup();
  respondMock.mockRejectedValueOnce(new Error('disk full')).mockResolvedValueOnce(undefined);
  render(<WindowClosePrompt />);

  await act(async () => {});
  act(() => closeListener?.({ payload: { request_id: 42 } }));
  expect(screen.getByRole('dialog', { name: 'Keep kukuri in the task tray?' })).toBeVisible();

  await user.click(screen.getByRole('checkbox', { name: 'Do not ask again' }));
  await user.click(screen.getByRole('button', { name: 'Quit kukuri' }));
  expect(respondMock).toHaveBeenLastCalledWith(42, 'quit', true);
  expect(await screen.findByText('The choice could not be applied. Please try again.')).toBeVisible();
  expect(screen.getByRole('dialog')).toBeVisible();

  await user.click(screen.getByRole('button', { name: 'Quit kukuri' }));
  expect(respondMock).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

test('recovers a close request that was emitted before the listener mounted and cancels without saving', async () => {
  const user = userEvent.setup();
  pendingMock.mockResolvedValue({ request_id: 7 });
  respondMock.mockResolvedValue(undefined);
  render(<WindowClosePrompt />);

  expect(await screen.findByRole('dialog')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(respondMock).toHaveBeenCalledWith(7, null, false);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
