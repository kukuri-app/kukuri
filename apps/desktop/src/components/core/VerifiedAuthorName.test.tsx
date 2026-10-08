import { act, render, screen } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';

import { STORY_AUTHOR_DETAIL_VIEW } from '@/components/storyFixtures';
import { ProfileOverviewPanel } from '@/components/extended/ProfileOverviewPanel';
import { verifyProfileNip05 } from '@/lib/profileNip05';

import { AuthorDetailCard } from './AuthorDetailCard';
import { PostCard } from './PostCard';
import { createView } from './PostCard.testHelpers';
import { VerifiedAuthorName } from './VerifiedAuthorName';

vi.mock('@/lib/profileNip05', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/profileNip05')>()),
  verifyProfileNip05: vi.fn(),
}));

const verify = vi.mocked(verifyProfileNip05);
const SOURCE = 'b'.repeat(64);

beforeEach(() => {
  verify.mockReset();
  verify.mockImplementation(async (_pubkey, nip05) => nip05.split('@')[1]);
});

// #1670 AC-2.1: 確認できた著者だけ、名前の後ろに @domain を出す。確認できない・識別子が無いときは名前だけで、照会しない。
test('post card shows the verified domain of the primary author only', async () => {
  const view = createView();
  const { rerender } = render(
    <PostCard
      view={{ ...view, post: { ...view.post, author_nip05: 'alice@example.com' } }}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );
  expect(await screen.findByText('@example.com')).toBeInTheDocument();
  expect(verify).toHaveBeenCalledWith(view.post.author_pubkey, 'alice@example.com');

  verify.mockResolvedValue(null);
  rerender(
    <PostCard
      view={{ ...view, post: { ...view.post, author_nip05: 'alice@other.example' } }}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );
  await vi.waitFor(() => expect(verify).toHaveBeenCalledTimes(2));
  expect(screen.queryByText(/^@/)).not.toBeInTheDocument();

  verify.mockClear();
  rerender(
    <PostCard view={view} onOpenAuthor={() => undefined} onOpenThread={() => undefined} onReply={() => undefined} />
  );
  expect(screen.queryByText(/^@/)).not.toBeInTheDocument();
  expect(verify).not.toHaveBeenCalled();
});

test('a pure repost verifies the original author, not the reposter', async () => {
  const view = createView();
  render(
    <PostCard
      view={{
        ...view,
        repostSourceAuthor: { pubkey: SOURCE, label: 'Source Author', picture: null },
        post: {
          ...view.post,
          author_nip05: 'reposter@example.com',
          object_kind: 'repost',
          content: '',
          repost_commentary: null,
          repost_of: {
            source_object_id: 'source-1',
            source_topic_id: 'kukuri:topic:source',
            source_author_pubkey: SOURCE,
            source_author_display_name: 'Source Author',
            source_author_nip05: 'source@source.example',
            source_object_kind: 'post',
            content: 'original body',
            attachments: [],
          },
        },
      }}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );
  expect(await screen.findByText('@source.example')).toBeInTheDocument();
  expect(verify).toHaveBeenCalledTimes(1);
  expect(verify).toHaveBeenCalledWith(SOURCE, 'source@source.example');
});

test('author detail and own profile overview show the verified domain', async () => {
  const author = { ...STORY_AUTHOR_DETAIL_VIEW.author!, nip05: 'bob@bob.example' };
  render(
    <>
      <AuthorDetailCard
        view={{ ...STORY_AUTHOR_DETAIL_VIEW, author }}
        localAuthorPubkey={'f'.repeat(64)}
        onToggleRelationship={vi.fn()}
        onToggleMute={vi.fn()}
      />
      <ProfileOverviewPanel
        authorLabel='Me'
        pubkey={'f'.repeat(64)}
        nip05='me@me.example'
        username={null}
        about={null}
        picture={null}
        status='ready'
        error={null}
        postCount={0}
        followingCount={0}
        followedCount={0}
        mutedCount={0}
        blockingCount={0}
        onEdit={vi.fn()}
        onOpenFollowing={vi.fn()}
        onOpenFollowed={vi.fn()}
        onOpenMuted={vi.fn()}
        onOpenBlocking={vi.fn()}
      />
    </>
  );
  expect(await screen.findByText('@bob.example')).toBeInTheDocument();
  expect(await screen.findByText('@me.example')).toBeInTheDocument();
  expect(verify).toHaveBeenCalledWith(author.author_pubkey, 'bob@bob.example');
  expect(verify).toHaveBeenCalledWith('f'.repeat(64), 'me@me.example');
});

// #1670 AC-2.2: 画面外の間と、画面が隠れている間は照会しない。
test('does not ask while offscreen or while the document is hidden', async () => {
  let notify: IntersectionObserverCallback = () => undefined;
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      constructor(callback: IntersectionObserverCallback) {
        notify = callback;
      }
      observe() {}
      disconnect() {}
      unobserve() {}
      takeRecords() { return []; }
      readonly root = null;
      readonly rootMargin = '0px';
      readonly thresholds = [0];
    }
  );
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  render(<VerifiedAuthorName label='Alice' pubkey={'a'.repeat(64)} nip05='alice@example.com' />);
  expect(verify).not.toHaveBeenCalled();

  visibility.mockReturnValue('hidden');
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  act(() =>
    notify([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver)
  );
  expect(verify).not.toHaveBeenCalled();

  visibility.mockReturnValue('visible');
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  expect(await screen.findByText('@example.com')).toBeInTheDocument();
  expect(verify).toHaveBeenCalledOnce();
  visibility.mockRestore();
  vi.unstubAllGlobals();
});
