import { act } from '@testing-library/react';
import type { FormEvent } from 'react';
import { expect, test, vi } from 'vitest';

import { renderActionsHook } from '@/shell/testSupport/renderShellActions';

// ID の受付は保存/queueの完了で、受信側のACKではない。
test('an accepted DM remains pending when its authoritative history cannot refresh', async () => {
  const peer = 'a'.repeat(64);
  const sendDirectMessage = vi.fn(async () => 'queued-message');
  const { result, store, mocks } = renderActionsHook({
    api: { sendDirectMessage },
    preset: (current) => ({
      syncStatus: { ...current.syncStatus, local_author_pubkey: 'f'.repeat(64) },
      selectedDirectMessagePeerPubkey: peer,
      directMessageComposer: 'offline DM',
    }),
  });
  mocks.openDirectMessagePane.mockRejectedValue(new Error('offline history'));
  const event = {
    preventDefault: vi.fn(),
    currentTarget: { querySelector: () => null },
  } as unknown as FormEvent<HTMLFormElement>;

  await act(async () => { await result.current.handleSendDirectMessage(event); });

  expect(sendDirectMessage).toHaveBeenCalledWith(peer, 'offline DM', [], null);
  expect(store.getState().directMessageTimelineByPeer[peer][0]).toMatchObject({
    message_id: 'queued-message', text: 'offline DM', outgoing: true, delivered: false,
  });
  expect(store.getState().directMessageComposer).toBe('');
  expect(store.getState().directMessageSending).toBe(false);
});
