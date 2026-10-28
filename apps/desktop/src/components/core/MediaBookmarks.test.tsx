import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createInstance } from 'i18next';
import { I18nextProvider } from 'react-i18next';
import { expect, test, vi } from 'vitest';
import { act } from '@testing-library/react';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';
import { resources } from '@/i18n';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import type { AttachmentView, BookmarkedMediaView } from '@/lib/api';
import { MediaBookmarksContext } from './mediaBookmarksContext';
import { MediaAssetActions } from '@/shell/page/MediaAssetActions';
import { MediaBookmarkPicker } from '@/shell/page/MediaBookmarkPicker';
import { PostMedia } from './PostMedia';
import type { MediaBookmarkTarget } from './types';

const source = { topic_id: 'topic', object_id: 'original-post', author_pubkey: 'a'.repeat(64), created_at: 1_700_000_000_000 };
const target: MediaBookmarkTarget = { topic: 'reused-topic', objectId: 'reused-post', channelRef: { kind: 'public' }, source: { ...source, object_id: 'reused-post', author_pubkey: 'b'.repeat(64) } };
const image: AttachmentView = { hash: 'c'.repeat(64), mime: 'image/png', bytes: 24, role: 'image_original', status: 'Available', origin: source };
const saved: BookmarkedMediaView = { bookmarked_from_object_id: source.object_id, hash: image.hash, source, attachments: [image], content_labels: [], bookmarked_at: 10 };

async function setup(children: React.ReactNode, overrides: Partial<ReturnType<typeof createDesktopMockApi>> = {}, gatedMediaHashes: readonly string[] = []) {
  const i18n = createInstance();
  await i18n.init({ resources, lng: 'ja', fallbackLng: 'en', defaultNS: 'common' });
  const api = { ...createDesktopMockApi(), getBookmarkedMedia: vi.fn(async () => null as BookmarkedMediaView | null), ...overrides };
  const store = createDesktopShellStore();
  store.getState().setField('timelineAdvisoryLookup', { active: false, settled: {} });
  render(<I18nextProvider i18n={i18n}><DesktopShellStoreContext.Provider value={store}><MediaBookmarksContext.Provider value={{ api, gatedMediaHashes }}>{children}</MediaBookmarksContext.Provider></DesktopShellStoreContext.Provider></I18nextProvider>);
  return { ...api, store };
}

test('media bookmark is a ghost button and saves independently without opening the image', async () => {
  const bookmarkMedia = vi.fn(async () => saved);
  const removeBookmarkedMedia = vi.fn(async () => undefined);
  const open = vi.fn();
  const api = await setup(<MediaAssetActions attachment={image} target={target}><button onClick={open}>画像</button></MediaAssetActions>, { bookmarkMedia, removeBookmarkedMedia });
  const button = screen.getByRole('button', { name: 'メディアをブックマーク' });
  expect(button.className).toContain('ghost');
  await userEvent.click(button);
  expect(bookmarkMedia).toHaveBeenCalledWith(target.topic, target.objectId, image.hash, target.channelRef);
  expect(screen.getByRole('button', { name: 'メディアのブックマークを解除' })).toHaveAttribute('aria-pressed', 'true');
  await userEvent.click(screen.getByRole('button', { name: 'メディアのブックマークを解除' }));
  expect(removeBookmarkedMedia).toHaveBeenCalledWith(image.hash);
  expect(open).not.toHaveBeenCalled();
  expect(api.getBookmarkedMedia).toHaveBeenCalledWith(image.hash);
});

test('right click and Shift+F10 show original source metadata, copy it and restore focus', async () => {
  const clipboard = vi.fn<(value: string) => Promise<void>>(async () => undefined);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: clipboard } });
  await setup(<MediaAssetActions attachment={image} target={target}><img src='test.png' alt='test media' /></MediaAssetActions>);
  const group = screen.getByRole('group');
  fireEvent.contextMenu(screen.getByAltText('test media'));
  const dialog = screen.getByRole('dialog', { name: 'メディアの情報' });
  expect(within(dialog).getByText(source.author_pubkey)).toBeVisible();
  expect(within(dialog).getByText(source.object_id)).toBeVisible();
  await userEvent.click(within(dialog).getByRole('button', { name: 'ハッシュをコピー' }));
  expect(clipboard).toHaveBeenLastCalledWith(image.hash);
  await userEvent.click(within(dialog).getByRole('button', { name: 'すべてのメタデータをコピー' }));
  expect(JSON.parse(clipboard.mock.calls.at(-1)![0] as string).origin).toEqual(source);
  await userEvent.keyboard('{Escape}');
  await waitFor(() => expect(group).toHaveFocus());
  fireEvent.keyDown(group, { key: 'F10', shiftKey: true });
  expect(screen.getByRole('dialog', { name: 'メディアの情報' })).toBeVisible();
});

test('picker pages replace the window and reuse the video and poster without loading video bytes', async () => {
  const video: BookmarkedMediaView = { ...saved, hash: 'video', attachments: [{ ...image, hash: 'video', mime: 'video/mp4', role: 'video_manifest' }, { ...image, hash: 'poster', role: 'video_poster' }] };
  const cursor = { bookmarked_at: 10, hash: 'page-end' };
  const list = vi.fn().mockResolvedValueOnce({ items: [saved], newer_cursor: null, older_cursor: cursor }).mockResolvedValueOnce({ items: [video], newer_cursor: cursor, older_cursor: null });
  const select = vi.fn();
  const getBlobMediaPayload = vi.fn<(hash: string, mime: string) => Promise<null>>(async () => null);
  await setup(<MediaBookmarkPicker onSelect={select} />, { listBookmarkedMedia: list, getBlobMediaPayload });
  await userEvent.click(screen.getByRole('button', { name: 'ブックマークから選択' }));
  await screen.findByRole('button', { name: '画像添付' });
  const dialog = screen.getByRole('dialog', { name: '画像・動画のブックマーク' });
  expect(dialog).not.toHaveAttribute('aria-describedby');
  expect(within(dialog).queryByText(new RegExp(`${image.hash.slice(0, 12)}|${source.author_pubkey.slice(0, 16)}`))).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: '次のページ' }));
  const choice = await screen.findByRole('button', { name: '動画' });
  expect(screen.queryByRole('button', { name: '画像添付' })).not.toBeInTheDocument();
  expect(list).toHaveBeenLastCalledWith(cursor, false);
  await userEvent.click(choice);
  expect(select.mock.calls[0][0].source_name).toBe('動画');
  expect(select.mock.calls[0][0].attachments[0]).toMatchObject({ data_base64: '', bookmarked_media_hash: 'video', reused_attachments: video.attachments });
  expect(getBlobMediaPayload.mock.calls.some((call) => call[0] === 'video')).toBe(false);
  await waitFor(() => expect(screen.getByRole('button', { name: 'ブックマークから選択' })).toHaveFocus());
});

test('gated post media never exposes media actions or fetches bookmarks', async () => {
  const api = await setup(<PostMedia media={{ objectId: 'gated', kind: 'image', state: 'gated', extraAttachmentCount: 0, videoUnsupportedOnClient: false, bookmarkTarget: target, imageGalleryItems: [{ hash: image.hash, src: 'test.png', mime: image.mime, attachment: image }] }} />);
  expect(screen.queryByRole('button', { name: 'メディアをブックマーク' })).not.toBeInTheDocument();
  expect(api.getBookmarkedMedia).not.toHaveBeenCalled();
});

test.each([false, true])('video media uses the main bookmark and the displayed attachment metadata (playable=%s)', async (playable) => {
  const video = { ...image, hash: 'video-hash', mime: 'video/mp4', role: 'video_manifest' };
  const poster = { ...image, hash: 'poster-hash', role: 'video_poster' };
  const bookmarkMedia = vi.fn(async () => ({ ...saved, hash: video.hash, attachments: [video, poster] }));
  await setup(<PostMedia media={{ objectId: 'video', kind: 'video', state: 'ready', extraAttachmentCount: 0, videoUnsupportedOnClient: !playable,
    videoAttachment: video, posterAttachment: poster, videoPosterHash: poster.hash, videoReportHash: video.hash, videoPosterPreviewSrc: 'poster.png',
    videoPlaybackSrc: playable ? 'video.mp4' : null, bookmarkTarget: target }} />, { bookmarkMedia });
  await userEvent.click(screen.getByRole('button', { name: 'メディアをブックマーク' }));
  expect(bookmarkMedia).toHaveBeenCalledWith(target.topic, target.objectId, video.hash, target.channelRef);
  fireEvent.contextMenu(screen.getByTestId(playable ? 'media-video-video' : 'media-preview-video'));
  const dialog = screen.getByRole('dialog', { name: 'メディアの情報' });
  expect(within(dialog).getByText(playable ? video.hash : poster.hash)).toBeVisible();
});

test('picker shows a load failure, retries and cancels without selecting', async () => {
  const select = vi.fn();
  const list = vi.fn().mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce({ items: [], newer_cursor: null, older_cursor: null });
  await setup(<MediaBookmarkPicker onSelect={select} />, { listBookmarkedMedia: list });
  await userEvent.click(screen.getByRole('button', { name: 'ブックマークから選択' }));
  const failure = await screen.findByRole('alert');
  await userEvent.click(within(failure).getByRole('button'));
  await screen.findByText('画像・動画のブックマークはありません。');
  await userEvent.keyboard('{Escape}');
  expect(select).not.toHaveBeenCalled();
  await waitFor(() => expect(screen.getByRole('button', { name: 'ブックマークから選択' })).toHaveFocus());
});

test.each(['post_id', 'blob_cid'] as const)('picker blocks previews while advisory is pending or gated (%s)', async (kind) => {
  const row = { ...saved, bookmarked_from_object_id: 'bookmarked-reuse-post' };
  const payload = vi.fn(async () => ({ mime: 'image/png', bytes_base64: 'AA==' }));
  const revoke = vi.spyOn(URL, 'revokeObjectURL');
  const api = await setup(<MediaBookmarkPicker onSelect={() => undefined} />, {
    listBookmarkedMedia: async () => ({ items: [row], newer_cursor: null, older_cursor: null }), getBlobMediaPayload: payload,
  });
  act(() => api.store.getState().setField('timelineAdvisoryLookup', { active: true, settled: {} }));
  await userEvent.click(screen.getByRole('button', { name: 'ブックマークから選択' }));
  await screen.findByRole('button', { name: '画像添付' });
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)); });
  expect(payload).not.toHaveBeenCalled();
  expect(api.store.getState().mediaBookmarkPreviews.picker[0].object_id).toBe(row.bookmarked_from_object_id);
  const id = kind === 'post_id' ? row.bookmarked_from_object_id : image.hash;
  act(() => {
    api.store.getState().setField('timelineContentAdvisories', { [`${kind}:${id}`]: [{ nodeBaseUrl: 'https://node.example', advisory: {
      issuer_node_id: 'd'.repeat(64), subject_kind: kind, subject_id: id, category: 'nsfw', label: 'adult', confidence: 84, signal_id: 'signal-1', basis: 'classifier_score',
    } }] });
    api.store.getState().setField('timelineAdvisoryLookup', { active: true, settled: { [`post_id:${row.bookmarked_from_object_id}`]: true, [`blob_cid:${image.hash}`]: true } });
  });
  expect(payload).not.toHaveBeenCalled();
  act(() => api.store.getState().setField('adultContentEnabled', true));
  await waitFor(() => expect(payload).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(document.querySelector('.media-bookmark-choice img')).not.toBeNull());
  act(() => api.store.getState().setField('adultContentEnabled', false));
  expect(document.querySelector('.media-bookmark-choice img')).toBeNull();
  await waitFor(() => expect(revoke).toHaveBeenCalled());
  await userEvent.click(document.querySelector('.ui-dialog-close') as HTMLButtonElement);
  await waitFor(() => expect(Object.values(api.store.getState().mediaBookmarkPreviews).flat()).toEqual([]));
});

test('picker applies the shared hash gate even without an advisory on the bookmarked post', async () => {
  const payload = vi.fn(async () => null);
  await setup(<MediaBookmarkPicker onSelect={() => undefined} />, {
    listBookmarkedMedia: async () => ({ items: [saved], newer_cursor: null, older_cursor: null }), getBlobMediaPayload: payload,
  }, [image.hash]);
  await userEvent.click(screen.getByRole('button', { name: 'ブックマークから選択' }));
  await screen.findByRole('button', { name: '画像添付' });
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)); });
  expect(payload).not.toHaveBeenCalled();
});
