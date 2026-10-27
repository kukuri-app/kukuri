import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import type { DesktopApi, LiveSessionView } from '@/lib/api';
import { createDesktopShellStore, DesktopShellStoreContext, timelineStorageKeyForChannel } from '@/shell/store';
import { LiveActionsContext, type LiveActions } from './liveActions';
import { LiveOverlayPushButton } from './LiveOverlayPushButton';

const target = { topic: 'topic', channel_id: 'pc-1700000000000-aaaaaaaaaaaaaaaa', object_id: 'post', attachment_index: 1, attachment_hash: 'gif' };
function setup(status: LiveSessionView['status'] | null) {
  const store = createDesktopShellStore();
  const key = timelineStorageKeyForChannel(target.topic, target.channel_id);
  if (status) store.getState().setField('liveSessionsByScopeKey', { [key]: [{ status, host_pubkey: 'host' } as LiveSessionView] });
  const push = vi.fn(() => new Promise<void>(() => {}));
  const actions = { api: { pushLiveOverlay: push } as unknown as DesktopApi } as LiveActions;
  render(<DesktopShellStoreContext.Provider value={store}><LiveActionsContext.Provider value={actions}><LiveOverlayPushButton target={target} /></LiveActionsContext.Provider></DesktopShellStoreContext.Provider>);
  return { store, key, push };
}

test('each pointer and keyboard press is accepted while previous sends remain pending', async () => {
  const { push } = setup('Live');
  const button = screen.getByRole('button', { name: 'Push attachment 2' });
  const user = userEvent.setup();
  for (let index = 0; index < 10; index++) await user.click(button);
  await user.keyboard('{Enter}{Enter} ');
  expect(push).toHaveBeenCalledTimes(13);
  expect(push).toHaveBeenLastCalledWith(target);
  expect(button).toBeEnabled();
  expect(button).toHaveFocus();
  expect(screen.getByRole('status')).toHaveTextContent('Sending');
});

test('unknown, scheduled, and ended states explain why Push is unavailable', () => {
  const { store, key, push } = setup(null);
  const button = screen.getByRole('button', { name: 'Push attachment 2' });
  expect(button).toBeDisabled();
  expect(screen.getByText('Checking Live availability')).toBeVisible();
  for (const status of ['Scheduled', 'Ended'] as const) {
    act(() => store.getState().setField('liveSessionsByScopeKey', { [key]: [{ status } as LiveSessionView] }));
    expect(button).toBeDisabled();
    expect(screen.getByText('No live session is broadcasting in this thread')).toBeVisible();
  }
  expect(push).not.toHaveBeenCalled();
});
