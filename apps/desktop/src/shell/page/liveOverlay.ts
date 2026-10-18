import { useEffect, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';

import type {
  DesktopApi,
  JoinedPrivateChannelView,
  PostLiveView,
  PostView,
  ReactionKeyView,
} from '@/lib/api';
import { isPublicChannelId } from '@/lib/publicChannels';
import { subscribeReactionReceived } from '@/shell/data/useRuntimeEventBridge';
import { timelineStorageKeyForChannel, useDesktopShellStore } from '@/shell/store';

import { useLiveSession } from './useLiveSession';

/// 重ねる投稿の数(最新から。#1841 判断 2)。
export const LIVE_OVERLAY_POST_LIMIT = 5;
/// 新着の判定に覚える、開いた時点の投稿の数。
const SNAPSHOT_LIMIT = 25;
/// 浮かべる演出の上限(#1841 D2)。1 個の寿命・同時に出す数・待ちの数。上限は描画の資源の管理で、元のリアクションは消さない。
export const LIVE_FLOAT_LIFETIME_MS = 3_000;
export const LIVE_FLOAT_CONCURRENT = 12;
export const LIVE_FLOAT_QUEUE = 24;
/// 重複の判定に覚える数。
const FLOAT_SEEN_LIMIT = 256;
/// 新着として扱うのは、開いた時点以降に作られた投稿と付いたリアクションだけ。開いた時点との比較では、端末の時計のずれを 2 秒まで
/// 許す。付いてから 30 秒を過ぎたリアクションの版は、runtime が知らせない(再接続や照合で届いた古い版)。
const CLOCK_SLACK_MS = 2_000;

const EMPTY_POSTS: PostView[] = [];

export type LiveOverlaySource = {
  // 重ねる Live(#1839 の参照と同じ形)。
  live: PostLiveView;
  // 会話を読む場所(ライブの列の scope)。
  conversation: { topic: string; channelId: string | null };
};

export type LiveOverlayPost = {
  post: PostView;
  // 開始 post の返信ツリーの投稿(#1841 判断 4)。
  liveReply: boolean;
  // 開いた後に届いた、開いた時点より新しい投稿(表示で fade する)。
  fresh: boolean;
};

export type LiveOverlayFloat = { id: string; reaction: ReactionKeyView };

/// 会話の場所を読めるか。topic の公開タイムラインと公開スレッドは誰でも、private channel は参加しているときだけ(#1841 INVAR-2)。
/// 参加の一覧をまだ読んでいなければ `loading`。
export type LiveOverlayConversation = 'readable' | 'unreadable' | 'loading';

function conversationAccess(
  channelId: string | null,
  joined: JoinedPrivateChannelView[] | undefined
): LiveOverlayConversation {
  if (!channelId || isPublicChannelId(channelId)) return 'readable';
  if (!joined) return 'loading';
  return joined.some((channel) => channel.channel_id === channelId) ? 'readable' : 'unreadable';
}

/**
 * #1841 AC-1: 重ねる Live・会話の場所・Live の開始 post を結び付ける。
 * - Live は参照から表示要求で 1 件読む(`element` が画面にある間)。開始 post は Live の `start_post_id` だけで、紹介 post は使わない。
 * - 投稿は、通常の列と共用する scope ごとの cache(保留中の新着を含む)から最新の `LIVE_OVERLAY_POST_LIMIT` 件を古い順に返す。
 *   取り下げた投稿・本文の無い投稿・`hidden`(成人向け・信頼値の評価で隠すもの)は除く。読めない場所の投稿は返さない。
 * - 開始 post は返信ツリーの root だけを読み、数の表示に使う。読み直しは同期の時刻の変化(timer は使わない)。
 * - `animate` の間だけ、開始 post へ新しく届いたリアクションを浮かべる候補にする。
 */
export function useLiveOverlay(
  api: DesktopApi | undefined,
  source: LiveOverlaySource,
  { hidden, animate }: { hidden?: (post: PostView) => boolean; animate: boolean }
) {
  const { element, session } = useLiveSession<HTMLDivElement>(api, source.live);
  const startPostId = session ? (session.start_post_id ?? null) : undefined;
  const startPost = useStartPost(api, source.live.topic_id, startPostId);
  const { topic, channelId } = source.conversation;
  const key = timelineStorageKeyForChannel(topic, channelId);
  const { pending, timeline, joined } = useDesktopShellStore(
    useShallow((state) => ({
      pending: state.pendingTimelineSnapshotsByKey[key],
      timeline: state.timelinesByKey[key],
      joined: state.joinedChannelsByTopic[topic],
    }))
  );
  const conversation = conversationAccess(channelId, joined);
  // `undefined` は、会話の場所の投稿をまだ読んでいない。
  const loaded = conversation === 'readable' ? (pending ?? timeline) : undefined;
  const shown = (loaded ?? EMPTY_POSTS).filter(
    (post) => !post.withdrawal && post.content_status !== 'Missing' && !hidden?.(post)
  );
  const [openedAt] = useState(() => Date.now());
  // 開いた時点の投稿。会話の場所の投稿を最初に読めたときの分(空でもよい)を覚え、それ以降に届いた分だけを新着とする(読めるまでは
  // 新着にしない)。
  const [snapshot, setSnapshot] = useState<ReadonlySet<string> | null>(null);
  const firstKey = loaded ? shown.slice(0, SNAPSHOT_LIMIT).map((post) => post.object_id).join('\n') : null;
  useEffect(() => {
    if (firstKey !== null) setSnapshot((current) => current ?? new Set(firstKey ? firstKey.split('\n') : []));
  }, [firstKey]);
  const posts: LiveOverlayPost[] = shown
    .slice(0, LIVE_OVERLAY_POST_LIMIT)
    .reverse()
    .map((post) => ({
      post,
      liveReply:
        Boolean(startPostId) &&
        post.object_id !== startPostId &&
        (post.reply_to === startPostId || post.root_id === startPostId),
      fresh:
        snapshot !== null && !snapshot.has(post.object_id) && post.created_at * 1000 >= openedAt - CLOCK_SLACK_MS,
    }));
  // 開始 post を読めて、取り下げられていない間だけ浮かべる(数の表示と同じ条件)。
  const floats = useLiveReactionFloats(startPost?.object_id ?? null, animate);
  return { element, session, startPost, conversation, posts, floats };
}

/// 開始 post を返信ツリーの root だけで読む(同期の時刻が変わったら読み直す)。開始 post の無い Live と取り下げた開始 post は `null`、
/// 読込み中は `undefined`。
function useStartPost(api: DesktopApi | undefined, topic: string, startPostId: string | null | undefined) {
  const lastSync = useDesktopShellStore((state) => state.syncStatus.last_sync_ts);
  const [loaded, setLoaded] = useState<{ id: string; post: PostView | null } | null>(null);
  useEffect(() => {
    if (!api || !startPostId) return;
    let current = true;
    void api
      .listThread(topic, startPostId, null, 1)
      .then(
        (view) => view.items.find((post) => post.object_id === startPostId) ?? null,
        () => null
      )
      .then((post) => {
        if (current) setLoaded({ id: startPostId, post: post && !post.withdrawal ? post : null });
      });
    return () => {
      current = false;
    };
  }, [api, topic, startPostId, lastSync]);
  if (startPostId === undefined) return undefined;
  if (startPostId === null) return null;
  return loaded?.id === startPostId ? loaded.post : undefined;
}

/**
 * 開始 post へ新しく届いたリアクションを浮かべる(#1841 D2)。届いた順に待ちへ積み、同時に `LIVE_FLOAT_CONCURRENT` 個まで
 * `LIVE_FLOAT_LIFETIME_MS` だけ出す。待ちが `LIVE_FLOAT_QUEUE` を超えたら古いものを捨てる。開いた時点より前に付いたものと
 * 同じ版の重複は浮かべない。`active` でない間(列が見えない・動きを減らす設定)は何も積まない。
 */
export function useLiveReactionFloats(startPostId: string | null, active: boolean) {
  const [floats, setFloats] = useState<LiveOverlayFloat[]>([]);
  useEffect(() => {
    if (!startPostId || !active) return;
    const openedAt = Date.now();
    const seen = new Set<string>();
    const queue: LiveOverlayFloat[] = [];
    const timers = new Set<number>();
    let showing = 0;
    const pump = () => {
      while (showing < LIVE_FLOAT_CONCURRENT && queue.length > 0) {
        const next = queue.shift()!;
        showing += 1;
        setFloats((current) => [...current, next]);
        const timer = window.setTimeout(() => {
          timers.delete(timer);
          showing -= 1;
          setFloats((current) => current.filter((float) => float.id !== next.id));
          pump();
        }, LIVE_FLOAT_LIFETIME_MS);
        timers.add(timer);
      }
    };
    const unsubscribe = subscribeReactionReceived((received) => {
      if (received.target_object_id !== startPostId || received.updated_at < openedAt - CLOCK_SLACK_MS) {
        return;
      }
      const id = `${received.reaction_id}:${received.updated_at}`;
      if (seen.has(id)) return;
      seen.add(id);
      if (seen.size > FLOAT_SEEN_LIMIT) seen.delete(seen.values().next().value!);
      queue.push({ id, reaction: received.reaction });
      if (queue.length > LIVE_FLOAT_QUEUE) queue.shift();
      pump();
    });
    return () => {
      unsubscribe();
      timers.forEach((timer) => window.clearTimeout(timer));
      setFloats([]);
    };
  }, [startPostId, active]);
  return floats;
}
