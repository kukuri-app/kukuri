import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { App } from '@/App';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { DEVELOPER_MODE_STORAGE_KEY } from '@/lib/developerMode';

import { openLiveCreateDialog, setViewportWidth } from './DesktopShellPage.testHelpers';

beforeEach(() => {
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'false');
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

// #1839 AC-2: Live を作ると開始 post が timeline に出て、その中に Live が出る。そこから別の topic を選んで紹介 post を出せる。
test('a live session starts with a post and can be shared to another topic', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const createLiveSession = vi.spyOn(api, 'createLiveSession');
  const createPost = vi.spyOn(api, 'createPost');
  render(<App api={api} />);

  const liveDialog = await openLiveCreateDialog(user);
  await user.type(within(liveDialog).getByPlaceholderText('Friday stream'), 'Launch Party');
  await user.type(
    within(liveDialog).getByPlaceholderText('Tell people about the live session (optional)'),
    'come watch'
  );
  await user.click(within(liveDialog).getByRole('button', { name: 'Start Live' }));
  await waitFor(() => expect(createLiveSession).toHaveBeenCalled());
  expect(createLiveSession.mock.calls[0]?.[4]).toBe('come watch');

  // 開始 post の中の Live。開始 post そのものなので「元の投稿を開く」は出さない。
  const startPost = (await screen.findByText('come watch')).closest('article');
  if (!(startPost instanceof HTMLElement)) throw new Error('start post not found');
  const embedded = await waitFor(() => {
    const card = startPost.querySelector('.post-live article');
    if (!(card instanceof HTMLElement)) throw new Error('live not shown in the start post');
    return card;
  });
  expect(within(embedded).getByText('Launch Party')).toBeInTheDocument();
  expect(within(embedded).queryByRole('button', { name: 'Open original post' })).toBeNull();

  await user.click(within(embedded).getByRole('button'));
  await user.click(within(await screen.findByRole('dialog', { name: 'Launch Party' })).getByRole('button', { name: 'Share' }));
  const shareDialog = await screen.findByRole('dialog', { name: 'Share live session' });
  const target = within(shareDialog).getByRole('combobox', { name: 'Post to' });
  const devIndex = Array.from((target as HTMLSelectElement).options).findIndex((option) =>
    option.textContent?.includes('dev')
  );
  await user.selectOptions(target, String(devIndex));
  await user.type(within(shareDialog).getByPlaceholderText('Add a note (optional)'), 'look');
  await user.click(within(shareDialog).getByRole('button', { name: 'Share' }));

  await waitFor(() =>
    expect(createPost).toHaveBeenLastCalledWith(
      'kukuri:topic:dev',
      'look',
      null,
      [],
      { kind: 'public' },
      [],
      { session_id: expect.any(String), topic_id: 'kukuri:topic:general', channel_id: null }
    )
  );
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Share live session' })).toBeNull());
}, 20_000);
