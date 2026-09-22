import type { ReactNode } from 'react';
import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { useDesktopShellSectionLoaders } from '@/shell/data/loaders/useDesktopShellSectionLoaders';
import { useNotificationLoaders } from '@/shell/data/loaders/useNotificationLoaders';
import { privateTimelineScope } from '@/shell/presentation';
import {
  createDesktopShellStore,
  DesktopShellStoreContext,
} from '@/shell/store';
import { columnIdentityId, openTransientColumn } from '@/shell/slices/workspace';
import { createDeferred } from '@/shell/DesktopShellPage.testHelpers';
import type { CommunityNodeManifestFetch, PostView, TimelineCursor, TimelineView } from '@/lib/api';

function cursor(createdAt: number, objectId: string): TimelineCursor {
  return { created_at: createdAt, object_id: objectId };
}

function profilePost(base: PostView, objectId: string, createdAt: number): PostView {
  return { ...base, object_id: objectId, envelope_id: `envelope-${objectId}`, created_at: createdAt };
}

function setup() {
  const api = createDesktopMockApi();
  const store = createDesktopShellStore();
  const loadReactionCatalogData = vi.fn().mockResolvedValue(undefined);
  const translate = (key: string) => key;
  const wrapper = ({ children }: { children: ReactNode }) => (
    <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>
  );
  const hook = renderHook(
    () => {
      const { loadNotificationsSection } = useNotificationLoaders({
        api, translate, activePrimarySection: 'notifications',
      });
      return useDesktopShellSectionLoaders({
        api,
        loadReactionCatalogData,
        loadNotificationsSection,
        storeApi: store,
        translate,
      });
    },
    { wrapper }
  );
  return { api, hook, store };
}

describe('useDesktopShellSectionLoaders', () => {
  test('section navigation does not refetch a confirmed profile', async () => {
    const { api, hook } = setup();
    await act(async () => hook.result.current.loadProfileSection());
    const read = vi.spyOn(api, 'getMyProfile');
    await act(async () => hook.result.current.loadShellSections('kukuri:topic:general'));
    expect(read).not.toHaveBeenCalled();
  });

  test('an empty profile remains confirmed after refresh failure and recovers on retry', async () => {
    const { api, hook, store } = setup();
    expect(store.getState().profileHasLoaded).toBe(false);
    await act(async () => hook.result.current.loadProfileSection());
    expect(store.getState().profileHasLoaded).toBe(true);
    vi.spyOn(api, 'listProfileTimeline').mockRejectedValueOnce(new Error('refresh failed'));
    await act(async () => hook.result.current.loadProfileSection());
    expect(store.getState()).toMatchObject({
      profileHasLoaded: true, profileRefreshing: false, profileTimeline: [], profileError: 'refresh failed',
    });
    await act(async () => hook.result.current.loadProfileSection());
    expect(store.getState()).toMatchObject({ profileHasLoaded: true, profileRefreshing: false, profileError: null });
  });

  test('an older completion cannot stop the latest profile refresh', async () => {
    const { api, hook, store } = setup();
    const first = createDeferred<TimelineView>();
    const latest = createDeferred<TimelineView>();
    const read = vi.spyOn(api, 'listProfileTimeline')
      .mockReturnValueOnce(first.promise).mockReturnValueOnce(latest.promise);
    let firstLoad!: Promise<void>;
    let latestLoad!: Promise<void>;
    act(() => { firstLoad = hook.result.current.loadProfileSection(); });
    await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    act(() => { latestLoad = hook.result.current.loadProfileSection(); });
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    await act(async () => { first.resolve({ items: [], next_cursor: null }); await firstLoad; });
    expect(store.getState().profileRefreshing).toBe(true);
    expect(store.getState().profileHasLoaded).toBe(false);
    await act(async () => { latest.resolve({ items: [], next_cursor: null }); await latestLoad; });
    expect(store.getState().profileRefreshing).toBe(false);
    expect(store.getState().profileHasLoaded).toBe(true);
  });

  test.each(['success', 'failure'])('a read started before saving cannot apply its %s', async (outcome) => {
    const { api, hook, store } = setup();
    await act(async () => hook.result.current.loadProfileSection());
    const old = createDeferred<TimelineView>();
    const read = vi.spyOn(api, 'listProfileTimeline').mockReturnValue(old.promise);
    let loading!: Promise<void>;
    act(() => { loading = hook.result.current.loadProfileSection(); });
    await waitFor(() => expect(read).toHaveBeenCalled());
    const saved = await api.setMyProfile({ display_name: 'Saved name', name: 'saved-user' });
    act(() => store.getState().patchState({
      localProfile: saved, profileSaveRevision: 1, profileDirty: true,
      profileDraft: { display_name: 'Next unsaved name' },
    }));
    await act(async () => {
      if (outcome === 'success') old.resolve({ items: [], next_cursor: null });
      else old.reject(new Error('old read failed'));
      await loading;
    });
    expect(store.getState().localProfile).toEqual(saved);
    expect(store.getState().profileDraft.display_name).toBe('Next unsaved name');
    expect(store.getState().profileError).toBeNull();
    expect(store.getState().profileRefreshing).toBe(false);
  });

  test('refreshing a confirmed empty profile retains its ready state', async () => {
    const { api, hook, store } = setup();
    await act(async () => hook.result.current.loadProfileSection());
    const deferred = createDeferred<TimelineView>();
    const read = vi.spyOn(api, 'listProfileTimeline').mockReturnValue(deferred.promise);
    let loading!: Promise<void>;
    act(() => { loading = hook.result.current.loadProfileSection(); });
    await waitFor(() => expect(read).toHaveBeenCalled());
    const pendingState = store.getState().profilePanelState;
    await act(async () => { deferred.resolve({ items: [], next_cursor: null }); await loading; });
    expect(pendingState).toEqual({ status: 'ready', error: null });
  });

  test('manifest completion resolves the index using consent received after the request started', async () => {
    const { api, hook, store } = setup();
    const node = 'https://first.example';
    const config = await api.setCommunityNodeConfig([{ base_url: node }]);
    const pending = await api.getCommunityNodeStatuses();
    const manifest = await api.fetchCommunityNodeManifest(node);
    store.getState().patchState({ communityNodeConfig: config, communityNodeStatuses: pending });
    const deferred = createDeferred<CommunityNodeManifestFetch>();
    const fetch = vi.spyOn(api, 'fetchCommunityNodeManifest').mockReturnValue(deferred.promise);
    let loading!: Promise<void>;
    act(() => { loading = hook.result.current.loadCommunityIndexCapability(); });
    await waitFor(() => expect(fetch).toHaveBeenCalled());
    const policies = await api.fetchCommunityNodePolicies(node);
    const ready = await api.acceptCommunityNodeConsents(node, policies.policies, 'en');
    act(() => store.getState().setField('communityNodeStatuses', [ready]));
    await act(async () => { deferred.resolve(manifest); await loading; });
    expect(store.getState().communityIndexNodeBaseUrl).toBe(node);
  });

  test('an older manifest response cannot overwrite a newer capability result', async () => {
    const { api, hook, store } = setup();
    const node = 'https://first.example';
    await api.setCommunityNodeConfig([{ base_url: node }]);
    const policies = await api.fetchCommunityNodePolicies(node);
    const ready = await api.acceptCommunityNodeConsents(node, policies.policies, 'en');
    store.getState().setField('communityNodeStatuses', [ready]);
    const manifest = await api.fetchCommunityNodeManifest(node);
    const deferred = createDeferred<CommunityNodeManifestFetch>();
    const fetch = vi.spyOn(api, 'fetchCommunityNodeManifest')
      .mockReturnValueOnce(deferred.promise).mockResolvedValue(manifest);
    let first!: Promise<void>;
    act(() => { first = hook.result.current.loadCommunityIndexCapability(); });
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
    await act(async () => hook.result.current.loadCommunityIndexCapability());
    await act(async () => { deferred.resolve({ status: 'absent', manifest: null }); await first; });
    expect(store.getState().communityNodeManifests[node]?.status).toBe('ok');
    expect(store.getState().communityIndexNodeBaseUrl).toBe(node);
  });

  test('loads an inactive own profile and preserves an unsaved profile draft', async () => {
    const { api, hook, store } = setup();
    const profile = await api.getMyProfile();
    const objectId = await api.createPost('kukuri:topic:general', 'saved public post');
    const activeColumnId = store.getState().workspaceState.activeColumnId;
    store.setState({ profileDirty: true, profileDraft: { display_name: 'unsaved name' } });

    await act(async () => hook.result.current.loadShellSections('kukuri:topic:general'));

    expect(store.getState().profileTimeline.map((post) => post.object_id)).toContain(objectId);
    expect(store.getState().localProfile?.pubkey).toBe(profile.pubkey);
    expect(store.getState().profileDraft.display_name).toBe('unsaved name');
    expect(store.getState().workspaceState.activeColumnId).toBe(activeColumnId);
  });

  test.each(['success', 'failure'])('ignores an older profile %s after a newer response', async (outcome) => {
    const { api, hook, store } = setup();
    const profile = await api.getMyProfile();
    await api.createPost('kukuri:topic:general', 'newest profile post');
    const latest = await api.listProfileTimeline(profile.pubkey);
    const old = createDeferred<TimelineView>();
    const listProfileTimeline = vi.spyOn(api, 'listProfileTimeline')
      .mockReturnValueOnce(old.promise)
      .mockResolvedValueOnce(latest);
    let first!: Promise<void>;
    act(() => { first = hook.result.current.loadProfileSection(); });
    await waitFor(() => expect(listProfileTimeline).toHaveBeenCalledTimes(1));
    await act(async () => hook.result.current.loadProfileSection());
    await act(async () => {
      if (outcome === 'success') old.resolve({ items: [], next_cursor: null });
      else old.reject(new Error('outdated failure'));
      await first;
    });
    expect(store.getState().profileTimeline).toEqual(latest.items);
    expect(store.getState().profilePanelState).toEqual({ status: 'ready', error: null });
  });

  test('retains confirmed posts on profile failure and recovers on retry', async () => {
    const { api, hook, store } = setup();
    await api.createPost('kukuri:topic:general', 'retained public post');
    await act(async () => hook.result.current.loadProfileSection());
    const confirmed = store.getState().profileTimeline;
    vi.spyOn(api, 'listProfileTimeline').mockRejectedValueOnce(new Error('profile read failed'));
    await act(async () => hook.result.current.loadProfileSection());
    expect(store.getState().profileTimeline).toEqual(confirmed);
    expect(store.getState().profileError).toBe('profile read failed');
    await act(async () => hook.result.current.loadProfileSection());
    expect(store.getState().profileTimeline).toEqual(confirmed);
    expect(store.getState().profileError).toBeNull();
  });

  test('loads profile pages through an empty page while preserving the advanced cursor', async () => {
    const { api, hook, store } = setup();
    const profile = await api.getMyProfile();
    await api.createPost('kukuri:topic:general', 'sample');
    const base = (await api.listProfileTimeline(profile.pubkey)).items[0];
    const newest = profilePost(base, 'profile-newest', 30);
    const oldest = profilePost(base, 'profile-oldest', 10);
    const firstCursor = cursor(30, newest.object_id);
    const hiddenCursor = cursor(20, 'hidden-row');
    const read = vi.spyOn(api, 'listProfileTimeline')
      .mockResolvedValueOnce({ items: [newest], next_cursor: firstCursor })
      .mockResolvedValueOnce({ items: [], next_cursor: hiddenCursor })
      .mockResolvedValueOnce({ items: [oldest], next_cursor: null });

    await act(async () => hook.result.current.loadProfileSection());
    await act(async () => hook.result.current.loadMoreProfileTimeline());
    expect(store.getState().profileTimeline).toEqual([newest]);
    expect(store.getState().profileTimelineNextCursor).toEqual(hiddenCursor);
    await act(async () => hook.result.current.loadMoreProfileTimeline());

    expect(read).toHaveBeenNthCalledWith(2, profile.pubkey, firstCursor, 20);
    expect(read).toHaveBeenNthCalledWith(3, profile.pubkey, hiddenCursor, 20);
    expect(store.getState().profileTimeline).toEqual([newest, oldest]);
    expect(store.getState().profileTimelineNextCursor).toBeNull();
  });

  test('keeps the profile cursor after a page failure and retries only when asked', async () => {
    const { api, hook, store } = setup();
    const profile = await api.getMyProfile();
    await api.createPost('kukuri:topic:general', 'sample');
    const base = (await api.listProfileTimeline(profile.pubkey)).items[0];
    const nextCursor = cursor(10, base.object_id);
    const read = vi.spyOn(api, 'listProfileTimeline')
      .mockResolvedValueOnce({ items: [base], next_cursor: nextCursor })
      .mockRejectedValueOnce(new Error('older page unavailable'))
      .mockResolvedValueOnce({ items: [], next_cursor: null });

    await act(async () => hook.result.current.loadProfileSection());
    await act(async () => hook.result.current.loadMoreProfileTimeline());
    expect(store.getState()).toMatchObject({
      profileTimeline: [base],
      profileTimelineNextCursor: nextCursor,
      profileTimelineLoadingMore: false,
      profileTimelineLoadMoreError: 'older page unavailable',
    });
    expect(read).toHaveBeenCalledTimes(2);

    await act(async () => hook.result.current.loadMoreProfileTimeline());
    expect(read).toHaveBeenCalledTimes(3);
    expect(store.getState().profileTimelineNextCursor).toBeNull();
    expect(store.getState().profileTimelineLoadMoreError).toBeNull();
  });

  test('a late profile page cannot overwrite a newer head refresh', async () => {
    const { api, hook, store } = setup();
    const profile = await api.getMyProfile();
    await api.createPost('kukuri:topic:general', 'sample');
    const base = (await api.listProfileTimeline(profile.pubkey)).items[0];
    const first = profilePost(base, 'first-page', 30);
    const stale = profilePost(base, 'stale-page', 10);
    const refreshed = profilePost(base, 'refreshed-head', 40);
    const nextCursor = cursor(30, first.object_id);
    const pending = createDeferred<TimelineView>();
    const read = vi.spyOn(api, 'listProfileTimeline')
      .mockResolvedValueOnce({ items: [first], next_cursor: nextCursor })
      .mockReturnValueOnce(pending.promise)
      .mockResolvedValueOnce({ items: [refreshed], next_cursor: null });

    await act(async () => hook.result.current.loadProfileSection());
    let loading!: Promise<void>;
    act(() => { loading = hook.result.current.loadMoreProfileTimeline(); });
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    await act(async () => hook.result.current.loadProfileSection());
    await act(async () => { pending.resolve({ items: [stale], next_cursor: null }); await loading; });

    expect(store.getState().profileTimeline).toEqual([refreshed]);
    expect(store.getState().profileTimelineLoadingMore).toBe(false);
  });

  test('keeps author pagination isolated by pubkey', async () => {
    const { api, hook, store } = setup();
    const profile = await api.getMyProfile();
    await api.createPost('kukuri:topic:general', 'sample');
    const base = (await api.listProfileTimeline(profile.pubkey)).items[0];
    const alice = 'a'.repeat(64);
    const bob = 'b'.repeat(64);
    const aliceFirst = profilePost(base, 'alice-first', 30);
    const aliceOlder = profilePost(base, 'alice-older', 10);
    const bobOnly = profilePost(base, 'bob-only', 20);
    const aliceCursor = cursor(30, aliceFirst.object_id);
    vi.spyOn(api, 'getAuthorSocialView').mockImplementation(async (pubkey) => ({
      ...(await api.getMyProfile()),
      author_pubkey: pubkey,
      display_name: pubkey === alice ? 'Alice' : 'Bob',
      following: false,
      followed_by: false,
      mutual: false,
      friend_of_friend: false,
      friend_of_friend_via_pubkeys: [],
      muted: false,
      blocking: false,
      blocked_by: false,
    }));
    vi.spyOn(api, 'listProfileTimeline').mockImplementation(async (pubkey, pageCursor) => {
      if (pubkey === alice && !pageCursor) return { items: [aliceFirst], next_cursor: aliceCursor };
      if (pubkey === alice) return { items: [aliceOlder], next_cursor: null };
      return { items: [bobOnly], next_cursor: null };
    });

    await act(async () => hook.result.current.loadAuthorSection(alice));
    await act(async () => hook.result.current.loadAuthorSection(bob));
    await act(async () => hook.result.current.loadMoreAuthorTimeline(alice));

    expect(store.getState().authorTimelinesByPubkey[alice]).toEqual([aliceFirst, aliceOlder]);
    expect(store.getState().authorTimelinesByPubkey[bob]).toEqual([bobOnly]);
    expect(store.getState().authorTimelineNextCursorByPubkey[alice]).toBeNull();
  });

  test('refreshing an own-author column does not replace another selected author', async () => {
    const { api, hook, store } = setup();
    const profile = await api.getMyProfile();
    await api.createPost('kukuri:topic:general', 'own-author refresh');
    const ownTimeline = await api.listProfileTimeline(profile.pubkey);
    const otherPubkey = 'b'.repeat(64);
    const other = { ...ownTimeline.items[0], object_id: 'other-post', author_pubkey: otherPubkey };
    store.setState({ selectedAuthorPubkey: otherPubkey, selectedAuthorTimeline: [other] });
    await act(async () => hook.result.current.loadAuthorSection(profile.pubkey));
    expect(store.getState().selectedAuthorTimeline).toEqual([other]);
    expect(store.getState().authorTimelinesByPubkey[profile.pubkey]).toEqual(ownTimeline.items);
  });

  test('does not fetch an own profile when its column is closed', async () => {
    const { api, hook, store } = setup();
    store.setState((state) => ({ workspaceState: {
      ...state.workspaceState,
      columns: state.workspaceState.columns.filter((column) => column.kind !== 'profile'),
    } }));
    const read = vi.spyOn(api, 'listProfileTimeline');
    await act(async () => hook.result.current.loadShellSections('kukuri:topic:general'));
    expect(read).not.toHaveBeenCalled();
  });

  test('loads only the active live section with the selected channel scope', async () => {
    const { api, hook, store } = setup();
    const listLiveSessions = vi.spyOn(api, 'listLiveSessions').mockResolvedValue([]);
    const listGameRooms = vi.spyOn(api, 'listGameRooms');
    store.setState((state) => ({
      workspaceState: openTransientColumn(state.workspaceState, {
        id: columnIdentityId('stream', { topicId: 'topic', channelId: 'channel-a' }),
        kind: 'stream',
        scope: { topicId: 'topic', channelId: 'channel-a' },
        pinned: false,
      }),
    }));

    await act(async () => hook.result.current.loadShellSections('topic'));

    expect(listLiveSessions).toHaveBeenCalledWith(
      'topic',
      privateTimelineScope('channel-a')
    );
    expect(store.getState().livePanelStateByScopeKey['topic::channel::channel-a']).toEqual({
      status: 'ready',
      error: null,
    });
    expect(listGameRooms).not.toHaveBeenCalled();
  });

  test('keeps fulfilled DM status when the timeline branch fails', async () => {
    const { api, hook, store } = setup();
    const peer = 'b'.repeat(64);
    const status = await api.getDirectMessageStatus(peer);
    store.setState((state) => ({
      selectedDirectMessagePeerPubkey: peer,
      workspaceState: openTransientColumn(state.workspaceState, {
        id: columnIdentityId('conversation', { topicId: 'topic', channelId: null }, peer),
        kind: 'conversation',
        scope: { topicId: 'topic', channelId: null },
        entityId: peer,
        pinned: false,
      }),
    }));
    vi.spyOn(api, 'listDirectMessages').mockResolvedValue([]);
    vi.spyOn(api, 'listDirectMessageMessages').mockRejectedValue(new Error('timeline failed'));
    vi.spyOn(api, 'getDirectMessageStatus').mockResolvedValue(status);

    await act(async () => hook.result.current.loadShellSections('topic'));

    expect(store.getState().directMessageStatusByPeer[peer]).toEqual(status);
    expect(store.getState().directMessageError).toBe('timeline failed');
  });

  test('uses the localized DM load fallback for a non-Error failure', async () => {
    const { api, hook, store } = setup();
    store.setState((state) => ({
      workspaceState: openTransientColumn(state.workspaceState, {
        id: columnIdentityId('messages', { topicId: 'topic', channelId: null }),
        kind: 'messages',
        scope: { topicId: 'topic', channelId: null },
        pinned: false,
      }),
    }));
    vi.spyOn(api, 'listDirectMessages').mockRejectedValue(null);

    await act(async () => hook.result.current.loadShellSections('topic'));

    expect(store.getState().directMessageError).toBe(
      'common:errors.failedToLoadDirectMessages'
    );
  });

  test('refreshes discovery config without overwriting a dirty editor', async () => {
    const { api, hook, store } = setup();
    const config = await api.getDiscoveryConfig();
    store.setState((state) => ({
      discoveryEditorDirty: true,
      discoverySeedInput: 'keep-local-editor',
      shellChromeState: {
        ...state.shellChromeState,
        settingsOpen: true,
        activeSettingsSection: 'discovery',
      },
    }));
    vi.spyOn(api, 'getDiscoveryConfig').mockResolvedValue(config);

    await act(async () => hook.result.current.loadShellSections('topic'));

    expect(store.getState().discoveryConfig).toEqual(config);
    expect(store.getState().discoverySeedInput).toBe('keep-local-editor');
  });
});
