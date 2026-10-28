import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { BookmarkedMediaView } from '@/lib/api';
import { Button } from '@/components/ui/button';
import type { useMediaShelf } from '@/shell/page/useMediaShelf';

type MediaShelfState<C> = ReturnType<typeof useMediaShelf<C>>;

/** 状態の表示・サムネイルの格子・前後のページ。1 行の数は幅に合わせる。`tile` は各行の操作(選択・削除)を描く。 */
export function MediaShelfView<C>({ shelf, labels, placeholder, tile }: { shelf: MediaShelfState<C>; labels: { loading: string; empty: string; loadFailed: string }; placeholder: ReactNode; tile: (item: BookmarkedMediaView, thumbnail: ReactNode) => ReactNode }) {
  const { t } = useTranslation('common');
  const { page, loading, failed } = shelf;
  return <>
    {loading ? <p role='status'>{labels.loading}</p> : failed ? <div role='alert'><p>{labels.loadFailed}</p><Button type='button' variant='secondary' onClick={() => void shelf.load()}>{t('actions.retry')}</Button></div> : page?.items.length === 0 ? <p>{labels.empty}</p> : null}
    <div className='media-bookmark-grid' aria-busy={loading}>{page?.items.map((item) => {
      const src = shelf.preview(item.hash);
      return <div key={item.hash} className='relative min-w-0'>{tile(item, src ? <img src={src} alt='' /> : <span className='media-bookmark-placeholder'>{placeholder}</span>)}</div>;
    })}</div>
    <div className='flex justify-between gap-2'>
      <Button type='button' variant='secondary' disabled={loading || !page?.newer_cursor} onClick={() => void shelf.load(page?.newer_cursor, true)}>{t('mediaBookmarks.newer')}</Button>
      <Button type='button' variant='secondary' disabled={loading || !page?.older_cursor} onClick={() => void shelf.load(page?.older_cursor)}>{t('mediaBookmarks.older')}</Button>
    </div>
  </>;
}
