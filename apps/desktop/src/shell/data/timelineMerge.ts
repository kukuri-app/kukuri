import type { PostView, TimelineCursor } from '@/lib/api';

export function postIdentityKey(post: Pick<PostView, 'object_id' | 'server_object_id'>): string {
  return post.server_object_id ?? post.object_id;
}

export function uniquePostsByIdentity(posts: PostView[]): PostView[] {
  const seen = new Set<string>();
  const nextPosts: PostView[] = [];

  for (const post of posts) {
    const key = postIdentityKey(post);
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    nextPosts.push(post);
  }

  return nextPosts;
}

export function mergeUniquePosts(current: PostView[], incoming: PostView[]): PostView[] {
  const seen = new Set(current.map((post) => post.object_id));
  return [...current, ...incoming.filter((post) => !seen.has(post.object_id))];
}

/**
 * 表示中のページの続きの位置(`current`)が、先頭のページの続きの位置(`head`)より先へ進んでいるか(#1239)。
 *
 * 非表示の著者の投稿が続く範囲では、取得は行の無いページと続きの位置を返す。表示中の行数だけで「読み進めたか」を
 * 判定すると、行が 0 件のまま読み進めた位置を、周期の refresh が先頭のページの位置へ戻してしまう。
 * `order` はページの並び(タイムラインは新しい順 `desc`、thread は古い順 `asc`)。
 */
export function cursorIsBeyond(
  current: TimelineCursor | null | undefined,
  head: TimelineCursor | null | undefined,
  order: 'asc' | 'desc'
): boolean {
  if (!current || !head) {
    return false;
  }
  if (current.created_at !== head.created_at) {
    return order === 'desc'
      ? current.created_at < head.created_at
      : current.created_at > head.created_at;
  }
  if (current.object_id === head.object_id) {
    return false;
  }
  return order === 'desc' ? current.object_id < head.object_id : current.object_id > head.object_id;
}

/**
 * 保存している続きの位置(`stored`)が、表示中の行を越えて読み進めた位置か(#1239)。
 *
 * 非表示の著者の投稿が続く範囲では、続きの読み込みが行を増やさずに位置だけを進める。そのとき、続きの位置は
 * 表示中の最後の行より先にある。逆に、読み進めていないときの続きの位置は、表示中の最後の行そのもの。
 * 新着が届くと先頭のページの続きの位置は新しい側へ動くので、「先頭のページの続きの位置より先か」だけで判定すると、
 * 読み進めていないのに古い位置を残してしまう(新着を適用した後の続きの読み込みが、押し出された行を飛ばす)。
 */
export function cursorIsBeyondVisiblePosts(
  stored: TimelineCursor | null | undefined,
  visible: PostView[],
  order: 'asc' | 'desc'
): boolean {
  if (!stored) {
    return false;
  }
  let furthest: TimelineCursor | null = null;
  for (const post of visible) {
    if (post.local_state) {
      continue;
    }
    const position = { created_at: post.created_at, object_id: post.object_id };
    if (!furthest || cursorIsBeyond(position, furthest, order)) {
      furthest = position;
    }
  }
  return !furthest || cursorIsBeyond(stored, furthest, order);
}

/**
 * 先頭のページと表示中の行のあいだに、読んでいない行が無いか(#1239)。
 *
 * 先頭のページに新しい行が無いか、先頭のページが表示中の行と重なっていれば、あいだは無い。新しい行があって
 * 重なっていなければ、1 ページを超える数の新着が届いていて、先頭のページの先から表示中の行までのあいだに、
 * まだ読んでいない新着がありうる。
 */
export function headPageReachesVisiblePosts(current: PostView[], incoming: PostView[]): boolean {
  const visibleIds = new Set(
    current.filter((post) => !post.local_state).map((post) => postIdentityKey(post))
  );
  let hasNewRows = false;
  for (const post of incoming) {
    if (visibleIds.has(postIdentityKey(post))) {
      return true;
    }
    hasNewRows = true;
  }
  return !hasNewRows;
}

/**
 * refresh(buffer)と新着の適用が、表示中の古い行と続きの位置を残すべきか(#1239、#1274)。
 *
 * 表示は「先頭から続きの位置まで」を欠けなく並べたものでなければならない。残すのは、先頭のページと表示中の行の
 * あいだに読んでいない行が無く(`headPageReachesVisiblePosts`)、かつ先頭のページより先を読んでいるときだけ。
 * 先を読んでいるとは、行を読み足した(行数)か、行を増やさずに読み進めた(非表示の著者の範囲。続きの位置)こと。
 *
 * あいだがあるとき(1 ページを超える数の新着が届いた)に古い行を残すと、あいだの新着が表示されないか、
 * 続きの読み込みが古い行の後ろへ並べる。そのときは先頭のページから読み直す(読んだ範囲は読み直しになるが、
 * 行は欠けず、順序も崩れない)。
 *
 * `storedCursor` は、表示中の続きの位置。新着の適用では、refresh が決めた保留の続きの位置を渡し、
 * `headCursor` には保留中の先頭のページの最後の行の位置を渡す。
 */
export function hasReadPastHeadPage(
  current: PostView[],
  incoming: PostView[],
  storedCursor: TimelineCursor | null | undefined,
  headCursor: TimelineCursor | null | undefined,
  order: 'asc' | 'desc'
): boolean {
  if (!headPageReachesVisiblePosts(current, incoming)) {
    return false;
  }
  return (
    hasLoadedOlderAuthoritativePosts(current, incoming) ||
    (cursorIsBeyond(storedCursor, headCursor, order) &&
      cursorIsBeyondVisiblePosts(storedCursor, current, order))
  );
}

export function hasLoadedOlderAuthoritativePosts(
  current: PostView[],
  incoming: PostView[]
): boolean {
  return current.filter((post) => !post.local_state).length > incoming.length;
}

export function mergeRefreshedVisiblePosts(
  current: PostView[],
  incoming: PostView[],
  preserveOlderPages: boolean
): PostView[] {
  const authoritativeIds = new Set(incoming.map((post) => postIdentityKey(post)));
  const localPosts = current.filter((post) => {
    if (!post.local_state) {
      return false;
    }
    const authoritativeId = postIdentityKey(post);
    return !authoritativeIds.has(authoritativeId);
  });
  const nextPosts = [...localPosts];
  const seenPostIds = new Set(nextPosts.map((post) => postIdentityKey(post)));

  for (const post of incoming) {
    const postId = postIdentityKey(post);
    if (seenPostIds.has(postId)) {
      continue;
    }
    nextPosts.push(post);
    seenPostIds.add(postId);
  }

  if (!preserveOlderPages) {
    return nextPosts;
  }

  for (const post of current) {
    if (post.local_state) {
      continue;
    }
    const authoritativeId = postIdentityKey(post);
    if (authoritativeIds.has(authoritativeId) || seenPostIds.has(authoritativeId)) {
      continue;
    }
    nextPosts.push(post);
    seenPostIds.add(authoritativeId);
  }

  return nextPosts;
}
