/**
 * WP-S5: routing/useRouteSynchronization の characterization テスト。
 *
 * 後続 WP-H6(shell store のスライス化・selector 化・prop drilling 除去)の
 * 安全網として「現時点で観測した挙動」をそのまま固定する。
 * このテストが落ちた場合、疑うべきは加えた変更でありテストではない。
 *
 * 方針:
 * - 全 13 依存が引数注入のため wrapper 不要。navigate / openThread / openAuthorDetail /
 *   openDirectMessagePane / syncRoute / loadTopics は vi.fn、scheduleAnimationFrame は
 *   即時実行 stub を注入する。normalize の rAF 連鎖は syncRoute / navigate を vi.fn に
 *   することで断つ(実 navigate は渡さない)。
 * - state は手渡しスナップショットであり、effect 内の setField では自動再レンダー
 *   されない。そのため「route 変化 1 回分の 1 ステップ挙動」を 1 テストで固定し、
 *   多段遷移は rerender で次のスナップショットを渡して観測する。
 * - 固定するのは注入コールバックの呼び出し引数(toHaveBeenCalledWith)と store の
 *   状態遷移(キー/フィールド単位)のみ。返り値やオブジェクト全量の snapshot・
 *   参照同一性は assert しない(H6 のリファクタで無意味に割れるため)。
 */
import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, test, vi } from 'vitest';

import type { JoinedPrivateChannelView, PostView } from '@/lib/api';
import { useRouteSynchronization } from '@/shell/routing/useRouteSynchronization';
import { createDesktopShellStore, type DesktopShellStoreApi } from '@/shell/store';
import { resetWindowHash, setWindowHash } from '@/shell/testSupport/renderShellHook';
import { selectShellRoutingSlice } from '@/shell/storeSelectors';
import {
  activeWorkspaceColumn,
  activeWorkspaceScope,
  columnIdentityId,
  openTransientColumn,
} from '@/shell/slices/workspace';

const DM_PEER_PUBKEY = 'd'.repeat(64);

type RouteSynchronizationArgs = Parameters<typeof useRouteSynchronization>[0];

/**
 * 引数一式を生成する。state は storeApi の「現時点のスナップショット」なので、
 * store をプリセットしてから呼ぶこと(プリセット後の値が effect に渡る)。
 */
function createHookArgs(
  storeApi: DesktopShellStoreApi,
  overrides: Partial<RouteSynchronizationArgs> = {}
): RouteSynchronizationArgs {
  return {
    loadTopics: vi.fn(() => Promise.resolve()),
    lastObservedRouteUrlRef: { current: '' },
    navigate: vi.fn(),
    openAuthorDetail: vi.fn(() => Promise.resolve()),
    openDirectMessagePane: vi.fn(() => Promise.resolve()),
    openThread: vi.fn(() => Promise.resolve()),
    pendingRouteUrlRef: { current: null },
    resolvedRouteLocation: { pathname: '/timeline', search: '?topic=kukuri%3Atopic%3Ageneral' },
    routeSection: 'timeline',
    scheduleAnimationFrame: (callback: () => void) => callback(),
    state: selectShellRoutingSlice(storeApi.getState()),
    storeApi,
    syncRoute: vi.fn(),
    ...overrides,
  };
}

function buildPost(overrides: Partial<PostView> = {}): PostView {
  return {
    object_id: 'post-1',
    envelope_id: 'envelope-post-1',
    author_pubkey: 'a'.repeat(64),
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

function buildJoinedChannel(
  channelId: string,
  label: string,
  overrides: Partial<JoinedPrivateChannelView> = {}
): JoinedPrivateChannelView {
  return {
    topic_id: 'kukuri:topic:general',
    channel_id: channelId,
    label,
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
    ...overrides,
  };
}

describe('useRouteSynchronization', () => {
  beforeEach(() => {
    resetWindowHash();
  });

  describe('unknown pathname', () => {
    test('redirects to /timeline preserving the search via replace navigation', () => {
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: { pathname: '/channels', search: '?topic=kukuri%3Atopic%3Ageneral' },
      });

      const view = renderHook(() => useRouteSynchronization(args));

      expect(args.navigate).toHaveBeenCalledTimes(1);
      expect(args.navigate).toHaveBeenCalledWith('/timeline?topic=kukuri%3Atopic%3Ageneral', {
        replace: true,
      });
      // リダイレクト判定より前に lastNonNotificationsRoute へ現 URL が記録される(観測挙動)
      expect(storeApi.getState().lastNonNotificationsRoute).toBe(
        '/channels?topic=kukuri%3Atopic%3Ageneral'
      );
      expect(args.syncRoute).not.toHaveBeenCalled();
      expect(args.loadTopics).not.toHaveBeenCalled();
      view.unmount();
    });
  });

  describe('topic param', () => {
    test('switches activeTopic and reloads when the topic param is a tracked topic', () => {
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: { pathname: '/timeline', search: '?topic=kukuri%3Atopic%3Adev' },
      });

      const view = renderHook(() => useRouteSynchronization(args));

      expect(activeWorkspaceScope(storeApi.getState().workspaceState).topicId).toBe('kukuri:topic:dev');
      expect(args.loadTopics).toHaveBeenCalledTimes(1);
      expect(args.loadTopics).toHaveBeenCalledWith(
        ['kukuri:topic:general', 'kukuri:topic:dev', 'kukuri:topic:test'],
        'kukuri:topic:dev',
        null
      );
      expect(args.syncRoute).not.toHaveBeenCalled();
      expect(args.navigate).not.toHaveBeenCalled();
      view.unmount();
    });

    test('passes the requested threadId to loadTopics when a tracked-topic switch combines with thread context', () => {
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: {
          pathname: '/timeline',
          search: '?topic=kukuri%3Atopic%3Adev&context=thread&threadId=post-9',
        },
      });

      const view = renderHook(() => useRouteSynchronization(args));

      expect(activeWorkspaceScope(storeApi.getState().workspaceState).topicId).toBe('kukuri:topic:dev');
      // topic 切替(shouldReload)× context=thread の複合では loadTopics の第 3 引数に
      // requestedThreadId がそのまま渡る(useRouteSynchronization.ts L827-833)。
      expect(args.loadTopics).toHaveBeenCalledTimes(1);
      expect(args.loadTopics).toHaveBeenCalledWith(
        ['kukuri:topic:general', 'kukuri:topic:dev', 'kukuri:topic:test'],
        'kukuri:topic:dev',
        'post-9'
      );
      // thread のオープン自体は openThread 側へ配線される(threadId が selectedThread と異なるため)。
      expect(args.openThread).toHaveBeenCalledTimes(1);
      expect(args.openThread).toHaveBeenCalledWith('post-9', {
        focusObjectId: null,
        historyMode: 'replace',
        normalizeOnEmpty: true,
        topic: 'kukuri:topic:dev',
      });
      expect(args.syncRoute).not.toHaveBeenCalled();
      view.unmount();
    });

    test('normalizes an untracked topic param via syncRoute replace without changing activeTopic', () => {
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: {
          pathname: '/timeline',
          search: '?topic=kukuri%3Atopic%3Aunknown',
        },
      });

      const view = renderHook(() => useRouteSynchronization(args));

      // activeTopic は現状維持のまま、URL 側だけを replace で正規化する
      expect(activeWorkspaceScope(storeApi.getState().workspaceState).topicId).toBe('kukuri:topic:general');
      expect(args.loadTopics).not.toHaveBeenCalled();
      expect(args.navigate).not.toHaveBeenCalled();
      expect(args.syncRoute).toHaveBeenCalledTimes(1);
      expect(args.syncRoute).toHaveBeenCalledWith('replace', {
        activeTopic: 'kukuri:topic:general',
        composeTarget: { kind: 'public' },
        focusedObjectId: null,
        primarySection: 'timeline',
        profileConnectionsView: 'following',
        profileMode: 'overview',
        publicThreadList: false,
        selectedAuthorPubkey: null,
        selectedDirectMessagePeerPubkey: null,
        selectedGameRoomId: null,
        selectedLiveSessionId: null,
        selectedThread: null,
        settingsOpen: false,
        settingsSection: 'connectivity',
        timelineScope: { kind: 'public' },
        timelineView: 'feed',
      });
      view.unmount();
    });
  });

  describe('channel param', () => {
    test('normalizes the channel param to null when the channel panel is ready and the channel is not joined', () => {
      const storeApi = createDesktopShellStore();
      const initial = storeApi.getState();
      storeApi.getState().patchState({
        channelPanelStateByTopic: {
          ...initial.channelPanelStateByTopic,
          'kukuri:topic:general': { status: 'ready', error: null },
        },
      });
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: {
          pathname: '/timeline',
          search: '?topic=kukuri%3Atopic%3Ageneral&channel=chan-1',
        },
      });

      const view = renderHook(() => useRouteSynchronization(args));

      // この assert は「store が書き換えられない(現状維持)」ことの確認。デフォルト値も
      // null であり、current===next===null のため map は書かれない
      // (useRouteSynchronization.ts L249-266)。実質的な normalize の固定は
      // 直後の syncRoute 引数 assert が担う。
      expect(activeWorkspaceScope(storeApi.getState().workspaceState).channelId).toBeNull();
      expect(args.loadTopics).not.toHaveBeenCalled();
      expect(args.syncRoute).toHaveBeenCalledTimes(1);
      // channel は落とされ public scope / public compose target へ正規化される
      expect(args.syncRoute).toHaveBeenCalledWith('replace', {
        activeTopic: 'kukuri:topic:general',
        composeTarget: { kind: 'public' },
        focusedObjectId: null,
        primarySection: 'timeline',
        profileConnectionsView: 'following',
        profileMode: 'overview',
        publicThreadList: false,
        selectedAuthorPubkey: null,
        selectedDirectMessagePeerPubkey: null,
        selectedGameRoomId: null,
        selectedLiveSessionId: null,
        selectedThread: null,
        settingsOpen: false,
        settingsSection: 'connectivity',
        timelineScope: { kind: 'public' },
        timelineView: 'feed',
      });
      view.unmount();
    });

    test('keeps the channel param pending while the channel panel is not ready yet', () => {
      // channelPanelStateByTopic はデフォルトで status='loading'(未 ready)
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: {
          pathname: '/timeline',
          search: '?topic=kukuri%3Atopic%3Ageneral&channel=chan-1',
        },
      });

      const view = renderHook(() => useRouteSynchronization(args));

      // pending validation 中は store も URL も書き換えない(channel は保持される)
      expect(args.syncRoute).not.toHaveBeenCalled();
      expect(args.navigate).not.toHaveBeenCalled();
      expect(args.loadTopics).not.toHaveBeenCalled();
      expect(activeWorkspaceScope(storeApi.getState().workspaceState).channelId).toBeNull();
      expect(storeApi.getState().timelineScopeByTopic['kukuri:topic:general']).toEqual({
        kind: 'public',
      });
      view.unmount();
    });

    test('adopts a joined channel param into selection, scope and compose target then reloads', () => {
      const storeApi = createDesktopShellStore();
      const initial = storeApi.getState();
      storeApi.getState().patchState({
        channelPanelStateByTopic: {
          ...initial.channelPanelStateByTopic,
          'kukuri:topic:general': { status: 'ready', error: null },
        },
        joinedChannelsByTopic: {
          ...initial.joinedChannelsByTopic,
          'kukuri:topic:general': [buildJoinedChannel('chan-1', 'General')],
        },
      });
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: {
          pathname: '/timeline',
          search: '?topic=kukuri%3Atopic%3Ageneral&channel=chan-1',
        },
      });

      const view = renderHook(() => useRouteSynchronization(args));

      const state = storeApi.getState();
      expect(activeWorkspaceScope(state.workspaceState).channelId).toBe('chan-1');
      expect(state.timelineScopeByTopic['kukuri:topic:general']).toEqual({
        kind: 'channel',
        channel_id: 'chan-1',
      });
      expect(state.composeChannelByTopic['kukuri:topic:general']).toEqual({
        kind: 'private_channel',
        channel_id: 'chan-1',
      });
      expect(args.loadTopics).toHaveBeenCalledTimes(1);
      expect(args.loadTopics).toHaveBeenCalledWith(
        ['kukuri:topic:general', 'kukuri:topic:dev', 'kukuri:topic:test'],
        'kukuri:topic:general',
        null
      );
      expect(args.syncRoute).not.toHaveBeenCalled();
      view.unmount();
    });
  });

  describe('thread context', () => {
    test('opens the thread once when threadId differs and does not re-open after the snapshot catches up', () => {
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: {
          pathname: '/timeline',
          search: '?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1',
        },
      });

      const view = renderHook(
        (props: RouteSynchronizationArgs) => useRouteSynchronization(props),
        { initialProps: args }
      );

      expect(args.openThread).toHaveBeenCalledTimes(1);
      expect(args.openThread).toHaveBeenCalledWith('post-1', {
        focusObjectId: null,
        historyMode: 'replace',
        normalizeOnEmpty: true,
        topic: 'kukuri:topic:general',
      });
      expect(args.syncRoute).not.toHaveBeenCalled();

      // openThread 完了相当の store 更新を模し、次のスナップショットで effect を再実行する
      act(() => {
        storeApi.getState().patchState({
          selectedThread: 'post-1',
          threadsById: { 'post-1': [buildPost({ object_id: 'post-1' })] },
        });
      });
      view.rerender({ ...args, state: selectShellRoutingSlice(storeApi.getState()) });

      // threadId が selectedThread と一致し thread が読み込み済みなら再オープンしない
      expect(args.openThread).toHaveBeenCalledTimes(1);
      expect(args.syncRoute).not.toHaveBeenCalled();
      view.unmount();
    });
  });

  describe('messages peerPubkey', () => {
    test('opens the direct message pane for a hex64 peerPubkey on the messages section', () => {
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: {
          pathname: '/messages',
          search: `?topic=kukuri%3Atopic%3Ageneral&peerPubkey=${DM_PEER_PUBKEY}`,
        },
        routeSection: 'messages',
      });

      const view = renderHook(() => useRouteSynchronization(args));

      expect(storeApi.getState().directMessagePaneOpen).toBe(true);
      expect(args.openDirectMessagePane).toHaveBeenCalledTimes(1);
      expect(args.openDirectMessagePane).toHaveBeenCalledWith(DM_PEER_PUBKEY, {
        historyMode: 'replace',
        normalizeOnError: true,
        preserveAuthorPane: false,
        preservedAuthorPubkey: null,
      });
      expect(args.syncRoute).not.toHaveBeenCalled();
      expect(args.navigate).not.toHaveBeenCalled();
      view.unmount();
    });

    test('normalizes a non-hex64 peerPubkey to null without opening a conversation', () => {
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: {
          pathname: '/messages',
          search: '?topic=kukuri%3Atopic%3Ageneral&peerPubkey=not-a-key',
        },
        routeSection: 'messages',
      });

      const view = renderHook(() => useRouteSynchronization(args));

      expect(args.openDirectMessagePane).not.toHaveBeenCalled();
      // ペイン自体は開かれる(観測挙動)が、peer は URL から落とされる
      expect(storeApi.getState().directMessagePaneOpen).toBe(true);
      expect(args.syncRoute).toHaveBeenCalledTimes(1);
      expect(args.syncRoute).toHaveBeenCalledWith('replace', {
        activeTopic: 'kukuri:topic:general',
        composeTarget: { kind: 'public' },
        focusedObjectId: null,
        primarySection: 'messages',
        profileConnectionsView: 'following',
        profileMode: 'overview',
        publicThreadList: false,
        selectedAuthorPubkey: null,
        selectedDirectMessagePeerPubkey: null,
        selectedGameRoomId: null,
        selectedLiveSessionId: null,
        selectedThread: null,
        settingsOpen: false,
        settingsSection: 'connectivity',
        timelineScope: { kind: 'public' },
        timelineView: 'feed',
      });
      view.unmount();
    });
  });

  describe('route observation gate', () => {
    test('skips processing while a pending route url has not been observed and the route did not change', () => {
      const storeApi = createDesktopShellStore();
      // pending 未観測 = 自分の navigate で実 URL(hash)は pending へ更新済みだが
      // router のレンダーがまだ追いついていない状態。
      setWindowHash('#/timeline?topic=kukuri%3Atopic%3Ageneral');
      // 未追跡 topic(本来なら normalize される URL)でも、pending 未観測なら何もしない
      const args = createHookArgs(storeApi, {
        lastObservedRouteUrlRef: { current: '/timeline?topic=kukuri%3Atopic%3Aunknown' },
        pendingRouteUrlRef: { current: '/timeline?topic=kukuri%3Atopic%3Ageneral' },
        resolvedRouteLocation: {
          pathname: '/timeline',
          search: '?topic=kukuri%3Atopic%3Aunknown',
        },
      });

      const view = renderHook(() => useRouteSynchronization(args));

      expect(args.syncRoute).not.toHaveBeenCalled();
      expect(args.navigate).not.toHaveBeenCalled();
      expect(args.loadTopics).not.toHaveBeenCalled();
      expect(args.pendingRouteUrlRef.current).toBe('/timeline?topic=kukuri%3Atopic%3Ageneral');
      expect(storeApi.getState().lastNonNotificationsRoute).toBeNull();
      view.unmount();
    });

    test('clears the pending route url and resumes processing when the route changed', () => {
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi, {
        lastObservedRouteUrlRef: { current: '/timeline?topic=kukuri%3Atopic%3Ageneral' },
        pendingRouteUrlRef: { current: '/timeline?topic=kukuri%3Atopic%3Ageneral' },
        resolvedRouteLocation: {
          pathname: '/timeline',
          search: '?topic=kukuri%3Atopic%3Aunknown',
        },
      });

      const view = renderHook(() => useRouteSynchronization(args));

      expect(args.pendingRouteUrlRef.current).toBeNull();
      expect(args.lastObservedRouteUrlRef.current).toBe('/timeline?topic=kukuri%3Atopic%3Aunknown');
      expect(storeApi.getState().lastNonNotificationsRoute).toBe(
        '/timeline?topic=kukuri%3Atopic%3Aunknown'
      );
      // 未追跡 topic の normalize が通常どおり走る
      expect(args.syncRoute).toHaveBeenCalledTimes(1);
      view.unmount();
    });

    test('discards a pending push superseded by history back and reprojects the observed url', () => {
      // hash-routing narrow flake の再現(React Router v7 の transition レンダー):
      // thread への push が router に commit される前に goBack が来ると、location 文字列は
      // last observed と同一のまま二度と変化せず、pending ガードが route 消費を止めて
      // thread Column が active のまま取り残されていた。実 URL(hash)が pending と一致
      // しない場合は追い越し済みと判定し、pending を破棄して現在 URL を再投影する。
      setWindowHash('#/timeline?topic=kukuri%3Atopic%3Ageneral');
      const storeApi = createDesktopShellStore();
      const args = createHookArgs(storeApi);
      const view = renderHook((props: RouteSynchronizationArgs) => useRouteSynchronization(props), {
        initialProps: args,
      });
      expect(activeWorkspaceColumn(storeApi.getState().workspaceState).kind).toBe('timeline');

      // thread push 直後の状態を模す: store は thread Column が active、
      // pending は push した URL のまま router には未観測。実 URL は goBack で timeline に settle 済み。
      const scope = { topicId: 'kukuri:topic:general', channelId: null };
      act(() => {
        storeApi.getState().patchState({
          selectedThread: 'post-1',
          threadsById: { 'post-1': [buildPost()] },
          workspaceState: openTransientColumn(storeApi.getState().workspaceState, {
            id: columnIdentityId('thread', scope, 'post-1'),
            kind: 'thread',
            scope,
            entityId: 'post-1',
            pinned: false,
          }),
        });
      });
      args.pendingRouteUrlRef.current =
        '/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1';
      view.rerender({ ...args, state: selectShellRoutingSlice(storeApi.getState()) });

      expect(args.pendingRouteUrlRef.current).toBeNull();
      expect(activeWorkspaceColumn(storeApi.getState().workspaceState).kind).toBe('timeline');
      expect(storeApi.getState().selectedThread).toBeNull();
      view.unmount();
    });

    test('does not reproject an older push that commits after a newer Column focus push (Issue #1053)', () => {
      // Profile → Timeline 本文 → Explore のボタン、を router の commit より速く操作した状態:
      // store は Explore が active、pending と実 hash は Explore の URL。先に Timeline の push が
      // commit されても、その古い URL の再投影で Explore から active を奪わない。
      const profileUrl = '/profile?topic=kukuri%3Atopic%3Ageneral';
      const exploreUrl = '/explore?topic=kukuri%3Atopic%3Ageneral';
      setWindowHash(`#${profileUrl}`);
      const storeApi = createDesktopShellStore();
      const scope = { topicId: 'kukuri:topic:general', channelId: null };
      act(() => {
        storeApi.getState().patchState({
          workspaceState: {
            ...storeApi.getState().workspaceState,
            activeColumnId: columnIdentityId('profile', scope),
          },
        });
      });
      const args = createHookArgs(storeApi, {
        resolvedRouteLocation: { pathname: '/profile', search: '?topic=kukuri%3Atopic%3Ageneral' },
        routeSection: 'profile',
      });
      const view = renderHook((props: RouteSynchronizationArgs) => useRouteSynchronization(props), {
        initialProps: args,
      });
      expect(activeWorkspaceColumn(storeApi.getState().workspaceState).kind).toBe('profile');
      const idsBefore = storeApi.getState().workspaceState.columns.map((column) => column.id);

      act(() => {
        storeApi.getState().patchState({
          workspaceState: {
            ...storeApi.getState().workspaceState,
            activeColumnId: columnIdentityId('explore', scope),
          },
        });
      });
      args.pendingRouteUrlRef.current = exploreUrl;
      setWindowHash(`#${exploreUrl}`);
      view.rerender({
        ...args,
        resolvedRouteLocation: { pathname: '/timeline', search: '?topic=kukuri%3Atopic%3Ageneral' },
        routeSection: 'timeline',
        state: selectShellRoutingSlice(storeApi.getState()),
      });

      expect(activeWorkspaceColumn(storeApi.getState().workspaceState).kind).toBe('explore');
      expect(args.pendingRouteUrlRef.current).toBe(exploreUrl);

      view.rerender({
        ...args,
        resolvedRouteLocation: { pathname: '/explore', search: '?topic=kukuri%3Atopic%3Ageneral' },
        routeSection: 'explore',
        state: selectShellRoutingSlice(storeApi.getState()),
      });

      expect(activeWorkspaceColumn(storeApi.getState().workspaceState).kind).toBe('explore');
      expect(args.pendingRouteUrlRef.current).toBeNull();
      expect(args.lastObservedRouteUrlRef.current).toBe(exploreUrl);
      expect(storeApi.getState().workspaceState.columns.map((column) => column.id)).toEqual(idsBefore);
      view.unmount();
    });

    test('does not normalize a stale rendered url back over a newer hash before the router renders it', () => {
      // 起動直後: topic の無い URL を既定の topic へ正規化する navigate の後、router が描画する前に hash が別の topic へ
      // 変わる(利用者の戻る・試験の hash の設定)。その間の store の更新で effect が古い描画の URL を処理し直し、正規化を
      // 予約し直すと、新しい hash を既定の topic で上書きしていた(実アプリの試験の smoke の時間切れ、#1837)。
      const generalUrl = '/timeline?topic=kukuri%3Atopic%3Ageneral';
      const testUrl = '/timeline?topic=kukuri%3Atopic%3Atest';
      const frames: Array<() => void> = [];
      const storeApi = createDesktopShellStore();
      const staleLocation = { pathname: '/timeline', search: '' };
      const args = createHookArgs(storeApi, {
        lastObservedRouteUrlRef: { current: '/timeline' },
        resolvedRouteLocation: staleLocation,
        scheduleAnimationFrame: (callback: () => void) => frames.push(callback),
        syncRoute: vi.fn(() => {
          args.pendingRouteUrlRef.current = generalUrl;
          setWindowHash(generalUrl);
        }),
      });
      const view = renderHook((props: RouteSynchronizationArgs) => useRouteSynchronization(props), {
        initialProps: args,
      });
      act(() => frames.splice(0).forEach((frame) => frame()));
      expect(args.syncRoute).toHaveBeenCalledTimes(1);

      setWindowHash(testUrl);
      for (const panel of ['loading', 'ready'] as const) {
        act(() => {
          storeApi.getState().patchState({
            channelPanelStateByTopic: { 'kukuri:topic:general': { status: panel, error: null } },
          });
        });
        view.rerender({
          ...args,
          resolvedRouteLocation: { ...staleLocation },
          state: selectShellRoutingSlice(storeApi.getState()),
        });
      }
      act(() => frames.splice(0).forEach((frame) => frame()));

      expect(args.syncRoute).toHaveBeenCalledTimes(1);
      expect(window.location.hash).toBe(`#${testUrl}`);
      view.unmount();
    });
  });
});
