import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createInstance } from 'i18next';
import { I18nextProvider } from 'react-i18next';
import { expect, test, vi } from 'vitest';
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
const saved: BookmarkedMediaView = { hash: image.hash, source, attachments: [image], content_labels: [], bookmarked_at: 10 };

async function setup(children: React.ReactNode, overrides: Partial<ReturnType<typeof createDesktopMockApi>> = {}) {
  const i18n = createInstance();
  await i18n.init({ resources, lng: 'ja', fallbackLng: 'en', defaultNS: 'common' });
  const api = { ...createDesktopMockApi(), getBookmarkedMedia: vi.fn(async () => null as BookmarkedMediaView | null), ...overrides };
  render(<I18nextProvider i18n={i18n}><MediaBookmarksContext.Provider value={{ api }}>{children}</MediaBookmarksContext.Provider></I18nextProvider>);
  return api;
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
  await userEvent.click(within(dialog).getByRole('button', { name: 'blobハッシュをコピー' }));
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
  await screen.findByText(`画像添付 · ${image.hash.slice(0, 12)}`);
  await userEvent.click(screen.getByRole('button', { name: '次のページ' }));
  const choice = await screen.findByRole('button', { name: /動画 · video/ });
  expect(screen.queryByText(`画像添付 · ${image.hash.slice(0, 12)}`)).not.toBeInTheDocument();
  expect(list).toHaveBeenLastCalledWith(cursor, false);
  await userEvent.click(choice);
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
