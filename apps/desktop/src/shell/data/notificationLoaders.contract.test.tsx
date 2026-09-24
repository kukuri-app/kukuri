import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import type { DesktopApi, NotificationView, RuntimeEvent } from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { useDesktopShellSectionLoaders } from '@/shell/data/loaders/useDesktopShellSectionLoaders';
import { useNotificationLoaders } from '@/shell/data/loaders/useNotificationLoaders';
import { columnIdentityId, openTransientColumn } from '@/shell/slices/workspace';
import { createShellHookHarness, resetWindowHash } from '@/shell/testSupport/renderShellHook';
import { useDesktopShellData } from '@/shell/useDesktopShellData';

const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }));

const translate = (key: string) => key;
const unread: NotificationView = {
  notification_id: 'private-reply', kind: 'reply', actor_pubkey: 'b'.repeat(64), actor_picture_asset: null,
  source_envelope_id: 'reply-envelope', source_replica_id: 'private-replica',
  topic_id: 'kukuri:topic:general', channel_id: 'members', object_id: 'reply',
  thread_root_object_id: 'root', preview_text: 'private preview', content_labels: ['adult'],
  created_at: 11, received_at: 12, read_at: null,
};
const read: NotificationView = {
  notification_id: 'read-dm', kind: 'direct_message', actor_pubkey: 'c'.repeat(64), actor_picture_asset: null,
  dm_id: 'conversation', message_id: 'message', preview_text: 'older message',
  created_at: 5, received_at: 6, read_at: 7,
};

beforeEach(() => {
  resetWindowHash();
  vi.useFakeTimers();
  vi.setSystemTime(100_000);
  listenMock.mockReset().mockResolvedValue(() => undefined);
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
});

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

async function flush(milliseconds = 0) {
  await act(async () => { await vi.advanceTimersByTimeAsync(milliseconds); });
}

function mountData(api: DesktopApi) {
  const harness = createShellHookHarness({ hash: '/timeline?topic=kukuri%3Atopic%3Ageneral' });
  const workspace = harness.store.getState().workspaceState;
  // このsuiteはbadge更新を測る。背景inboxの独立したsection取得は別testで確認する。
  harness.store.getState().patchState({
    workspaceState: { ...workspace, columns: workspace.columns.filter(c => c.kind !== 'notifications') },
    notifications: [read], notificationStatus: { unread_count: 9 },
  });
  const args = {
    api, translate,
    loadTopicsRequestRef: { current: new Map<string, number>() },
    remoteObjectUrlRef: { current: new Map<string, string>() },
    draftPreviewUrlRef: { current: new Map<string, string>() },
    directMessageDraftPreviewUrlRef: { current: new Map<string, string>() },
    mediaFetchAttemptRef: { current: new Map<string, number>() },
    draftSequenceRef: { current: 0 },
  };
  const hook = renderHook(() => useDesktopShellData(args), { wrapper: harness.wrapper });
  return { ...harness, hook };
}

function mountInbox() {
  const api = createDesktopMockApi({ notifications: [unread, read] });
  const harness = createShellHookHarness();
  harness.store.getState().patchState({
    notifications: [read], notificationStatus: { unread_count: 9 },
    notificationPanelState: { status: 'loading', error: null },
    notificationAutoReadError: 'previous read failure',
  });
  const loadReactionCatalogData = vi.fn().mockResolvedValue(undefined);
  const hook = renderHook(() => {
    const { loadNotificationsSection } = useNotificationLoaders({
      api, translate, activePrimarySection: 'notifications',
    });
    return useDesktopShellSectionLoaders({
      api, storeApi: harness.store, translate, loadReactionCatalogData, loadNotificationsSection,
    });
  }, { wrapper: harness.wrapper });
  return { api, ...harness, hook };
}

describe('notification badge contract (#919 TR-1/2/3/8)', () => {
  test('hidden mount, interval and runtime event issue no notification requests', async () => {
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    let receive: ((event: { payload: RuntimeEvent }) => void) | undefined;
    listenMock.mockImplementation(async (_name: string, callback: typeof receive) => {
      receive = callback;
      return () => { receive = undefined; };
    });
    const api = createDesktopMockApi({ notifications: [unread] });
    const status = vi.spyOn(api, 'getNotificationStatus');
    const list = vi.spyOn(api, 'listNotificationsPage');
    const mark = vi.spyOn(api, 'markAllNotificationsRead');
    const { hook, store } = mountData(api);
    await flush(60_000);
    expect(receive).toBeDefined();
    await act(async () => { receive?.({ payload: { type: 'notification_status_changed' } }); });
    expect(status).not.toHaveBeenCalled();
    expect(list).not.toHaveBeenCalled();
    expect(mark).not.toHaveBeenCalled();
    expect(store.getState().notifications).toEqual([read]);
    expect(store.getState().notificationStatus).toEqual({ unread_count: 9 });
    hook.unmount();
  });

  test.each([0, 1])('outside inbox, badge count %i refreshes without reading the inbox', async count => {
    const api = createDesktopMockApi();
    vi.spyOn(api, 'getNotificationStatus').mockResolvedValue({ unread_count: count });
    const list = vi.spyOn(api, 'listNotificationsPage');
    const mark = vi.spyOn(api, 'markAllNotificationsRead');
    const { hook, store } = mountData(api);
    const panel = store.getState().notificationPanelState;
    await flush();
    expect(list).not.toHaveBeenCalled();
    expect(store.getState().notifications).toEqual([read]);
    expect(store.getState().notificationStatus).toEqual({ unread_count: count });
    expect(store.getState().notificationPanelState).toEqual(panel);
    expect(mark).not.toHaveBeenCalled();
    hook.unmount();
  });

  test('badge status failure keeps previous rows and does not set inbox error', async () => {
    const api = createDesktopMockApi();
    vi.spyOn(api, 'getNotificationStatus').mockRejectedValue(new Error('offline'));
    const list = vi.spyOn(api, 'listNotificationsPage');
    const mark = vi.spyOn(api, 'markAllNotificationsRead');
    const { hook, store } = mountData(api);
    const panel = store.getState().notificationPanelState;
    await flush();
    expect(store.getState().notificationStatus).toEqual({ unread_count: 9 });
    expect(store.getState().notifications).toEqual([read]);
    expect(store.getState().notificationPanelState).toEqual(panel);
    expect(list).not.toHaveBeenCalled();
    expect(mark).not.toHaveBeenCalled();
    hook.unmount();
  });

  test.each([0, 1])('active inbox badge count %i refreshes one page on event without marking read', async count => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    let receive: ((event: { payload: RuntimeEvent }) => void) | undefined;
    const unsubscribe = vi.fn();
    listenMock.mockImplementation(async (_name: string, callback: typeof receive) => {
      receive = callback;
      return () => { receive = undefined; unsubscribe(); };
    });
    const api = createDesktopMockApi();
    const status = vi.spyOn(api, 'getNotificationStatus').mockResolvedValue({ unread_count: count });
    const list = vi.spyOn(api, 'listNotificationsPage').mockResolvedValue({
      items: [read], newer_cursor: null, older_cursor: null,
    });
    const mark = vi.spyOn(api, 'markAllNotificationsRead');
    const { hook, store } = mountData(api);
    await flush(30_000);
    status.mockClear();
    act(() => {
      store.getState().patchState({ workspaceState: openTransientColumn(store.getState().workspaceState, {
        id: 'contract-inbox', kind: 'notifications', pinned: false,
      }) });
    });
    await flush();
    expect(status).toHaveBeenCalledTimes(1);
    status.mockClear(); list.mockClear(); mark.mockClear();
    await flush(30_000);
    expect(status).toHaveBeenCalledTimes(1);
    await flush(30_000);
    expect(status).toHaveBeenCalledTimes(1);
    await act(async () => { receive?.({ payload: { type: 'notification_status_changed' } }); });
    expect(status).toHaveBeenCalledTimes(2);
    expect(list).toHaveBeenCalledTimes(1);
    expect(mark).not.toHaveBeenCalled();
    expect(store.getState().notifications).toEqual([read]);
    expect(listenMock).toHaveBeenCalledTimes(1);
    hook.unmount();
    expect(unsubscribe).toHaveBeenCalledTimes(1);
    expect(receive).toBeUndefined();
    await flush(60_000);
    expect(status).toHaveBeenCalledTimes(2);
    const remount = mountData(api);
    await flush();
    expect(listenMock).toHaveBeenCalledTimes(2);
    remount.hook.unmount();
    expect(unsubscribe).toHaveBeenCalledTimes(2);
  });
});

describe('notification inbox contract (#919 TR-4/5/6/7)', () => {
  test('an account switch rejects an older notification page response', async () => {
    const api = createDesktopMockApi();
    let resolvePage!: (page: Awaited<ReturnType<DesktopApi['listNotificationsPage']>>) => void;
    const readPage = vi.spyOn(api, 'listNotificationsPage').mockReturnValue(
      new Promise((resolve) => { resolvePage = resolve; })
    );
    const harness = createShellHookHarness();
    harness.store.getState().patchState({ syncStatus: {
      ...harness.store.getState().syncStatus, local_author_pubkey: 'account-a',
    } });
    const hook = renderHook(() => useNotificationLoaders({ api, translate, activePrimarySection: 'notifications' }),
      { wrapper: harness.wrapper });
    let loading!: Promise<void>;
    act(() => { loading = hook.result.current.loadNotificationsSection({ markAsRead: false }); });
    expect(readPage).toHaveBeenCalledTimes(1);
    act(() => harness.store.getState().patchState({ syncStatus: {
      ...harness.store.getState().syncStatus, local_author_pubkey: 'account-b',
    } }));
    await act(async () => {
      resolvePage({ items: [unread], newer_cursor: null, older_cursor: null });
      await loading;
    });
    expect(harness.store.getState().notifications).toEqual([]);
    hook.unmount();
  });

  test('an unchanged notification event leaves the displayed store snapshot untouched', async () => {
    const api = createDesktopMockApi({ notifications: [read] });
    const harness = createShellHookHarness();
    const hook = renderHook(() => useNotificationLoaders({ api, translate, activePrimarySection: 'notifications' }),
      { wrapper: harness.wrapper });
    await act(async () => hook.result.current.loadNotificationsSection({ markAsRead: false }));
    const snapshot = harness.store.getState();
    await act(async () => hook.result.current.refreshNotificationsFromEvent());
    expect(harness.store.getState()).toBe(snapshot);
    hook.unmount();
  });

  test('replaces notification pages in both directions without reading the full inbox', async () => {
    const api = createDesktopMockApi({ notifications: Array.from({ length: 25 }, (_, index) => ({
      ...read, notification_id: `page-${index}`, received_at: 100 - index,
    })) });
    const pageRead = vi.spyOn(api, 'listNotificationsPage');
    const harness = createShellHookHarness();
    const hook = renderHook(() => useNotificationLoaders({ api, translate, activePrimarySection: 'notifications' }),
      { wrapper: harness.wrapper });
    await act(async () => hook.result.current.loadNotificationsSection({ markAsRead: false }));
    expect(harness.store.getState().notifications).toHaveLength(20);
    expect(harness.store.getState().notificationsOlderCursor).not.toBeNull();
    await act(async () => hook.result.current.navigateNotificationPage(false));
    expect(harness.store.getState().notifications).toHaveLength(5);
    expect(harness.store.getState().notificationsNewerCursor).not.toBeNull();
    await act(async () => hook.result.current.navigateNotificationPage(true));
    expect(harness.store.getState().notifications).toHaveLength(20);
    expect(pageRead).toHaveBeenCalledTimes(3);
    hook.unmount();
  });

  test('background refresh preserves mixed private/adult/read payloads without read mutation', async () => {
    const { api, hook, store } = mountInbox();
    const mark = vi.spyOn(api, 'markAllNotificationsRead');
    await act(async () => { await hook.result.current.loadNotificationsSection({ markAsRead: false }); });
    expect(mark).not.toHaveBeenCalled();
    expect(store.getState().notifications).toEqual([unread, read]);
    expect((await api.listNotificationsPage()).items).toEqual([unread, read]);
    expect(store.getState().notificationStatus).toEqual({ unread_count: 1 });
    expect(store.getState().notificationPanelState).toEqual({ status: 'ready', error: null });
    hook.unmount();
  });

  test.each(['default', 'batch'])('%s inbox read marks only unread rows and repeat load has no extra mutation', async mode => {
    const { api, hook, store } = mountInbox();
    const mark = vi.spyOn(api, 'markAllNotificationsRead');
    if (mode === 'batch') {
      act(() => { store.getState().patchState({ workspaceState: openTransientColumn(store.getState().workspaceState, {
        id: columnIdentityId('notifications'), kind: 'notifications', pinned: false,
      }) }); });
    }
    const load = () => mode === 'batch'
      ? hook.result.current.loadShellSections('kukuri:topic:general')
      : hook.result.current.loadNotificationsSection();
    await act(async () => { await load(); });
    expect(mark).toHaveBeenCalledTimes(1);
    expect(store.getState().notifications).toEqual([{ ...unread, read_at: 100_000 }, read]);
    expect(store.getState().notificationAutoReadError).toBeNull();
    expect(store.getState().notificationStatus).toEqual({ unread_count: 0 });
    await act(async () => { await load(); });
    expect(mark).toHaveBeenCalledTimes(1);
    expect(store.getState().notifications).toEqual([{ ...unread, read_at: 100_000 }, read]);
    hook.unmount();
  });

  test.each(['status', 'list'])('inbox %s failure keeps both previous results and forbids marking read', async failure => {
    const { api, hook, store } = mountInbox();
    vi.spyOn(api, failure === 'status' ? 'getNotificationStatus' : 'listNotificationsPage')
      .mockRejectedValue(new Error('offline'));
    const mark = vi.spyOn(api, 'markAllNotificationsRead');
    await act(async () => { await hook.result.current.loadNotificationsSection(); });
    expect(mark).not.toHaveBeenCalled();
    expect(store.getState().notificationStatus).toEqual({ unread_count: 9 });
    expect(store.getState().notifications).toEqual([read]);
    expect(store.getState().notificationPanelState).toEqual({ status: 'error', error: 'offline' });
    hook.unmount();
  });

  test('read failure keeps unread data with ready panel and separate auto-read error', async () => {
    const { api, hook, store } = mountInbox();
    const mark = vi.spyOn(api, 'markAllNotificationsRead').mockRejectedValue(new Error('read failed'));
    await act(async () => { await hook.result.current.loadNotificationsSection(); });
    expect(mark).toHaveBeenCalledTimes(1);
    expect(store.getState().notifications).toEqual([unread, read]);
    expect((await api.listNotificationsPage()).items).toEqual([unread, read]);
    expect(store.getState().notificationStatus).toEqual({ unread_count: 1 });
    expect(store.getState().notificationPanelState).toEqual({ status: 'ready', error: null });
    expect(store.getState().notificationAutoReadError).toBe('read failed');
    hook.unmount();
  });
});
