import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';


import { PostCard } from './PostCard';
import { type PostCardView } from './types';
import type { LinkPreviewOutcome } from '@/lib/api';

import { createView } from './PostCard.testHelpers';
function setViewportWidth(width: number) {
  Object.defineProperty(window, 'innerWidth', {
    configurable: true,
    writable: true,
    value: width,
  });
  window.dispatchEvent(new Event('resize'));
}

test('post card hides the object kind and shows a placeholder avatar when no picture is available', () => {
  render(
    <PostCard
      view={createView()}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  expect(screen.queryByText(/^post$/i)).not.toBeInTheDocument();
  expect(screen.getByText('core contributors')).toHaveClass('post-meta-chip');
  expect(screen.getByTestId('post-1-author-avatar')).toHaveTextContent('A');
});

test('post card renders an absolute HTTP URL as an external link without changing adjacent text', () => {
  const url = 'https://example.test/articles/1174?q=ogp';
  const base = createView();
  const onOpenThread = vi.fn();

  render(
    <PostCard
      view={createView({
        post: {
          ...base.post,
          content: `URL before ${url}. URL after`,
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={onOpenThread}
      onReply={() => undefined}
    />
  );

  const link = screen.getByRole('link', { name: url });
  expect(link).toHaveAttribute('href', url);
  expect(screen.getByText('URL before')).toBeInTheDocument();
  expect(screen.getByText('. URL after')).toBeInTheDocument();
  link.addEventListener('click', (event) => event.preventDefault());
  fireEvent.click(link);
  expect(onOpenThread).not.toHaveBeenCalled();
});

test('post card requests one preview for an eligible public primary URL', async () => {
  const url = 'https://example.test/articles/1174';
  const base = createView();
  const onOpenThread = vi.fn();
  const fetcher = vi.fn(async (requestedUrl: string): Promise<LinkPreviewOutcome> => ({
    status: 'available',
    preview: {
      url: requestedUrl,
      source_label: 'example.test',
      title: 'Issue 1174 preview',
      description: null,
      image_data_url: null,
    },
  }));

  render(
    <PostCard
      enableLinkPreview
      linkPreviewFetcher={fetcher}
      view={createView({
        post: {
          ...base.post,
          content: `first ${url} second https://second.example/path`,
          channel_id: null,
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={onOpenThread}
      onReply={() => undefined}
    />
  );

  const preview = await screen.findByRole('link', {
    name: 'Issue 1174 preview — example.test',
  });
  expect(preview).toHaveAttribute('href', url);
  expect(fetcher).toHaveBeenCalledOnce();
  expect(fetcher).toHaveBeenCalledWith(url);
  expect(screen.getByRole('link', { name: 'https://second.example/path' })).toBeInTheDocument();
  preview.addEventListener('click', (event) => event.preventDefault());
  fireEvent.click(preview);
  expect(onOpenThread).not.toHaveBeenCalled();
});

test.each([
  ['private channel', { channel_id: 'private-1' }, {}],
  ['local pending', { channel_id: null, local_state: 'pending' as const }, {}],
  ['adult gate', { channel_id: null }, { adultContentGated: true }],
  [
    'trust gate',
    { channel_id: null },
    {
      trustGate: {
        authorPubkey: 'a'.repeat(64),
        nodeBaseUrl: 'https://node.example',
        reasons: ['risk_signals' as const],
        fromRepostSource: false,
      },
    },
  ],
])('post card does not request a preview for %s content', async (_label, postOverride, viewOverride) => {
  const base = createView();
  const fetcher = vi.fn(async (): Promise<LinkPreviewOutcome> => ({
    status: 'unavailable',
    reason: 'network',
  }));

  render(
    <PostCard
      enableLinkPreview
      linkPreviewFetcher={fetcher}
      view={createView({
        ...viewOverride,
        post: {
          ...base.post,
          content: 'https://example.test/private',
          ...postOverride,
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  await waitFor(() => expect(fetcher).not.toHaveBeenCalled());
});

test('post card renders the author image when one is available', () => {
  render(
    <PostCard
      view={createView({ authorPicture: 'blob:avatar' })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  expect(screen.getByTestId('post-1-author-avatar').querySelector('img')).toHaveAttribute(
    'src',
    'blob:avatar'
  );
});

test('post card omits unavailable body and media from normal UI', () => {
  const base = createView();
  render(
    <PostCard
      view={createView({
        post: {
          ...base.post,
          content: '[blob pending]',
          content_status: 'Missing',
          attachments: [
            {
              hash: 'b'.repeat(64),
              mime: 'image/png',
              bytes: 2048,
              role: 'image_original',
              status: 'Missing',
            },
          ],
        },
        media: {
          ...base.media,
          kind: 'image',
          state: 'unavailable',
          metaMime: 'image/png',
          metaBytesLabel: '2.0 KB',
        },
        showUnavailableDiagnostics: false,
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  expect(screen.queryByText('[blob pending]')).not.toBeInTheDocument();
  expect(screen.queryByText('Content unavailable.')).not.toBeInTheDocument();
  // #1207: 取得不可のメディアは通常 mode でも失敗表示に置き換える(再取得の提供元が無ければ button は出さない)。
  expect(screen.getByText('Failed to load.')).toHaveAttribute('role', 'status');
  expect(screen.queryByRole('button', { name: 'Retry loading' })).not.toBeInTheDocument();
  expect(screen.queryByText('image/png')).not.toBeInTheDocument();
  expect(screen.queryByText('2.0 KB')).not.toBeInTheDocument();
});

test('post card exposes concise unavailable diagnostics in developer mode', () => {
  const base = createView();
  render(
    <PostCard
      view={createView({
        post: {
          ...base.post,
          content: '[blob pending]',
          content_status: 'Missing',
        },
        media: {
          ...base.media,
          kind: 'image',
          state: 'unavailable',
        },
        showUnavailableDiagnostics: true,
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  expect(screen.getByText('Content unavailable.')).toHaveAttribute('role', 'status');
  expect(screen.getByText('Failed to load.')).toHaveAttribute('role', 'status');
  expect(screen.queryByText('[blob pending]')).not.toBeInTheDocument();
});

test('clicking the author avatar triggers the same author action as the name', async () => {
  const user = userEvent.setup();
  const onOpenAuthor = vi.fn();

  render(
    <PostCard
      view={createView()}
      onOpenAuthor={onOpenAuthor}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  await user.click(screen.getByTestId('post-1-author-avatar'));

  expect(onOpenAuthor).toHaveBeenCalledWith('a'.repeat(64));
});

test('withdrawn post shows an author placeholder and disables content interactions', () => {
  const base = createView();
  render(
    <PostCard
      view={createView({
        post: {
          ...base.post,
          content: '',
          attachments: [],
          withdrawal: {
            withdrawn_at: 1,
            replacement_object_id: null,
            reason_visibility: 'public',
            reason: 'author_request',
          },
        },
      })}
      localAuthorPubkey={base.post.author_pubkey}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onWithdraw={() => undefined}
    />
  );

  expect(screen.getByText('This post was withdrawn by the user who posted it.')).toBeInTheDocument();
  expect(screen.queryByText('hello')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Reply' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Withdraw post' })).not.toBeInTheDocument();
});

test('author can confirm withdrawal from an active post', async () => {
  const user = userEvent.setup();
  const view = createView();
  const onWithdraw = vi.fn();
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
  render(
    <PostCard
      view={view}
      localAuthorPubkey={view.post.author_pubkey}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onWithdraw={onWithdraw}
    />
  );

  await user.click(screen.getByRole('button', { name: 'Withdraw post' }));
  expect(confirm).toHaveBeenCalledOnce();
  expect(onWithdraw).toHaveBeenCalledWith(view.post);
  confirm.mockRestore();
});

test('post card renders repost source context for quote reposts', () => {
  render(
    <PostCard
      view={createView({
        post: {
          ...createView().post,
          object_kind: 'repost',
          content: 'adding context',
          repost_commentary: 'adding context',
          repost_of: {
            source_object_id: 'source-1',
            source_topic_id: 'kukuri:topic:source',
            source_author_pubkey: 'b'.repeat(64),
            source_author_display_name: 'Source Author',
            source_author_name: null,
            source_object_kind: 'post',
            content: 'original body',
            attachments: [],
            reply_to: null,
            root_id: 'source-1',
          },
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  expect(screen.getByText('Repost with comment')).toBeInTheDocument();
  expect(screen.getByText('Source Author')).toBeInTheDocument();
  expect(screen.getByText('original body')).toBeInTheDocument();
});

function createReplyView(overrides?: Partial<PostCardView>): PostCardView {
  return createView({
    replyParentAuthor: { pubkey: 'b'.repeat(64), label: 'Parent Author', picture: null },
    post: {
      ...createView().post,
      reply_to: 'parent-1',
      reply_preview: {
        object_id: 'parent-1',
        topic: 'kukuri:topic:source',
        author: {
          pubkey: 'b'.repeat(64),
          name: 'parent-author',
          display_name: 'Parent Author',
          picture_asset: null,
        },
        content: 'parent body',
        attachments: [],
        root_id: 'parent-1',
        reply_to: null,
      },
    },
    ...overrides,
  });
}

test('post card renders a compact reply context distinct from a quote repost', () => {
  render(
    <PostCard
      view={createReplyView()}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  expect(screen.getByText('Replying to Parent Author')).toBeInTheDocument();
  // The parent avatar is a separate button labelled with the author name.
  expect(screen.getByRole('button', { name: 'Parent Author' })).toBeInTheDocument();
  expect(screen.getByText('parent body')).toBeInTheDocument();
  // Reply context is its own block, not the quote/repost source card.
  expect(document.querySelector('.post-reply-context')).not.toBeNull();
  expect(document.querySelector('.repost-source-card')).toBeNull();
  const parent = screen.getByText('parent body');
  const replyCard = screen.getByText('hello').closest('article')!;
  expect(replyCard).not.toContainElement(parent);
  expect(parent.compareDocumentPosition(replyCard) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test('post timestamp includes the full local date and seconds', () => {
  const view = createView();
  view.post.created_at = new Date(2026, 8, 14, 3, 32, 1).getTime() / 1000;
  const { container } = render(<PostCard view={view} onOpenAuthor={vi.fn()} onOpenThread={vi.fn()} onReply={vi.fn()} />);
  const time = container.querySelector('time');
  expect(time).toHaveAttribute('datetime', new Date(view.post.created_at * 1000).toISOString());
  expect(time).toHaveTextContent('2026');
  expect(time).toHaveTextContent('09');
  expect(time).toHaveTextContent('14');
  expect(time).toHaveTextContent(/3:32:01/);
});

test('post card hides the reply context in thread/tree context', () => {
  render(
    <PostCard
      view={createReplyView({ context: 'thread', suppressReplyPreview: true })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  expect(screen.queryByText('Replying to Parent Author')).not.toBeInTheDocument();
  expect(document.querySelector('.post-reply-context')).toBeNull();
  // The reply's own content still renders.
  expect(screen.getByText('hello')).toBeInTheDocument();
});

test('reply context uses the immediate parent and preserves author and reply targets', async () => {
  const view = createReplyView();
  view.post.root_id = 'root-ancestor';
  view.post.reply_preview!.root_id = 'root-ancestor';
  view.post.reply_preview!.reply_to = 'root-ancestor';
  const onOpenAuthor = vi.fn();
  const onReply = vi.fn();
  const onOpenThread = vi.fn();
  render(<PostCard view={view} onOpenAuthor={onOpenAuthor} onOpenThread={onOpenThread} onReply={onReply} />);
  await userEvent.click(screen.getByText('parent body'));
  expect(onOpenThread).toHaveBeenCalledWith(view.threadTargetId);
  screen.getByText('parent body').closest('[role="button"]')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  expect(onOpenThread).toHaveBeenCalledTimes(2);
  await userEvent.click(screen.getByRole('button', { name: 'Parent Author' }));
  expect(onOpenAuthor).toHaveBeenCalledWith('b'.repeat(64));
  await userEvent.click(screen.getByRole('button', { name: /^Reply$/ }));
  expect(onReply).toHaveBeenCalledWith(view.post);
  expect(screen.getByText('parent body')).toBeVisible();
  expect(screen.queryByText('root-ancestor')).not.toBeInTheDocument();
});

test.each(['missing', 'gated'] as const)('reply context stays hidden when %s', (state) => {
  const view = createReplyView();
  if (state === 'missing') view.post.reply_preview = null;
  else view.adultContentGated = true;
  const { container } = render(<PostCard view={view} onOpenAuthor={vi.fn()} onOpenThread={vi.fn()} onReply={vi.fn()} />);
  expect(container.querySelector('.post-reply-context')).toBeNull();
  expect(screen.queryByText('parent body')).not.toBeInTheDocument();
});

test('attachment-only parent shows a compact label without loading its media', () => {
  const view = createReplyView();
  view.post.reply_preview!.content = '';
  view.post.reply_preview!.attachments = [{ hash: 'd'.repeat(64), mime: 'video/mp4', bytes: 20, role: 'video_original', status: 'Available' }];
  const { container } = render(<PostCard view={view} onOpenAuthor={vi.fn()} onOpenThread={vi.fn()} onReply={vi.fn()} />);
  const context = container.querySelector('.post-reply-context')!;
  expect(context.querySelector('.post-reply-context-body')).not.toBeEmptyDOMElement();
  expect(context.querySelector('video, img, audio')).toBeNull();
});

test('post card promotes the original post for a pure repost', async () => {
  const user = userEvent.setup();
  const onOpenAuthor = vi.fn();

  render(
    <PostCard
      view={createView({
        canReply: false,
        repostSourceAuthor: { pubkey: 'b'.repeat(64), label: 'Source Author', picture: null },
        post: {
          ...createView().post,
          object_kind: 'repost',
          content: '',
          repost_commentary: null,
          repost_of: {
            source_object_id: 'source-1',
            source_topic_id: 'kukuri:topic:source',
            source_author_pubkey: 'b'.repeat(64),
            source_author_display_name: 'Source Author',
            source_author_name: null,
            source_object_kind: 'post',
            content: 'original body',
            attachments: [],
            reply_to: null,
            root_id: 'source-root',
          },
        },
      })}
      onOpenAuthor={onOpenAuthor}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  // Reposter is demoted to a small attribution header…
  expect(screen.getByText('Alice reposted')).toBeInTheDocument();
  // …and the original post becomes the primary content.
  expect(screen.getByText('original body')).toBeInTheDocument();
  // The primary identity is now the source author.
  expect(screen.getByTestId('post-1-author-avatar')).toHaveTextContent('S');

  await user.click(screen.getByTestId('post-1-author-avatar'));
  expect(onOpenAuthor).toHaveBeenCalledWith('b'.repeat(64));
});

test('post card hides technical identifiers even when developer diagnostics are enabled', () => {
  const longContent = 'channel_payload_'.repeat(48);
  const longEnvelopeId = 'f'.repeat(192);

  render(
    <PostCard
      view={createView({
        post: {
          ...createView().post,
          content: longContent,
          envelope_id: longEnvelopeId,
        },
        showUnavailableDiagnostics: true,
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  expect(screen.getByText(longContent)).toHaveClass('post-copy-wrap');
  expect(screen.queryByText(longEnvelopeId)).not.toBeInTheDocument();
  expect(screen.queryByText('post-1')).not.toBeInTheDocument();
});

test('post card copies hidden post identifiers from pointer and keyboard context menus', async () => {
  const user = userEvent.setup();
  const clipboardWriteText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });
  const view = createView();

  render(
    <PostCard
      view={view}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  const target = screen.getByTestId('post-identifier-target');
  fireEvent.contextMenu(target, { clientX: 24, clientY: 36 });
  await user.click(screen.getByRole('menuitem', { name: 'Copy post ID' }));
  expect(clipboardWriteText).toHaveBeenLastCalledWith(view.post.object_id);

  target.focus();
  fireEvent.keyDown(target, { key: 'F10', shiftKey: true });
  await user.click(screen.getByRole('menuitem', { name: 'Copy envelope ID' }));
  expect(clipboardWriteText).toHaveBeenLastCalledWith(view.post.envelope_id);
});

test('post card context menu exposes only identifiers supplied by a read-only result', async () => {
  const user = userEvent.setup();
  const clipboardWriteText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });

  render(
    <PostCard
      view={createView({
        identifierCopy: {
          postId: 'indexed-object-1',
          authorId: 'b'.repeat(64),
        },
      })}
      readOnly
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  const target = screen.getByTestId('post-identifier-target');
  fireEvent.contextMenu(target, { clientX: 24, clientY: 36 });
  expect(screen.getByRole('menuitem', { name: 'Copy post ID' })).toBeInTheDocument();
  expect(screen.getByRole('menuitem', { name: 'Copy user ID' })).toBeInTheDocument();
  expect(screen.queryByRole('menuitem', { name: 'Copy envelope ID' })).not.toBeInTheDocument();

  await user.click(screen.getByRole('menuitem', { name: 'Copy post ID' }));
  expect(clipboardWriteText).toHaveBeenLastCalledWith('indexed-object-1');
});

test('post card opens a media dialog and navigates multi-image attachments', async () => {
  const user = userEvent.setup();

  render(
    <PostCard
      view={createView({
        media: {
          ...createView().media,
          kind: 'image',
          imagePreviewSrc: 'https://example.com/one.png',
          imageGalleryItems: [
            {
              hash: 'image-1',
              src: 'https://example.com/one.png',
              mime: 'image/png',
            },
            {
              hash: 'image-2',
              src: 'https://example.com/two.png',
              mime: 'image/png',
            },
          ],
          currentImageIndex: 0,
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  await user.click(screen.getByRole('button', { name: 'image attachment' }));

  const dialog = screen.getByRole('dialog');
  expect(dialog).toHaveClass('media-viewer-dialog');
  expect(dialog.querySelector('.media-viewer-counter')).toBeNull();
  expect(within(dialog).getByRole('img', { name: 'image attachment' })).toHaveAttribute(
    'src',
    'https://example.com/one.png'
  );

  await user.click(within(dialog).getByRole('button', { name: 'Next image' }));

  expect(within(dialog).getByRole('img', { name: 'image attachment' })).toHaveAttribute(
    'src',
    'https://example.com/two.png'
  );
});

test('simple repost opens the source thread in its published topic', async () => {
  const user = userEvent.setup();
  const onOpenThread = vi.fn();
  const onOpenThreadInTopic = vi.fn();

  render(
    <PostCard
      view={createView({
        canReply: false,
        threadTargetId: 'source-root',
        threadTopicId: 'kukuri:topic:source',
        post: {
          ...createView().post,
          object_kind: 'repost',
          content: '',
          repost_commentary: null,
          repost_of: {
            source_object_id: 'source-1',
            source_topic_id: 'kukuri:topic:source',
            source_author_pubkey: 'b'.repeat(64),
            source_author_display_name: 'Source Author',
            source_author_name: null,
            source_object_kind: 'post',
            content: 'original body',
            attachments: [],
            reply_to: null,
            root_id: 'source-root',
          },
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={onOpenThread}
      onOpenThreadInTopic={onOpenThreadInTopic}
      onReply={() => undefined}
    />
  );

  await user.click(screen.getByRole('button', { name: /Source Author/i }));

  expect(onOpenThread).not.toHaveBeenCalled();
  expect(onOpenThreadInTopic).toHaveBeenCalledWith('source-root', 'kukuri:topic:source');
});

test('post card opens a custom reaction context menu and keeps the reaction popover search flow', async () => {
  setViewportWidth(280);
  const user = userEvent.setup();
  const onToggleReaction = vi.fn();
  const onBookmarkCustomReaction = vi.fn();
  const clipboardWriteText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText: clipboardWriteText,
    },
  });
  const customAsset = {
    asset_id: 'parrot-asset',
    owner_pubkey: 'b'.repeat(64),
    blob_hash: 'blob-1',
    search_key: 'party-parrot',
    mime: 'image/png',
    bytes: 128,
    width: 128,
    height: 128,
  };
  const bookmarkedAsset = {
    asset_id: 'asset-2',
    owner_pubkey: 'c'.repeat(64),
    blob_hash: 'blob-2',
    search_key: 'saved-cat',
    mime: 'image/gif',
    bytes: 128,
    width: 128,
    height: 128,
  };
  const view = createView({
    post: {
      ...createView().post,
      reaction_summary: [
        {
          reaction_key_kind: 'emoji',
          normalized_reaction_key: 'emoji:👍',
          emoji: '👍',
          custom_asset: null,
          count: 2,
        },
        {
          reaction_key_kind: 'custom_asset',
          normalized_reaction_key: 'custom_asset:parrot-asset',
          emoji: null,
          custom_asset: customAsset,
          count: 1,
        },
      ],
      my_reactions: [
        {
          reaction_key_kind: 'emoji',
          normalized_reaction_key: 'emoji:👍',
          emoji: '👍',
          custom_asset: null,
        },
      ],
    },
  });

  render(
    <PostCard
      view={view}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      localAuthorPubkey={'a'.repeat(64)}
      mediaObjectUrls={{
        'blob-1': 'https://example.com/reaction.png',
        'blob-2': 'https://example.com/bookmarked.png',
      }}
      bookmarkedReactionAssets={[bookmarkedAsset]}
      recentReactions={[
        {
          reaction_key_kind: 'emoji',
          normalized_reaction_key: 'emoji:🔥',
          emoji: '🔥',
          custom_asset: null,
          updated_at: 2,
        },
      ]}
      onToggleReaction={onToggleReaction}
      onBookmarkCustomReaction={onBookmarkCustomReaction}
    />
  );

  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();

  await user.click(screen.getAllByRole('button', { name: /👍/ })[0]);
  expect(onToggleReaction).toHaveBeenNthCalledWith(1, view.post, { kind: 'emoji', emoji: '👍' });

  const customReactionChip = screen.getByRole('button', {
    name: `${customAsset.search_key} 1`,
  });
  expect(within(customReactionChip).queryByText(customAsset.search_key)).not.toBeInTheDocument();
  expect(customReactionChip).toHaveAttribute('aria-label', `${customAsset.search_key} 1`);
  expect(customReactionChip).toHaveAttribute('data-tooltip', customAsset.search_key);
  expect(customReactionChip).toHaveAccessibleName(`${customAsset.search_key} 1`);
  expect(customReactionChip).not.toHaveAccessibleName(new RegExp(customAsset.asset_id, 'i'));
  expect(screen.queryByText(customAsset.asset_id.slice(0, 6))).not.toBeInTheDocument();

  await user.hover(customReactionChip);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(customAsset.search_key);
  await user.unhover(customReactionChip);
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());

  customReactionChip.focus();
  expect(await screen.findByRole('tooltip')).toHaveTextContent(customAsset.search_key);
  customReactionChip.blur();
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());

  fireEvent.contextMenu(customReactionChip);
  await user.click(screen.getByRole('menuitem', { name: 'Copy hash' }));
  expect(clipboardWriteText).toHaveBeenCalledWith(customAsset.blob_hash);
  expect(onToggleReaction).toHaveBeenCalledTimes(1);

  fireEvent.contextMenu(customReactionChip);
  await user.click(screen.getByRole('menuitem', { name: 'Save' }));
  expect(onBookmarkCustomReaction).toHaveBeenCalledWith(customAsset);

  await user.click(screen.getByRole('button', { name: 'React' }));
  expect(screen.queryByRole('button', { name: 'Manage reactions' })).not.toBeInTheDocument();
  expect(screen.getByText('Recent')).toBeInTheDocument();
  expect(screen.getByText('Emoji')).toBeInTheDocument();
  expect(screen.getByText('Custom')).toBeInTheDocument();
  const reactionPopover = screen.getByPlaceholderText('Search reactions').closest('.post-reaction-popover');
  expect(reactionPopover).toHaveClass('post-reaction-popover-wide');
  expect(reactionPopover).not.toHaveClass('post-action-popover');
  expect(reactionPopover).toHaveStyle({ width: '248px' });
  expect(reactionPopover).toHaveStyle({ '--reaction-grid-columns': '6' });
  expect(screen.queryByText(bookmarkedAsset.asset_id)).not.toBeInTheDocument();
  const emojiSection = screen.getByText('Emoji').closest('section');
  if (!(emojiSection instanceof HTMLElement)) {
    throw new Error('emoji section not found');
  }
  expect(emojiSection.querySelector('.post-reaction-picker-grid-8')).not.toBeNull();
  expect(
    within(emojiSection).getByRole('button', { name: 'thumbs-up' })
  ).toHaveAttribute('data-tooltip', 'thumbs-up');
  expect(screen.getByRole('button', { name: /saved-cat/i })).toHaveAttribute(
    'data-tooltip',
    'saved-cat'
  );
  setViewportWidth(1024);
  await waitFor(() => {
    expect(reactionPopover).toHaveStyle({ width: '410px' });
    expect(reactionPopover).toHaveStyle({ '--reaction-grid-columns': '8' });
  });
  await user.click(
    within(screen.getByText('Recent').closest('section') as HTMLElement).getByRole('button', {
      name: 'fire',
    })
  );
  expect(onToggleReaction).toHaveBeenNthCalledWith(2, view.post, { kind: 'emoji', emoji: '🔥' });

  await user.click(screen.getByRole('button', { name: 'React' }));
  await user.type(screen.getByPlaceholderText('Search reactions'), 'saved');
  await user.click(screen.getByRole('button', { name: /saved-cat/i }));
  expect(onToggleReaction).toHaveBeenNthCalledWith(3, view.post, {
    kind: 'custom_asset',
    asset: bookmarkedAsset,
  });
});

test('post card keeps an unresolved custom reaction identifiable without showing its full name', async () => {
  const user = userEvent.setup();
  const onToggleReaction = vi.fn();
  const customAsset = {
    asset_id: 'unresolved-asset',
    owner_pubkey: 'b'.repeat(64),
    blob_hash: 'blob-unresolved',
    search_key: 'very-long-custom-reaction-name',
    mime: 'image/png',
    bytes: 128,
    width: 128,
    height: 128,
  };
  const view = createView({
    post: {
      ...createView().post,
      reaction_summary: [
        {
          reaction_key_kind: 'custom_asset',
          normalized_reaction_key: `custom_asset:${customAsset.asset_id}`,
          emoji: null,
          custom_asset: customAsset,
          count: 3,
        },
      ],
      my_reactions: [],
    },
  });

  render(
    <PostCard
      view={view}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onToggleReaction={onToggleReaction}
    />
  );

  const customReactionChip = screen.getByRole('button', {
    name: `${customAsset.search_key} 3`,
  });
  expect(within(customReactionChip).queryByText(customAsset.search_key)).not.toBeInTheDocument();
  expect(within(customReactionChip).getByText(customAsset.search_key.slice(0, 2))).toHaveAttribute(
    'aria-hidden',
    'true'
  );
  expect(customReactionChip).toHaveAttribute('data-tooltip', customAsset.search_key);

  await user.hover(customReactionChip);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(customAsset.search_key);
  await user.click(customReactionChip);
  expect(onToggleReaction).toHaveBeenCalledWith(view.post, {
    kind: 'custom_asset',
    asset: customAsset,
  });
});

test('read-only post card hides reaction affordances and keeps the original topic action', async () => {
  const user = userEvent.setup();
  const onOpenOriginalTopic = vi.fn();

  render(
    <PostCard
      view={createView({
        post: {
          ...createView().post,
          published_topic_id: 'kukuri:topic:source',
          reaction_summary: [
            {
              reaction_key_kind: 'emoji',
              normalized_reaction_key: 'emoji:👍',
              emoji: '👍',
              custom_asset: null,
              count: 3,
            },
          ],
        },
      })}
      readOnly
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onOpenOriginalTopic={onOpenOriginalTopic}
    />
  );

  expect(screen.queryByRole('button', { name: 'React' })).not.toBeInTheDocument();
  expect(screen.queryByText('3')).not.toBeInTheDocument();

  await user.click(screen.getByRole('button', { name: 'Open original topic' }));
  expect(onOpenOriginalTopic).toHaveBeenCalledWith('kukuri:topic:source');
});

test('post card renders bookmark as an icon-only action with an accessible label', async () => {
  const user = userEvent.setup();
  const onToggleBookmark = vi.fn();

  render(
    <PostCard
      view={createView()}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      showBookmarkAction
      isBookmarked
      onToggleBookmark={onToggleBookmark}
    />
  );

  const bookmarkButton = screen.getByRole('button', { name: 'Remove bookmark' });
  expect(bookmarkButton).toHaveAttribute('aria-pressed', 'true');
  expect(bookmarkButton).toHaveClass('post-action-button-active');
  expect(bookmarkButton).not.toHaveTextContent(/bookmark/i);

  await user.click(bookmarkButton);
  expect(onToggleBookmark).toHaveBeenCalledWith(createView().post);
});
