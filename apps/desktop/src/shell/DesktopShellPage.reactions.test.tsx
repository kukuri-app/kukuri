import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import {
  installObjectUrlMocks,
  getActiveColumn,
  openSettingsSection,
  publishPost,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';
import type { DesktopApi } from '@/lib/api';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
});

test('desktop shell can create a simple repost from timeline', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const originalCreateRepost = api.createRepost;
  const createRepostSpy = vi.fn((topic, sourceTopic, sourceObjectId, commentary) =>
    originalCreateRepost(topic, sourceTopic, sourceObjectId, commentary)
  );
  api.createRepost = createRepostSpy;

  render(<App api={api} />);

  await publishPost(user, 'source post');
  const sourcePost = await within(getActiveColumn('Timeline')).findByText('source post');
  const card = sourcePost.closest('article');
  if (!card) {
    throw new Error('source post card not found');
  }

  await user.click(within(card).getByRole('button', { name: 'Repost' }));
  await user.click((await screen.findAllByRole('button', { name: 'Repost' }))[1]);

  await waitFor(() => {
    expect(createRepostSpy).toHaveBeenCalledWith(
      'kukuri:topic:general',
      'kukuri:topic:general',
      expect.any(String),
      null
    );
  });
  // The repost renders X-style: the reposter is demoted to a small attribution header.
  expect(await within(getActiveColumn('Timeline')).findByText(/reposted$/i)).toBeInTheDocument();
  expect(document.querySelector('.post-repost-attribution')).not.toBeNull();
});

test('desktop shell can create a quote repost from the Column composer', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const originalCreateRepost = api.createRepost;
  const createRepostSpy = vi.fn((topic, sourceTopic, sourceObjectId, commentary) =>
    originalCreateRepost(topic, sourceTopic, sourceObjectId, commentary)
  );
  api.createRepost = createRepostSpy;

  render(<App api={api} />);

  await publishPost(user, 'source post');
  const sourcePost = await within(getActiveColumn('Timeline')).findByText('source post');
  const card = sourcePost.closest('article');
  if (!card) {
    throw new Error('source post card not found');
  }

  await user.click(within(card).getByRole('button', { name: 'Repost' }));
  await user.click(await screen.findByRole('button', { name: 'Add comment' }));

  const quoteInput = await screen.findByPlaceholderText('Add a comment');
  const composer = quoteInput.closest('form');
  if (!composer) {
    throw new Error('quote repost composer form not found');
  }
  expect(within(composer).getByText('Adding a comment')).toBeInTheDocument();
  expect(within(composer).getByText(/Original post.*source post/)).toBeInTheDocument();
  expect(within(composer).getByLabelText(/attachment/i)).toBeDisabled();
  expect(within(composer).getByRole('button', { name: 'Choose files' })).toBeDisabled();

  await user.type(quoteInput, 'quoted take');
  const submitButton = within(composer).getByRole('button', { name: 'Add comment' });
  await user.click(submitButton);

  await waitFor(() => {
    expect(createRepostSpy).toHaveBeenCalledWith(
      'kukuri:topic:general',
      'kukuri:topic:general',
      expect.any(String),
      'quoted take'
    );
  });
  expect(within(getActiveColumn('Timeline')).getByText('quoted take')).toBeInTheDocument();
});

test('reaction popover supports search and recent reactions without legacy management actions', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await publishPost(user, 'reactable post');
  const postCard = (await within(getActiveColumn('Timeline')).findByText('reactable post')).closest('article');
  if (!(postCard instanceof HTMLElement)) {
    throw new Error('reactable post card not found');
  }

  await user.click(within(postCard).getByRole('button', { name: 'React' }));
  const searchInput = await screen.findByPlaceholderText('Search reactions');
  expect(screen.queryByRole('button', { name: 'Manage reactions' })).not.toBeInTheDocument();

  await user.type(searchInput, 'party');
  await user.click(screen.getByRole('button', { name: 'party-popper' }));

  await waitFor(() => {
    expect(within(postCard).getByText('🎉')).toBeInTheDocument();
  });

  await user.click(within(postCard).getByRole('button', { name: 'React' }));
  expect(await screen.findByText('Recent')).toBeInTheDocument();
  expect(screen.getByText('Emoji')).toBeInTheDocument();
  expect(screen.getByText('Custom')).toBeInTheDocument();
  expect(
    within(screen.getByText('Recent').closest('section') as HTMLElement).getByRole('button', {
      name: 'party-popper',
    })
  ).toBeInTheDocument();
});

test('reaction picker lazily loads recent and custom reactions when opened', async () => {
  const user = userEvent.setup();
  const baseApi = createDesktopMockApi();
  const api: DesktopApi = {
    ...baseApi,
    listRecentReactions: vi.fn(baseApi.listRecentReactions),
    listMyCustomReactionAssets: vi.fn(baseApi.listMyCustomReactionAssets),
    listBookmarkedCustomReactions: vi.fn(baseApi.listBookmarkedCustomReactions),
  };

  render(<App api={api} />);

  await publishPost(user, 'reaction preload');
  const postCard = (await within(getActiveColumn('Timeline')).findByText('reaction preload')).closest('article');
  if (!(postCard instanceof HTMLElement)) {
    throw new Error('reaction preload post card not found');
  }

  expect(api.listRecentReactions).not.toHaveBeenCalled();
  expect(api.listMyCustomReactionAssets).not.toHaveBeenCalled();
  expect(api.listBookmarkedCustomReactions).not.toHaveBeenCalled();

  await user.click(within(postCard).getByRole('button', { name: 'React' }));

  await waitFor(() => {
    expect(api.listRecentReactions).toHaveBeenCalledTimes(1);
    expect(api.listMyCustomReactionAssets).toHaveBeenCalledTimes(1);
    expect(api.listBookmarkedCustomReactions).toHaveBeenCalledTimes(1);
  });
});

test('visible custom reactions auto-fetch media before save, and saved reactions require explicit save', async () => {
  const user = userEvent.setup();
  installObjectUrlMocks();
  const remoteReactionAsset = {
    asset_id: 'asset-remote',
    owner_pubkey: 'd'.repeat(64),
    blob_hash: 'blob-remote',
    search_key: 'remote-cat',
    mime: 'image/png',
    bytes: 128,
    width: 128,
    height: 128,
  };
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        {
          object_id: 'post-remote-reaction',
          envelope_id: 'envelope-post-remote-reaction',
          author_pubkey: 'f'.repeat(64),
          author_name: 'frank',
          author_display_name: 'Frank',
          following: false,
          followed_by: false,
          mutual: false,
          friend_of_friend: false,
          object_kind: 'post',
          content: 'remote custom reaction',
          content_status: 'Available',
          attachments: [],
          created_at: 10,
          reply_to: null,
          root_id: 'post-remote-reaction',
          channel_id: null,
          audience_label: 'Public',
          published_topic_id: 'kukuri:topic:general',
          origin_topic_id: 'kukuri:topic:general',
          reaction_summary: [
            {
              reaction_key_kind: 'custom_asset',
              normalized_reaction_key: 'custom_asset:asset-remote',
              emoji: null,
              custom_asset: remoteReactionAsset,
              count: 1,
            },
          ],
          my_reactions: [],
        },
      ],
    },
  });
  const getBlobMediaPayload = vi.fn(async (hash: string, mime: string) =>
    hash === remoteReactionAsset.blob_hash
      ? {
          bytes_base64: 'ZmFrZS1pbWFnZQ==',
          mime,
        }
      : null
  );
  const bookmarkCustomReaction = vi.fn(api.bookmarkCustomReaction.bind(api));
  api.getBlobMediaPayload = getBlobMediaPayload;
  api.bookmarkCustomReaction = bookmarkCustomReaction;

  render(<App api={api} />);

  const remoteReactionChip = await screen.findByRole('button', {
    name: `${remoteReactionAsset.search_key} 1`,
  });
  const remoteReactionImage = remoteReactionChip.querySelector('img');
  if (!(remoteReactionImage instanceof HTMLImageElement)) {
    throw new Error('remote reaction image not found');
  }
  expect(remoteReactionImage.getAttribute('src')).toContain('blob:mock-');
  await waitFor(() => {
    expect(getBlobMediaPayload).toHaveBeenCalledWith(
      remoteReactionAsset.blob_hash,
      remoteReactionAsset.mime,
      undefined
    );
  });

  let drawer = await openSettingsSection(user, 'reactions');
  expect(within(drawer).queryByRole('img', { name: remoteReactionAsset.search_key })).toBeNull();

  await user.click(within(drawer).getByRole('button', { name: 'Close settings' }));
  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument();
  });

  fireEvent.contextMenu(remoteReactionChip);
  await user.click(screen.getByRole('menuitem', { name: 'Save to use it myself' }));
  expect(bookmarkCustomReaction).toHaveBeenCalledWith(remoteReactionAsset);

  drawer = await openSettingsSection(user, 'reactions');
  expect(await within(drawer).findByRole('img', { name: remoteReactionAsset.search_key })).toBeInTheDocument();
}, 15_000); // 複数回の設定開閉と保存を含む実行枠。個々の待機・副作用の検証は維持する。


// #1232 AC-4: 投稿に貼られたセットを取り込むと保存済みへ加わり、取れないセットは何も保存せずに失敗を示す。
test('a reaction set pasted in a post imports its reactions into saved reactions', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const shared = {
    asset_id: 'asset-set-cat',
    owner_pubkey: 'd'.repeat(64),
    blob_hash: 'blob-set-cat',
    search_key: 'set-cat',
    mime: 'image/png',
    bytes: 128,
    width: 128,
    height: 128,
  };
  const set = await api.createCustomReactionSet('cats', [shared]);
  const bookmarkedBefore = await api.listBookmarkedCustomReactions();

  render(<App api={api} />);
  await publishPost(
    user,
    `kukuri:reaction-set:${set.set_hash} kukuri:reaction-set:${'e'.repeat(64)}`,
    { input: 'paste' }
  );
  const [importable, missing] = await within(getActiveColumn('Timeline')).findAllByRole('button', {
    name: 'Import reaction set',
  });

  await user.click(missing);
  expect(
    await screen.findByText('Could not import the reaction set. Please try again later.')
  ).toBeInTheDocument();
  expect(await api.listBookmarkedCustomReactions()).toEqual(bookmarkedBefore);

  await user.click(importable);
  expect(await screen.findByText('Imported “cats”: 1 saved.')).toBeInTheDocument();
  const drawer = await openSettingsSection(user, 'reactions');
  expect(within(drawer).getByRole('article', { name: 'set-cat' })).toBeInTheDocument();
});
