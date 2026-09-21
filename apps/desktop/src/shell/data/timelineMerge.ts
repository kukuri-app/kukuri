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
