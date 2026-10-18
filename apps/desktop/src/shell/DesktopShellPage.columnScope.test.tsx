import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import {
  getActiveColumn,
  openChannelManager,
  openControlCenter,
  publishPost,
  renderAtHash,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
});

const DEMO_TOPIC_HASH = '#/timeline?topic=kukuri%3Atopic%3Ageneral';
const CHANNEL_HASH = `${DEMO_TOPIC_HASH}&channel=channel-1`;

function findTimelineColumnByScope(scopeLabel: string) {
  const column = screen
    .getAllByRole('region', { name: /^Timeline Column,/ })
    .find((candidate) => candidate.querySelector('.shell-column-header')?.textContent?.includes(scopeLabel));
  if (!column) {
    throw new Error(`Timeline Column (${scopeLabel}) が見つかりません`);
  }
  return column;
}

async function setUpPublicAndPrivateColumns(user: ReturnType<typeof userEvent.setup>) {
  // private channel を作成すると選択 channel が channel-1 になり、Timeline Column が Public / core の 2 本になる。
  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'core');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));
  await waitFor(() => {
    expect(window.location.hash).toBe(CHANNEL_HASH);
  });
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  // private channel 側に投稿を 1 件入れる。
  await publishPost(user, 'channel post');
  await waitFor(() => {
    expect(within(findTimelineColumnByScope('core · general')).getByText('channel post')).toBeInTheDocument();
  });

  // Control Center の topic 行で「Public」を押して global 選択を public に戻す。
  const controlCenter = await openControlCenter(user);
  const generalTopicRow = within(controlCenter).getByRole('button', { name: 'general' }).closest('li');
  if (!(generalTopicRow instanceof HTMLElement)) {
    throw new Error('general topic row not found');
  }
  await user.click(within(generalTopicRow).getByRole('button', { name: /^Public(?! thread)/ }));
  await waitFor(() => {
    expect(window.location.hash).toBe(DEMO_TOPIC_HASH);
  });
  await waitFor(() => {
    expect(findTimelineColumnByScope('Public · general')).toHaveAttribute('aria-current', 'true');
  });
  const privateColumn = findTimelineColumnByScope('core · general');
  expect(privateColumn).not.toHaveAttribute('aria-current');
  return privateColumn;
}

async function submitThreadReply(
  user: ReturnType<typeof userEvent.setup>,
  threadColumn: HTMLElement,
  content: string
) {
  await user.click(within(threadColumn).getByRole('button', { name: /^Reply to Thread · core · general/ }));
  const replyInput = await within(threadColumn).findByPlaceholderText('Write a reply');
  await user.type(replyInput, content);
  const composer = replyInput.closest('form');
  if (!composer) {
    throw new Error('reply composer form not found');
  }
  await user.click(within(composer).getByRole('button', { name: 'Reply' }));
}

test('non-active な private Column の投稿本文クリックで開いた Thread は private channel scope を保ち、返信も同 channel に送られる', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const createPost = vi.spyOn(api, 'createPost');
  render(<App api={api} />);

  const privateColumn = await setUpPublicAndPrivateColumns(user);

  // 非 active な private Column の投稿本文(role=button)をクリックして Thread を開く。
  const postBody = within(privateColumn).getByText('channel post').closest('[role="button"]');
  if (!(postBody instanceof HTMLElement)) {
    throw new Error('post body button not found');
  }
  await user.click(postBody);

  const threadColumn = await screen.findByRole('region', { name: /^Thread Column,/ });
  await waitFor(() => {
    expect(threadColumn.querySelector('.shell-column-header')).toHaveTextContent('Thread · core · general');
    expect(window.location.hash).toContain('channel=channel-1');
  });
  expect(threadColumn.querySelector('.shell-column-header')).not.toHaveTextContent('Thread · Public · general');

  await submitThreadReply(user, threadColumn, 'private reply');
  await waitFor(() => {
    expect(createPost).toHaveBeenLastCalledWith(
      'kukuri:topic:general',
      'private reply',
      expect.any(String),
      expect.any(Array),
      { kind: 'private_channel', channel_id: 'channel-1' },
      [],
      null
    );
  });
}, 15_000); // 複数Columnの作成・切替・返信を含む実行枠。個々のwaitForは維持する。

// Reply ボタン経路(beginColumnReply)も返信元投稿の channel を openThread に渡し、
// Thread Column が Public / private の 2 本に分裂しないことを固定する。
test('non-active な private Column の Reply ボタンで開いた Thread も private channel scope を保つ', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const createPost = vi.spyOn(api, 'createPost');
  render(<App api={api} />);

  const privateColumn = await setUpPublicAndPrivateColumns(user);

  await user.click(within(privateColumn).getAllByRole('button', { name: 'Reply' })[0]);

  const threadColumn = await screen.findByRole('region', { name: /^Thread Column,/ });
  await waitFor(() => {
    expect(threadColumn.querySelector('.shell-column-header')).toHaveTextContent('Thread · core · general');
    expect(window.location.hash).toContain('channel=channel-1');
  });

  const replyInput = await within(threadColumn).findByPlaceholderText('Write a reply');
  await user.type(replyInput, 'private reply via button');
  const composer = replyInput.closest('form');
  if (!composer) {
    throw new Error('reply composer form not found');
  }
  await user.click(within(composer).getByRole('button', { name: 'Reply' }));
  await waitFor(() => {
    expect(createPost).toHaveBeenLastCalledWith(
      'kukuri:topic:general',
      'private reply via button',
      expect.any(String),
      expect.any(Array),
      { kind: 'private_channel', channel_id: 'channel-1' },
      [],
      null
    );
  });
});

function columnIds() {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-column-id]')).map(
    (column) => column.dataset.columnId
  );
}

// 返信の列を active にしたまま再起動しても、参加中の一覧を読む前に公開の scope へ移らない(#1875)。
test('private channel の返信の列は、再起動の後も公開の scope の返信の列を開かずに戻る', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const channel = await api.createPrivateChannel('kukuri:topic:general', 'core', 'invite_only');
  await api.createPost('kukuri:topic:general', 'channel post', null, [], {
    kind: 'private_channel',
    channel_id: channel.channel_id,
  });
  const first = renderAtHash(`${DEMO_TOPIC_HASH}&channel=${channel.channel_id}`, api);
  const post = (await screen.findByText('channel post')).closest('article');
  if (!(post instanceof HTMLElement)) {
    throw new Error('channel post not found');
  }
  await user.click(within(post).getByRole('button', { name: 'Reply' }));
  await waitFor(() => expect(window.location.hash).toContain('context=thread'));
  const replyRoute = window.location.hash;
  const columns = columnIds();

  first.unmount();
  window.history.replaceState(null, '', '/');
  render(<App api={api} />);
  // 返信の投稿を読み込んだ後に比べる。
  await waitFor(() => {
    expect(within(getActiveColumn('Thread')).getByText('channel post')).toBeInTheDocument();
  });
  expect(columnIds()).toEqual(columns);
  expect(getActiveColumn('Thread').querySelector('.shell-column-header')).toHaveTextContent(
    'Thread · core · general'
  );
  expect(window.location.hash).toBe(replyRoute);
});

test('private Column 選択直後に別 Column の本文ボタンを押しても Column 順は変わらず、押した Column が active になる (Issue #1053)', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const privateColumn = await setUpPublicAndPrivateColumns(user);

  await user.click(privateColumn);
  await waitFor(() => {
    expect(privateColumn).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe(CHANNEL_HASH);
  });
  const idsBefore = Array.from(document.querySelectorAll<HTMLElement>('[data-column-id]')).map(
    (column) => column.dataset.columnId
  );

  const explore = screen.getByRole('region', { name: /^Explore Column,/ });
  await user.click(within(explore).getByRole('tab', { name: 'Discover' }));

  await waitFor(() => {
    expect(explore).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe('#/explore?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(
    Array.from(document.querySelectorAll<HTMLElement>('[data-column-id]')).map(
      (column) => column.dataset.columnId
    )
  ).toEqual(idsBefore);
});
