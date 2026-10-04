import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test } from 'vitest';

import type { DesktopApi } from '@/lib/api';
import { runtimeApi } from '@/lib/api/commands/runtimeApi';
import { CONTROLLER_PENDING } from '@/lib/api/invoke/dispatch';
import { InvokeError } from '@/lib/api/invoke/error';
import { ControllerPendingNotice } from '@/shell/page/ControllerPendingNotice';

afterEach(() => {
  delete window.__KUKURI_DESKTOP__;
});

const pending = async () => {
  throw new InvokeError(
    CONTROLLER_PENDING,
    "PRIVATE_CHANNEL_CONTROLLER_PENDING: the channel key update waits for the owner's controlling device"
  );
};
const description = /new access to be handed out, which happens on another of your devices/;

test('an operation held for another device shows one dialog, and other failures do not', async () => {
  window.__KUKURI_DESKTOP__ = {
    createPost: pending,
    exportChannelAccessToken: pending,
    toggleReaction: async () => {
      throw new InvokeError('command_failed', 'reaction failed');
    },
  } as unknown as DesktopApi;
  render(<ControllerPendingNotice />);

  // 他の失敗は、そのまま呼出し元へ返り、ダイアログを出さない。
  await act(async () => {
    await expect(
      runtimeApi.toggleReaction('kukuri:topic:general', 'post-1', { kind: 'emoji', emoji: '👍' })
    ).rejects.toThrow('reaction failed');
  });
  expect(screen.queryByRole('dialog')).toBeNull();

  // 投稿の card などの失敗の表示は、backend の文言でなく画面の文言になる。
  await act(async () => {
    await expect(runtimeApi.createPost('kukuri:topic:general', 'hello')).rejects.toMatchObject({
      code: CONTROLLER_PENDING,
      message: 'On hold',
    });
  });
  expect(await screen.findByText(description)).toBeTruthy();
  act(() => screen.getByRole('button', { name: 'Close' }).click());
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());

  await act(async () => {
    await runtimeApi
      .exportChannelAccessToken('kukuri:topic:general', 'channel-1', null)
      .catch(() => undefined);
  });
  expect(await screen.findByText(description)).toBeTruthy();
});
