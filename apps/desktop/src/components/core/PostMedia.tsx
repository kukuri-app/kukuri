import type * as React from 'react';
import { useTranslation } from 'react-i18next';
import { Flag } from 'lucide-react';

import { IconButton } from '@/components/ui/icon-button';

import { MediaFetchFailure } from './MediaFetchFailure';
import { type PostMediaView } from './types';

type PostMediaProps = {
  media: PostMediaView;
  onOpenImage?: (index: number) => void;
  /// 動画添付そのものを media として通報する(#697)。未指定なら操作を出さない。
  onReportVideo?: (hash: string) => void;
  /// #1108: Community Node の推定による代替表示で、枠そのものから詳細 dialog を開く。
  /// 指定時は説明文を出さず、枠の上に短いラベルだけを置く。
  onOpenGatedDetails?: (trigger: HTMLElement) => void;
};

export function PostMedia({
  media,
  onOpenImage,
  onReportVideo,
  onOpenGatedDetails,
}: PostMediaProps) {
  const { t } = useTranslation(['common', 'shell']);
  const videoReportHash = media.kind === 'video' ? media.videoReportHash : null;

  if (!media.kind) {
    return null;
  }
  // #858: 成人向けラベル付きメディアは、表示設定 OFF の間は取得もデコードもせず
  // 一貫したプレースホルダーだけを出す(全表示経路共通)。
  if (media.state === 'gated' && onOpenGatedDetails) {
    // 画像 / 動画は取得・表示しない。枠全体を button にして Enter / Space でも開ける。
    return (
      <button
        type='button'
        className='media-frame media-frame-gated media-gated-trigger'
        aria-haspopup='dialog'
        data-testid={`media-adult-gated-${media.objectId}`}
        onClick={(event) => onOpenGatedDetails(event.currentTarget)}
      >
        <span className='media-gated-placeholder' aria-hidden='true' />
        <span className='media-gated-label'>
          {media.kind === 'video' ? t('media.advisoryDetailsVideo') : t('media.advisoryDetailsImage')}
        </span>
      </button>
    );
  }
  if (media.state === 'gated') {
    return (
      <div
        className='media-frame media-frame-gated'
        data-testid={`media-adult-gated-${media.objectId}`}
      >
        <div className='media-gated-placeholder' aria-hidden='true' />
        <p className='topic-diagnostic topic-diagnostic-secondary' role='status'>
          {/* #1055: 判定元が Community Node の推定か、投稿者の自己申告かで文言を分ける。 */}
          {media.gatedBy === 'advisory'
            ? t('media.advisoryGated')
            : media.gatedBy === 'shared_media'
              ? t('media.sharedMediaGated')
              : t('media.adultGated')}
        </p>
      </div>
    );
  }
  // #1056: Community Node への推定の照会中。取得せず、確定後の代替表示とは別のスケルトンだけを出す。
  if (media.state === 'pending') {
    return (
      <div
        className='media-frame media-frame-loading'
        data-testid={`media-advisory-pending-${media.objectId}`}
        aria-busy='true'
      >
        <div className='media-skeleton' aria-hidden='true' />
        <span className='sr-only' role='status'>
          {t('media.advisoryPending')}
        </span>
      </div>
    );
  }
  // #1207: 自動取得が上限に達した。失敗した部分だけを置き換え、明示再試行を出す。
  if (media.state === 'unavailable') {
    return (
      <MediaFetchFailure
        hashes={media.retryHashes ?? []}
        retrying={media.retrying ?? false}
        testId={`media-fetch-failure-${media.objectId}`}
      />
    );
  }

  return (
    <>
      <div
        className={
          media.state === 'loading' ? 'media-frame media-frame-loading' : 'media-frame media-frame-ready'
        }
      >
        <div className='media-badges'>
          {media.kind === 'video' ? <span className='media-type-badge'>{t('media.video')}</span> : null}
          {media.extraAttachmentCount > 0 ? (
            <span className='media-count-badge'>+{media.extraAttachmentCount}</span>
          ) : null}
        </div>
        {onReportVideo && videoReportHash ? (
          <IconButton
            variant='secondary'
            type='button'
            className='media-video-report'
            onClick={() => onReportVideo(videoReportHash)}
            label={t('media.reportVideo')}
            data-testid={`media-video-report-${media.objectId}`}
          >
            <Flag className='size-4' aria-hidden='true' />
          </IconButton>
        ) : null}

        {media.kind === 'video' && media.videoPlaybackSrc && !media.videoUnsupportedOnClient ? (
          <video
            className='media-video'
            controls
            src={media.videoPlaybackSrc}
            preload='metadata'
            poster={media.videoPosterPreviewSrc ?? undefined}
            data-testid={`media-video-${media.objectId}`}
            {...media.videoProps}
          />
        ) : media.kind === 'video' && media.videoPosterPreviewSrc ? (
          <img
            className='media-preview'
            src={media.videoPosterPreviewSrc}
            alt={t('media.videoPosterAlt')}
            data-testid={`media-preview-${media.objectId}`}
          />
        ) : media.kind === 'image' && media.imagePreviewSrc ? (
          <button
            className='media-image-trigger'
            type='button'
            onClick={() => onOpenImage?.(media.currentImageIndex ?? 0)}
            aria-label={t('media.imageAlt')}
          >
            <img
              className='media-preview'
              src={media.imagePreviewSrc}
              alt={t('media.imageAlt')}
              data-testid={`media-preview-${media.objectId}`}
            />
          </button>
        ) : (
          <div
            className='media-skeleton'
            data-testid={`media-skeleton-${media.objectId}`}
            aria-hidden='true'
          />
        )}
      </div>

      {media.metaMime || media.metaBytesLabel ? (
        <div className='media-meta'>
          <span>{media.metaMime}</span>
          <span>{media.metaBytesLabel}</span>
        </div>
      ) : null}
    </>
  );
}
