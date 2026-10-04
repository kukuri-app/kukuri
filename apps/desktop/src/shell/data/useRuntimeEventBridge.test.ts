import { renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import type { RuntimeEvent } from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';

const listenMock = vi.fn();

vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

import { useRuntimeEventBridge } from './useRuntimeEventBridge';

type EventCallback = (event: { payload: RuntimeEvent }) => void;

describe('useRuntimeEventBridge', () => {
  beforeEach(() => {
    listenMock.mockReset();
    listenMock.mockResolvedValue(() => undefined);
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  test('does not subscribe outside the Tauri runtime', () => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    renderHook(() => useRuntimeEventBridge(vi.fn(), vi.fn(), vi.fn(), vi.fn()));
    expect(listenMock).not.toHaveBeenCalled();
  });

  test('dispatches notification and partial sync status events without API calls', async () => {
    let capturedCallback: EventCallback | undefined;
    listenMock.mockImplementation(async (_event: string, callback: EventCallback) => {
      capturedCallback = callback;
      return () => undefined;
    });
    const onNotificationStatusChanged = vi.fn();
    const onSyncStatusChanged = vi.fn();
    const onAdultMediaLabelEvicted = vi.fn();
    const onAuthorRelationshipChanged = vi.fn();
    const api = createDesktopMockApi();
    const syncStatus = await api.getSyncStatus();
    const communityNodeStatuses = await api.getCommunityNodeStatuses();

    renderHook(() =>
      useRuntimeEventBridge(
        onNotificationStatusChanged,
        onSyncStatusChanged,
        onAdultMediaLabelEvicted,
        onAuthorRelationshipChanged
      )
    );
    await vi.waitFor(() => expect(capturedCallback).toBeDefined());

    capturedCallback?.({ payload: { type: 'notification_status_changed' } });
    expect(onNotificationStatusChanged).toHaveBeenCalledTimes(1);

    const statusDelta = {
      type: 'sync_status_changed' as const,
      sync_status: syncStatus,
      removed_topics: ['kukuri:topic:gone'],
      community_node_statuses: [],
      removed_community_nodes: [],
    };
    const nodeDelta = {
      type: 'sync_status_changed' as const,
      sync_status: null,
      removed_topics: [],
      community_node_statuses: communityNodeStatuses,
      removed_community_nodes: ['https://gone.example'],
    };
    capturedCallback?.({ payload: statusDelta });
    capturedCallback?.({ payload: nodeDelta });
    expect(onSyncStatusChanged).toHaveBeenNthCalledWith(1, statusDelta);
    expect(onSyncStatusChanged).toHaveBeenNthCalledWith(2, nodeDelta);
    capturedCallback?.({ payload: { type: 'adult_media_label_evicted', hash: 'hash-1' } });
    capturedCallback?.({ payload: { type: 'adult_media_label_evicted', hash: null } });
    expect(onAdultMediaLabelEvicted).toHaveBeenNthCalledWith(1, 'hash-1');
    expect(onAdultMediaLabelEvicted).toHaveBeenNthCalledWith(2, null);
    capturedCallback?.({ payload: { type: 'author_relationship_changed', pubkey: 'a'.repeat(64) } });
    capturedCallback?.({ payload: { type: 'author_relationship_changed', pubkey: null } });
    expect(onAuthorRelationshipChanged).toHaveBeenNthCalledWith(1, 'a'.repeat(64));
    expect(onAuthorRelationshipChanged).toHaveBeenNthCalledWith(2, null);
  });

  test('ignores unknown runtime event types', async () => {
    let capturedCallback: EventCallback | undefined;
    listenMock.mockImplementation(async (_event: string, callback: EventCallback) => {
      capturedCallback = callback;
      return () => undefined;
    });
    const onNotificationStatusChanged = vi.fn();
    const onSyncStatusChanged = vi.fn();
    const onAdultMediaLabelEvicted = vi.fn();
    const onAuthorRelationshipChanged = vi.fn();

    renderHook(() =>
      useRuntimeEventBridge(
        onNotificationStatusChanged,
        onSyncStatusChanged,
        onAdultMediaLabelEvicted,
        onAuthorRelationshipChanged
      )
    );
    await vi.waitFor(() => expect(capturedCallback).toBeDefined());

    capturedCallback?.({ payload: { type: 'future_event' } as unknown as RuntimeEvent });
    expect(onNotificationStatusChanged).not.toHaveBeenCalled();
    expect(onSyncStatusChanged).not.toHaveBeenCalled();
    expect(onAdultMediaLabelEvicted).not.toHaveBeenCalled();
    expect(onAuthorRelationshipChanged).not.toHaveBeenCalled();
  });
});
