import { useEffect, useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { MediaDemandObserver } from '@/components/core/MediaDemandObserver';
import type { PostMediaView } from '@/components/core/types';
import { prefersReducedMotion } from '@/lib/reducedMotion';

/** 画像bytesとURLは通常の投稿と共用し、透明な表示面だけを作る。 */
export function LiveOverlayImage({ media, count }: { media: PostMediaView; count?: number }) {
  const { t } = useTranslation(['common', 'live']);
  const image = media.state === 'gated' || media.state === 'pending' ? undefined : media.imageGalleryItems?.[0];
  const canvas = useRef<HTMLCanvasElement>(null);
  const playback = useId();
  const still = prefersReducedMotion();
  const src = image?.src;
  // Blob/asset自体のURLは共用し、表示のfragmentだけを分けて先頭から再生する。連打中は変えない。
  const animatedSrc = count !== undefined && image?.mime === 'image/gif' && src ? `${src}#kukuri-overlay-${playback}` : src;
  useEffect(() => {
    if (!still || !src || !canvas.current) return;
    const target = canvas.current;
    const original = new Image();
    original.onload = () => {
      const scale = Math.min(1, 192 / original.naturalWidth, 160 / original.naturalHeight);
      target.width = Math.max(1, Math.round(original.naturalWidth * scale));
      target.height = Math.max(1, Math.round(original.naturalHeight * scale));
      target.getContext('2d')?.drawImage(original, 0, 0, target.width, target.height);
      original.src = '';
    };
    original.src = src;
    return () => { original.onload = null; original.src = ''; };
  }, [src, still]);
  if (!image) return null;
  return (
    <span className='live-overlay-image' data-asset-hash={image.hash}>
      <MediaDemandObserver hash={image.hash} />
      {src ? (still ? <canvas ref={canvas} /> : <img src={animatedSrc ?? undefined} alt='' />) : (
        <span className='live-stage-post-copy'>{t(image.failed ? 'common:media.fetchFailed' : 'live:loading')}</span>
      )}
      {count !== undefined ? <span className='live-overlay-push-count'>{`x${count}`}</span> : null}
    </span>
  );
}
