import type { AttachmentView, BookmarkedMediaView, CreateAttachmentInput, DesktopApi } from '@/lib/api';
import type { MockRuntime } from '../mockRuntime';

const savedByRuntime = new WeakMap<MockRuntime, Map<string, BookmarkedMediaView>>();
function saved(runtime: MockRuntime) {
  let rows = savedByRuntime.get(runtime);
  if (!rows) { rows = new Map(); savedByRuntime.set(runtime, rows); }
  return rows;
}

export function reusedMediaAttachments(runtime: MockRuntime, upload: CreateAttachmentInput): AttachmentView[] | null {
  if (!upload.bookmarked_media_hash) return null;
  const row = saved(runtime).get(upload.bookmarked_media_hash);
  if (!row) throw new Error('bookmarked media was not found');
  return row.attachments.map((asset) => ({ ...asset, origin: asset.origin ?? row.source }));
}

export function createMediaBookmarksMock(runtime: MockRuntime): Pick<DesktopApi, 'bookmarkMedia' | 'getBookmarkedMedia' | 'listBookmarkedMedia' | 'removeBookmarkedMedia'> {
  const rows = saved(runtime);
  return {
    async bookmarkMedia(topic, objectId, hash) {
      const post = runtime.postsByTopic[topic]?.find((post) => post.object_id === objectId);
      const asset = post?.attachments.find((asset) => asset.hash === hash);
      if (!post || !asset) throw new Error('media bookmark target was not found');
      const ordinal = post.attachments.slice(0, post.attachments.indexOf(asset)).filter((candidate) => candidate.role === asset.role).length;
      const attachments = ['video_manifest', 'video_poster'].includes(asset.role)
        ? [post.attachments.filter((candidate) => candidate.role === 'video_manifest')[ordinal], post.attachments.filter((candidate) => candidate.role === 'video_poster')[ordinal]].filter((asset): asset is AttachmentView => Boolean(asset)) : [asset];
      const primary = attachments.find((asset) => asset.role === 'video_manifest') ?? asset;
      const row: BookmarkedMediaView = { hash: primary.hash, bookmarked_from_object_id: objectId, source: primary.origin ?? { topic_id: topic, object_id: objectId, author_pubkey: post.author_pubkey, created_at: post.created_at },
        attachments, content_labels: post.content_labels ?? [], bookmarked_at: Date.now() };
      rows.set(row.hash, row);
      return structuredClone(row);
    },
    async getBookmarkedMedia(hash) { return structuredClone(rows.get(hash) ?? null); },
    async listBookmarkedMedia(cursor, before = false) {
      let items = [...rows.values()].sort((a, b) => b.bookmarked_at - a.bookmarked_at || b.hash.localeCompare(a.hash));
      if (cursor) items = items.filter((item) => before ? item.bookmarked_at > cursor.bookmarked_at || item.bookmarked_at === cursor.bookmarked_at && item.hash > cursor.hash : item.bookmarked_at < cursor.bookmarked_at || item.bookmarked_at === cursor.bookmarked_at && item.hash < cursor.hash);
      if (before) items.reverse();
      const more = items.length > 20;
      items = items.slice(0, 20);
      if (before) items.reverse();
      const at = (item: BookmarkedMediaView) => ({ hash: item.hash, bookmarked_at: item.bookmarked_at });
      return structuredClone({ items, newer_cursor: (before ? more : Boolean(cursor)) && items.length ? at(items[0]) : null, older_cursor: (before ? Boolean(cursor) : more) && items.length ? at(items[items.length - 1]) : null });
    },
    async removeBookmarkedMedia(hash) { rows.delete(hash); },
  };
}
