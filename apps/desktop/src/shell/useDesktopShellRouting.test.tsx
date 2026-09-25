/**
 * WP-S5: useDesktopShellRouting の characterization テスト。
 *
 * 後続 WP-H6(shell store のスライス化・selector 化・prop drilling 除去)の
 * 安全網として「現時点で観測した挙動」をそのまま固定する。
 * このテストが落ちた場合、疑うべきは加えた変更でありテストではない。
 *
 * 方針:
 * - Provider + HashRouter の実 wrapper でマウントする(MemoryRouter は不可 —
 *   resolveHashBackedRouteLocation が window.location.hash を直読するため URL と乖離する)。
 * - renderHook 前に hash を正規形(buildShellUrl が生成する形)へ設定し、内包する
 *   useRouteSynchronization の normalize が発火しない安定状態から始める。
 * - rAF はフォーカス移動と normalize 遅延にのみ使われるため microtask 実行の stub に
 *   差し替える(同期実行 stub は scheduleAnimationFrame 内の frameId 参照が
 *   const 初期化中の TDZ となり例外になるため不可)。
 * - 期待値は観測した生リテラル。api 呼び出し引数・store 状態遷移・hash・
 *   キー/フィールド単位の値のみ固定し、返り値の全量比較・snapshot・参照同一性は
 *   assert しない(H6 耐性)。
 */
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, test, vi } from 'vitest';

import type {
  AuthorSocialView,
  DesktopApi,
  DirectMessageConversationView,
  JoinedPrivateChannelView,
  PostView,
} from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { useDesktopShellRouting } from '@/shell/useDesktopShellRouting';
import {
  createShellHookHarness,
  resetWindowHash,
  type ShellHookHarness,
} from '@/shell/testSupport/renderShellHook';
import {
  activeWorkspaceColumn,
  activeWorkspaceScope,
  columnIdentityId,
  openTransientColumn,
  primarySectionForColumn,
} from '@/shell/slices/workspace';

const AUTHOR_PUBKEY = 'a'.repeat(64);
const DM_PEER_PUBKEY = 'd'.repeat(64);
const OTHER_PEER_PUBKEY = 'b'.repeat(64);
const STRANGER_PUBKEY = 'e'.repeat(64);
// buildShellUrl(URLSearchParams)が生成する正規形。':' は %3A にエンコードされる。
const BASE_TIMELINE_HASH = '#/timeline?topic=kukuri%3Atopic%3Ageneral';

// key をそのまま返す stub。文言 assert を locale リソースから切り離す。
const stubTranslate = (key: string) => key;

function buildPost(overrides: Partial<PostView> = {}): PostView {
  return {
    object_id: 'post-1',
    envelope_id: 'envelope-post-1',
    author_pubkey: AUTHOR_PUBKEY,
    author_name: null,
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    is_threadable: true,
    content: 'hello world',
    content_status: 'Available',
    attachments: [],
    created_at: 1,
    reply_to: null,
    root_id: 'post-1',
    channel_id: null,
    audience_label: 'Public',
    ...overrides,
  };
}

function buildAuthor(pubkey: string, overrides: Partial<AuthorSocialView> = {}): AuthorSocialView {
  return {
    author_pubkey: pubkey,
    name: null,
    display_name: null,
    about: null,
    picture_asset: null,
    updated_at: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    friend_of_friend_via_pubkeys: [],
    muted: false,
    blocking: false,
    blocked_by: false,
    ...overrides,
  };
}

function buildConversation(peerPubkey: string): DirectMessageConversationView {
  return {
    dm_id: `dm:${peerPubkey}`,
    peer_pubkey: peerPubkey,
    peer_name: null,
    peer_display_name: null,
    peer_picture_asset: null,
    updated_at: 0,
    last_message_at: null,
    last_message_id: null,
    last_message_preview: null,
    status: {
      peer_pubkey: peerPubkey,
      dm_id: `dm:${peerPubkey}`,
      mutual: true,
      send_enabled: true,
      pending_outbox_count: 0,
      pending_outbox_has_more: false,
    },
  };
}

function buildJoinedChannel(channelId: string): JoinedPrivateChannelView {
  return {
    topic_id: 'kukuri:topic:general',
    channel_id: channelId,
    label: 'core',
    creator_pubkey: 'c'.repeat(64),
    owner_pubkey: 'c'.repeat(64),
    joined_via_pubkey: null,
    audience_kind: 'invite_only',
    is_owner: false,
    current_epoch_id: 'epoch-1',
    archived_epoch_ids: [],
    sharing_state: 'open',
    rotation_required: false,
    participant_count: 2,
    stale_participant_count: 0,
  };
}

type RenderRoutingHookOptions = {
  /** renderHook 前に window.location.hash に反映される正規ルート('#/...' 形式)。 */
  hash: string;
  api?: DesktopApi;
  /** store 生成後・renderHook 前に呼ばれるプリセット(render 前なので act 不要)。 */
  preset?: (store: ShellHookHarness['store']) => void;
};

/**
 * useDesktopShellRouting を Provider + HashRouter 配下で単体マウントする。
 * ref 5 本は DesktopShellPage.tsx(L257-268)の useRef 初期値と同じ
 * { current: ... } リテラルで注入する。
 */
function renderRoutingHook(options: RenderRoutingHookOptions) {
  const harness = createShellHookHarness({ hash: options.hash, router: true });
  options.preset?.(harness.store);
  const api = options.api ?? createDesktopMockApi();
  const loadTopics = vi.fn(async () => {});
  const settingsTriggerRef = { current: null as HTMLButtonElement | null };
  const pendingRouteUrlRef = { current: null as string | null };
  const view = renderHook(
    () =>
      useDesktopShellRouting({
        api,
        translate: stubTranslate,
        loadTopics,
        settingsTriggerRef,
        pendingRouteUrlRef,
      }),
    { wrapper: harness.wrapper }
  );
  return { api, harness, loadTopics, view };
}

describe('useDesktopShellRouting', () => {
  test('a newer thread focus request wins over a delayed older read', async () => {
    const api = createDesktopMockApi();
    let finish!: (value: Awaited<ReturnType<DesktopApi['listThread']>>) => void;
    api.listThread = vi.fn(async (_topic, id) => id === 'slow'
      ? new Promise<Awaited<ReturnType<DesktopApi['listThread']>>>((resolve) => { finish = resolve; })
      : { items: [buildPost({ object_id: 'fast-post', root_id: 'fast' })], next_cursor: null });
    const { view, harness } = renderRoutingHook({ hash: BASE_TIMELINE_HASH, api });
    let pending!: Promise<void>;
    act(() => { pending = view.result.current.openThread('slow', { focusObjectId: 'slow-post' }); });
    await act(async () => { await view.result.current.openThread('fast', { focusObjectId: 'fast-post' }); });
    await act(async () => {
      finish({ items: [buildPost({ object_id: 'slow-post', root_id: 'slow' })], next_cursor: null });
      await pending;
    });
    expect(harness.store.getState().selectedThread).toBe('fast');
    expect(harness.store.getState().focusedObjectId).toBe('fast-post');
    expect(window.location.hash).toContain('threadId=fast');
  });

  test('leaving the route while a focused thread loads cancels its later navigation', async () => {
    const api = createDesktopMockApi();
    let finish!: (value: Awaited<ReturnType<DesktopApi['listThread']>>) => void;
    api.listThread = vi.fn<DesktopApi['listThread']>(() => new Promise((resolve) => { finish = resolve; }));
    const { view, harness } = renderRoutingHook({ hash: BASE_TIMELINE_HASH, api });
    let pending!: Promise<void>;
    act(() => { pending = view.result.current.openThread('slow', { focusObjectId: 'slow-post' }); });
    act(() => { window.location.hash = '#/notifications?topic=kukuri%3Atopic%3Ageneral'; });
    await act(async () => {
      finish({ items: [buildPost({ object_id: 'slow-post', root_id: 'slow' })], next_cursor: null });
      await pending;
    });
    expect(harness.store.getState().selectedThread).toBeNull();
    expect(window.location.hash).toContain('#/notifications');
  });

  beforeEach(() => {
    // setup.ts は hash を掃除しないため、各テストで '/' に戻す。
    resetWindowHash();
    // rAF を microtask 実行に差し替える(act / waitFor の await で必ず flush される)。
    // cancelAnimationFrame は no-op(残った callback はフォーカス移動のみで無害)。
    let rafSequence = 0;
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((callback) => {
      rafSequence += 1;
      const frameId = rafSequence;
      queueMicrotask(() => callback(frameId));
      return frameId;
    });
    vi.spyOn(window, 'cancelAnimationFrame').mockImplementation(() => {});
  });

  test('openThread success loads thread state, clears author/DM panes, and pushes a thread URL', async () => {
    const baseApi = createDesktopMockApi({
      seedPosts: {
        'kukuri:topic:general': [
          buildPost({ object_id: 'post-1', root_id: 'post-1' }),
          buildPost({
            object_id: 'comment-1',
            root_id: 'post-1',
            reply_to: 'post-1',
            object_kind: 'comment',
            created_at: 2,
          }),
        ],
      },
    });
    const api = { ...baseApi, listThread: vi.fn(baseApi.listThread) };
    // URL と整合する author ペイン(+DM ペイン)が開いた状態から始めて
    // 「openThread が他ペインを閉じる」ことを観測する。nav も開いておき、
    const { harness, loadTopics, view } = renderRoutingHook({
      hash: `#/timeline?topic=kukuri%3Atopic%3Ageneral&context=author&authorPubkey=${AUTHOR_PUBKEY}`,
      api,
      preset: (store) => {
        store.getState().patchState({
          selectedAuthorPubkey: AUTHOR_PUBKEY,
          selectedAuthor: buildAuthor(AUTHOR_PUBKEY),
          directMessagePaneOpen: true,
          selectedDirectMessagePeerPubkey: DM_PEER_PUBKEY,
        });
      },
    });

    const historyLengthBefore = window.history.length;
    await act(async () => {
      await view.result.current.openThread('post-1');
    });

    // THREAD_TIMELINE_LIMIT(=30)・cursor null で 1 回だけ取得する。
    expect(api.listThread).toHaveBeenCalledTimes(1);
    expect(api.listThread).toHaveBeenCalledWith('kukuri:topic:general', 'post-1', null, 30);

    await waitFor(() => {
      expect(harness.store.getState().selectedThread).toBe('post-1');
    });
    const state = harness.store.getState();
    expect(state.threadsById['post-1'].map((item) => item.object_id)).toEqual([
      'post-1',
      'comment-1',
    ]);
    expect(state.threadNextCursorById).toEqual({ 'post-1': null });
    expect(state.focusedObjectId).toBeNull();
    // author / DM ペインは閉じる。
    expect(state.selectedAuthorPubkey).toBeNull();
    expect(state.selectedAuthor).toBeNull();
    expect(state.directMessagePaneOpen).toBe(false);
    expect(state.selectedDirectMessagePeerPubkey).toBeNull();
    expect(primarySectionForColumn(activeWorkspaceColumn(state.workspaceState))).toBe('timeline');
    expect(activeWorkspaceColumn(state.workspaceState).kind).toBe('thread');
    expect(state.error).toBeNull();
    await waitFor(() => {
      expect(window.location.hash).toBe(
        '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1'
      );
    });
    // historyMode 既定は push(履歴が 1 つ積まれる)。
    expect(window.history.length).toBe(historyLengthBefore + 1);
    expect(loadTopics).not.toHaveBeenCalled();
    view.unmount();
  });

  test('openThread with normalizeOnEmpty clears all panes and replaces to a thread-less URL', async () => {
    const baseApi = createDesktopMockApi({
      seedPosts: {
        'kukuri:topic:general': [buildPost({ object_id: 'post-1', root_id: 'post-1' })],
      },
    });
    const api = { ...baseApi, listThread: vi.fn(baseApi.listThread) };
    // thread ペインが開いた状態(URL とも整合)から、存在しない thread を開く。
    // live/game の選択も持たせ、normalizeOnEmpty がクリアすることを観測する
    // (timeline+thread route の mount 同期は live/game 選択を触らないため preset が残る)。
    const { harness, view } = renderRoutingHook({
      hash: '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1',
      api,
      preset: (store) => {
        store.getState().patchState({
          selectedThread: 'post-1',
          threadsById: {
            'post-1': [buildPost({ object_id: 'post-1', root_id: 'post-1' })],
          },
          selectedLiveSessionId: 'live-1',
          selectedGameRoomId: 'room-1',
        });
      },
    });

    const historyLengthBefore = window.history.length;
    await act(async () => {
      await view.result.current.openThread('post-gone', { normalizeOnEmpty: true });
    });

    expect(api.listThread).toHaveBeenCalledWith('kukuri:topic:general', 'post-gone', null, 30);
    await waitFor(() => {
      expect(harness.store.getState().selectedThread).toBeNull();
    });
    const state = harness.store.getState();
    expect(state.threadsById['post-1']?.map((item) => item.object_id)).toEqual(['post-1']);
    expect(state.focusedObjectId).toBeNull();
    expect(state.selectedAuthorPubkey).toBeNull();
    expect(state.selectedAuthor).toBeNull();
    expect(state.directMessagePaneOpen).toBe(false);
    expect(state.selectedDirectMessagePeerPubkey).toBeNull();
    // preset で非 null にしてある live/game の選択も normalizeOnEmpty がクリアする。
    expect(state.selectedLiveSessionId).toBeNull();
    expect(state.selectedGameRoomId).toBeNull();
    // 0 件はエラーではない。
    expect(state.error).toBeNull();
    await waitFor(() => {
      expect(window.location.hash).toBe(BASE_TIMELINE_HASH);
    });
    // replace なので履歴は積まれない(thread param は履歴に残らない)。
    expect(window.history.length).toBe(historyLengthBefore);
    view.unmount();
  });

  test('openThread with channelId pins the topic channel selection and puts channel on the thread URL', async () => {
    const baseApi = createDesktopMockApi({
      seedPosts: {
        'kukuri:topic:general': [
          buildPost({ object_id: 'post-1', root_id: 'post-1', channel_id: 'channel-1' }),
        ],
      },
    });
    const api = { ...baseApi, listThread: vi.fn(baseApi.listThread) };
    // global の選択 channel は public(未選択)のまま、非 active な private Column から
    // Thread を開く状況を再現する。channel-1 は joined 済みにして route 正規化で落とされないようにする。
    const { harness, view } = renderRoutingHook({
      hash: BASE_TIMELINE_HASH,
      api,
      preset: (store) => {
        store.getState().patchState({
          joinedChannelsByTopic: { 'kukuri:topic:general': [buildJoinedChannel('channel-1')] },
          channelPanelStateByTopic: {
            'kukuri:topic:general': { status: 'ready', error: null },
          },
        });
      },
    });

    await act(async () => {
      await view.result.current.openThread('post-1', {
        topic: 'kukuri:topic:general',
        channelId: 'channel-1',
      });
    });

    expect(api.listThread).toHaveBeenCalledWith('kukuri:topic:general', 'post-1', null, 30);
    await waitFor(() => {
      expect(harness.store.getState().selectedThread).toBe('post-1');
    });
    const state = harness.store.getState();
    // handleSelectPrivateChannel と同じ 3 つの状態が channel-1 に揃う。
    expect(activeWorkspaceScope(state.workspaceState).channelId).toBe('channel-1');
    expect(state.timelineScopeByTopic['kukuri:topic:general']).toEqual({
      kind: 'channel',
      channel_id: 'channel-1',
    });
    expect(state.composeChannelByTopic['kukuri:topic:general']).toEqual({
      kind: 'private_channel',
      channel_id: 'channel-1',
    });
    await waitFor(() => {
      expect(window.location.hash).toBe(
        '#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1&context=thread&threadId=post-1'
      );
    });
    view.unmount();
  });

  test('openThread with channelId null resets the topic channel selection to public', async () => {
    const baseApi = createDesktopMockApi({
      seedPosts: {
        'kukuri:topic:general': [buildPost({ object_id: 'post-1', root_id: 'post-1' })],
      },
    });
    const api = { ...baseApi, listThread: vi.fn(baseApi.listThread) };
    const { harness, view } = renderRoutingHook({
      hash: '#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1',
      api,
      preset: (store) => {
        store.getState().patchState({
          joinedChannelsByTopic: { 'kukuri:topic:general': [buildJoinedChannel('channel-1')] },
          channelPanelStateByTopic: {
            'kukuri:topic:general': { status: 'ready', error: null },
          },
          workspaceState: openTransientColumn(store.getState().workspaceState, {
            id: columnIdentityId('timeline', {
              topicId: 'kukuri:topic:general',
              channelId: 'channel-1',
            }),
            kind: 'timeline',
            scope: { topicId: 'kukuri:topic:general', channelId: 'channel-1' },
            pinned: false,
          }),
          timelineScopeByTopic: {
            'kukuri:topic:general': { kind: 'channel', channel_id: 'channel-1' },
          },
          composeChannelByTopic: {
            'kukuri:topic:general': { kind: 'private_channel', channel_id: 'channel-1' },
          },
        });
      },
    });

    await act(async () => {
      await view.result.current.openThread('post-1', {
        topic: 'kukuri:topic:general',
        channelId: null,
      });
    });

    await waitFor(() => {
      expect(harness.store.getState().selectedThread).toBe('post-1');
    });
    const state = harness.store.getState();
    expect(activeWorkspaceScope(state.workspaceState).channelId).toBeNull();
    expect(state.timelineScopeByTopic['kukuri:topic:general']).toEqual({ kind: 'public' });
    expect(state.composeChannelByTopic['kukuri:topic:general']).toEqual({ kind: 'public' });
    await waitFor(() => {
      expect(window.location.hash).toBe(
        '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1'
      );
    });
    view.unmount();
  });

  test('openDirectMessagePane fetches conversation, timeline and status and opens the messages pane', async () => {
    const baseApi = createDesktopMockApi({
      authorSocialViews: {
        [DM_PEER_PUBKEY]: { name: 'dana', following: true, followed_by: true, mutual: true },
      },
    });
    const api = {
      ...baseApi,
      openDirectMessage: vi.fn(baseApi.openDirectMessage),
      listDirectMessageMessages: vi.fn(baseApi.listDirectMessageMessages),
      getDirectMessageStatus: vi.fn(baseApi.getDirectMessageStatus),
    };
    // 既存の会話 1 件を持たせ「先頭挿入」を観測する。
    const { harness, view } = renderRoutingHook({
      hash: BASE_TIMELINE_HASH,
      api,
      preset: (store) => {
        store.getState().patchState({
          directMessages: [buildConversation(OTHER_PEER_PUBKEY)],
        });
      },
    });

    await act(async () => {
      await view.result.current.openDirectMessagePane(DM_PEER_PUBKEY);
    });

    expect(api.openDirectMessage).toHaveBeenCalledTimes(1);
    expect(api.openDirectMessage).toHaveBeenCalledWith(DM_PEER_PUBKEY);
    expect(api.listDirectMessageMessages).toHaveBeenCalledWith(DM_PEER_PUBKEY, null, 100);
    expect(api.getDirectMessageStatus).toHaveBeenCalledWith(DM_PEER_PUBKEY);

    await waitFor(() => {
      expect(harness.store.getState().selectedDirectMessagePeerPubkey).toBe(DM_PEER_PUBKEY);
    });
    const state = harness.store.getState();
    // 取得した会話が先頭に挿入され、既存の会話は残る。
    expect(state.directMessages.map((entry) => entry.peer_pubkey)).toEqual([
      DM_PEER_PUBKEY,
      OTHER_PEER_PUBKEY,
    ]);
    expect(state.directMessages[0]?.peer_name).toBe('dana');
    expect(state.directMessageTimelineByPeer[DM_PEER_PUBKEY]).toEqual([]);
    expect(state.directMessageStatusByPeer[DM_PEER_PUBKEY]).toMatchObject({
      peer_pubkey: DM_PEER_PUBKEY,
      mutual: true,
      send_enabled: true,
    });
    // 会話から知り得た author 情報が knownAuthors に merge される。
    expect(state.knownAuthorsByPubkey[DM_PEER_PUBKEY]).toMatchObject({
      author_pubkey: DM_PEER_PUBKEY,
      name: 'dana',
      mutual: true,
    });
    expect(primarySectionForColumn(activeWorkspaceColumn(state.workspaceState))).toBe('messages');
    expect(state.directMessagePaneOpen).toBe(true);
    expect(state.directMessageError).toBeNull();
    expect(state.selectedThread).toBeNull();
    expect(state.selectedAuthorPubkey).toBeNull();
    expect(state.selectedLiveSessionId).toBeNull();
    expect(state.selectedGameRoomId).toBeNull();
    await waitFor(() => {
      expect(window.location.hash).toBe(
        `#/messages?topic=kukuri%3Atopic%3Ageneral&peerPubkey=${DM_PEER_PUBKEY}`
      );
    });
    view.unmount();
  });

  test('openDirectMessagePane failure with normalizeOnError replaces to a peer-less messages URL', async () => {
    // mutual でない peer への openDirectMessage は mock api が reject する。
    const baseApi = createDesktopMockApi();
    const api = { ...baseApi, openDirectMessage: vi.fn(baseApi.openDirectMessage) };
    const { harness, view } = renderRoutingHook({ hash: BASE_TIMELINE_HASH, api });

    // directMessageError は「設定 → normalize 後の route 同期で null クリア」と遷移するため、
    // 最終値ではなく購読で遷移列そのものを固定する。
    const observedErrors: Array<string | null> = [];
    const unsubscribe = harness.store.subscribe((state, previous) => {
      if (state.directMessageError !== previous.directMessageError) {
        observedErrors.push(state.directMessageError);
      }
    });

    await act(async () => {
      await view.result.current.openDirectMessagePane(STRANGER_PUBKEY, {
        normalizeOnError: true,
      });
    });

    await waitFor(() => {
      expect(observedErrors).toEqual(['direct message requires a mutual relationship', null]);
    });
    await waitFor(() => {
      // peerPubkey param が消えた messages URL へ normalize(replace)される。
      expect(window.location.hash).toBe('#/messages?topic=kukuri%3Atopic%3Ageneral');
    });
    const state = harness.store.getState();
    expect(state.directMessagePaneOpen).toBe(true);
    expect(state.selectedDirectMessagePeerPubkey).toBeNull();
    expect(primarySectionForColumn(activeWorkspaceColumn(state.workspaceState))).toBe('messages');
    unsubscribe();
    view.unmount();
  });

  test('openDirectMessagePane uses the localized fallback for a non-Error failure', async () => {
    const api = {
      ...createDesktopMockApi(),
      openDirectMessage: vi.fn().mockRejectedValue(null),
    };
    const { harness, view } = renderRoutingHook({ hash: BASE_TIMELINE_HASH, api });

    await act(async () => {
      await view.result.current.openDirectMessagePane(DM_PEER_PUBKEY);
    });

    expect(harness.store.getState().directMessageError).toBe(
      'common:errors.failedToOpenDirectMessage'
    );
    view.unmount();
  });

  test('focusPrimarySection updates chrome, clears thread/author/DM panes, and pushes the section URL', async () => {
    const { harness, view } = renderRoutingHook({
      hash:
        '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1' +
        `&authorPubkey=${AUTHOR_PUBKEY}`,
      preset: (store) => {
        store.getState().patchState({
          selectedThread: 'post-1',
          threadsById: {
            'post-1': [buildPost({ object_id: 'post-1', root_id: 'post-1' })],
          },
          selectedAuthorPubkey: AUTHOR_PUBKEY,
          selectedAuthor: buildAuthor(AUTHOR_PUBKEY),
        });
        // profileMode / profileConnectionsView を非デフォルト値にして遷移後の挙動を観測する。
        store.getState().patchState({
          shellChromeState: {
            ...store.getState().shellChromeState,
            profileMode: 'edit',
            profileConnectionsView: 'muted',
          },
        });
      },
    });

    // mount 時の route 同期は、profile 以外の route では profileMode を常に 'overview' へ
    // 強制する(useRouteSynchronization.ts L286-318)ため preset の 'edit' は残らない。
    // 一方 profileConnectionsView は現値が引き継がれ preset の 'muted' のまま。
    expect(harness.store.getState().shellChromeState.profileMode).toBe('overview');
    expect(harness.store.getState().shellChromeState.profileConnectionsView).toBe('muted');

    const historyLengthBefore = window.history.length;
    act(() => {
      view.result.current.focusPrimarySection('live');
    });

    await waitFor(() => {
      expect(window.location.hash).toBe('#/live?topic=kukuri%3Atopic%3Ageneral');
    });
    await waitFor(() => {
      expect(
        primarySectionForColumn(
          activeWorkspaceColumn(harness.store.getState().workspaceState)
        )
      ).toBe('live');
    });
    const state = harness.store.getState();
    // focusPrimarySection は section!=='profile' では profileMode / profileConnectionsView を
    // 触らない(useDesktopShellRouting.ts L509-510)。profileMode は上記 mount 時同期で既に
    // 'overview'、preset した profileConnectionsView='muted' は遷移後もそのまま残る。
    expect(state.shellChromeState.profileMode).toBe('overview');
    expect(state.shellChromeState.profileConnectionsView).toBe('muted');
    expect(state.selectedThread).toBeNull();
    expect(state.focusedObjectId).toBeNull();
    expect(state.selectedAuthorPubkey).toBeNull();
    expect(state.selectedAuthor).toBeNull();
    expect(state.authorError).toBeNull();
    expect(state.directMessagePaneOpen).toBe(false);
    expect(state.selectedDirectMessagePeerPubkey).toBeNull();
    // push なので履歴が 1 つ積まれる。
    expect(window.history.length).toBe(historyLengthBefore + 1);
    view.unmount();
  });

  test('toggleNotificationsSection saves the current URL, then returns to lastNonNotificationsRoute', async () => {
    const { harness, view } = renderRoutingHook({ hash: BASE_TIMELINE_HASH });

    // mount 時の route 同期が現 URL を lastNonNotificationsRoute に保存している。
    await waitFor(() => {
      expect(harness.store.getState().lastNonNotificationsRoute).toBe(
        '/timeline?topic=kukuri%3Atopic%3Ageneral'
      );
    });

    // 1 回目: notifications 以外にいる → 現 URL を保存して notifications へ。
    act(() => {
      view.result.current.toggleNotificationsSection();
    });
    await waitFor(() => {
      expect(window.location.hash).toBe('#/notifications?topic=kukuri%3Atopic%3Ageneral');
    });
    expect(
      primarySectionForColumn(activeWorkspaceColumn(harness.store.getState().workspaceState))
    ).toBe('notifications');
    // notifications 滞在中も直前の非 notifications ルートを保持し続ける。
    expect(harness.store.getState().lastNonNotificationsRoute).toBe(
      '/timeline?topic=kukuri%3Atopic%3Ageneral'
    );

    // 2 回目: notifications にいる → 保存済みルートへ戻る。
    act(() => {
      view.result.current.toggleNotificationsSection();
    });
    await waitFor(() => {
      expect(window.location.hash).toBe(BASE_TIMELINE_HASH);
    });
    expect(
      primarySectionForColumn(activeWorkspaceColumn(harness.store.getState().workspaceState))
    ).toBe('timeline');
    view.unmount();
  });

  test('Escape cascade closes exactly one layer per press: settings -> author -> thread', async () => {
    // settings drawer + author pane + thread paneを全て開いた状態から始める。
    const { harness, view } = renderRoutingHook({
      hash:
        '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1' +
        `&authorPubkey=${AUTHOR_PUBKEY}&settings=connectivity`,
      preset: (store) => {
        const scope = { topicId: 'kukuri:topic:general', channelId: null };
        const threadColumnId = columnIdentityId('thread', scope, 'post-1');
        store.getState().patchState({
          selectedThread: 'post-1',
          threadsById: {
            'post-1': [buildPost({ object_id: 'post-1', root_id: 'post-1' })],
          },
          selectedAuthorPubkey: AUTHOR_PUBKEY,
          selectedAuthor: buildAuthor(AUTHOR_PUBKEY),
          workspaceState: openTransientColumn(store.getState().workspaceState, {
            id: threadColumnId,
            kind: 'thread',
            scope,
            entityId: 'post-1',
            parentColumnId: store.getState().workspaceState.activeColumnId,
            pinned: false,
          }),
        });
      },
    });
    expect(harness.store.getState().shellChromeState.settingsOpen).toBe(true);

    const dispatchEscape = () => {
      const event = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true });
      act(() => {
        window.dispatchEvent(event);
      });
      return event;
    };

    // 1 回目: settings drawer だけが閉じる(URL から settings param が消える)。
    const first = dispatchEscape();
    expect(first.defaultPrevented).toBe(true);
    await waitFor(() => {
      expect(harness.store.getState().shellChromeState.settingsOpen).toBe(false);
    });
    await waitFor(() => {
      expect(window.location.hash).toBe(
        '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1' +
          `&authorPubkey=${AUTHOR_PUBKEY}`
      );
    });
    expect(harness.store.getState().selectedAuthorPubkey).toBe(AUTHOR_PUBKEY);
    expect(harness.store.getState().selectedThread).toBe('post-1');

    // 2 回目: author ペインだけが閉じる。
    const second = dispatchEscape();
    expect(second.defaultPrevented).toBe(true);
    await waitFor(() => {
      expect(harness.store.getState().selectedAuthorPubkey).toBeNull();
    });
    await waitFor(() => {
      expect(window.location.hash).toBe(
        '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1'
      );
    });
    expect(harness.store.getState().selectedAuthor).toBeNull();
    expect(harness.store.getState().selectedThread).toBe('post-1');

    // 3 回目: thread ペインだけが閉じる。
    const third = dispatchEscape();
    expect(third.defaultPrevented).toBe(true);
    await waitFor(() => {
      expect(harness.store.getState().selectedThread).toBeNull();
    });
    await waitFor(() => {
      expect(window.location.hash).toBe(BASE_TIMELINE_HASH);
    });
    // 4 回目: 閉じる対象が無く、イベントは消費されない。
    const fourth = dispatchEscape();
    expect(fourth.defaultPrevented).toBe(false);
    expect(window.location.hash).toBe(BASE_TIMELINE_HASH);
    view.unmount();
  });

  test('Escape cascade skips default-prevented events and editable targets', async () => {
    // Issue #765 T3: Radix Dialog(dismissable layer が document capture で
    // preventDefault する)や Composer の textarea からの Escape では
    // selection cascade を発火しないことを固定する。
    const { harness, view } = renderRoutingHook({
      hash: '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1',
      preset: (store) => {
        store.getState().patchState({
          selectedThread: 'post-1',
          threadsById: {
            'post-1': [buildPost({ object_id: 'post-1', root_id: 'post-1' })],
          },
        });
      },
    });
    expect(harness.store.getState().selectedThread).toBe('post-1');

    // Radix dismissable layer 相当: preventDefault 済みの Escape は何もしない。
    const prevented = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true });
    prevented.preventDefault();
    act(() => {
      window.dispatchEvent(prevented);
    });
    expect(harness.store.getState().selectedThread).toBe('post-1');

    // Composer 相当: textarea から bubble してきた Escape は何もしない
    // (消費もしないので defaultPrevented は false のまま)。
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);
    const fromEditable = new KeyboardEvent('keydown', {
      key: 'Escape',
      bubbles: true,
      cancelable: true,
    });
    act(() => {
      textarea.dispatchEvent(fromEditable);
    });
    expect(fromEditable.defaultPrevented).toBe(false);
    expect(harness.store.getState().selectedThread).toBe('post-1');
    textarea.remove();

    // contenteditable からの Escape も何もしない。
    const editableRegion = document.createElement('div');
    editableRegion.setAttribute('contenteditable', 'true');
    document.body.appendChild(editableRegion);
    const fromContentEditable = new KeyboardEvent('keydown', {
      key: 'Escape',
      bubbles: true,
      cancelable: true,
    });
    act(() => {
      editableRegion.dispatchEvent(fromContentEditable);
    });
    expect(fromContentEditable.defaultPrevented).toBe(false);
    expect(harness.store.getState().selectedThread).toBe('post-1');
    editableRegion.remove();

    // 通常の Escape は従来どおり thread を閉じる。
    const plain = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true });
    act(() => {
      window.dispatchEvent(plain);
    });
    expect(plain.defaultPrevented).toBe(true);
    await waitFor(() => {
      expect(harness.store.getState().selectedThread).toBeNull();
    });
    view.unmount();
  });
});
