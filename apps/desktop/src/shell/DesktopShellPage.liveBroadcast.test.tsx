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
      .find((article) => article instanceof HTMLElement && !article.closest('.post-live'));
    if (!(card instanceof HTMLElement)) throw new Error('live session card not found');
    return card;
  });
  await user.click(within(listCard).getByRole('button', { name: 'Open broadcast controls' }));
  const bar = screen.getByRole('region', { name: 'Broadcast controls for Launch Party' });
  await user.click(within(bar).getByRole('button', { name: 'Start video broadcast' }));
  expect(await within(bar).findByText('Broadcasting video')).toBeInTheDocument();
  expect(encodeVideo).toHaveBeenCalledTimes(1);

  // Live の列を離れても、配信の取り込みと送信は続く。
  await selectWorkspace(user, 'Timeline');
  expect(screen.getByRole('region', { name: 'Broadcast controls for Launch Party' })).toBeInTheDocument();
  expect(stopEncoding).not.toHaveBeenCalled();
  expect(stopLiveBroadcast).not.toHaveBeenCalled();

  await user.click(within(bar).getByRole('button', { name: 'Stop video broadcast' }));
  expect(stopEncoding).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(stopLiveBroadcast).toHaveBeenCalledTimes(1));

  // Live を終えると bar も閉じる。
  await user.click(within(bar).getByRole('button', { name: 'Start video broadcast' }));
  await within(bar).findByText('Broadcasting video');
  const embedded = await waitFor(() => {
    const card = document.querySelector('.post-live article');
    if (!(card instanceof HTMLElement)) throw new Error('live not shown in the start post');
    return card;
  });
  await user.click(within(embedded).getByRole('button', { name: 'End' }));
  await waitFor(() =>
    expect(screen.queryByRole('region', { name: 'Broadcast controls for Launch Party' })).toBeNull()
  );
  expect(stopEncoding).toHaveBeenCalledTimes(2);
}, 20_000);
