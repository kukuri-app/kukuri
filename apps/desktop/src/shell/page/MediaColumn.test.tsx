import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createInstance } from 'i18next';
import { I18nextProvider } from 'react-i18next';
import { expect, test, vi } from 'vitest';
import { createDesktopShellStore, DesktopShellStoreContext, useDesktopShellStore } from '@/shell/store';
import { resources } from '@/i18n';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import type { AttachmentView, BookmarkedMediaView, PostView } from '@/lib/api';
import { MediaBookmarksContext } from '@/components/core/mediaBookmarksContext';
import { columnIdentityId, type ColumnState } from '@/shell/slices/workspace';
import { MediaColumn, MediaShelfTabs } from './MediaColumn';

const me = 'f'.repeat(64);
const image: AttachmentView = { hash: 'a'.repeat(64), mime: 'image/png', bytes: 1, role: 'image_original', status: 'Available' };
const row = (hash: string): BookmarkedMediaView => ({ hash, bookmarked_from_object_id: `post-${hash}`, attachments: [{ ...image, hash }], content_labels: [], bookmarked_at: 1,
  source: { topic_id: 'topic', object_id: `post-${hash}`, author_pubkey: 'b'.repeat(64), created_at: 1 } });
const column: ColumnState = { id: columnIdentityId('media'), kind: 'media', pinned: true, preferredDesktopSpan: 1 };

function Column() {
  const current = useDesktopShellStore((state) => state.workspaceState.columns.find((candidate) => candidate.id === column.id)!);
  return <><MediaShelfTabs column={current} /><MediaColumn column={current} /></>;
}

async function setup(overrides: Partial<ReturnType<typeof createDesktopMockApi>>, withdrawPost = vi.fn(async () => true)) {
  const i18n = createInstance();
  await i18n.init({ resources, lng: 'ja', fallbackLng: 'en', defaultNS: 'common' });
  const api = { ...createDesktopMockApi(), getBlobMediaPayload: vi.fn(async () => null), ...overrides };
  const store = createDesktopShellStore();
  store.getState().setField('timelineAdvisoryLookup', { active: false, settled: {} });
  store.getState().setField('syncStatus', (current) => ({ ...current, local_author_pubkey: me }));
  store.getState().setField('workspaceState', (current) => ({ ...current, columns: [...current.columns, column] }));
  render(<I18nextProvider i18n={i18n}><DesktopShellStoreContext.Provider value={store}><MediaBookmarksContext.Provider value={{ api, withdrawPost }}>
    <Column />
  </MediaBookmarksContext.Provider></DesktopShellStoreContext.Provider></I18nextProvider>);
  return { api, store, withdrawPost };
}

test('media column switches cache, bookmarks and posts in the header and keeps the choice per column', async () => {
  const listBookmarkedMedia = vi.fn(async (_cursor, _before, shelf) => ({ items: [row(shelf === 'cache' ? 'c'.repeat(64) : 'd'.repeat(64))], newer_cursor: null, older_cursor: null }));
  const { store } = await setup({ listBookmarkedMedia });
  const tabs = screen.getByRole('tablist', { name: 'メディアの表示' });
  expect(within(tabs).getByRole('tab', { name: 'キャッシュ' })).toHaveAttribute('aria-selected', 'true');
  await screen.findByRole('img', { name: '画像添付' });
  expect(listBookmarkedMedia).toHaveBeenLastCalledWith(null, false, 'cache');
  within(tabs).getByRole('tab', { name: 'キャッシュ' }).focus();
  await userEvent.keyboard('{ArrowRight}');
  expect(within(tabs).getByRole('tab', { name: 'ブックマーク' })).toHaveFocus();
  await waitFor(() => expect(listBookmarkedMedia).toHaveBeenLastCalledWith(null, false, 'bookmarks'));
  expect(store.getState().workspaceState.columns.find((candidate) => candidate.id === column.id)?.mediaShelf).toBe('bookmarks');
});

test('cache and bookmark tiles delete from their own list', async () => {
  const listBookmarkedMedia = vi.fn(async () => ({ items: [row('c'.repeat(64))], newer_cursor: null, older_cursor: null }));
  const removeBookmarkedMedia = vi.fn(async () => undefined);
  await setup({ listBookmarkedMedia, removeBookmarkedMedia });
  await userEvent.click(await screen.findByRole('button', { name: 'キャッシュから削除' }));
  expect(removeBookmarkedMedia).toHaveBeenCalledWith('c'.repeat(64), 'cache');
  await screen.findByText('キャッシュした画像・動画はありません。');
  await userEvent.click(screen.getByRole('tab', { name: 'ブックマーク' }));
  await userEvent.click(await screen.findByRole('button', { name: 'メディアのブックマークを解除' }));
  expect(removeBookmarkedMedia).toHaveBeenLastCalledWith('c'.repeat(64), 'bookmarks');
});

test('posts list own image and video media, skips reposts and withdrawn posts, and withdraws after confirmation', async () => {
  const post = (id: string, attachments: AttachmentView[], extra: Partial<PostView> = {}) => ({ object_id: id, author_pubkey: me, attachments, created_at: 1, published_topic_id: 'topic', ...extra }) as PostView;
  const video = { ...image, hash: 'e'.repeat(64), mime: 'video/mp4', role: 'video_manifest' };
  const poster = { ...image, hash: '9'.repeat(64), role: 'video_poster' };
  const listProfileTimeline = vi.fn(async () => ({ items: [
    post('own', [image, video, poster]), post('repost', [image], { repost_of: { source_object_id: 'x' } as unknown as PostView['repost_of'] }),
    post('withdrawn', [image], { withdrawal: {} as unknown as PostView['withdrawal'] }), post('text', []),
  ], next_cursor: null, unavailable_count: 0 }));
  const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(false).mockReturnValueOnce(true);
  const { store, withdrawPost } = await setup({ listProfileTimeline });
  store.getState().setField('workspaceState', (current) => ({ ...current, columns: current.columns.map((candidate) => candidate.id === column.id ? { ...candidate, mediaShelf: 'posts' as const } : candidate) }));
  expect(await screen.findAllByRole('button', { name: '投稿を撤回' })).toHaveLength(2);
  expect(screen.getByRole('img', { name: '動画' })).toBeInTheDocument();
  expect(listProfileTimeline).toHaveBeenCalledWith(me, null, 20);
  await userEvent.click(screen.getAllByRole('button', { name: '投稿を撤回' })[0]);
  expect(withdrawPost).not.toHaveBeenCalled();
  await userEvent.click(screen.getAllByRole('button', { name: '投稿を撤回' })[0]);
  expect(withdrawPost).toHaveBeenCalledWith(expect.objectContaining({ object_id: 'own' }));
  await screen.findByText('画像・動画のある投稿はありません。');
  confirm.mockRestore();
});

test('posts pages hold at most 20 media and continue inside a read batch', async () => {
  const posts = Array.from({ length: 15 }, (_, index) => ({ object_id: `p${index}`, author_pubkey: me, created_at: 15 - index, published_topic_id: 'topic',
    attachments: [0, 1].map((n) => ({ ...image, hash: `${index}-${n}` })) }) as PostView);
  const listProfileTimeline = vi.fn(async () => ({ items: posts, next_cursor: null, unavailable_count: 0 }));
  const { store } = await setup({ listProfileTimeline });
  store.getState().setField('workspaceState', (current) => ({ ...current, columns: current.columns.map((candidate) => candidate.id === column.id ? { ...candidate, mediaShelf: 'posts' as const } : candidate) }));
  await waitFor(() => expect(screen.getAllByRole('img', { name: '画像添付' })).toHaveLength(20));
  await userEvent.click(screen.getByRole('button', { name: '次のページ' }));
  await waitFor(() => expect(screen.getAllByRole('img', { name: '画像添付' })).toHaveLength(10));
  expect(listProfileTimeline).toHaveBeenLastCalledWith(me, null, 20);
  expect(screen.getByRole('button', { name: '次のページ' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: '前のページ' }));
  await waitFor(() => expect(screen.getAllByRole('img', { name: '画像添付' })).toHaveLength(20));
});
