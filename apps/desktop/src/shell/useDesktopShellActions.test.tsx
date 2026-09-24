/**
 * WP-S5: useDesktopShellActions の characterization テスト(ハンドラ単位)。
 *
 * 後続 WP-H6(shell store のスライス化・selector 化・prop drilling 除去)の
 * 安全網として「現時点で観測した挙動」をそのまま固定する。
 * このテストが落ちた場合、疑うべきは加えた変更でありテストではない。
 *
 * 方針:
 * - 引数 21 個は全て vi.fn / mock api / stub で注入し、返り値のハンドラ単位で
 *   「操作 → api 呼び出し引数 + store 状態遷移」を固定する。
 * - translate は key をそのまま返す stub を注入し、composerError 等の期待値は
 *   訳キー文字列で assert する(locale リソースの変更で割れないようにする)。
 * - ハンドラの参照同一性・返り値 58 キーの全量比較・snapshot は書かない
 *   (H6 のリファクタで無意味に割れるため)。固定するのは api 呼び出し引数
 *   (toHaveBeenCalledWith)・store 状態遷移・キー/フィールド単位の値のみ。
 */
import { act } from '@testing-library/react';
import type { FormEvent } from 'react';
import { beforeEach, describe, expect, test, vi } from 'vitest';

import type {
  BookmarkedPostView,
  CommunityNodeNodeStatus,
  CommunityNodePoliciesResponse,
  PostView,
  ReactionStateView,
  RecentReactionView,
} from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import i18n from '@/i18n';
import { columnDraftKey, setColumnDraft } from '@/shell/slices/columnDrafts';
import {
  activeWorkspaceColumn,
  activeWorkspaceScope,
  primarySectionForColumn,
} from '@/shell/slices/workspace';
import { renderActionsHook } from '@/shell/testSupport/renderShellActions';
import { resetWindowHash } from '@/shell/testSupport/renderShellHook';

const SELF_PUBKEY = 'f'.repeat(64);
const AUTHOR_A_PUBKEY = 'a'.repeat(64);

function buildPost(overrides: Partial<PostView> = {}): PostView {
  return {
    object_id: 'post-1',
    envelope_id: 'envelope-post-1',
    author_pubkey: AUTHOR_A_PUBKEY,
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

// Column Draft submit は FormEvent を受けるため preventDefault 記録付きの stub を渡す。
function publishFormEvent() {
  const preventDefault = vi.fn();
  return {
    event: { preventDefault } as unknown as FormEvent<HTMLFormElement>,
    preventDefault,
  };
}

function directMessageFormEvent(value: string) {
  const preventDefault = vi.fn();
  return {
    event: {
      preventDefault,
      currentTarget: {
        querySelector: () => ({ value }),
      },
    } as unknown as FormEvent<HTMLFormElement>,
    preventDefault,
  };
}

beforeEach(() => {
  resetWindowHash();
});

describe('useDesktopShellActions', () => {
  test('a late consent response cannot roll a newer ready event back to retrying', async () => {
    const baseApi = createDesktopMockApi();
    const config = await baseApi.getCommunityNodeConfig();
    const [ready] = await baseApi.getCommunityNodeStatuses();
    const pending = { ...ready, auth_state: { authenticated: false, expires_at: null }, consent_state: null };
    let complete!: (status: CommunityNodeNodeStatus) => void;
    const accept = vi.fn(() => new Promise<CommunityNodeNodeStatus>((resolve) => { complete = resolve; }));
    const { result, store } = renderActionsHook({ api: { ...baseApi, acceptCommunityNodeConsents: accept },
      preset: () => ({ communityNodeConfig: config, communityNodeStatuses: [pending] }),
    });
    let accepting!: Promise<void>;
    act(() => { accepting = result.current.handleAcceptCommunityNodeConsents(ready.base_url, [], 'en'); });
    act(() => store.getState().setField('communityNodeStatuses', [ready]));
    await act(async () => { complete({ ...pending, session_phase: 'retrying', last_error: 'old failure' }); await accepting; });
    expect(store.getState().communityNodeStatuses[0].session_phase).toBe('ready');
    expect(store.getState().communityNodeStatuses[0].auth_state.authenticated).toBe(true);
  });

  test('a late settings policy response cannot replace the newest language catalog', async () => {
    const baseApi = createDesktopMockApi();
    const node = (await baseApi.getCommunityNodeConfig()).nodes[0].base_url;
    const catalog = await baseApi.fetchCommunityNodePolicies(node);
    let complete!: (response: CommunityNodePoliciesResponse) => void;
    const fetch = vi.fn().mockImplementationOnce(() => new Promise<CommunityNodePoliciesResponse>((resolve) => { complete = resolve; }))
      .mockResolvedValueOnce({ policies: catalog.policies.map((policy) => ({ ...policy, body_markdown: 'new English document' })) });
    const { result, store } = renderActionsHook({ api: { ...baseApi, fetchCommunityNodePolicies: fetch } });
    await act(async () => i18n.changeLanguage('ja'));
    let first!: Promise<unknown>;
    act(() => { first = result.current.handleFetchCommunityNodeConsents(node); });
    await act(async () => i18n.changeLanguage('en'));
    await act(async () => result.current.handleFetchCommunityNodeConsents(node));
    await act(async () => { complete({ policies: catalog.policies.map((policy) => ({ ...policy, body_markdown: 'old Japanese document' })) }); await first; });
    const entry = store.getState().communityNodePolicies[node];
    expect(entry.status === 'ok' && entry.policies[0].body_markdown).toBe('new English document');
  });

  test('only the latest notification navigates after delayed topic loading', async () => {
    const { result, mocks } = renderActionsHook();
    let finish!: () => void;
    mocks.loadTopics.mockImplementationOnce(() => new Promise<undefined>((resolve) => {
      finish = () => resolve(undefined);
    }));
    const notification = (id: string) => ({
      notification_id: `notif-${id}`, kind: 'reply', actor_pubkey: AUTHOR_A_PUBKEY,
      topic_id: 'kukuri:topic:general', channel_id: null,
      object_id: id, thread_root_object_id: 'root', created_at: 1, received_at: 1,
    }) as Parameters<typeof result.current.handleOpenNotification>[0];
    let pending!: Promise<void>;
    act(() => { pending = result.current.handleOpenNotification(notification('old')); });
    await act(async () => { await result.current.handleOpenNotification(notification('new')); });
    await act(async () => { finish(); await pending; });
    expect(mocks.openThread).toHaveBeenCalledExactlyOnceWith('root', expect.objectContaining({
      topic: 'kukuri:topic:general', channelId: null, focusObjectId: 'new',
    }));
  });

  test('DM mutations use localized fallbacks for non-Error failures', async () => {
    const peerPubkey = 'd'.repeat(64);
    const sendDirectMessage = vi.fn().mockRejectedValue(null);
    const deleteDirectMessageMessage = vi.fn().mockRejectedValue(null);
    const clearDirectMessage = vi.fn().mockRejectedValue(null);
    const view = renderActionsHook({
      api: { sendDirectMessage, deleteDirectMessageMessage, clearDirectMessage },
      preset: (current) => ({
        selectedDirectMessagePeerPubkey: peerPubkey,
        directMessageComposer: 'hello',
        syncStatus: { ...current.syncStatus, local_author_pubkey: SELF_PUBKEY },
      }),
    });
    const { event } = directMessageFormEvent('hello');

    await act(async () => view.result.current.handleSendDirectMessage(event));
    expect(view.store.getState().directMessageError).toBe(
      'common:errors.failedToSendDirectMessage'
    );

    await act(async () =>
      view.result.current.handleDeleteDirectMessageMessage(peerPubkey, 'message-1')
    );
    expect(view.store.getState().directMessageError).toBe(
      'common:errors.failedToDeleteDirectMessage'
    );

    await act(async () => view.result.current.handleClearDirectMessage(peerPubkey));
    expect(view.store.getState().directMessageError).toBe(
      'common:errors.failedToClearDirectMessages'
    );
  });

  test('DM mutations preserve concrete Error messages', async () => {
    const peerPubkey = 'd'.repeat(64);
    const view = renderActionsHook({
      api: {
        sendDirectMessage: vi.fn().mockRejectedValue(new Error('send failed concretely')),
        deleteDirectMessageMessage: vi
          .fn()
          .mockRejectedValue(new Error('delete failed concretely')),
        clearDirectMessage: vi.fn().mockRejectedValue(new Error('clear failed concretely')),
      },
      preset: (current) => ({
        selectedDirectMessagePeerPubkey: peerPubkey,
        directMessageComposer: 'hello',
        syncStatus: { ...current.syncStatus, local_author_pubkey: SELF_PUBKEY },
      }),
    });
    const { event } = directMessageFormEvent('hello');

    await act(async () => view.result.current.handleSendDirectMessage(event));
    expect(view.store.getState().directMessageError).toBe('send failed concretely');

    await act(async () =>
      view.result.current.handleDeleteDirectMessageMessage(peerPubkey, 'message-1')
    );
    expect(view.store.getState().directMessageError).toBe('delete failed concretely');

    await act(async () => view.result.current.handleClearDirectMessage(peerPubkey));
    expect(view.store.getState().directMessageError).toBe('clear failed concretely');
  });

  test('handleToggleBookmarkedPost bookmarks then removes the post through the api', async () => {
    const post = buildPost({
      object_id: 'bookmark-post-1',
      origin_topic_id: 'kukuri:topic:general',
    });
    const bookmarkedView: BookmarkedPostView = {
      bookmarked_at: 1111,
      post: buildPost({
        object_id: 'bookmark-post-1',
        origin_topic_id: 'kukuri:topic:general',
      }),
    };
    const bookmarkPost = vi.fn(async () => bookmarkedView);
    const removeBookmarkedPost = vi.fn(async () => undefined);
    const view = renderActionsHook({
      api: { bookmarkPost, removeBookmarkedPost },
      preset: () => ({ error: 'previous error' }),
    });

    await act(async () => {
      await view.result.current.handleToggleBookmarkedPost(post);
    });

    expect(bookmarkPost).toHaveBeenCalledTimes(1);
    expect(bookmarkPost).toHaveBeenCalledWith('kukuri:topic:general', 'bookmark-post-1', {
      kind: 'public',
    });
    expect(removeBookmarkedPost).not.toHaveBeenCalled();
    expect(view.store.getState().bookmarkMembershipById['bookmark-post-1']).toBe(true);
    expect(view.store.getState().error).toBeNull();

    // 一覧ページ外のbookmarkでも判定を保持し、同じpostの2回目は解除する。
    await act(async () => {
      await view.result.current.handleToggleBookmarkedPost(post);
    });

    expect(removeBookmarkedPost).toHaveBeenCalledTimes(1);
    expect(removeBookmarkedPost).toHaveBeenCalledWith('bookmark-post-1');
    expect(bookmarkPost).toHaveBeenCalledTimes(1);
    expect(view.store.getState().bookmarkMembershipById['bookmark-post-1']).toBe(false);

    view.unmount();
  });

  test('handleToggleReaction patches reaction state into timelines and refreshes recent reactions', async () => {
    const post = buildPost({
      object_id: 'reaction-post-1',
      origin_topic_id: 'kukuri:topic:general',
    });
    const reactionState: ReactionStateView = {
      target_object_id: 'reaction-post-1',
      source_replica_id: 'replica-local-1',
      reaction_summary: [
        {
          reaction_key_kind: 'emoji',
          normalized_reaction_key: 'emoji:👍',
          emoji: '👍',
          custom_asset: null,
          count: 1,
        },
      ],
      my_reactions: [
        {
          reaction_key_kind: 'emoji',
          normalized_reaction_key: 'emoji:👍',
          emoji: '👍',
          custom_asset: null,
        },
      ],
    };
    const recentReactions: RecentReactionView[] = [
      {
        reaction_key_kind: 'emoji',
        normalized_reaction_key: 'emoji:👍',
        emoji: '👍',
        custom_asset: null,
        updated_at: 2222,
      },
    ];
    const toggleReaction = vi.fn(async () => reactionState);
    const listRecentReactions = vi.fn(async () => recentReactions);
    const view = renderActionsHook({
      api: { toggleReaction, listRecentReactions },
      preset: (current) => ({
        timelinesByKey: {
          ...current.timelinesByKey,
          'kukuri:topic:general::public': [post],
        },
        error: 'previous error',
      }),
    });

    await act(async () => {
      await view.result.current.handleToggleReaction(post, { kind: 'emoji', emoji: '👍' });
    });

    // channel_id 無しの post は public channel ref で toggle される
    expect(toggleReaction).toHaveBeenCalledTimes(1);
    expect(toggleReaction).toHaveBeenCalledWith(
      'kukuri:topic:general',
      'reaction-post-1',
      { kind: 'emoji', emoji: '👍' },
      { kind: 'public' }
    );
    const state = view.store.getState();
    expect(state.timelinesByKey['kukuri:topic:general::public'][0]?.reaction_summary).toEqual([
      {
        reaction_key_kind: 'emoji',
        normalized_reaction_key: 'emoji:👍',
        emoji: '👍',
        custom_asset: null,
        count: 1,
      },
    ]);
    expect(state.timelinesByKey['kukuri:topic:general::public'][0]?.my_reactions).toEqual([
      {
        reaction_key_kind: 'emoji',
        normalized_reaction_key: 'emoji:👍',
        emoji: '👍',
        custom_asset: null,
      },
    ]);
    expect(state.timelinesByKey['kukuri:topic:general::public'][0]?.reaction_summary).toEqual([
      {
        reaction_key_kind: 'emoji',
        normalized_reaction_key: 'emoji:👍',
        emoji: '👍',
        custom_asset: null,
        count: 1,
      },
    ]);
    expect(listRecentReactions).toHaveBeenCalledTimes(1);
    expect(listRecentReactions).toHaveBeenCalledWith(8);
    expect(state.recentReactions).toEqual([
      {
        reaction_key_kind: 'emoji',
        normalized_reaction_key: 'emoji:👍',
        emoji: '👍',
        custom_asset: null,
        updated_at: 2222,
      },
    ]);
    expect(state.error).toBeNull();

    view.unmount();
  });

  test('handleSelectTopic activates the topic with public scope and resets thread context', async () => {
    const threadPost = buildPost({ object_id: 'thread-post-1' });
    const view = renderActionsHook({
      preset: (current) => ({
        selectedThread: 'thread-post-1',
        threadsById: { 'thread-post-1': [threadPost] },
        selectedAuthorPubkey: AUTHOR_A_PUBKEY,
        selectedAuthorTimeline: [threadPost],
        authorError: 'previous author error',
        // 選択し直した topic の channel 選択と scope は public にリセットされる
        timelineScopeByTopic: {
          ...current.timelineScopeByTopic,
          'kukuri:topic:dev': { kind: 'channel', channel_id: 'channel-x' },
        },
        composeChannelByTopic: {
          ...current.composeChannelByTopic,
          'kukuri:topic:dev': { kind: 'private_channel', channel_id: 'channel-x' },
        },
        shellChromeState: {
          ...current.shellChromeState,
        },
      }),
    });

    await act(async () => {
      await view.result.current.handleSelectTopic('kukuri:topic:dev');
    });

    const state = view.store.getState();
    expect(activeWorkspaceScope(state.workspaceState).topicId).toBe('kukuri:topic:dev');
    expect(activeWorkspaceScope(state.workspaceState).channelId).toBeNull();
    expect(state.timelineScopeByTopic['kukuri:topic:dev']).toEqual({ kind: 'public' });
    expect(state.composeChannelByTopic['kukuri:topic:dev']).toEqual({ kind: 'public' });
    expect(primarySectionForColumn(activeWorkspaceColumn(state.workspaceState))).toBe('timeline');
    // clearThreadContext 相当のリセット
    expect(state.selectedThread).toBeNull();
    expect(state.threadsById['thread-post-1']).toEqual([threadPost]);
    expect(state.selectedAuthorPubkey).toBeNull();
    expect(state.selectedAuthor).toBeNull();
    expect(state.selectedAuthorTimeline).toEqual([]);
    expect(state.authorError).toBeNull();
    expect(view.mocks.syncRoute).toHaveBeenCalledTimes(1);
    expect(view.mocks.syncRoute).toHaveBeenCalledWith('replace', {
      activeTopic: 'kukuri:topic:dev',
      primarySection: 'timeline',
      timelineScope: { kind: 'public' },
      composeTarget: { kind: 'public' },
    });
    expect(view.mocks.loadTopics).toHaveBeenCalledTimes(1);
    expect(view.mocks.loadTopics).toHaveBeenCalledWith(
      ['kukuri:topic:general', 'kukuri:topic:dev', 'kukuri:topic:test'],
      'kukuri:topic:dev',
      null
    );

    view.unmount();
  });

  test('Column post sends to its own private scope and clears only that Draft', async () => {
    const createPost = vi.fn(async () => 'private-post-1');
    const privateTarget = {
      columnId: 'timeline-private',
      action: 'post' as const,
      scope: { topicId: 'topic-a', channelId: 'friends' },
    };
    const publicTarget = {
      columnId: 'timeline-public',
      action: 'post' as const,
      scope: { topicId: 'topic-a', channelId: null },
    };
    const view = renderActionsHook({
      api: { createPost },
      preset: (current) => ({
        columnDraftsByKey: setColumnDraft(
          setColumnDraft(current.columnDraftsByKey, privateTarget, (draft) => ({
            ...draft,
            content: 'private hello',
            expanded: true,
          })),
          publicTarget,
          (draft) => ({ ...draft, content: 'public remains' })
        ),
      }),
    });

    await act(async () => {
      await view.result.current.handleSubmitColumnDraft(
        privateTarget,
        publishFormEvent().event
      );
    });

    expect(createPost).toHaveBeenCalledWith(
      'topic-a',
      'private hello',
      null,
      [],
      { kind: 'private_channel', channel_id: 'friends' },
      []
    );
    const state = view.store.getState();
    expect(state.columnDraftsByKey[columnDraftKey(privateTarget)]).toBeUndefined();
    expect(state.columnDraftsByKey[columnDraftKey(publicTarget)]?.content).toBe('public remains');
    expect(state.timelinesByKey['topic-a::channel::friends']?.[0]).toMatchObject({
      content: 'private hello',
      channel_id: 'friends',
      local_state: 'syncing',
    });
    expect(state.profileTimeline).toEqual([]);
    expect(state.selectedAuthorTimeline).toEqual([]);
    expect(view.mocks.refreshVisibleTimelineAfterPublish).toHaveBeenCalledWith(
      'topic-a',
      null,
      'friends'
    );
  });

  test('pending and failed public drafts never enter a confirmed profile feed', async () => {
    let rejectPost!: (reason: Error) => void;
    const createPost = vi.fn(() => new Promise<string>((_resolve, reject) => { rejectPost = reject; }));
    const localProfile = await createDesktopMockApi().getMyProfile();
    const target = {
      columnId: 'timeline-public', action: 'post' as const,
      scope: { topicId: 'topic-a', channelId: null },
    };
    const confirmed = buildPost({ author_pubkey: SELF_PUBKEY });
    const view = renderActionsHook({
      api: { createPost },
      preset: (current) => ({
        localProfile,
        syncStatus: { ...current.syncStatus, local_author_pubkey: SELF_PUBKEY },
        profileTimeline: [confirmed],
        workspaceState: { ...current.workspaceState,
          activeColumnId: current.workspaceState.columns.find((column) => column.kind === 'profile')!.id },
        columnDraftsByKey: setColumnDraft(current.columnDraftsByKey, target,
          (draft) => ({ ...draft, content: 'failed public draft', expanded: true })),
      }),
    });
    let submission!: Promise<void>;
    act(() => {
      submission = view.result.current.handleSubmitColumnDraft(target, publishFormEvent().event);
    });
    expect(createPost).toHaveBeenCalledTimes(1);
    expect(view.store.getState().profileTimeline).toEqual([confirmed]);
    await act(async () => {
      rejectPost(new Error('post write failed'));
      await submission;
    });
    expect(view.store.getState().profileTimeline).toEqual([confirmed]);
    expect(view.store.getState().timelinesByKey['topic-a::public'][0].local_state).toBe('failed');
    expect(view.mocks.refreshVisibleTimelineAfterPublish).not.toHaveBeenCalled();
  });

  test('pending Column Draft rejects a repeated form submission', async () => {
    const createPost = vi.fn(async () => 'duplicate-post');
    const target = {
      columnId: 'timeline-pending',
      action: 'post' as const,
      scope: { topicId: 'topic-a', channelId: null },
    };
    const view = renderActionsHook({
      api: { createPost },
      preset: (current) => ({
        columnDraftsByKey: setColumnDraft(current.columnDraftsByKey, target, (draft) => ({
          ...draft,
          content: 'already sending',
          expanded: true,
          pending: true,
        })),
      }),
    });
    const form = publishFormEvent();

    await act(async () => {
      await view.result.current.handleSubmitColumnDraft(target, form.event);
    });

    expect(form.preventDefault).toHaveBeenCalledTimes(1);
    expect(createPost).not.toHaveBeenCalled();
    expect(view.store.getState().columnDraftsByKey[columnDraftKey(target)]).toMatchObject({
      content: 'already sending',
      pending: true,
    });
  });

  // #964: 本文も添付もない送信(Ctrl+Enter / ボタン)は無言で無視せず、理由を composer に表示する。
  test('empty Column Draft submission shows a reason instead of silently returning', async () => {
    const createPost = vi.fn(async () => 'never-created');
    const target = {
      columnId: 'timeline-empty',
      action: 'post' as const,
      scope: { topicId: 'topic-a', channelId: null },
    };
    const view = renderActionsHook({
      api: { createPost },
      preset: (current) => ({
        columnDraftsByKey: setColumnDraft(current.columnDraftsByKey, target, (draft) => ({
          ...draft,
          content: '   ',
          expanded: true,
        })),
      }),
    });
    const form = publishFormEvent();

    await act(async () => {
      await view.result.current.handleSubmitColumnDraft(target, form.event);
    });

    expect(form.preventDefault).toHaveBeenCalledTimes(1);
    expect(createPost).not.toHaveBeenCalled();
    expect(view.store.getState().columnDraftsByKey[columnDraftKey(target)]).toMatchObject({
      content: '   ',
      expanded: true,
      pending: false,
      error: 'common:composer.emptyDraft',
    });
  });

  test('quote repost expands the source topic public Column Draft and submits through createRepost', async () => {
    const createRepost = vi.fn(async () => 'repost-1');
    const source = buildPost({
      object_id: 'source-post-1',
      published_topic_id: 'kukuri:topic:dev',
      origin_topic_id: 'kukuri:topic:dev',
    });
    const view = renderActionsHook({ api: { createRepost } });

    act(() => view.result.current.beginColumnQuoteRepost(source));

    const scope = { topicId: 'kukuri:topic:dev', channelId: null };
    const target = {
      columnId: 'column:timeline:kukuri%3Atopic%3Adev:-:-',
      action: 'post' as const,
      scope,
    };
    expect(activeWorkspaceScope(view.store.getState().workspaceState)).toEqual(scope);
    expect(view.store.getState().columnDraftsByKey[columnDraftKey(target)]).toMatchObject({
      expanded: true,
      replyTarget: null,
      repostTarget: { object_id: 'source-post-1' },
    });

    await act(async () => {
      await view.result.current.handleSubmitColumnDraft(target, publishFormEvent().event);
    });
    expect(createRepost).toHaveBeenCalledWith(
      'kukuri:topic:dev',
      'kukuri:topic:dev',
      'source-post-1',
      null
    );
    expect(view.store.getState().columnDraftsByKey[columnDraftKey(target)]).toBeUndefined();
  });

  test('failed quote repost restores its source snapshot after the canonical post is evicted', async () => {
    const createRepost = vi.fn(async () => 'repost-restored-1');
    const failed = buildPost({
      object_id: 'local-failed-repost-1',
      object_kind: 'repost',
      content: 'quote me',
      local_state: 'failed',
      local_error: 'offline',
      local_draft: {
        kind: 'repost',
        topic: 'kukuri:topic:general',
        content: 'quote me',
        source_topic: 'kukuri:topic:dev',
        source_object_id: 'source-evicted-1',
        attachments: [],
      },
      repost_of: {
        source_object_id: 'source-evicted-1',
        source_topic_id: 'kukuri:topic:dev',
        source_author_pubkey: AUTHOR_A_PUBKEY,
        source_author_name: 'source-author',
        source_object_kind: 'post',
        content: 'source snapshot',
        attachments: [],
        reply_to: null,
        root_id: 'source-evicted-1',
      },
      repost_commentary: 'quote me',
    });
    const view = renderActionsHook({ api: { createRepost } });

    act(() => view.result.current.handleRestoreLocalPost(failed));

    const target = {
      columnId: 'column:timeline:kukuri%3Atopic%3Ageneral:-:-',
      action: 'post' as const,
      scope: { topicId: 'kukuri:topic:general', channelId: null },
    };
    expect(view.store.getState().columnDraftsByKey[columnDraftKey(target)]).toMatchObject({
      content: 'quote me',
      expanded: true,
      error: 'offline',
      repostTarget: {
        object_id: 'source-evicted-1',
        published_topic_id: 'kukuri:topic:dev',
        author_name: 'source-author',
        content: 'source snapshot',
      },
    });

    await act(async () => {
      await view.result.current.handleSubmitColumnDraft(target, publishFormEvent().event);
    });
    expect(createRepost).toHaveBeenCalledWith(
      'kukuri:topic:general',
      'kukuri:topic:dev',
      'source-evicted-1',
      'quote me'
    );
  });

  test('failed reply restore expands the matching Thread Column Draft with media and error', () => {
    const parent = buildPost({ object_id: 'parent-1', root_id: 'root-1' });
    const failed = buildPost({
      object_id: 'local-failed-1',
      root_id: 'root-1',
      reply_to: 'parent-1',
      local_state: 'failed',
      local_error: 'offline',
      local_draft: {
        kind: 'post',
        topic: 'kukuri:topic:general',
        content: 'restore me',
        reply_to: 'parent-1',
        channel_ref: { kind: 'public' },
        attachments: [],
      },
      local_draft_media_items: [
        {
          id: 'media-1',
          source_name: 'image.png',
          preview_url: 'blob:media-1',
          attachments: [
            {
              mime: 'image/png',
              byte_size: 10,
              data_base64: 'aGVsbG8=',
              role: 'image_original',
            },
          ],
        },
      ],
    });
    const view = renderActionsHook({
      preset: (current) => ({
        timelinesByKey: {
          ...current.timelinesByKey,
          'kukuri:topic:general::public': [parent],
        },
      }),
    });

    act(() => view.result.current.handleRestoreLocalPost(failed));

    const target = {
      columnId: 'column:thread:kukuri%3Atopic%3Ageneral:-:root-1',
      action: 'reply' as const,
      scope: { topicId: 'kukuri:topic:general', channelId: null },
      threadId: 'root-1',
    };
    expect(view.store.getState().columnDraftsByKey[columnDraftKey(target)]).toMatchObject({
      content: 'restore me',
      expanded: true,
      error: 'offline',
      replyTarget: { object_id: 'parent-1' },
      repostTarget: null,
      mediaItems: [{ id: 'media-1' }],
    });
    expect(activeWorkspaceColumn(view.store.getState().workspaceState)).toMatchObject({
      kind: 'thread',
      entityId: 'root-1',
    });
    expect(view.mocks.rememberDraftPreview).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'media-1' })
    );
  });

  test('Column message uses its peer Draft without clearing another conversation Draft', async () => {
    const sendDirectMessage = vi.fn(async () => 'message-1');
    const peerTarget = {
      columnId: 'conversation-peer-a',
      action: 'message' as const,
      peerPubkey: AUTHOR_A_PUBKEY,
    };
    const otherTarget = {
      columnId: 'conversation-peer-b',
      action: 'message' as const,
      peerPubkey: 'b'.repeat(64),
    };
    const view = renderActionsHook({
      api: { sendDirectMessage },
      preset: (current) => ({
        columnDraftsByKey: setColumnDraft(
          setColumnDraft(current.columnDraftsByKey, peerTarget, (draft) => ({
            ...draft,
            content: 'hello peer',
            expanded: true,
          })),
          otherTarget,
          (draft) => ({ ...draft, content: 'keep this' })
        ),
      }),
    });

    await act(async () => {
      await view.result.current.handleSubmitColumnDraft(peerTarget, publishFormEvent().event);
    });

    expect(sendDirectMessage).toHaveBeenCalledWith(AUTHOR_A_PUBKEY, 'hello peer', [], null);
    const state = view.store.getState();
    expect(state.columnDraftsByKey[columnDraftKey(peerTarget)]).toBeUndefined();
    expect(state.columnDraftsByKey[columnDraftKey(otherTarget)]?.content).toBe('keep this');
    expect(state.directMessageTimelineByPeer[AUTHOR_A_PUBKEY]?.[0]).toMatchObject({
      message_id: 'message-1',
      text: 'hello peer',
    });
  });

  test('failed Column post keeps the addressed Draft with its error', async () => {
    const target = {
      columnId: 'timeline-b',
      action: 'post' as const,
      scope: { topicId: 'topic-b', channelId: null },
    };
    const view = renderActionsHook({
      api: { createPost: vi.fn(async () => Promise.reject(new Error('offline'))) },
      preset: (current) => ({
        columnDraftsByKey: setColumnDraft(current.columnDraftsByKey, target, (draft) => ({
          ...draft,
          content: 'retry me',
          expanded: true,
        })),
      }),
    });

    await act(async () => {
      await view.result.current.handleSubmitColumnDraft(target, publishFormEvent().event);
    });

    expect(view.store.getState().columnDraftsByKey[columnDraftKey(target)]).toMatchObject({
      content: 'retry me',
      pending: false,
      error: 'offline',
    });
  });

});
