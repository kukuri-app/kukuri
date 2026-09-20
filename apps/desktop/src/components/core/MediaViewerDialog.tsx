import { useMemo, useRef } from 'react';
import { ChevronLeft, ChevronRight, Flag, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { IconButton } from '@/components/ui/icon-button';

import { MediaFetchFailure } from './MediaFetchFailure';

type MediaViewerDialogProps = {
  items: Array<{
    hash: string;
    src: string | null;
    failed?: boolean;
    retrying?: boolean;
    mime: string;
  }>;
  index: number;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onIndexChange: (index: number) => void;
  onReportCurrent?: (hash: string) => void;
};

function clampIndex(index: number, length: number) {
  if (length === 0) {
    return 0;
  }
  if (index < 0) {
    return length - 1;
  }
  if (index >= length) {
    return 0;
  }
  return index;
}

export function MediaViewerDialog({
  items,
  index,
  open,
  onOpenChange,
  onIndexChange,
  onReportCurrent,
}: MediaViewerDialogProps) {
  const { t } = useTranslation('common');
  const pointerStartXRef = useRef<number | null>(null);
  const currentIndex = clampIndex(index, items.length);
  const currentItem = items[currentIndex] ?? null;
  const canNavigate = items.length > 1;
  const descriptionLabel = useMemo(
    () =>
      canNavigate ? `${t('media.imageAlt')} ${currentIndex + 1} / ${items.length}` : t('media.imageAlt'),
    [canNavigate, currentIndex, items.length, t]
  );

  const moveBy = (delta: number) => {
    if (!canNavigate) {
      return;
    }
    onIndexChange(clampIndex(currentIndex + delta, items.length));
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='media-viewer-dialog' hideClose>
        <DialogTitle className='sr-only'>{t('media.imageAlt')}</DialogTitle>
        <DialogDescription className='sr-only'>{descriptionLabel}</DialogDescription>
        <div
          className='media-viewer-body'
          tabIndex={-1}
          onKeyDown={(event) => {
            if (event.key === 'ArrowLeft') {
              event.preventDefault();
              moveBy(-1);
            }
            if (event.key === 'ArrowRight') {
              event.preventDefault();
              moveBy(1);
            }
          }}
        >
          <IconButton
            variant='ghost'
            type='button'
            className='media-viewer-close'
            onClick={() => onOpenChange(false)}
            label={t('media.closeDialog')}
          >
            <X className='size-5' aria-hidden='true' />
          </IconButton>
          {currentItem && onReportCurrent ? (
            <IconButton
              variant='secondary'
              type='button'
              className='media-viewer-report'
              onClick={() => onReportCurrent(currentItem.hash)}
              label={t('report.actionLabel', { ns: 'shell' })}
            >
              <Flag className='size-4' aria-hidden='true' />
            </IconButton>
          ) : null}
          {canNavigate ? (
            <IconButton
              variant='secondary'
              type='button'
              className='media-viewer-nav media-viewer-nav-prev'
              onClick={() => moveBy(-1)}
              label={t('media.previousImage')}
            >
              <ChevronLeft className='size-5' aria-hidden='true' />
            </IconButton>
          ) : null}
          <div
            className='media-viewer-stage'
            onPointerDown={(event) => {
              pointerStartXRef.current = event.clientX;
            }}
            onPointerUp={(event) => {
              const startX = pointerStartXRef.current;
              pointerStartXRef.current = null;
              if (startX === null) {
                return;
              }
              const deltaX = event.clientX - startX;
              if (Math.abs(deltaX) < 40) {
                return;
              }
              moveBy(deltaX > 0 ? -1 : 1);
            }}
          >
            {currentItem?.src ? (
              <img
                className='media-viewer-image'
                src={currentItem.src}
                alt={t('media.imageAlt')}
              />
            ) : currentItem?.failed ? (
              <MediaFetchFailure
                className='media-viewer-empty'
                hashes={[currentItem.hash]}
                retrying={currentItem.retrying ?? false}
                testId='media-viewer-fetch-failure'
              />
            ) : (
              <div className='media-viewer-empty'>{t('media.syncingImage')}</div>
            )}
          </div>
          {canNavigate ? (
            <IconButton
              variant='secondary'
              type='button'
              className='media-viewer-nav media-viewer-nav-next'
              onClick={() => moveBy(1)}
              label={t('media.nextImage')}
            >
              <ChevronRight className='size-5' aria-hidden='true' />
            </IconButton>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}
