import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { MediaDemandObserver } from '@/components/core/MediaDemandObserver';
import type { PostMediaView } from '@/components/core/types';
import { prefersReducedMotion } from '@/lib/reducedMotion';

/** 画像bytesとURLは通常の投稿と共用し、透明な表示面だけを作る。 */
export function LiveOverlayImage({ media }: { media: PostMediaView }) {
  const { t } = useTranslation(['common', 'live']);
  const image = media.state === 'gated' || media.state === 'pending' ? undefined : media.imageGalleryItems?.[0];
  const canvas = useRef<HTMLCanvasElement>(null);
  const still = prefersReducedMotion();
  const src = image?.src;
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
    <span className='live-overlay-image'>
      <MediaDemandObserver hash={image.hash} />
      {src ? (still ? <canvas ref={canvas} /> : <img src={src} alt='' />) : (
        <span className='live-stage-post-copy'>{t(image.failed ? 'common:media.fetchFailed' : 'live:loading')}</span>
      )}
    </span>
  );
}
