import type { AuthorSocialView, PublicChannelView } from '@/lib/api';
import { isPublicChannelId } from '@/lib/publicChannels';
import type { ColumnState } from '@/shell/slices/workspace';

/// 覚えておく公開スレッドの数。超えたら、古く覚えたものから外す。
export const KNOWN_PUBLIC_THREAD_LIMIT = 256;

/// この端末で読んだ公開スレッド（#1840）。列の見出し・投稿の公開範囲・本文のリンクの表示に使う。
export type PublicThreadsSliceState = {
  publicThreadsById: Record<string, PublicChannelView>;
};

export function createInitialPublicThreadsSlice(): PublicThreadsSliceState {
  return { publicThreadsById: {} };
}

/// 読んだ公開スレッドを覚える。内容の変わったものだけを置き換えて新しい側へ回し、変わらなければ同じ値を返す
/// （一覧の定期の読み直しで、覚えた値を読む画面を描き直さない）。
export function rememberPublicThreads(
  current: Record<string, PublicChannelView>,
  threads: PublicChannelView[]
): Record<string, PublicChannelView> {
  const changed = threads.filter(
    (thread) => JSON.stringify(current[thread.channel_id]) !== JSON.stringify(thread)
  );
  if (changed.length === 0) return current;
  const next = { ...current };
  for (const thread of changed) {
    delete next[thread.channel_id];
    next[thread.channel_id] = thread;
  }
  const keys = Object.keys(next);
  for (const key of keys.slice(0, Math.max(0, keys.length - KNOWN_PUBLIC_THREAD_LIMIT))) {
    delete next[key];
  }
  return next;
}

/// 非表示の作者（mute と、どちらの向きの block）。一覧と ID からの取得が除く作者と同じ（#1840 AC-3）。
export function isHiddenAuthor(author: AuthorSocialView | undefined): boolean {
  return Boolean(author && (author.muted || author.blocking || author.blocked_by));
}

/// 覚えた公開スレッドのタイトル。知らないか、作成者を非表示にしているか、削除されていれば(#1888) `undefined`。
export function knownPublicThreadTitle(
  threads: Record<string, PublicChannelView>,
  authors: Record<string, AuthorSocialView>,
  channelId: string
): string | undefined {
  const thread = threads[channelId];
  return thread && !thread.deleted && !isHiddenAuthor(authors[thread.creator_pubkey]) ? thread.title : undefined;
}

/// 公開スレッドの詳細の列の上部に出す配信中の Live の数(#1840 判断 14)。より多いときは「すべての配信」から列を開く。
export const SHOWN_THREAD_LIVE_SESSIONS = 3;

/// 公開スレッドの詳細の列で、上部にそのスレッドで配信中の Live を出すか(#1840 AC-4)。出す列は、そのスレッドの Live を読む。
export function showsThreadLives(column: Pick<ColumnState, 'kind' | 'scope'>, developerModeEnabled: boolean): boolean {
  return (
    developerModeEnabled && column.kind === 'timeline' && isPublicChannelId(column.scope?.channelId)
  );
}
