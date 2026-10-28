import { useContext, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Bookmark } from 'lucide-react';
import type { BookmarkedMediaView, MediaBookmarkCursor } from '@/lib/api';
import type { DraftMediaItem } from '@/shell/store';
import { IconButton } from '@/components/ui/icon-button';
import { Button } from '@/components/ui/button';
import { Dialog, DialogBody, DialogContent, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog';
import { MediaBookmarksContext, MEDIA_BOOKMARK_CHANGED } from '@/components/core/mediaBookmarksContext';
import { MediaShelfView } from '@/shell/page/MediaShelfView';
import { useMediaShelf } from '@/shell/page/useMediaShelf';

export function MediaBookmarkPicker({ disabled, onSelect }: { disabled?: boolean; onSelect: (item: DraftMediaItem) => void }) {
  const api = useContext(MediaBookmarksContext)?.api;
  const { t } = useTranslation('common');
  const [open, setOpen] = useState(false);
  const [removing, setRemoving] = useState(false);
  const shelf = useMediaShelf<MediaBookmarkCursor>('picker', open, (cursor, before) => api!.listBookmarkedMedia(cursor, before));
  const remove = async (item: BookmarkedMediaView) => {
    if (!api || removing) return;
    setRemoving(true);
    try {
      await api.removeBookmarkedMedia(item.hash);
      window.dispatchEvent(new CustomEvent(MEDIA_BOOKMARK_CHANGED, { detail: { hash: item.hash, saved: false } }));
      shelf.drop([item.hash]);
    } catch { shelf.setFailed(true); }
    finally { setRemoving(false); }
  };
  const select = (item: BookmarkedMediaView) => {
    const primary = item.attachments.find((asset) => asset.hash === item.hash) ?? item.attachments[0];
    if (!primary) return;
    onSelect({ id: crypto.randomUUID(), source_name: t(primary.mime.startsWith('video/') ? 'media.video' : 'media.imageAlt'),
      preview_url: shelf.takePreview(item.hash), attachments: [{ mime: primary.mime, byte_size: primary.bytes, role: primary.role, data_base64: '', bookmarked_media_hash: item.hash,
        reused_attachments: item.attachments.map((asset) => ({ ...asset, origin: asset.origin ?? item.source })) }] });
    setOpen(false);
  };
  if (!api) return null;
  return <Dialog open={open} onOpenChange={(next) => { setOpen(next); if (next) void shelf.load(); else shelf.reset(); }}>
    <DialogTrigger asChild><Button type='button' variant='secondary' disabled={disabled}><Bookmark className='size-4' aria-hidden='true' />{t('mediaBookmarks.choose')}</Button></DialogTrigger>
    <DialogContent className='media-bookmark-picker' aria-describedby={undefined}>
      <DialogHeader><DialogTitle>{t('mediaBookmarks.title')}</DialogTitle></DialogHeader>
      <DialogBody>
        <MediaShelfView shelf={shelf} labels={{ loading: t('mediaBookmarks.loading'), empty: t('mediaBookmarks.empty'), loadFailed: t('mediaBookmarks.loadFailed') }} placeholder={<Bookmark aria-hidden='true' />} tile={(item, thumbnail) => <>
          <Button type='button' variant='ghost' className='media-bookmark-choice' aria-label={t(item.attachments.some((asset) => asset.role === 'video_manifest') ? 'media.video' : 'media.imageAlt')} disabled={shelf.loading || disabled || removing} onClick={() => select(item)}>{thumbnail}</Button>
          <IconButton className='media-bookmark-button' type='button' variant='ghost' label={t('mediaBookmarks.remove')} aria-pressed='true' disabled={shelf.loading || removing} onClick={() => void remove(item)}><Bookmark className='size-4 fill-current' aria-hidden='true' /></IconButton>
        </>} />
      </DialogBody>
    </DialogContent>
  </Dialog>;
}
