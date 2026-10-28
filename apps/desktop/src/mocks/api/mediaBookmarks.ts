import type { AttachmentView, BookmarkedMediaView, CreateAttachmentInput, DesktopApi, MediaShelf } from '@/lib/api';
import type { MockRuntime } from '../mockRuntime';

const shelvesByRuntime = new WeakMap<MockRuntime, Record<MediaShelf, Map<string, BookmarkedMediaView>>>();
function shelves(runtime: MockRuntime) {
  let rows = shelvesByRuntime.get(runtime);
  if (!rows) { rows = { bookmarks: new Map(), cache: new Map() }; shelvesByRuntime.set(runtime, rows); }
  return rows;
}

/** 投稿の `hash` の画像・動画(動画は本体と poster の組)の一覧の行。 */
function mediaRow(runtime: MockRuntime, objectId: string, hash: string): BookmarkedMediaView | null {
  const [topic, post] = Object.entries(runtime.postsByTopic).flatMap(([topic, posts]) => posts.filter((post) => post.object_id === objectId).map((post) => [topic, post] as const))[0] ?? [];
  const asset = post?.attachments.find((asset) => asset.hash === hash);
  if (!topic || !post || !asset) return null;
  const ordinal = post.attachments.slice(0, post.attachments.indexOf(asset)).filter((candidate) => candidate.role === asset.role).length;
  const attachments = ['video_manifest', 'video_poster'].includes(asset.role)
    ? [post.attachments.filter((candidate) => candidate.role === 'video_manifest')[ordinal], post.attachments.filter((candidate) => candidate.role === 'video_poster')[ordinal]].filter((asset): asset is AttachmentView => Boolean(asset)) : [asset];
  const primary = attachments.find((asset) => asset.role === 'video_manifest') ?? asset;
  return { hash: primary.hash, bookmarked_from_object_id: objectId, source: primary.origin ?? { topic_id: topic, object_id: objectId, author_pubkey: post.author_pubkey, created_at: post.created_at },
    attachments, content_labels: post.content_labels ?? [], bookmarked_at: Date.now() };
}

/** 他人の投稿の画像・動画を表示したら、キャッシュの一覧へ載せる(ブックマーク済みは除く。#1949)。 */
export function recordCachedMedia(runtime: MockRuntime, objectId: string, hash: string) {
  const row = mediaRow(runtime, objectId, hash);
  const { bookmarks, cache } = shelves(runtime);
  if (row && row.source.author_pubkey !== runtime.syncStatus.local_author_pubkey && !bookmarks.has(row.hash) && !cache.has(row.hash)) cache.set(row.hash, row);
}

export function reusedMediaAttachments(runtime: MockRuntime, upload: CreateAttachmentInput): AttachmentView[] | null {
  if (!upload.bookmarked_media_hash) return null;
  const row = shelves(runtime).bookmarks.get(upload.bookmarked_media_hash);
  if (!row) throw new Error('bookmarked media was not found');
  return row.attachments.map((asset) => ({ ...asset, origin: asset.origin ?? row.source }));
}

export function createMediaBookmarksMock(runtime: MockRuntime): Pick<DesktopApi, 'bookmarkMedia' | 'getBookmarkedMedia' | 'listBookmarkedMedia' | 'removeBookmarkedMedia'> {
  const rows = shelves(runtime);
  return {
    async bookmarkMedia(_topic, objectId, hash) {
      const row = mediaRow(runtime, objectId, hash);
      if (!row) throw new Error('media bookmark target was not found');
      rows.cache.delete(row.hash);
      rows.bookmarks.set(row.hash, row);
      return structuredClone(row);
    },
    async getBookmarkedMedia(hash) { return structuredClone(rows.bookmarks.get(hash) ?? null); },
    async listBookmarkedMedia(cursor, before = false, shelf = 'bookmarks') {
      let items = [...rows[shelf].values()].sort((a, b) => b.bookmarked_at - a.bookmarked_at || b.hash.localeCompare(a.hash));
      if (cursor) items = items.filter((item) => before ? item.bookmarked_at > cursor.bookmarked_at || item.bookmarked_at === cursor.bookmarked_at && item.hash > cursor.hash : item.bookmarked_at < cursor.bookmarked_at || item.bookmarked_at === cursor.bookmarked_at && item.hash < cursor.hash);
      if (before) items.reverse();
      const more = items.length > 20;
      items = items.slice(0, 20);
      if (before) items.reverse();
      const at = (item: BookmarkedMediaView) => ({ hash: item.hash, bookmarked_at: item.bookmarked_at });
      return structuredClone({ items, newer_cursor: (before ? more : Boolean(cursor)) && items.length ? at(items[0]) : null, older_cursor: (before ? Boolean(cursor) : more) && items.length ? at(items[items.length - 1]) : null });
    },
    async removeBookmarkedMedia(hash, shelf = 'bookmarks') {
      const row = rows[shelf].get(hash);
      rows[shelf].delete(hash);
      if (shelf === 'bookmarks' && row) rows.cache.set(hash, { ...row, bookmarked_at: Date.now() });
    },
  };
}
