import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import i18n from '@/i18n';
import type { DesktopApi, PostView } from '@/lib/api';
import { InvokeError } from '@/lib/api/invoke/error';

import { CommunityIndexWorkspace } from './CommunityIndexWorkspace';
import {
  INDEX_IMAGE_HASH,
  NODE_A,
  NODE_B,
  indexEntry,
  knownAuthor,
  manifest,
  deferred,
  resolvedImageIndexEntry,
  resolvedIndexEntry,
  runSearch,
  workspaceProps,
} from './CommunityIndexWorkspace.testSupport';

test('healthy query node selection stays automatic and out of the primary surface', () => {
  const api = {} as DesktopApi;
  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);

  expect(screen.queryByLabelText('Search provider')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Search query')).toBeInTheDocument();
});

test('topic search sends the active public scope and renders results with the shared post card', async () => {
  const user = userEvent.setup();
  const onOpenAuthor = vi.fn();
  const searchCommunityNodeIndex = vi.fn().mockResolvedValue({
    entries: [
      {
        scope_kind: 'public_topic',
        scope_id: 'rust',
        object_id: 'post-1',
        author_pubkey: 'author-1',
        text: 'hello\nderived-tag',
        created_at: 42,
      },
    ],
  });
  const api = { searchCommunityNodeIndex } as unknown as DesktopApi;
  render(
    <CommunityIndexWorkspace
      {...workspaceProps(api, {
        knownAuthorsByPubkey: { 'author-1': knownAuthor('author-1') },
        mediaObjectUrls: { 'avatar-hash': 'blob:avatar-hash' },
        onOpenAuthor,
      })}
    />
  );

  runSearch();

  await waitFor(() => expect(searchCommunityNodeIndex).toHaveBeenCalledTimes(1));
  expect(searchCommunityNodeIndex).toHaveBeenCalledWith(
    expect.objectContaining({
      scope_kind: 'public_topic',
      scope_id: 'rust',
      query: 'hello',
    })
  );
  const result = await screen.findByText('derived-tag');
  expect(result.closest('article')).toHaveClass('post-card');
  expect(screen.getByText('Alice')).toBeInTheDocument();
  expect(screen.getByTestId('post-1-author-avatar').querySelector('img')).toHaveAttribute(
    'src',
    'blob:avatar-hash'
  );
  expect(screen.queryByText(/Search preview; may include derived tags/)).not.toBeInTheDocument();
  expect(screen.queryByText('rust')).not.toBeInTheDocument();
  expect(screen.queryByText('public_topic')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Reply' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Repost' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Alice' }));
  expect(onOpenAuthor).toHaveBeenCalledWith('author-1');
});

test('Explore results expose the same post actions as the timeline', async () => {
  const user = userEvent.setup();
  const onReply = vi.fn();
  const onRepost = vi.fn();
  const onToggleBookmark = vi.fn();
  const searchCommunityNodeIndex = vi.fn().mockResolvedValue({
    entries: [indexEntry('explore-actions', 'actionable result')],
  });
  const resolveCommunityIndexPosts = vi.fn().mockResolvedValue({
    entries: [resolvedIndexEntry('explore-actions')],
  });
  const api = { searchCommunityNodeIndex, resolveCommunityIndexPosts } as unknown as DesktopApi;
  const interactiveActions = {
    onOpenThread: vi.fn(),
    onOpenThreadInTopic: vi.fn(),
    onReply,
    onRepost,
    onQuoteRepost: vi.fn(),
    onToggleReaction: vi.fn(),
    showBookmarkAction: true,
    onToggleBookmark,
    onCopyPostLink: vi.fn(),
  };
  render(
    <CommunityIndexWorkspace
      {...workspaceProps(api, { mode: 'explore' })}
      {...interactiveActions}
    />
  );

  runSearch();

  const result = await screen.findByText('canonical content');
  const card = result.closest('article');
  if (!(card instanceof HTMLElement)) throw new Error('Explore result card not found');

  await waitFor(() => expect(resolveCommunityIndexPosts).toHaveBeenCalledTimes(1));

  expect(within(card).getByRole('button', { name: 'React' })).toBeEnabled();
  expect(within(card).getByRole('button', { name: 'Repost' })).toBeInTheDocument();
  expect(within(card).getByRole('button', { name: 'Reply' })).toBeInTheDocument();
  expect(within(card).getByRole('button', { name: 'Copy link' })).toBeInTheDocument();
  expect(within(card).getByRole('button', { name: 'Bookmark' })).toBeInTheDocument();
  expect(within(card).getByRole('button', { name: 'Report' })).toBeInTheDocument();

  await user.click(within(card).getByRole('button', { name: 'Reply' }));
  expect(onReply).toHaveBeenCalledWith(
    expect.objectContaining({
      object_id: 'explore-actions',
      published_topic_id: 'rust',
      is_threadable: true,
      content: 'canonical content',
    })
  );

  await user.click(within(card).getByRole('button', { name: 'Bookmark' }));
  expect(onToggleBookmark).toHaveBeenCalledWith(
    expect.objectContaining({ object_id: 'explore-actions', published_topic_id: 'rust' })
  );

  await user.click(within(card).getByRole('button', { name: 'Repost' }));
  await user.click(screen.getAllByRole('button', { name: 'Repost' })[1]);
  expect(onRepost).toHaveBeenCalledWith(
    expect.objectContaining({
      object_id: 'explore-actions',
      published_topic_id: 'rust',
      content: 'canonical content',
    })
  );
});

test('reaction results are refreshed into the Community Index card', async () => {
  const user = userEvent.setup();
  const entry = indexEntry('reaction-result', 'reaction preview');
  const first = resolvedIndexEntry(entry.object_id);
  const initialPost = {
    ...first.post,
    reaction_summary: [
      {
        reaction_key_kind: 'emoji',
        normalized_reaction_key: 'emoji:👍',
        emoji: '👍',
        custom_asset: null,
        count: 1,
      },
    ],
  };
  const refreshedPost = {
    ...initialPost,
    reaction_summary: [{ ...initialPost.reaction_summary[0], count: 2 }],
    my_reactions: [
      {
        reaction_key_kind: 'emoji',
        normalized_reaction_key: 'emoji:👍',
        emoji: '👍',
        custom_asset: null,
      },
    ],
  };
  const resolveCommunityIndexPosts = vi
    .fn()
    .mockResolvedValueOnce({ entries: [{ ...first, post: initialPost }] })
    .mockResolvedValueOnce({ entries: [{ ...first, post: refreshedPost }] });
  const onToggleReaction = vi.fn().mockResolvedValue(undefined);
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
    resolveCommunityIndexPosts,
  } as unknown as DesktopApi;

  render(
    <CommunityIndexWorkspace
      {...workspaceProps(api, {
        onToggleReaction,
        knownAuthorsByPubkey: { [entry.author_pubkey]: knownAuthor(entry.author_pubkey) },
      })}
    />
  );
  runSearch();

  const firstChip = await screen.findByRole('button', { name: /👍\s*1/ });
  await user.click(firstChip);

  expect(onToggleReaction).toHaveBeenCalledWith(
    expect.objectContaining({ object_id: entry.object_id, content: 'canonical content' }),
    { kind: 'emoji', emoji: '👍' }
  );
  await waitFor(() => expect(resolveCommunityIndexPosts).toHaveBeenCalledTimes(2));
  expect(await screen.findByRole('button', { name: /👍\s*2/ })).toBeInTheDocument();
});

test('unresolved results stay fail-closed and expose only reporting and identifier actions', async () => {
  const entry = indexEntry('unresolved', 'read-only result');
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
    resolveCommunityIndexPosts: vi.fn().mockResolvedValue({
      entries: [
        {
          key: `public_topic:rust:${entry.object_id}`,
          post: null,
          capabilities: {
            open_thread: false,
            reply: false,
            repost: false,
            quote_repost: false,
            react: false,
            copy_link: false,
            bookmark: false,
            withdraw: false,
          },
        },
      ],
    }),
  } as unknown as DesktopApi;

  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);
  runSearch();

  await waitFor(() => expect(api.resolveCommunityIndexPosts).toHaveBeenCalledTimes(1));
  expect(screen.queryByText(entry.text)).not.toBeInTheDocument();
  const safePlaceholder = await screen.findByText(
    'Post content is unavailable because its safety labels could not be verified.'
  );
  const card = safePlaceholder.closest('article');
  if (!(card instanceof HTMLElement)) throw new Error('Explore result card not found');

  expect(within(card).queryByRole('button', { name: 'React' })).not.toBeInTheDocument();
  expect(within(card).queryByRole('button', { name: 'Repost' })).not.toBeInTheDocument();
  expect(within(card).queryByRole('button', { name: 'Reply' })).not.toBeInTheDocument();
  expect(within(card).queryByRole('button', { name: 'Copy link' })).not.toBeInTheDocument();
  expect(within(card).queryByRole('button', { name: 'Bookmark' })).not.toBeInTheDocument();
  expect(within(card).getByRole('button', { name: 'Report' })).toBeInTheDocument();
});

test('node-provided result text stays hidden while canonical resolution is pending', async () => {
  const entry = indexEntry('pending-resolution', 'untrusted pending result');
  const pending = deferred<{ entries: ReturnType<typeof resolvedIndexEntry>[] }>();
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
    resolveCommunityIndexPosts: vi.fn().mockReturnValue(pending.promise),
  } as unknown as DesktopApi;

  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);
  runSearch();

  await waitFor(() => expect(api.resolveCommunityIndexPosts).toHaveBeenCalledTimes(1));
  expect(screen.queryByText(entry.text)).not.toBeInTheDocument();
  expect(
    screen.getByText('Checking the post before showing its content…')
  ).toBeInTheDocument();
});

test('node-provided result text stays hidden when canonical resolution fails', async () => {
  const entry = indexEntry('failed-resolution', 'untrusted failed result');
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
    resolveCommunityIndexPosts: vi.fn().mockRejectedValue(new Error('resolver unavailable')),
  } as unknown as DesktopApi;

  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);
  runSearch();

  await waitFor(() => expect(api.resolveCommunityIndexPosts).toHaveBeenCalledTimes(1));
  expect(screen.queryByText(entry.text)).not.toBeInTheDocument();
  expect(
    await screen.findByText(
      'Post content is unavailable because its safety labels could not be verified.'
    )
  ).toBeInTheDocument();
});

test('missing author profiles are resolved instead of being labeled unknown', async () => {
  const entry = indexEntry('remote-author', 'profile lookup result');
  const getAuthorSocialView = vi.fn().mockResolvedValue(knownAuthor(entry.author_pubkey));
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
    resolveCommunityIndexPosts: vi.fn().mockResolvedValue({ entries: [] }),
    getAuthorSocialView,
  } as unknown as DesktopApi;

  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);
  runSearch();

  expect(await screen.findByText('Alice')).toBeInTheDocument();
  expect(getAuthorSocialView).toHaveBeenCalledTimes(1);
  expect(getAuthorSocialView).toHaveBeenCalledWith(entry.author_pubkey);
  expect(screen.queryByText('Unknown user')).not.toBeInTheDocument();
});

test('the local profile is used without a redundant author lookup', async () => {
  const entry = indexEntry('local-author', 'local profile result');
  const getAuthorSocialView = vi.fn();
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
    resolveCommunityIndexPosts: vi.fn().mockResolvedValue({ entries: [] }),
    getAuthorSocialView,
  } as unknown as DesktopApi;

  render(
    <CommunityIndexWorkspace
      {...workspaceProps(api, {
        localAuthorPubkey: entry.author_pubkey,
        localProfile: {
          pubkey: entry.author_pubkey,
          name: 'local-alice',
          display_name: 'Local Alice',
          about: null,
          picture_asset: null,
          updated_at: 42,
        },
      })}
    />
  );
  runSearch();

  expect(await screen.findByText('Local Alice')).toBeInTheDocument();
  expect(getAuthorSocialView).not.toHaveBeenCalled();
});

test('unknown author is used only after a fetched profile has no configured name', async () => {
  const entry = indexEntry('nameless-author', 'nameless profile result');
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
    resolveCommunityIndexPosts: vi.fn().mockResolvedValue({ entries: [] }),
    getAuthorSocialView: vi.fn().mockResolvedValue({
      ...knownAuthor(entry.author_pubkey),
      name: null,
      display_name: null,
    }),
  } as unknown as DesktopApi;

  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);
  runSearch();

  expect(await screen.findByText('Unknown user')).toBeInTheDocument();
  expect(screen.queryByText('User information unavailable')).not.toBeInTheDocument();
});

test('author lookup failures are distinct from fetched nameless profiles', async () => {
  const entry = indexEntry('failed-author', 'failed profile result');
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
    resolveCommunityIndexPosts: vi.fn().mockResolvedValue({ entries: [] }),
    getAuthorSocialView: vi.fn().mockRejectedValue(new Error('offline')),
  } as unknown as DesktopApi;

  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);
  runSearch();

  expect(await screen.findByText('User information unavailable')).toBeInTheDocument();
  expect(screen.queryByText('Unknown user')).not.toBeInTheDocument();
});

test('index results hide identifiers and copy their complete values from context actions', async () => {
  const user = userEvent.setup();
  const clipboardWriteText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });
  const entry = indexEntry('post-context', 'context result');
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
  } as unknown as DesktopApi;
  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);

  runSearch();
  const resultText = await screen.findByText(entry.text);
  expect(screen.queryByText(new RegExp(entry.author_pubkey))).not.toBeInTheDocument();
  expect(screen.queryByText(new RegExp(entry.object_id))).not.toBeInTheDocument();

  const target = resultText.closest('article')?.querySelector('[data-testid="post-identifier-target"]');
  if (!(target instanceof HTMLElement)) throw new Error('index result target not found');
  fireEvent.contextMenu(target, { clientX: 40, clientY: 50 });
  await user.click(screen.getByRole('menuitem', { name: 'Copy user ID' }));
  expect(clipboardWriteText).toHaveBeenLastCalledWith(entry.author_pubkey);

  target.focus();
  fireEvent.keyDown(target, { key: 'F10', shiftKey: true });
  await user.click(screen.getByRole('menuitem', { name: 'Copy post ID' }));
  expect(clipboardWriteText).toHaveBeenLastCalledWith(entry.object_id);
});

test('changing the selected node clears results and prevents reporting them to the new node', async () => {
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({
      entries: [indexEntry('post-a', 'result from node A')],
    }),
  } as unknown as DesktopApi;
  const props = workspaceProps(api);
  const { rerender } = render(<CommunityIndexWorkspace {...props} />);

  runSearch();
  expect(await screen.findByText('result from node A')).toBeInTheDocument();

  rerender(<CommunityIndexWorkspace {...props} selectedNodeBaseUrl={NODE_B} />);

  await waitFor(() => expect(screen.queryByText('result from node A')).not.toBeInTheDocument());
  expect(screen.queryByRole('button', { name: 'Report' })).not.toBeInTheDocument();
});

test('changing the Explore operation clears cards from the previous report context', async () => {
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({
      entries: [indexEntry('search-result', 'search result from node A')],
    }),
  } as unknown as DesktopApi;
  render(<CommunityIndexWorkspace {...workspaceProps(api, { mode: 'explore' })} />);

  runSearch();
  expect(await screen.findByText('search result from node A')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('tab', { name: 'Discover' }));

  expect(screen.queryByText('search result from node A')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Report' })).not.toBeInTheDocument();
});

test('a pending response is discarded when its node or scope is no longer active', async () => {
  const pending = deferred<{ entries: ReturnType<typeof indexEntry>[] }>();
  const api = {
    searchCommunityNodeIndex: vi.fn().mockReturnValue(pending.promise),
  } as unknown as DesktopApi;
  const props = workspaceProps(api);
  const { rerender } = render(<CommunityIndexWorkspace {...props} />);

  runSearch();
  rerender(
    <CommunityIndexWorkspace
      {...props}
      selectedNodeBaseUrl={NODE_B}
      activeTimelineScope={{ kind: 'channel', channel_id: 'private-1' }}
    />
  );
  pending.resolve({ entries: [indexEntry('post-a', 'late result from node A')] });

  await waitFor(() => expect(api.searchCommunityNodeIndex).toHaveBeenCalledTimes(1));
  expect(screen.queryByText('late result from node A')).not.toBeInTheDocument();
});

test('responses that complete in reverse order keep only the current request context', async () => {
  const responseA = deferred<{ entries: ReturnType<typeof indexEntry>[] }>();
  const responseB = deferred<{ entries: ReturnType<typeof indexEntry>[] }>();
  const searchCommunityNodeIndex = vi.fn((request: { base_url: string }) =>
    request.base_url === NODE_A ? responseA.promise : responseB.promise
  );
  const api = { searchCommunityNodeIndex } as unknown as DesktopApi;
  const props = workspaceProps(api);
  const { rerender } = render(<CommunityIndexWorkspace {...props} />);

  runSearch('node A');
  rerender(<CommunityIndexWorkspace {...props} selectedNodeBaseUrl={NODE_B} />);
  runSearch('node B');
  responseB.resolve({ entries: [indexEntry('post-b', 'current result from node B')] });
  expect(await screen.findByText('current result from node B')).toBeInTheDocument();

  responseA.resolve({ entries: [indexEntry('post-a', 'stale result from node A')] });
  await waitFor(() => expect(searchCommunityNodeIndex).toHaveBeenCalledTimes(2));
  expect(screen.queryByText('stale result from node A')).not.toBeInTheDocument();
  expect(screen.getByText('current result from node B')).toBeInTheDocument();
});

test('recommendation reports use the source node latest manifest and recommendation identity', async () => {
  const freshManifest = {
    ...manifest,
    node_id: 'node-a-fresh',
    report_endpoint: `${NODE_A}/v2/report`,
  };
  const fetchCommunityNodeManifest = vi.fn().mockResolvedValue({
    status: 'ok',
    manifest: freshManifest,
  });
  const submitCommunityNodeReport = vi.fn().mockResolvedValue({
    accepted: true,
    reference_id: 'report-1',
  });
  const api = {
    recommendCommunityNodeIndex: vi.fn().mockResolvedValue({
      entries: [indexEntry('recommendation-1', 'recommended result')],
    }),
    fetchCommunityNodeManifest,
    submitCommunityNodeReport,
  } as unknown as DesktopApi;
  render(<CommunityIndexWorkspace {...workspaceProps(api, { mode: 'explore' })} />);

  fireEvent.click(screen.getByRole('tab', { name: 'Recommendations' }));
  fireEvent.click(screen.getByRole('button', { name: 'Show results' }));
  expect(await screen.findByText('recommended result')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Report' }));

  const dialog = await screen.findByRole('dialog', { name: 'Report content' });
  await waitFor(() => expect(fetchCommunityNodeManifest).toHaveBeenCalledWith(NODE_A));
  // 著者情報の解決は非同期に settle する(Loading… → unavailable)ため、完了を待って検証する。
  expect(
    await within(dialog).findByText(/^Recommendation · User information unavailable$/)
  ).toBeInTheDocument();
  expect(within(dialog).getByText('Recommendation')).toBeInTheDocument();
  fireEvent.click(await within(dialog).findByRole('button', { name: 'Send report' }));

  await waitFor(() =>
    expect(submitCommunityNodeReport).toHaveBeenCalledWith(
      expect.objectContaining({
        node_base_url: NODE_A,
        report_endpoint: `${NODE_A}/v2/report`,
        subject_kind: 'recommendation',
        subject_id: 'recommendation-1',
        capability: 'recommendation',
      })
    )
  );
});

test('a failed latest manifest fetch does not fall back to a cached report target', async () => {
  const fetchCommunityNodeManifest = vi.fn().mockResolvedValue({
    status: 'absent',
    manifest: null,
  });
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({
      entries: [indexEntry('post-a', 'report target requires a fresh manifest')],
    }),
    fetchCommunityNodeManifest,
  } as unknown as DesktopApi;
  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);

  runSearch();
  expect(await screen.findByText('report target requires a fresh manifest')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Report' }));

  const dialog = await screen.findByRole('dialog', { name: 'Report content' });
  await waitFor(() => expect(fetchCommunityNodeManifest).toHaveBeenCalledWith(NODE_A));
  expect(
    await within(dialog).findByText(
      'Could not refresh report targets. No default destination will be used.'
    )
  ).toBeInTheDocument();
  expect(within(dialog).queryByRole('button', { name: 'Send report' })).not.toBeInTheDocument();
});

test.each([
  [new InvokeError('AUTH_REQUIRED', 'server auth error', 401), '選択したコミュニティノードへの認証が必要です。'],
  [new InvokeError('CONSENT_REQUIRED', 'server consent error', 403), '選択したコミュニティノードの必須同意が必要です。'],
  [new InvokeError('INDEX_QUERY_NOT_CONFIGURED', 'server config error'), '選択したコミュニティノードではコミュニティ索引が設定されていません。'],
  [new InvokeError('INDEX_QUERY_NOT_ACTIVATED', 'server activation error'), '選択したコミュニティノードのコミュニティ索引は一時的に利用できません。'],
  [new InvokeError('RATE_LIMITED', 'server rate error', 429, 12), '要求が多すぎます。12秒後にもう一度お試しください。'],
])('known query error is localized in Japanese: %s', async (cause, expected) => {
  await i18n.changeLanguage('ja');
  const api = {
    searchCommunityNodeIndex: vi.fn().mockRejectedValue(cause),
  } as unknown as DesktopApi;
  render(<CommunityIndexWorkspace {...workspaceProps(api)} />);

  fireEvent.change(screen.getByLabelText('検索語'), { target: { value: '検索' } });
  fireEvent.click(screen.getByRole('button', { name: '結果を表示' }));

  expect(await screen.findByText(expected)).toBeInTheDocument();
});

// #698: 選択値が適格一覧から外れている間は古いノードへ要求を送らない。
test('a selected node that is no longer eligible does not receive queries until it is eligible again', async () => {
  const searchCommunityNodeIndex = vi.fn().mockResolvedValue({ entries: [] });
  const api = { searchCommunityNodeIndex } as unknown as DesktopApi;
  const { rerender } = render(
    <CommunityIndexWorkspace
      {...workspaceProps(api, { eligibleNodeBaseUrls: [NODE_B], selectedNodeBaseUrl: NODE_A })}
    />
  );

  // 適格一覧 [B] と古い選択値 A が同時に渡っても、A へ検索語を送らない。
  const runButton = screen.queryByRole('button', { name: 'Show results' });
  if (runButton) fireEvent.click(runButton);
  await new Promise((done) => setTimeout(done, 0));
  expect(searchCommunityNodeIndex).not.toHaveBeenCalled();

  // 再調整で選択が適格ノードになれば送れる。
  rerender(
    <CommunityIndexWorkspace
      {...workspaceProps(api, { eligibleNodeBaseUrls: [NODE_B], selectedNodeBaseUrl: NODE_B })}
    />
  );
  runSearch('hello');
  await waitFor(() => expect(searchCommunityNodeIndex).toHaveBeenCalledTimes(1));
  expect(searchCommunityNodeIndex).toHaveBeenCalledWith(
    expect.objectContaining({ base_url: NODE_B, query: 'hello' })
  );
});

test('an unavailable explicit node reports the stopped state instead of offering a query form', () => {
  const api = { searchCommunityNodeIndex: vi.fn() } as unknown as DesktopApi;
  render(
    <CommunityIndexWorkspace
      {...workspaceProps(api, { eligibleNodeBaseUrls: [NODE_B], selectedNodeBaseUrl: null })}
    />
  );

  expect(
    screen.getByText('The explicitly selected Community Node is unavailable. Queries are paused.')
  ).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Show results' })).not.toBeInTheDocument();
});

// #1052: 解決済み投稿の添付をタイムラインと同じ表示経路へ通す。

test('resolved results render their attachments through the shared media path', async () => {
  const searchCommunityNodeIndex = vi
    .fn()
    .mockResolvedValue({ entries: [indexEntry('media-post', 'indexed text')] });
  const resolveCommunityIndexPosts = vi
    .fn()
    .mockResolvedValue({ entries: [resolvedImageIndexEntry('media-post')] });
  const api = { searchCommunityNodeIndex, resolveCommunityIndexPosts } as unknown as DesktopApi;

  render(
    <CommunityIndexWorkspace
      {...workspaceProps(api, {
        mediaObjectUrls: { [INDEX_IMAGE_HASH]: 'blob:index-image' },
      })}
    />
  );
  runSearch();

  const preview = await screen.findByTestId('media-preview-media-post');
  expect(preview).toHaveAttribute('src', 'blob:index-image');
  expect(screen.queryByTestId('media-adult-gated-media-post')).not.toBeInTheDocument();
});

test('an adult-labeled resolved result keeps the shared gated placeholder', async () => {
  const searchCommunityNodeIndex = vi
    .fn()
    .mockResolvedValue({ entries: [indexEntry('adult-media-post', 'indexed text')] });
  const resolveCommunityIndexPosts = vi
    .fn()
    .mockResolvedValue({ entries: [resolvedImageIndexEntry('adult-media-post', ['adult'])] });
  const api = { searchCommunityNodeIndex, resolveCommunityIndexPosts } as unknown as DesktopApi;

  render(
    <CommunityIndexWorkspace
      {...workspaceProps(api, {
        mediaObjectUrls: { [INDEX_IMAGE_HASH]: 'blob:index-image' },
        adultContentEnabled: false,
      })}
    />
  );
  runSearch();

  expect(await screen.findByTestId('media-adult-gated-adult-media-post')).toBeInTheDocument();
  expect(screen.queryByTestId('media-preview-adult-media-post')).not.toBeInTheDocument();
});

test('unresolved and failed results keep rendering no media', async () => {
  const entry = indexEntry('unresolved-media', 'indexed text');
  const api = {
    searchCommunityNodeIndex: vi.fn().mockResolvedValue({ entries: [entry] }),
    resolveCommunityIndexPosts: vi.fn().mockResolvedValue({
      entries: [
        {
          key: `public_topic:rust:${entry.object_id}`,
          post: null,
          capabilities: {
            open_thread: false,
            reply: false,
            repost: false,
            quote_repost: false,
            react: false,
            copy_link: false,
            bookmark: false,
            withdraw: false,
          },
        },
      ],
    }),
  } as unknown as DesktopApi;

  render(
    <CommunityIndexWorkspace
      {...workspaceProps(api, {
        mediaObjectUrls: { [INDEX_IMAGE_HASH]: 'blob:index-image' },
      })}
    />
  );
  runSearch();

  await waitFor(() => expect(api.resolveCommunityIndexPosts).toHaveBeenCalledTimes(1));
  expect(screen.queryByTestId('media-preview-unresolved-media')).not.toBeInTheDocument();
  expect(screen.queryByTestId('media-skeleton-unresolved-media')).not.toBeInTheDocument();
  expect(screen.queryByTestId('media-adult-gated-unresolved-media')).not.toBeInTheDocument();
});

test('resolved posts are published for media prefetch and cleared when results expire', async () => {
  const onResolvedPostsChange = vi.fn();
  const searchCommunityNodeIndex = vi
    .fn()
    .mockResolvedValue({ entries: [indexEntry('prefetch-post', 'indexed text')] });
  const resolveCommunityIndexPosts = vi
    .fn()
    .mockResolvedValue({ entries: [resolvedImageIndexEntry('prefetch-post')] });
  const api = { searchCommunityNodeIndex, resolveCommunityIndexPosts } as unknown as DesktopApi;

  const { unmount } = render(
    <CommunityIndexWorkspace {...workspaceProps(api, { onResolvedPostsChange })} />
  );
  runSearch();

  await waitFor(() => {
    const published = onResolvedPostsChange.mock.calls.at(-1)?.[0] as PostView[] | undefined;
    expect(published?.map((post) => post.object_id)).toEqual(['prefetch-post']);
    expect(published?.[0]?.attachments.map((attachment) => attachment.hash)).toEqual([
      INDEX_IMAGE_HASH,
    ]);
  });

  unmount();
  expect(onResolvedPostsChange.mock.calls.at(-1)?.[0]).toEqual([]);
});

// #1192: 「見つける」カラムではカラム見出しと重複するため、カード内の見出しと説明文を出さない。
// トピック内カードはカラム見出しと文言が異なるため従来どおり残す。
test('the explore card drops the heading and summary that repeat the column title', () => {
  const api = {} as DesktopApi;
  const explore = render(
    <CommunityIndexWorkspace {...workspaceProps(api, { mode: 'explore' })} />
  );

  expect(screen.queryByRole('heading', { name: 'Community Index' })).not.toBeInTheDocument();
  expect(screen.queryByText(/across indexed topics/i)).not.toBeInTheDocument();
  expect(screen.getByRole('tablist', { name: 'Community Index surfaces' })).toBeInTheDocument();

  explore.unmount();
  render(<CommunityIndexWorkspace {...workspaceProps(api, { mode: 'topic' })} />);

  expect(screen.getByRole('heading', { name: 'Community Index' })).toBeInTheDocument();
});
