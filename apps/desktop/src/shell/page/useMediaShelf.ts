import { useContext, useEffect, useMemo, useRef, useState } from 'react';
import type { BookmarkedMediaView } from '@/lib/api';
import { useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';
import { resolvePostAdvisory } from '@/shell/contentAdvisories';
import { createObjectUrlFromPayload } from '@/shell/media';
import { MediaBookmarksContext } from '@/components/core/mediaBookmarksContext';

/** 画像・動画の一覧の 1 ページ(20 件まで)と、前後のページの位置。 */
export type MediaShelfPage<C> = { items: BookmarkedMediaView[]; newer_cursor: C | null; older_cursor: C | null };

const EMPTY_HASHES: readonly string[] = [];

/**
 * 画像・動画の一覧(ブックマークの選択・メディアの列、#1949)。ページを置き換えて読み、表示中の行を `owner` の名で advisory
 * の照会へ加える。表示してよい行だけ、1 件ずつ poster か画像のプレビューを読む(動画の本体は読まない)。
 */
export function useMediaShelf<C>(owner: string, active: boolean, fetchPage: (cursor: C | null, before: boolean) => Promise<MediaShelfPage<C>>) {
  const context = useContext(MediaBookmarksContext);
  const api = context?.api;
  const gatedHashes = context?.gatedMediaHashes ?? EMPTY_HASHES;
  const adultEnabled = useDesktopShellStore((state) => state.adultContentEnabled);
  const advisories = useDesktopShellStore((state) => state.timelineContentAdvisories);
  const lookup = useDesktopShellStore((state) => state.timelineAdvisoryLookup);
  const setAdvisoryPreviews = useDesktopShellFieldSetter('mediaBookmarkPreviews');
  const [page, setPage] = useState<MediaShelfPage<C> | null>(null);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  const [previews, setPreviews] = useState<Record<string, string>>({});
  const advisoryPosts = useMemo(() => active ? (page?.items ?? []).map((item) => ({
    object_id: item.bookmarked_from_object_id || item.source.object_id, attachments: item.attachments,
    content_labels: item.content_labels, repost_of: null, reply_preview: null,
  })) : [], [active, page]);
  useEffect(() => {
    setAdvisoryPreviews((current) => ({ ...current, [owner]: advisoryPosts }));
    return () => setAdvisoryPreviews((current) => {
      const next = { ...current };
      delete next[owner];
      return next;
    });
  }, [advisoryPosts, owner, setAdvisoryPreviews]);
  const allowedSignature = advisoryPosts.flatMap((post, index) => {
    const gate = resolvePostAdvisory(post, advisories, lookup);
    return !gate.pending && (adultEnabled || (!post.content_labels.includes('adult') && !gate.advisory && !post.attachments.some((asset) => gatedHashes.includes(asset.hash)))) ? [page!.items[index].hash] : [];
  }).join('|');
  const previewItems = useMemo(() => {
    const allowed = new Set(allowedSignature.split('|'));
    return (page?.items ?? []).filter((item) => allowed.has(item.hash));
  }, [allowedSignature, page]);
  const urls = useRef(new Map<string, string>());
  const request = useRef(0);
  const load = async (cursor: C | null = null, before = false) => {
    const revision = ++request.current;
    setLoading(true); setFailed(false);
    try {
      const result = await fetchPage(cursor, before);
      if (request.current === revision) setPage(result);
    } catch { if (request.current === revision) setFailed(true); }
    finally { if (request.current === revision) setLoading(false); }
  };
  const reset = () => { request.current += 1; setPage(null); };
  useEffect(() => {
    if (!active || !api || !page) return;
    let current = true;
    const owned = new Map<string, string>();
    urls.current = owned;
    setPreviews({});
    void (async () => {
      for (const item of previewItems) {
        if (!current) break;
        const preview = item.attachments.find((asset) => asset.role === 'video_poster') ?? item.attachments.find((asset) => asset.mime.startsWith('image/'));
        if (!preview) continue;
        try {
          const payload = await api.getBlobMediaPayload(preview.hash, preview.mime);
          if (!current || !payload) continue;
          const url = createObjectUrlFromPayload(payload);
          owned.set(item.hash, url);
          setPreviews((shown) => ({ ...shown, [item.hash]: url }));
        } catch { /* previewが無くても一覧と操作は使える */ }
      }
    })().catch(() => undefined);
    return () => { current = false; owned.forEach((url) => URL.revokeObjectURL(url)); owned.clear(); };
  }, [active, api, page, previewItems]);
  const preview = (hash: string) => previewItems.some((row) => row.hash === hash) ? previews[hash] : undefined;
  /** 表示中のプレビューの URL の所有を呼出元へ移す(下書きへ渡す)。 */
  const takePreview = (hash: string) => {
    const url = preview(hash) ? urls.current.get(hash) ?? '' : '';
    urls.current.delete(hash);
    return url;
  };
  const drop = (hashes: readonly string[]) => setPage((shown) => shown ? { ...shown, items: shown.items.filter((row) => !hashes.includes(row.hash)) } : shown);
  return { api, page, loading, failed, setFailed, load, reset, preview, takePreview, drop };
}

