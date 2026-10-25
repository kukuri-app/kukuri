import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { App } from '@/App';
import { captureVideo, encodeVideo } from '@/components/extended/liveMedia';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';

import { openLiveCreateDialog, selectWorkspace, setViewportWidth } from './DesktopShellPage.testHelpers';

vi.mock('@/components/extended/liveMedia', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/components/extended/liveMedia')>()),
  captureVideo: vi.fn(),
  encodeVideo: vi.fn(),
}));

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

// #1839 AC-3: 配信は shell の上部の bar が持つ。Live の列を離れても止まらず、bar の停止と Live の終了で止まる。
test('a broadcast keeps running after leaving the live column and stops from the top bar', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const stopLiveBroadcast = vi.spyOn(api, 'stopLiveBroadcast');
  const stopEncoding = vi.fn();
  vi.mocked(captureVideo).mockResolvedValue({ getTracks: () => [] } as unknown as MediaStream);
  vi.mocked(encodeVideo).mockReturnValue({ stop: stopEncoding });
  render(<App api={api} />);

  const liveDialog = await openLiveCreateDialog(user);
  await user.type(within(liveDialog).getByPlaceholderText('Friday stream'), 'Launch Party');
  await user.click(within(liveDialog).getByRole('button', { name: 'Start Live' }));
  const listCard = await waitFor(() => {
    const card = screen
      .queryAllByText('Launch Party')
      .map((title) => title.closest('article'))
      .find((article) => article instanceof HTMLElement && article.closest('.post-live'));
    if (!(card instanceof HTMLElement)) throw new Error('live session card not found');
    return card;
  });
  await user.click(within(listCard).getByRole('button'));
  const details = await screen.findByRole('dialog', { name: 'Launch Party' });
  await user.click(within(details).getByRole('button', { name: 'Broadcast camera' }));
  await user.keyboard('{Escape}');
  const bar = () => screen.queryByRole('region', { name: 'Broadcast controls for Launch Party' });
  expect(await within(bar()!).findByText('Broadcasting video')).toBeInTheDocument();
  expect(encodeVideo).toHaveBeenCalledTimes(1);
  // 配信中は、ほかの card から配信を始めない(同時に配信するのは 1 つ)。
  expect(screen.queryByRole('button', { name: 'Broadcast camera' })).toBeNull();

  // Live の列を離れても、配信の取り込みと送信は続く。
  await selectWorkspace(user, 'Timeline');
  expect(bar()).toBeInTheDocument();
  expect(stopEncoding).not.toHaveBeenCalled();
  expect(stopLiveBroadcast).not.toHaveBeenCalled();

  // bar の停止で取り込みと送信を止め、bar を閉じる。
  await user.click(within(bar()!).getByRole('button', { name: 'Stop video broadcast' }));
  expect(stopEncoding).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(stopLiveBroadcast).toHaveBeenCalledTimes(1));
  expect(bar()).toBeNull();

  // 開始 post の中の card からも始められ、Live を終えると配信が止まる。
  const embedded = await waitFor(() => {
    const card = document.querySelector('.post-live article');
    if (!(card instanceof HTMLElement)) throw new Error('live not shown in the start post');
    return card;
  });
  await user.click(within(embedded).getByRole('button'));
  const again = await screen.findByRole('dialog', { name: 'Launch Party' });
  await user.click(within(again).getByRole('button', { name: 'Broadcast camera' }));
  await within(bar()!).findByText('Broadcasting video');
  await user.click(within(again).getByRole('button', { name: 'End' }));
  await waitFor(() => expect(bar()).toBeNull());
  expect(stopEncoding).toHaveBeenCalledTimes(2);
}, 20_000);
