import type { DesktopApi, JoinedPrivateChannelPage, JoinedPrivateChannelView } from '@/lib/api';
import type { DesktopShellStoreApi } from '@/shell/store';

type JoinedChannels = { items: JoinedPrivateChannelView[]; nextCursor: string | null };

/// 定期の再読み込みで得た最初の page を、読み込んだ続きを残して表示中の一覧へ重ねる(#1218 AC-4d)。
/// 一覧は channel id の順(runtime の cursor と同じ順)なので、page の最後より後ろの行は続きの page の分として残す。
export function refreshedJoinedChannels(
  current: JoinedPrivateChannelView[] | undefined,
  currentCursor: string | null | undefined,
  page: JoinedPrivateChannelPage
): JoinedChannels {
  const last = page.items.at(-1);
  if (!page.next_cursor || !last) {
    return { items: page.items, nextCursor: page.next_cursor ?? null };
  }
  const ids = new Set(page.items.map((channel) => channel.channel_id));
  const rest = (current ?? []).filter(
    (channel) => !ids.has(channel.channel_id) && channel.channel_id > last.channel_id
  );
  return {
    items: rest.length > 0 ? [...page.items, ...rest] : page.items,
    nextCursor: rest.length > 0 && currentCursor !== undefined ? currentCursor : page.next_cursor,
  };
}

export function applyJoinedChannels(
  store: DesktopShellStoreApi,
  topic: string,
  { items, nextCursor }: JoinedChannels
) {
  const state = store.getState();
  const previous = state.joinedChannelsByTopic[topic];
  state.patchState({
    // 同じ内容なら配列を差し替えない。差し替えると表示中の全行の view を作り直す(#1425)。
    joinedChannelsByTopic: previous && JSON.stringify(previous) === JSON.stringify(items)
      ? state.joinedChannelsByTopic
      : { ...state.joinedChannelsByTopic, [topic]: items },
    joinedChannelsNextCursorByTopic: { ...state.joinedChannelsNextCursorByTopic, [topic]: nextCursor },
  });
}

/// 「さらに表示」: 保持している cursor の続きの 1 page を一覧の後ろへ足す。失敗は呼び出し元へ投げる。
export async function loadMoreJoinedChannels(
  api: DesktopApi,
  store: DesktopShellStoreApi,
  topic: string
) {
  const cursor = store.getState().joinedChannelsNextCursorByTopic[topic];
  if (!cursor) {
    return;
  }
  const page = await api.listJoinedPrivateChannels(topic, cursor);
  const ids = new Set(page.items.map((channel) => channel.channel_id));
  const current = store.getState().joinedChannelsByTopic[topic] ?? [];
  applyJoinedChannels(store, topic, {
    items: [...current.filter((channel) => !ids.has(channel.channel_id)), ...page.items],
    nextCursor: page.next_cursor ?? null,
  });
}
