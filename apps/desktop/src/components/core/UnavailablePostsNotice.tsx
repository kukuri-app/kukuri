import { useTranslation } from 'react-i18next';

type UnavailablePostsNoticeProps = {
  /** 読んだ範囲にあるが、本体がまだ届かず表示できない投稿の数(#1239 AC-4)。0 以下なら何も描かない。 */
  count: number;
};

/**
 * 遡って読んだ範囲に、まだ取得できていない投稿があることを示す(#1239 AC-4)。
 * 続きを読む操作は止めない。取得できた投稿は、その範囲を読み込み直したときに一覧に並ぶ。
 */
export function UnavailablePostsNotice({ count }: UnavailablePostsNoticeProps) {
  const { t } = useTranslation('common');
  if (count <= 0) {
    return null;
  }
  return (
    <p className='empty' role='status' data-testid='unavailable-posts-notice'>
      {t('feed.unavailablePosts', { count })}
    </p>
  );
}
