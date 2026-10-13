import { renderHook, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';
import type { MetaverseRoomActions } from './MetaverseRoomActions';
import { connectionFixtureRooms as rooms, connectionFixtureTopology } from './DomeConnectionFixtures';
import { useDomeTransitionNeighbors } from './useDomeTransitionNeighbors';
import type { DomeConnectionsState } from './useDomeConnections';

describe('connection passage information', () => {
  test('does not start an access preview after leaving during a hosting read', async () => {
    let release!: (value: unknown) => void;
    const actions = { getHosting: vi.fn().mockImplementation(() => new Promise(resolve => { release = resolve; })),
      previewTransitionAccess: vi.fn().mockResolvedValue({ status: 'allowed' }), getBlobPreviewUrl: vi.fn() } as unknown as MetaverseRoomActions;
    const connections: DomeConnectionsState = { scope: 'a', topology: connectionFixtureTopology(), status: 'ready', error: null, refresh: async () => undefined };
    const { rerender } = renderHook(({ selected }) => useDomeTransitionNeighbors(actions, selected, rooms, 'a', connections), { initialProps: { selected: rooms[0] as typeof rooms[0] | null } });
    await waitFor(() => expect(actions.getHosting).toHaveBeenCalled());
    rerender({ selected: null });
    release({ state: { kind: 'closed' }, participants: 0 });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(actions.previewTransitionAccess).not.toHaveBeenCalled();
    expect(actions.getBlobPreviewUrl).not.toHaveBeenCalled();
  });
  test.each(['denied', 'error'])('keeps %s access distinct and does not fetch denied assets', async decision => {
    const actions = { getHosting: vi.fn().mockResolvedValue({ state: { kind: 'owner_hosted', session_id: 'session' }, participants: 0 }),
      previewTransitionAccess: decision === 'error' ? vi.fn().mockRejectedValue(new Error('unavailable')) : vi.fn().mockResolvedValue({ status: 'denied' }),
      getBlobPreviewUrl: vi.fn(), listConnections: vi.fn() } as unknown as MetaverseRoomActions;
    const connections: DomeConnectionsState = { scope: 'a', topology: connectionFixtureTopology(), status: 'ready', error: null, refresh: async () => undefined };
    const { result } = renderHook(() => useDomeTransitionNeighbors(actions, rooms[0], rooms, 'a', connections));
    await waitFor(() => expect(result.current[0][0]?.boundaryState).toBe(decision === 'error' ? 'error' : 'closed'));
    expect(actions.listConnections).not.toHaveBeenCalled(); expect(actions.getBlobPreviewUrl).not.toHaveBeenCalled();
    expect(actions.getHosting).toHaveBeenCalledTimes(1);
  });
  test('keeps a ready neighbor ready while the connections read again', async () => {
    let release!: (value: unknown) => void;
    const hosted = { state: { kind: 'owner_hosted', session_id: 'session' }, participants: 0 };
    const actions = { getHosting: vi.fn().mockResolvedValueOnce(hosted).mockImplementation(() => new Promise(resolve => { release = resolve; })),
      previewTransitionAccess: vi.fn().mockResolvedValue({ status: 'allowed' }), getBlobPreviewUrl: vi.fn().mockResolvedValue('blob:asset') } as unknown as MetaverseRoomActions;
    const connections = (): DomeConnectionsState => ({ scope: 'a', topology: connectionFixtureTopology(), status: 'ready', error: null, refresh: async () => undefined });
    const { result, rerender } = renderHook(({ state }) => useDomeTransitionNeighbors(actions, rooms[0], rooms, 'a', state), { initialProps: { state: connections() } });
    await waitFor(() => expect(result.current[0][0]?.boundaryState).toBe('ready'));
    // 5 秒ごとの読み直しは同じ内容の新しい object を返す。確かめ直す間も、通れる隣を通れないものとして扱わない。
    rerender({ state: connections() });
    await waitFor(() => expect(actions.getHosting).toHaveBeenCalledTimes(2));
    expect(result.current[0][0].boundaryState).toBe('ready');
    release(hosted);
    await waitFor(() => expect(actions.previewTransitionAccess).toHaveBeenCalledTimes(2));
    expect(result.current[0][0].boundaryState).toBe('ready');
  });
  test('does not keep the previous side of a connection after moving through it', async () => {
    const actions = { getHosting: vi.fn().mockResolvedValue({ state: { kind: 'owner_hosted', session_id: 'session' }, participants: 0 }),
      previewTransitionAccess: vi.fn().mockResolvedValue({ status: 'allowed' }), getBlobPreviewUrl: vi.fn().mockResolvedValue('blob:asset') } as unknown as MetaverseRoomActions;
    const connections: DomeConnectionsState = { scope: 'a', topology: connectionFixtureTopology(), status: 'ready', error: null, refresh: async () => undefined };
    const { result, rerender } = renderHook(({ selected }) => useDomeTransitionNeighbors(actions, selected, rooms, 'a', connections), { initialProps: { selected: rooms[0] } });
    await waitFor(() => expect(result.current[0][0]?.boundaryState).toBe('ready'));
    rerender({ selected: rooms[1] });
    expect(result.current[0].find(neighbor => neighbor.connectionId === 'a-b')).toMatchObject({ direction: 'west', room: { room_id: 'a' } });
  });
  test('keeps draining and blocked reasons even when access is denied', async () => {
    const topology = connectionFixtureTopology(); topology.connections[0].record.status = 'draining';
    const actions = { getHosting: vi.fn().mockRejectedValue(new Error('offline')), previewTransitionAccess: vi.fn().mockResolvedValue({ status: 'denied' }), getBlobPreviewUrl: vi.fn() } as unknown as MetaverseRoomActions;
    const connections: DomeConnectionsState = { scope: 'a', topology, status: 'ready', error: null, refresh: async () => undefined };
    const { result } = renderHook(() => useDomeTransitionNeighbors(actions, rooms[0], rooms, 'a', connections));
    await waitFor(() => expect(actions.previewTransitionAccess).toHaveBeenCalled());
    expect(result.current[0][0].boundaryState).toBe('draining');
  });
});
