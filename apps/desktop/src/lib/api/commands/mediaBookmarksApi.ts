import type { DesktopApi, BookmarkedMediaView, BookmarkedMediaPageView } from '../types';
import { command } from '../invoke/dispatch';
import { invokeDesktop } from '../invoke/desktop';

export const mediaBookmarksApi: Pick<DesktopApi, 'bookmarkMedia' | 'getBookmarkedMedia' | 'listBookmarkedMedia' | 'removeBookmarkedMedia'> = {
  bookmarkMedia: command('bookmarkMedia', (topic, objectId, hash, channelRef = { kind: 'public' }) =>
    invokeDesktop<BookmarkedMediaView>('bookmark_media', { request: { topic, object_id: objectId, hash, channel_ref: channelRef } })),
  getBookmarkedMedia: command('getBookmarkedMedia', (hash) =>
    invokeDesktop<BookmarkedMediaView | null>('get_bookmarked_media', { request: { hash } })),
  listBookmarkedMedia: command('listBookmarkedMedia', (cursor = null, before = false, shelf = 'bookmarks') =>
    invokeDesktop<BookmarkedMediaPageView>('list_bookmarked_media', { request: { cursor, before, shelf } })),
  removeBookmarkedMedia: command('removeBookmarkedMedia', (hash, shelf = 'bookmarks') =>
    invokeDesktop<void>('remove_bookmarked_media', { request: { hash, shelf } })),
};
