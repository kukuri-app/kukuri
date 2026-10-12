import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import type { GameRoomView, MetaverseRoomEventView } from '@/lib/api';
import type { AvatarTransform } from '../MetaverseSceneModel';
import { createDefaultMetaverseRoomState } from './DomeSceneModel';
import type { MetaverseRoomActions } from './MetaverseRoomActions';
import type { DomeNeighborTransitionView } from './DomeTransitionModel';
import { useMetaverseBackendEvents } from './useMetaverseBackendEvents';

const room: GameRoomView = {
  room_id: 'room-1',
  host_pubkey: 'f'.repeat(64),
  title: 'Atrium',
  description: '',
  status: 'Waiting',
  phase_label: 'fixed-dome-v1',
  scores: [],
  room_kind: 'metaverse_room',
  metaverse: createDefaultMetaverseRoomState(8),
  dome_hosting: { kind: 'owner_hosted' },
  manifest_blob_hash: 'manifest-1',
  updated_at: 1,
  channel_id: null,
  audience_label: 'Public',
};

const visibleRemoteTransforms: Record<string, AvatarTransform> = {
  'remote-peer': {
    roomId: room.room_id,
    peerId: 'remote-peer',
    seq: 1,
    position: [0, 0, 0],
    rotation: [0, 0, 0],
    animation: 'idle',
    sentAt: 1,
  },
};

function presenceJoin(seq: number, hash = 'avatar-a'): MetaverseRoomEventView {
  return {
    envelope_id: `event-${seq}`,
    content: {
      event_id: `event-${seq}`,
      topic_id: 'kukuri:topic:demo',
      channel_id: null,
      room_id: room.room_id,
      spatial_context: { kind: 'topic', topic_id: 'kukuri:topic:demo' },
      instance_generation: 1,
      session_id: 'session-1',
      peer_id: 'remote-peer',
      seq,
      sent_at: seq,
      event: {
        type: 'presence_join',
        presence: {
          room_id: room.room_id,
          peer_id: 'remote-peer',
          display_name: 'Remote',
          avatar_asset_ref: { kind: 'vrm', blob_hash: hash, mime_type: 'model/vrm' },
          joined_at: 1,
          last_seen_at: Date.now(),
          microphone_enabled: false,
        },
      },
    },
    envelope: {},
    received_at: seq,
    source_peer: 'remote-peer',
  };
}

function presenceLeave(seq: number): MetaverseRoomEventView {
  const view = presenceJoin(seq);
  return {
    ...view,
    content: {
      ...view.content,
      event: {
        type: 'presence_leave',
        room_id: room.room_id,
        peer_id: 'remote-peer',
        left_at: Date.now(),
      },
    },
  };
}

function actionsWith(
  listRoomEvents: MetaverseRoomActions['listRoomEvents'],
  getBlobPreviewUrl: MetaverseRoomActions['getBlobPreviewUrl'] = vi.fn().mockResolvedValue('blob:avatar-a')
) {
  return {
    listRoomEvents,
    getBlobPreviewUrl,
  } as unknown as MetaverseRoomActions;
}

function renderBackendEvents(
  actions: MetaverseRoomActions,
  initialProps = { avatarFetchActive: true, remoteTransforms: visibleRemoteTransforms }
) {
  const transitionNeighbors: DomeNeighborTransitionView[] = [];
  const setRemoteTransforms = vi.fn();
  return renderHook(
    ({ avatarFetchActive, remoteTransforms }) => useMetaverseBackendEvents({
      actions,
      selectedRoom: room,
      transitionNeighbors,
      localPeerId: 'local-peer',
      avatarFetchActive,
      remoteTransforms,
      setRemoteTransforms,
    }),
    { initialProps }
  );
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('useMetaverseBackendEvents', () => {
  test('reuses the resolved avatar URL across repeated presence heartbeats', async () => {
    vi.useFakeTimers();
    const getBlobPreviewUrl = vi.fn().mockResolvedValue('blob:avatar-a');
    const listRoomEvents = vi.fn()
      .mockResolvedValueOnce([presenceJoin(1)])
      .mockResolvedValueOnce([presenceJoin(2)])
      .mockResolvedValue([]);
    const view = renderBackendEvents(actionsWith(listRoomEvents, getBlobPreviewUrl));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
    });

    expect(getBlobPreviewUrl).toHaveBeenCalledTimes(1);
    expect(view.result.current.peerPresence['remote-peer']?.avatarAssetUrl).toBe('blob:avatar-a');
    view.unmount();
  });

  test('backs off after consecutive failures and restores the normal poll interval on success', async () => {
    const setTimeout = vi.spyOn(window, 'setTimeout');
    const listRoomEvents = vi.fn()
      .mockRejectedValueOnce(new Error('poll failed'))
      .mockRejectedValueOnce(new Error('poll failed'))
      .mockResolvedValue([]);
    const view = renderBackendEvents(actionsWith(listRoomEvents));

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    const runAfter = async (delay: number) => {
      const scheduled = setTimeout.mock.calls.find((call) => call[1] === delay);
      expect(scheduled).toBeDefined();
      await act(async () => {
        (scheduled![0] as () => void)();
        await Promise.resolve();
        await Promise.resolve();
      });
    };

    await runAfter(360);
    await runAfter(720);
    expect(setTimeout.mock.calls.some((call) => call[1] === 180)).toBe(true);
    expect(listRoomEvents).toHaveBeenCalledTimes(3);
    view.unmount();
  });

  test('retries a missing visible avatar twice and pauses while the column is inactive', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(100_000);
    let sequence = 0;
    const listRoomEvents = vi.fn().mockImplementation(async () => [presenceJoin(++sequence)]);
    const getBlobPreviewUrl = vi.fn().mockResolvedValue(null);
    const view = renderBackendEvents(actionsWith(listRoomEvents, getBlobPreviewUrl));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(getBlobPreviewUrl).toHaveBeenCalledTimes(1);

    view.rerender({ avatarFetchActive: false, remoteTransforms: visibleRemoteTransforms });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6_000);
    });
    expect(getBlobPreviewUrl).toHaveBeenCalledTimes(1);

    view.rerender({ avatarFetchActive: true, remoteTransforms: visibleRemoteTransforms });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(getBlobPreviewUrl).toHaveBeenCalledTimes(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_001);
    });
    expect(getBlobPreviewUrl).toHaveBeenCalledTimes(3);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(getBlobPreviewUrl).toHaveBeenCalledTimes(3);
    view.unmount();
  });

  test('does not fetch an avatar until its peer is visible', async () => {
    vi.useFakeTimers();
    const listRoomEvents = vi.fn()
      .mockResolvedValueOnce([presenceJoin(1)])
      .mockResolvedValue([]);
    const getBlobPreviewUrl = vi.fn().mockResolvedValue('blob:avatar-a');
    const view = renderBackendEvents(
      actionsWith(listRoomEvents, getBlobPreviewUrl),
      { avatarFetchActive: true, remoteTransforms: {} }
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(getBlobPreviewUrl).not.toHaveBeenCalled();

    view.rerender({ avatarFetchActive: true, remoteTransforms: visibleRemoteTransforms });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(getBlobPreviewUrl).toHaveBeenCalledTimes(1);
    view.unmount();
  });

  test('ignores a late avatar result after the peer leaves', async () => {
    vi.useFakeTimers();
    let resolveAvatar!: (value: string | null) => void;
    const avatar = new Promise<string | null>((resolve) => { resolveAvatar = resolve; });
    const listRoomEvents = vi.fn()
      .mockResolvedValueOnce([presenceJoin(1)])
      .mockResolvedValueOnce([presenceLeave(2)])
      .mockResolvedValue([]);
    const view = renderBackendEvents(actionsWith(listRoomEvents, vi.fn().mockReturnValue(avatar)));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
    });
    await act(async () => resolveAvatar('blob:avatar-a'));

    expect(view.result.current.peerPresence['remote-peer']).toBeUndefined();
    view.unmount();
  });

  test('ignores a late result for an old avatar hash', async () => {
    vi.useFakeTimers();
    let resolveOldAvatar!: (value: string | null) => void;
    const oldAvatar = new Promise<string | null>((resolve) => { resolveOldAvatar = resolve; });
    const getBlobPreviewUrl = vi.fn()
      .mockReturnValueOnce(oldAvatar)
      .mockResolvedValueOnce('blob:avatar-b');
    const listRoomEvents = vi.fn()
      .mockResolvedValueOnce([presenceJoin(1, 'avatar-a')])
      .mockResolvedValueOnce([presenceJoin(2, 'avatar-b')])
      .mockResolvedValue([]);
    const view = renderBackendEvents(actionsWith(listRoomEvents, getBlobPreviewUrl));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(getBlobPreviewUrl).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(180);
    });
    await act(async () => resolveOldAvatar('blob:avatar-a'));

    expect(getBlobPreviewUrl).toHaveBeenCalledTimes(2);
    expect(view.result.current.peerPresence['remote-peer']).toMatchObject({
      avatarAssetRef: { blob_hash: 'avatar-b' },
      avatarAssetUrl: 'blob:avatar-b',
    });
    view.unmount();
  });
});
