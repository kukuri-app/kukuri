import type {
  PublicChannelCursor,
  PublicChannelListView,
  PublicChannelOrder,
  PublicChannelView,
} from '@/lib/api';
import { MAX_VISIBLE_POSTS } from '@/shell/pagination';

/// 公開スレッドの一覧の列の表示(#1840)。行は `MAX_VISIBLE_POSTS` までで、タイムラインと同じく、続きを読んで
/// 超えたら先頭の行を外す。
export type PublicThreadListState = {
  rows: PublicChannelView[];
  /// 続きを読む位置。無ければ続きは無い。
  next: PublicChannelCursor | null;
  unavailable: number;
  status: 'loading' | 'ready' | 'error';
  loadingMore: boolean;
  loadMoreFailed: boolean;
  /// 続きを読んで先頭の行を外した。最新に戻るまで、定期の読み直しを重ねない(重ねると読んでいる位置が飛ぶ)。
  slid: boolean;
};

export const INITIAL_PUBLIC_THREAD_LIST: PublicThreadListState = {
  rows: [],
  next: null,
  unavailable: 0,
  status: 'loading',
  loadingMore: false,
  loadMoreFailed: false,
  slid: false,
};

function cursorOf(row: PublicChannelView, order: PublicChannelOrder): PublicChannelCursor {
  return { at: order === 'created' ? row.created_at : row.last_post_at, channel_id: row.channel_id };
}

/// 先頭のページを、読み込んだ続きの行の前に重ねる。上限を超えた古い側の行は外し、続きはその前から読む。
export function withHeadPage(
  current: PublicThreadListState,
  page: PublicChannelListView,
  order: PublicChannelOrder
): PublicThreadListState {
  const ids = new Set(page.items.map((row) => row.channel_id));
  const older = current.rows.filter((row) => !ids.has(row.channel_id));
  const rows = [...page.items, ...older].slice(0, MAX_VISIBLE_POSTS);
  const last = rows.at(-1);
  const next =
    last && rows.length < page.items.length + older.length
      ? cursorOf(last, order)
      : older.length > 0
        ? current.next
        : page.next_cursor ?? null;
  return { ...current, rows, next, unavailable: page.unavailable_count ?? 0, status: 'ready' };
}

/// 続きのページを後ろへ足す。上限を超えたら先頭の行を外す。
export function withNextPage(
  current: PublicThreadListState,
  page: PublicChannelListView
): PublicThreadListState {
  const ids = new Set(current.rows.map((row) => row.channel_id));
  const rows = [...current.rows, ...page.items.filter((row) => !ids.has(row.channel_id))];
  const overflow = Math.max(0, rows.length - MAX_VISIBLE_POSTS);
  return {
    ...current,
    rows: rows.slice(overflow),
    next: page.next_cursor ?? null,
    unavailable: page.unavailable_count ?? 0,
    loadingMore: false,
    slid: current.slid || overflow > 0,
  };
}
