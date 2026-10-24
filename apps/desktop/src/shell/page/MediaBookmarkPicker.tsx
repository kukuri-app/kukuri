import { useContext, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Bookmark } from 'lucide-react';
import type { BookmarkedMediaPageView, BookmarkedMediaView, MediaBookmarkCursor } from '@/lib/api';
import type { DraftMediaItem } from '@/shell/store';
import { createObjectUrlFromPayload } from '@/shell/media';
import { IconButton } from '@/components/ui/icon-button';
import { Button } from '@/components/ui/button';
import { Dialog, DialogBody, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog';
import { MediaBookmarksContext, MEDIA_BOOKMARK_CHANGED } from '@/components/core/mediaBookmarksContext';

export function MediaBookmarkPicker({ disabled, onSelect }: { disabled?: boolean; onSelect: (item: DraftMediaItem) => void }) {
  const api = useContext(MediaBookmarksContext)?.api;
  const { t } = useTranslation('common');
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState<BookmarkedMediaPageView | null>(null);
  const [loading, setLoading] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [failed, setFailed] = useState(false);
  const [previews, setPreviews] = useState<Record<string, string>>({});
  const urls = useRef(new Map<string, string>());
  const request = useRef(0);
  const load = async (cursor: MediaBookmarkCursor | null = null, before = false) => {
    if (!api) return;
    const revision = ++request.current;
    setLoading(true); setFailed(false);
    try {
      const result = await api.listBookmarkedMedia(cursor, before);
      if (request.current === revision) setPage(result);
    } catch { if (request.current === revision) setFailed(true); }
    finally { if (request.current === revision) setLoading(false); }
  };
  useEffect(() => {
    if (!open || !api || !page) return;
    let active = true;
    const owned = new Map<string, string>();
    urls.current = owned;
    setPreviews({});
    // 1 件ずつpreviewを読む。動画の本体は一覧では取得しない。
    void (async () => {
      const settings = await api.getContentDisplaySettings();
      for (const item of page.items) {
        if (!active) break;
        if (!settings.adult_content_enabled && item.content_labels.includes('adult')) continue;
        const preview = item.attachments.find((asset) => asset.role === 'video_poster') ?? item.attachments.find((asset) => asset.mime.startsWith('image/'));
        if (!preview) continue;
        try {
          const payload = await api.getBlobMediaPayload(preview.hash, preview.mime);
          if (!active || !payload) continue;
          const url = createObjectUrlFromPayload(payload);
          owned.set(item.hash, url);
          setPreviews((current) => ({ ...current, [item.hash]: url }));
        } catch { /* previewが無くてもメタデータから選べる */ }
      }
    })().catch(() => undefined);
    return () => { active = false; owned.forEach((url) => URL.revokeObjectURL(url)); owned.clear(); };
  }, [api, open, page]);
  const remove = async (item: BookmarkedMediaView) => {
    if (!api || removing) return;
    setRemoving(true);
    try {
      await api.removeBookmarkedMedia(item.hash);
      window.dispatchEvent(new CustomEvent(MEDIA_BOOKMARK_CHANGED, { detail: { hash: item.hash, saved: false } }));
      setPage((current) => current ? { ...current, items: current.items.filter((row) => row.hash !== item.hash) } : current);
    } catch { setFailed(true); }
    finally { setRemoving(false); }
  };
  const select = (item: BookmarkedMediaView) => {
    const primary = item.attachments.find((asset) => asset.hash === item.hash) ?? item.attachments[0];
    if (!primary) return;
    const preview = urls.current.get(item.hash) ?? '';
    urls.current.delete(item.hash); // 選んだURLの所有を下書きへ渡す。
    onSelect({ id: crypto.randomUUID(), source_name: `${t(primary.mime.startsWith('video/') ? 'media.video' : 'media.imageAlt')} · ${item.hash.slice(0, 12)}`,
      preview_url: preview, attachments: [{ mime: primary.mime, byte_size: primary.bytes, role: primary.role, data_base64: '', bookmarked_media_hash: item.hash,
        reused_attachments: item.attachments.map((asset) => ({ ...asset, origin: asset.origin ?? item.source })) }] });
    setOpen(false);
  };
  if (!api) return null;
  return <Dialog open={open} onOpenChange={(next) => { setOpen(next); if (next) void load(); else { request.current += 1; setPage(null); } }}>
    <DialogTrigger asChild><Button type='button' variant='secondary' disabled={disabled}><Bookmark className='size-4' aria-hidden='true' />{t('mediaBookmarks.choose')}</Button></DialogTrigger>
    <DialogContent className='media-bookmark-picker'>
      <DialogHeader><DialogTitle>{t('mediaBookmarks.title')}</DialogTitle><DialogDescription>{t('mediaBookmarks.description')}</DialogDescription></DialogHeader>
      <DialogBody>
        {loading ? <p role='status'>{t('mediaBookmarks.loading')}</p> : failed ? <div role='alert'><p>{t('mediaBookmarks.loadFailed')}</p><Button type='button' variant='secondary' onClick={() => void load()}>{t('actions.retry')}</Button></div> : page?.items.length === 0 ? <p>{t('mediaBookmarks.empty')}</p> : null}
        <div className='media-bookmark-grid' aria-busy={loading}>{page?.items.map((item) => <div key={item.hash} className='relative min-w-0'><Button type='button' variant='ghost' className='media-bookmark-choice' disabled={loading || disabled || removing} onClick={() => select(item)}>
          {previews[item.hash] ? <img src={previews[item.hash]} alt='' /> : <span className='media-bookmark-placeholder'><Bookmark aria-hidden='true' /></span>}
          <span>{item.attachments.some((asset) => asset.role === 'video_manifest') ? t('media.video') : t('media.imageAlt')} · {item.hash.slice(0, 12)}</span>
          <small>{item.source.author_pubkey.slice(0, 16)}</small>
        </Button><IconButton className='media-bookmark-button' type='button' variant='ghost' label={t('mediaBookmarks.remove')} aria-pressed='true' disabled={loading || removing} onClick={() => void remove(item)}><Bookmark className='size-4 fill-current' aria-hidden='true' /></IconButton></div>)}</div>
        <div className='flex justify-between gap-2'>
          <Button type='button' variant='secondary' disabled={loading || !page?.newer_cursor} onClick={() => void load(page?.newer_cursor, true)}>{t('mediaBookmarks.newer')}</Button>
          <Button type='button' variant='secondary' disabled={loading || !page?.older_cursor} onClick={() => void load(page?.older_cursor)}>{t('mediaBookmarks.older')}</Button>
        </div>
      </DialogBody>
    </DialogContent>
  </Dialog>;
}
