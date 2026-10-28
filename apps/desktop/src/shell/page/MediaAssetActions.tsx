import { useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { Bookmark, Copy } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { AttachmentView } from '@/lib/api';
import { formatPostDateTime } from '@/i18n/format';
import { buildPostLink } from '@/lib/internalLinks';
import { copyTextToClipboard } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Dialog, DialogBody, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { MediaBookmarksContext, MEDIA_BOOKMARK_CHANGED } from '@/components/core/mediaBookmarksContext';
import type { MediaBookmarkTarget } from '@/components/core/types';
import { LiveOverlayPushButton } from './LiveOverlayPushButton';


export function MediaAssetActions({ attachment, bookmarkHash, target, children, className = '', pushTarget }: {
  attachment: AttachmentView;
  bookmarkHash?: string;
  target: MediaBookmarkTarget;
  children: ReactNode;
  className?: string;
  pushTarget?: import('@/lib/api').PushLiveOverlayRequest;
}) {
  const { t, i18n } = useTranslation('common');
  const api = useContext(MediaBookmarksContext)?.api;
  const hash = bookmarkHash ?? attachment.hash;
  const [saved, setSaved] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  const [details, setDetails] = useState(false);
  const [copyResult, setCopyResult] = useState<string | null>(null);
  const statusRevision = useRef(0);
  const trigger = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!api) return;
    let current = true;
    const refresh = () => {
      const revision = statusRevision.current;
      void api.getBookmarkedMedia(hash).then((row) => { if (current && revision === statusRevision.current) setSaved(row !== null); }).catch(() => { if (current && revision === statusRevision.current) setError(true); });
    };
    const changed = (event: Event) => {
      const detail = (event as CustomEvent<{ hash: string; saved: boolean }>).detail;
      if (detail.hash === hash) { statusRevision.current += 1; setSaved(detail.saved); }
    };
    refresh();
    window.addEventListener(MEDIA_BOOKMARK_CHANGED, changed);
    return () => { current = false; window.removeEventListener(MEDIA_BOOKMARK_CHANGED, changed); };
  }, [api, hash]);
  const toggle = async () => {
    if (!api || pending) return;
    statusRevision.current += 1;
    setPending(true); setError(false);
    try {
      if (saved) await api.removeBookmarkedMedia(hash);
      else await api.bookmarkMedia(target.topic, target.objectId, hash, target.channelRef);
      setSaved(!saved);
      window.dispatchEvent(new CustomEvent(MEDIA_BOOKMARK_CHANGED, { detail: { hash, saved: !saved } }));
    } catch { setError(true); }
    finally { setPending(false); }
  };
  const source = attachment.origin ?? target.source;
  const values = [
    ['hash', attachment.hash], ['mime', attachment.mime], ['bytes', String(attachment.bytes)], ['role', attachment.role],
    ['author', source.author_pubkey], ['post', source.object_id], ['postLink', buildPostLink(source.topic_id, source.object_id)],
    ['topic', source.topic_id], ['createdAt', formatPostDateTime(source.created_at, i18n.resolvedLanguage)],
    ...(attachment.provenance ? [['provenance', JSON.stringify(attachment.provenance)]] : []),
  ];
  const copy = async (value: string) => {
    try { setCopyResult(await copyTextToClipboard(value) ? t('mediaBookmarks.copied') : t('mediaBookmarks.copyFailed')); }
    catch { setCopyResult(t('mediaBookmarks.copyFailed')); }
  };
  return <div ref={trigger} className={`media-asset-actions ${className}`} role='group' tabIndex={0}
    aria-label={t('mediaBookmarks.details')}
    onContextMenu={(event) => { event.preventDefault(); event.stopPropagation(); setCopyResult(null); setDetails(true); }}
    onKeyDown={(event) => {
      if (event.key === 'ContextMenu' || (event.key === 'F10' && event.shiftKey)) {
        event.preventDefault(); event.stopPropagation(); setCopyResult(null); setDetails(true);
      }
    }}>
    {children}
    {pushTarget ? <LiveOverlayPushButton target={pushTarget} /> : null}
    {api ? <IconButton type='button' variant='ghost' className='media-bookmark-button'
      label={t(saved ? 'mediaBookmarks.remove' : 'mediaBookmarks.save')} aria-pressed={saved} aria-busy={pending} disabled={pending}
      onClick={(event) => { event.stopPropagation(); void toggle(); }}>
      <Bookmark className={`size-4 ${saved ? 'fill-current' : ''}`} aria-hidden='true' />
    </IconButton> : null}
    {error ? <span className='media-bookmark-error' role='alert'>{t('mediaBookmarks.saveFailed')}</span> : null}
    <Dialog open={details} onOpenChange={setDetails}>
      <DialogContent aria-describedby={undefined} onCloseAutoFocus={(event) => { event.preventDefault(); trigger.current?.focus(); }}>
        <DialogHeader><DialogTitle>{t('mediaBookmarks.details')}</DialogTitle></DialogHeader>
        <DialogBody>
          <dl className='media-metadata-list'>{values.map(([key, value]) => <div key={key}>
            <dt>{t(`mediaBookmarks.fields.${key}`)}</dt>
            <dd><span>{value}</span><IconButton variant='ghost' type='button' label={t('mediaBookmarks.copyField', { field: t(`mediaBookmarks.fields.${key}`) })} onClick={() => void copy(value)}><Copy className='size-4' aria-hidden='true' /></IconButton></dd>
          </div>)}</dl>
          <Button type='button' variant='secondary' onClick={() => void copy(JSON.stringify({ ...attachment, origin: source, source_post_url: buildPostLink(source.topic_id, source.object_id) }, null, 2))}>{t('mediaBookmarks.copyAll')}</Button>
          {copyResult ? <p role='status'>{copyResult}</p> : null}
        </DialogBody>
      </DialogContent>
    </Dialog>
  </div>;
}
