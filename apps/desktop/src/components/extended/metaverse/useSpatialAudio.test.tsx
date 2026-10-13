import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import type { GameRoomView, MetaverseAudioStatus, SetMetaverseAudioRequest } from '@/lib/api';
import type { MetaverseRoomActions } from './MetaverseRoomActions';
import { useSpatialAudio } from './useSpatialAudio';

function dome(generation: number): GameRoomView {
  return {
    room_id: 'dome-a',
    host_pubkey: 'owner',
    metaverse: {
      instance_id: 'dome-a',
      spatial_context: { kind: 'topic', topic_id: 'kukuri:topic:demo' },
      instance_generation: generation,
      session_id: 'dome-a',
    },
  } as unknown as GameRoomView;
}

describe('useSpatialAudio', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  test('applies the requests one by one, so a late request for the previous Dome cannot replace the next', async () => {
    const pending: (() => void)[] = [];
    const setAudio = vi.fn<(request: SetMetaverseAudioRequest) => Promise<MetaverseAudioStatus>>(() =>
      new Promise((resolve) => {
        pending.push(() => resolve({ microphone: 'off', listening: 0, rejected_frames: 0 }));
      }));
    const actions = { setAudio } as unknown as MetaverseRoomActions;
    const listenerTransformRef = { current: null };
    const mutedAuthorPubkeys = new Set<string>();
    const { rerender } = renderHook(
      (selectedRoom: GameRoomView | null) => useSpatialAudio({
        actions,
        selectedRoom,
        listenerTransformRef,
        mutedAuthorPubkeys,
        transitionNeighbors: [],
        peerPresence: {},
        remoteTransforms: {},
      }),
      { initialProps: dome(1) as GameRoomView | null }
    );
    const rooms = () => setAudio.mock.calls.map(([request]) => request.room?.instance_generation ?? null);
    const answer = async () => act(async () => pending.shift()?.());
    await waitFor(() => expect(rooms()).toEqual([1]));

    // 世代 1 への要求が終わる前に世代 2 へ移っても、停止と世代 2 は世代 1 の後に送る。
    rerender(dome(2));
    await answer();
    await waitFor(() => expect(rooms()).toEqual([1, null]));
    await answer();
    await waitFor(() => expect(rooms()).toEqual([1, null, 2]));

    // 退出(offline grace を含め、画面が room を外す)で止める。
    rerender(null);
    await answer();
    await waitFor(() => expect(rooms()).toEqual([1, null, 2, null]));
    expect(setAudio).toHaveBeenLastCalledWith(expect.objectContaining({ microphone_enabled: false, speakers: [] }));
  });

  test('re-sends the same request every 5 seconds and keeps running when the actions are rebuilt', async () => {
    vi.useFakeTimers();
    const answer = async (): Promise<MetaverseAudioStatus> => ({ microphone: 'off', listening: 0, rejected_frames: 0 });
    const first = vi.fn<(request: SetMetaverseAudioRequest) => Promise<MetaverseAudioStatus>>(answer);
    const second = vi.fn<(request: SetMetaverseAudioRequest) => Promise<MetaverseAudioStatus>>(answer);
    const listenerTransformRef = { current: null };
    const mutedAuthorPubkeys = new Set<string>();
    const { rerender } = renderHook(
      (actions: MetaverseRoomActions) => useSpatialAudio({
        actions,
        selectedRoom: dome(1),
        listenerTransformRef,
        mutedAuthorPubkeys,
        transitionNeighbors: [],
        peerPresence: {},
        remoteTransforms: {},
      }),
      { initialProps: { setAudio: first } as unknown as MetaverseRoomActions }
    );
    await act(async () => vi.advanceTimersByTimeAsync(4_900));
    expect(first).toHaveBeenCalledTimes(1);
    // 同じ内容でも 5 秒ごとに送り直し、終わった購読を Rust が試し直す。
    await act(async () => vi.advanceTimersByTimeAsync(200));
    expect(first).toHaveBeenCalledTimes(2);
    expect(first.mock.calls[1][0]).toEqual(first.mock.calls[0][0]);

    // 画面の他の操作で actions を作り直しても、止めずに新しい呼出し先へ送る。
    rerender({ setAudio: second } as unknown as MetaverseRoomActions);
    await act(async () => vi.advanceTimersByTimeAsync(5_000));
    expect(second).toHaveBeenCalledTimes(1);
    expect([...first.mock.calls, ...second.mock.calls].every(([request]) => request.room !== null)).toBe(true);
  });
});
