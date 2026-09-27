import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import type {
  DesktopApi,
  DomeHostingView,
  DomePhysicsSnapshotV1,
  GameRoomView,
  SpatialContextV1,
  SyncStatus,
} from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { METAVERSE_ROOM_RECOVERY_MS, type MetaverseRoomEvent } from '../MetaverseSceneModel';
import { useMetaverseRoomSession } from './useMetaverseRoomSession';
import { createMetaverseRoomActions } from '@/shell/actions/metaverse';
import { createDefaultMetaverseRoomState } from './DomeSceneModel';
import { readLastVisitedDome } from './DomeEntryModel';

const room: GameRoomView = {
  room_id: 'metaverse-room-1',
  host_pubkey: 'f'.repeat(64),
  title: 'Atrium',
  description: 'Small social space',
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

function syncStatus(connected = true): SyncStatus {
  return {
    connected,
    delivery_state: connected ? 'Live' : 'Offline',
    peer_count: connected ? 1 : 0,
    pending_events: 0,
    status_detail: connected ? 'connected' : 'offline',
    configured_peer_count: 0,
    subscribed_topics: ['kukuri:topic:demo'],
    active_path: 'direct_p2p',
    fallback_peer_count: 0,
    topic_diagnostics: [],
    local_author_pubkey: 'f'.repeat(64),
    discovery: {
      mode: 'seeded_dht',
      connect_mode: 'direct_only',
      active_path: 'direct_p2p',
      env_locked: false,
      configured_seed_peer_count: 0,
      bootstrap_seed_peer_count: 0,
      connected_peer_count: 0,
      docs_assist_peer_count: 0,
      blob_assist_peer_count: 0,
      local_endpoint_id: 'local-endpoint-a',
    },
    gossip_disabled_topics: [],
    gossip_disabled_channels: [],
  };
}

function physicsSnapshot(sequence: number, x: number): DomePhysicsSnapshotV1 {
  return {
    instance_id: room.metaverse!.instance_id,
    instance_generation: room.metaverse!.instance_generation,
    lease_epoch: 1,
    session_id: 'session-1',
    host_pubkey: 'e'.repeat(64),
    sequence,
    simulated_at: sequence,
    sleeping: false,
    bodies: [
      {
        entity_id: `avatar:${'f'.repeat(64)}`,
        kind: 'avatar',
        position: [0, 0, 0],
        rotation: [0, 0, 0],
        linear_velocity: [0, 0, 0],
        animation: 'idle',
        grabbed_by: null,
        expires_at: null,
      },
      {
        entity_id: 'remote-peer',
        kind: 'avatar',
        position: [x, 0, 0],
        rotation: [0, 0, 0],
        linear_velocity: [0, 0, 0],
        animation: 'idle',
        grabbed_by: null,
        expires_at: null,
      },
    ],
  };
}

class MockBroadcastChannel {
  static instances: MockBroadcastChannel[] = [];

  onmessage: ((event: MessageEvent<MetaverseRoomEvent>) => void) | null = null;
  readonly postMessage = vi.fn();
  readonly close = vi.fn();

  constructor(readonly name: string) {
    MockBroadcastChannel.instances.push(this);
  }
}

type SessionProps = {
  rooms: GameRoomView[];
  sync: SyncStatus;
};

function renderSession({
  api = createDesktopMockApi(),
  rooms = [room],
  sync = syncStatus(),
  onRefresh = vi.fn().mockResolvedValue(undefined),
  initialSelectedRoomId,
}: {
  api?: DesktopApi;
  rooms?: GameRoomView[];
  sync?: SyncStatus;
  onRefresh?: () => Promise<void>;
  initialSelectedRoomId?: string | null;
} = {}) {
  const onError = vi.fn();
  const actions = createMetaverseRoomActions({
    api,
    activeTopic: 'kukuri:topic:demo',
    activeComposeChannel: { kind: 'public' },
    onRefresh,
  });
  const rendered = renderHook(
    ({ rooms: currentRooms, sync: currentSync }: SessionProps) =>
      useMetaverseRoomSession({
        actions,
        activeTopic: 'kukuri:topic:demo',
        rooms: currentRooms,
        syncStatus: currentSync,
        locale: 'en',
        localDisplayName: 'Local Author',
        localAvatarAssetRef: null,
        localAvatarAssetUrl: null,
        initialSelectedRoomId,
        onError,
      }),
    { initialProps: { rooms, sync } }
  );
  return { ...rendered, api, onRefresh, onError };
}

beforeEach(() => {
  MockBroadcastChannel.instances = [];
  vi.stubGlobal(
    'BroadcastChannel',
    MockBroadcastChannel as unknown as typeof BroadcastChannel
  );
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('Dome transition boundaries (#923)', () => {
  async function setupTransition() {
    vi.useFakeTimers();
    const rooms = ['source', 'target'].map((id, index) => ({
      ...room, room_id: id, host_pubkey: (index ? 'e' : 'f').repeat(64),
      metaverse: createDefaultMetaverseRoomState(8, {
        roomId: id, topicId: 'kukuri:topic:demo', ownerPubkey: (index ? 'e' : 'f').repeat(64),
      }),
    }));
    const context = rooms[0].metaverse.spatial_context;
    const api = createDesktopMockApi();
    const topology = await api.listDomeConnectionTopology(context);
    topology.connections = [{ record: {
      agreement: { connection_id: 'boundary', proposal_id: 'proposal', spatial_context: context,
        proposer: { instance_id: 'source', instance_generation: 1, owner_pubkey: rooms[0].host_pubkey, direction: 'north' },
        receiver: { instance_id: 'target', instance_generation: 1, owner_pubkey: rooms[1].host_pubkey, direction: 'south' },
        activation_generation: 1 },
      receiver_slot_generation: 1, observed_active_connection_ids: [], status: 'active',
      lifecycle_generation: 1, lifecycle_actor: null, lifecycle_reason: null, lifecycle_deadline_at: null,
    } }];
    topology.resolution.topology = { spatial_context: context,
      components: [{ root_instance_id: 'source', instance_ids: ['source', 'target'], connection_ids: ['boundary'],
        coordinates_cm: { source: [0, 0, 0], target: [0, 0, -5700] } }],
      active_connection_ids: ['boundary'], topology_digest: 'boundary-digest' };
    vi.spyOn(api, 'listDomeConnectionTopology').mockResolvedValue(topology);
    const hosting = vi.spyOn(api, 'getDomeHosting').mockImplementation(async (_context, instanceId) => ({
      instance_id: instanceId, state: { kind: 'community_node_hosted', lease_epoch: 1,
        session_id: `session-${instanceId}`, lease_expires_at: Date.now() + 60_000,
        host: { kind: 'community_node', node_id: 'node', api_base_url: 'https://node.example' } },
      lease: null, signed_lease_json: null, signed_activation_json: null, signed_close_json: null,
      instance_manifest_json: '{}', preset_manifest_json: '{}', participants: 1, sleeping: false,
      resource_budget: {} as DomeHostingView['resource_budget'], resource_metrics: {} as DomeHostingView['resource_metrics'],
    }));
    const snapshot = (instance: string, sequence: number): DomePhysicsSnapshotV1 => ({
      ...physicsSnapshot(sequence, 0), instance_id: instance, session_id: `session-${instance}`,
    });
    const submit = vi.spyOn(api, 'submitDomeSessionInput').mockImplementation(async (_c, id, sequence) => snapshot(id, sequence));
    const prepare = vi.spyOn(api, 'prepareDomeTransition').mockImplementation(async request => ({
      request, target_lease_epoch: 1, target_session_id: 'session-target', expires_at: Date.now() + 15_000,
    }));
    const commit = vi.spyOn(api, 'commitDomeTransition').mockResolvedValue(undefined);
    const abort = vi.spyOn(api, 'abortDomeTransition').mockResolvedValue(undefined);
    const publish = vi.spyOn(api, 'publishMetaverseRoomEvent');
    const session = renderSession({ api, rooms, initialSelectedRoomId: 'source' });
    const flush = async (ms = 0) => { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); };
    await flush();
    expect(session.result.current.admittedRoom?.room_id).toBe('source');
    expect(session.result.current.transitionNeighbors[0]?.boundaryState).toBe('ready');
    const move = (z: number) => act(() => session.result.current.handleLocalTransform({
      roomId: 'source', peerId: 'local-peer', seq: 1, position: [0, 90, z],
      rotation: [0, 0, 0], animation: 'walk', sentAt: Date.now(),
    }));
    const entered = async () => { move(-2200); await flush(); expect(prepare).toHaveBeenCalledTimes(1); };
    const lastVisited = () => readLastVisitedDome('f'.repeat(64), context);
    const sourceInputs = (type: string) => submit.mock.calls.filter(call => call[1] === 'source' && call[3].type === type);
    return { session, submit, prepare, commit, abort, hosting, publish, snapshot, flush, move, entered, lastVisited, sourceInputs };
  }

  test('cancelling prepare aborts a late ticket without committing or handing off', async () => {
    const f = await setupTransition();
    let deliver!: () => void;
    f.prepare.mockImplementationOnce(request => new Promise(resolve => { deliver = () => resolve({
      request, target_lease_epoch: 1, target_session_id: 'session-target', expires_at: Date.now() + 15_000,
    }); }));
    await f.entered();
    f.move(0);
    await f.flush();
    expect(f.sourceInputs('abort_transition')).toHaveLength(1);
    expect(f.abort).not.toHaveBeenCalled();
    await act(async () => deliver());
    expect(f.abort).toHaveBeenCalledTimes(1);
    expect(f.abort).toHaveBeenCalledWith(await f.prepare.mock.results[0].value);
    expect(f.sourceInputs('abort_transition')).toHaveLength(2);
    expect(f.sourceInputs('abort_transition').every(call => call[3].type === 'abort_transition'
      && call[3].transition_id === f.prepare.mock.calls[0][0].transition_id)).toBe(true);
    expect(f.commit).not.toHaveBeenCalled();
    expect(f.sourceInputs('complete_transition')).toHaveLength(0);
    expect(f.session.result.current.selectedRoomId).toBe('source');
    expect(f.lastVisited()).toBe('source');
    f.session.unmount();
  });

  test('lost ack and hosting lookup failure keep the same commit pending until confirmation', async () => {
    const f = await setupTransition();
    let confirm!: () => void;
    f.commit.mockRejectedValueOnce(new Error('ack lost')).mockImplementationOnce(() => new Promise(resolve => { confirm = resolve; }));
    await f.entered();
    f.hosting.mockRejectedValueOnce(new Error('lookup unavailable'));
    f.move(-2870);
    await f.flush();
    expect(f.commit).toHaveBeenCalledTimes(2);
    expect(f.commit.mock.calls[1]).toEqual(f.commit.mock.calls[0]);
    f.move(0);
    await f.flush();
    expect(f.abort).not.toHaveBeenCalled();
    expect(f.sourceInputs('abort_transition')).toHaveLength(0);
    expect(f.sourceInputs('complete_transition')).toHaveLength(0);
    expect(f.session.result.current.selectedRoomId).toBe('source');
    expect(f.lastVisited()).toBe('source');
    await act(async () => confirm());
    expect(f.session.result.current.selectedRoomId).toBe('target');
    expect(f.lastVisited()).toBe('target');
    expect(f.sourceInputs('complete_transition')).toHaveLength(1);
    f.session.unmount();
  });

  test.each(['definitive rejection', 'replaced session'])('%s rolls back without a successful handoff', async failure => {
    const f = await setupTransition();
    await f.entered();
    f.commit.mockRejectedValue(new Error(failure === 'definitive rejection' ? 'DOME_TRANSITION_INVALID_TICKET' : 'ack lost'));
    if (failure === 'replaced session') {
      const current = await f.hosting.mock.results.find((_result, index) => f.hosting.mock.calls[index][1] === 'target')!.value;
      f.hosting.mockResolvedValueOnce({ ...current, state: { ...current.state, lease_epoch: 2, session_id: 'replacement' } });
    }
    f.move(-2870);
    await f.flush();
    expect(f.commit).toHaveBeenCalledTimes(1);
    expect(f.abort).toHaveBeenCalledTimes(1);
    expect(f.abort).toHaveBeenCalledWith(await f.prepare.mock.results[0].value);
    expect(f.sourceInputs('abort_transition')).toHaveLength(1);
    expect(f.sourceInputs('abort_transition')[0][3]).toEqual({ type: 'abort_transition', transition_id: f.prepare.mock.calls[0][0].transition_id });
    expect(f.sourceInputs('complete_transition')).toHaveLength(0);
    expect(f.session.result.current.selectedRoomId).toBe('source');
    expect(f.lastVisited()).toBe('source');
    expect(f.session.onError).toHaveBeenCalledWith(expect.any(String));
    f.session.unmount();
  });

  test.each([
    ['join', false], ['leave', false], ['join', true], ['leave', true],
  ] as const)('%s keeps its own effects and respects the attempt guard (committing: %s)', async (action, committing) => {
    const f = await setupTransition();
    await f.entered();
    let confirm!: () => void;
    if (committing) {
      f.commit.mockImplementationOnce(() => new Promise(resolve => { confirm = resolve; }));
      f.move(-2870);
      await f.flush();
      expect(f.commit).toHaveBeenCalledTimes(1);
    }
    if (action === 'join') {
      await act(async () => { expect(await f.session.result.current.joinRoom('target')).toBe(true); });
    } else {
      act(() => f.session.result.current.leaveRoom());
    }
    await f.flush();
    expect(f.abort).toHaveBeenCalledTimes(committing ? 0 : 1);
    const aborted = f.sourceInputs('abort_transition');
    expect(aborted).toHaveLength(committing ? 0 : 1);
    if (!committing) {
      expect(f.abort).toHaveBeenCalledWith(await f.prepare.mock.results[0].value);
      expect(aborted[0][3]).toEqual({ type: 'abort_transition', transition_id: f.prepare.mock.calls[0][0].transition_id });
      expect(f.commit).not.toHaveBeenCalled();
    }
    expect(f.sourceInputs('complete_transition')).toHaveLength(0);
    expect(f.session.result.current.selectedRoomId).toBe(action === 'join' ? 'target' : null);
    expect(f.session.result.current.admissionStatus).toBe(action === 'join' ? 'joined' : 'selection');
    expect(f.lastVisited()).toBe(action === 'join' ? 'target' : 'source');
    if (action === 'join') {
      expect(f.submit.mock.calls.some(call => call[1] === 'target' && call[3].type === 'join')).toBe(true);
    } else {
      expect(f.sourceInputs('leave')).toHaveLength(1);
      expect(f.publish.mock.calls.filter(call => call[1] === 'source' && call[4].type === 'presence_leave')).toHaveLength(1);
      expect(f.session.result.current.admittedRoom).toBeNull();
    }
    if (committing) {
      await act(async () => confirm());
      expect(f.abort).not.toHaveBeenCalled();
      expect(f.sourceInputs('abort_transition')).toHaveLength(0);
    }
    f.session.unmount();
  });

  test.each([false, true])('source cleanup retries without rolling back target (all attempts fail: %s)', async allFail => {
    const f = await setupTransition();
    let attempts = 0;
    f.submit.mockImplementation(async (_context, id, sequence, input) => {
      if (id === 'source' && input.type === 'complete_transition') {
        attempts += 1;
        if (allFail || attempts === 1) throw new Error('source unavailable');
      }
      return f.snapshot(id, sequence);
    });
    await f.entered();
    f.move(-2870);
    await f.flush();
    expect(attempts).toBe(1);
    expect(f.session.result.current.selectedRoomId).toBe('target');
    expect(f.lastVisited()).toBe('target');
    await f.flush(249);
    expect(attempts).toBe(1);
    await f.flush(1);
    expect(attempts).toBe(2);
    await f.flush(1000);
    expect(attempts).toBe(allFail ? 3 : 2);
    expect(f.session.result.current.selectedRoomId).toBe('target');
    expect(f.lastVisited()).toBe('target');
    expect(f.abort).not.toHaveBeenCalled();
    expect(f.sourceInputs('abort_transition')).toHaveLength(0);
    const complete = f.sourceInputs('complete_transition');
    expect(complete.every(call => call[3].type === 'complete_transition'
      && call[3].transition_id === f.prepare.mock.calls[0][0].transition_id)).toBe(true);
    expect(new Set(complete.map(call => call[3].type === 'complete_transition' ? call[3].transition_id : null)).size).toBe(1);
    expect(new Set(complete.map(call => call[2])).size).toBe(complete.length);
    expect(f.publish.mock.calls.filter(call => call[1] === 'source' && call[4].type === 'presence_leave')).toHaveLength(allFail ? 0 : 1);
    if (allFail) expect(f.session.onError).toHaveBeenCalledWith('Destination committed; source Dome cleanup will require resynchronization');
    f.session.unmount();
  });
});

describe('useMetaverseRoomSession', () => {
  test('does not expose the Dome scene before authoritative admission confirms a safe spawn', async () => {
    let resolveAdmission!: (snapshot: DomePhysicsSnapshotV1) => void;
    const api: DesktopApi = {
      ...createDesktopMockApi(),
      submitDomeSessionInput: vi.fn(() => new Promise<DomePhysicsSnapshotV1>((resolve) => {
        resolveAdmission = resolve;
      })),
    };
    const session = renderSession({ api });

    await waitFor(() => expect(api.submitDomeSessionInput).toHaveBeenCalled());
    expect(session.result.current.admissionStatus).toBe('admitting');
    expect(session.result.current.admittedRoom).toBeNull();

    await act(async () => resolveAdmission(physicsSnapshot(1, 0)));
    await waitFor(() => expect(session.result.current.admittedRoom?.room_id).toBe(room.room_id));
  });

  test('admits the best available room on first render', async () => {
    const session = renderSession({ initialSelectedRoomId: room.room_id });

    await waitFor(() => expect(session.result.current.admittedRoom?.room_id).toBe(room.room_id));
  });

  test('Return Home keeps the source scene until the destination avatar is confirmed', async () => {
    const target = {
      ...room,
      room_id: 'metaverse-room-2',
      title: 'Safe Home',
      metaverse: createDefaultMetaverseRoomState(8, { roomId: 'metaverse-room-2' }),
    };
    let resolveTarget!: (snapshot: DomePhysicsSnapshotV1) => void;
    const submitDomeSessionInput = vi.fn((
      _context: SpatialContextV1,
      instanceId: string,
      sequence: number
    ) => {
      if (instanceId === target.metaverse!.instance_id) {
        return new Promise<DomePhysicsSnapshotV1>((resolve) => { resolveTarget = resolve; });
      }
      return Promise.resolve(physicsSnapshot(sequence, 0));
    });
    const api: DesktopApi = { ...createDesktopMockApi(), submitDomeSessionInput };
    const session = renderSession({ api, rooms: [room, target], initialSelectedRoomId: room.room_id });
    await waitFor(() => expect(session.result.current.admittedRoom?.room_id).toBe(room.room_id));

    act(() => session.result.current.returnHome());
    await waitFor(() => expect(session.result.current.domeRecovery.state).toBe('evacuating'));
    expect(session.result.current.admittedRoom?.room_id).toBe(room.room_id);

    await act(async () => resolveTarget({
      ...physicsSnapshot(20, 0),
      instance_id: target.metaverse!.instance_id,
    }));
    await waitFor(() => expect(session.result.current.admittedRoom?.room_id).toBe(target.room_id));
    expect(session.result.current.domeRecovery.state).toBe('online');
  });

  test('clears a missing selected room but preserves a pending created-room selection', async () => {
    const session = renderSession();
    await act(async () => {
      await session.result.current.joinRoom(room.room_id);
    });
    expect(session.result.current.admittedRoom?.room_id).toBe(room.room_id);

    session.rerender({ rooms: [], sync: syncStatus() });
    await waitFor(() => expect(session.result.current.selectedRoomId).toBeNull());

    act(() => session.result.current.selectCreatedRoom('created-room'));
    expect(session.result.current.selectedRoomId).toBe('created-room');
    session.rerender({ rooms: [], sync: syncStatus() });
    expect(session.result.current.selectedRoomId).toBe('created-room');

    const createdRoom = { ...room, room_id: 'created-room', title: 'Created Room' };
    session.rerender({ rooms: [createdRoom], sync: syncStatus() });
    expect(session.result.current.selectedRoom?.room_id).toBe('created-room');
    session.rerender({ rooms: [], sync: syncStatus() });
    await waitFor(() => expect(session.result.current.selectedRoomId).toBeNull());
  });

  test('closes the room BroadcastChannel on cleanup', async () => {
    const session = renderSession();
    await waitFor(() => expect(session.result.current.admittedRoom?.room_id).toBe(room.room_id));
    await waitFor(() => expect(MockBroadcastChannel.instances).toHaveLength(1));
    const channel = MockBroadcastChannel.instances[0];
    session.unmount();
    expect(channel.close).toHaveBeenCalledTimes(1);
  });

  test('does not let an out-of-order authoritative snapshot roll back the scene', async () => {
    const api: DesktopApi = {
      ...createDesktopMockApi(),
      submitDomeSessionInput: vi
        .fn()
        .mockResolvedValueOnce(physicsSnapshot(1, 1))
        .mockResolvedValueOnce(physicsSnapshot(3, 3))
        .mockResolvedValueOnce(physicsSnapshot(2, 2)),
    };
    const session = renderSession({ api });
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(session.result.current.admittedRoom?.room_id).toBe(room.room_id);
    await waitFor(() =>
      expect(session.result.current.remoteTransforms['remote-peer']?.position[0]).toBe(1)
    );

    act(() => {
      session.result.current.handleLocalTransform({
        roomId: room.room_id,
        peerId: 'local-peer',
        seq: 2,
        position: [0, 0, 0],
        rotation: [0, 0, 0],
        animation: 'idle',
        sentAt: 2,
      });
      session.result.current.handleLocalTransform({
        roomId: room.room_id,
        peerId: 'local-peer',
        seq: 3,
        position: [0, 0, 0],
        rotation: [0, 0, 0],
        animation: 'idle',
        sentAt: 3,
      });
    });

    await waitFor(() =>
      expect(session.result.current.remoteTransforms['remote-peer']?.position[0]).toBe(3)
    );
  });

  test('marks the room offline after three consecutive backend poll failures', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(100_000);
    const api: DesktopApi = {
      ...createDesktopMockApi(),
      listMetaverseRoomEvents: vi.fn().mockRejectedValue(new Error('poll failed')),
    };
    const session = renderSession({ api });
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(session.result.current.admittedRoom?.room_id).toBe(room.room_id);

    await act(async () => {
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(1_100);
    });

    expect(vi.mocked(api.listMetaverseRoomEvents).mock.calls.length).toBeGreaterThanOrEqual(3);
    expect(session.result.current.roomConnectionState).toBe('offline');
  });

  test('enforces recovery cooldown and clears heartbeat/poll timers on cleanup', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(100_000);
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    const clearInterval = vi.spyOn(window, 'clearInterval');
    const clearTimeout = vi.spyOn(window, 'clearTimeout');
    const session = renderSession({ onRefresh });
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(session.result.current.admittedRoom?.room_id).toBe(room.room_id);

    session.rerender({ rooms: [room], sync: syncStatus(false) });
    await act(async () => Promise.resolve());
    expect(onRefresh).toHaveBeenCalledTimes(1);

    session.rerender({ rooms: [room], sync: syncStatus(true) });
    session.rerender({ rooms: [room], sync: syncStatus(false) });
    await act(async () => Promise.resolve());
    expect(onRefresh).toHaveBeenCalledTimes(1);

    vi.setSystemTime(100_000 + METAVERSE_ROOM_RECOVERY_MS + 1);
    session.rerender({ rooms: [room], sync: syncStatus(true) });
    session.rerender({ rooms: [room], sync: syncStatus(false) });
    await act(async () => Promise.resolve());
    expect(onRefresh).toHaveBeenCalledTimes(2);

    session.unmount();
    expect(clearInterval).toHaveBeenCalled();
    expect(clearTimeout).toHaveBeenCalled();
  });

  test('reserves the destination and commits once when the avatar crosses the center line', async () => {
    const domeA = {
      ...room,
      room_id: 'dome-a',
      metaverse: createDefaultMetaverseRoomState(8, {
        roomId: 'dome-a',
        topicId: 'kukuri:topic:demo',
      }),
    };
    const domeB = {
      ...room,
      room_id: 'dome-b',
      title: 'Neighbor',
      metaverse: createDefaultMetaverseRoomState(8, {
        roomId: 'dome-b',
        topicId: 'kukuri:topic:demo',
      }),
    };
    const hosted = (instanceId: string): DomeHostingView => ({
      instance_id: instanceId,
      state: {
        kind: 'community_node_hosted',
        host: { kind: 'community_node', node_id: 'cn-1', api_base_url: 'https://cn.example' },
        lease_id: `lease-${instanceId}`,
        lease_epoch: 1,
        lease_expires_at: Date.now() + 60_000,
        session_id: `session-${instanceId}`,
        reason: null,
        last_heartbeat_at: Date.now(),
      },
      lease: null,
      signed_lease_json: null,
      signed_activation_json: null,
      signed_close_json: null,
      instance_manifest_json: '{}',
      preset_manifest_json: '{}',
      participants: 0,
      sleeping: false,
      resource_budget: {} as DomeHostingView['resource_budget'],
      resource_metrics: {} as DomeHostingView['resource_metrics'],
    });
    const prepareDomeTransition = vi.fn(async (request) => ({
      request,
      target_lease_epoch: 1,
      target_session_id: 'session-dome-b',
      expires_at: Date.now() + 15_000,
    }));
    const commitDomeTransition = vi.fn().mockResolvedValue(undefined);
    const abortDomeTransition = vi.fn().mockResolvedValue(undefined);
    const api: DesktopApi = {
      ...createDesktopMockApi(),
      listDomeConnectionTopology: vi.fn().mockResolvedValue({
        proposals: [],
        connections: [{
          record: {
            agreement: {
              connection_id: 'connection-1',
              proposal_id: 'proposal-1',
              spatial_context: domeA.metaverse.spatial_context,
              proposer: {
                instance_id: 'dome-a',
                instance_generation: 1,
                owner_pubkey: domeA.host_pubkey,
                direction: 'north',
              },
              receiver: {
                instance_id: 'dome-b',
                instance_generation: 1,
                owner_pubkey: domeB.host_pubkey,
                direction: 'south',
              },
              activation_generation: 1,
            },
            receiver_slot_generation: 1,
            observed_active_connection_ids: [],
            status: 'active',
            lifecycle_generation: 1,
            lifecycle_actor: null,
            lifecycle_reason: null,
            lifecycle_deadline_at: null,
          },
        }],
        resolution: {
          topology: {
            spatial_context: domeA.metaverse.spatial_context,
            components: [{
              root_instance_id: 'dome-a',
              instance_ids: ['dome-a', 'dome-b'],
              connection_ids: ['connection-1'],
              coordinates_cm: { 'dome-a': [0, 0, 0], 'dome-b': [0, 0, -5_700] },
            }],
            active_connection_ids: ['connection-1'],
            topology_digest: 'topology-1',
          },
          rejected_connections: [],
        },
      }),
      getDomeHosting: vi.fn(async (_context, instanceId) => hosted(instanceId)),
      submitDomeSessionInput: vi.fn(async (
        _context: SpatialContextV1,
        instanceId: string,
        sequence: number
      ): Promise<DomePhysicsSnapshotV1> => ({
        instance_id: instanceId,
        instance_generation: 1,
        lease_epoch: 1,
        session_id: `session-${instanceId}`,
        host_pubkey: 'e'.repeat(64),
        sequence,
        simulated_at: Date.now(),
        sleeping: false,
        bodies: [{
          entity_id: `avatar:${'f'.repeat(64)}`,
          kind: 'avatar',
          position: [0, 0, 260],
          rotation: [0, 180, 0],
          linear_velocity: [0, 0, 0],
          animation: 'idle',
          grabbed_by: null,
          expires_at: null,
        }],
      })),
      prepareDomeTransition,
      commitDomeTransition,
      abortDomeTransition,
    };
    const session = renderSession({
      api,
      rooms: [domeA, domeB],
      initialSelectedRoomId: 'dome-a',
    });
    await waitFor(() =>
      expect(session.result.current.transitionNeighbors[0]?.boundaryState).toBe('ready')
    );

    act(() => session.result.current.handleLocalTransform({
      roomId: 'dome-a',
      peerId: 'local-peer',
      seq: 1,
      position: [0, 90, -2_200],
      rotation: [0, 0, 0],
      animation: 'walk',
      sentAt: Date.now(),
    }));
    await waitFor(() => expect(prepareDomeTransition).toHaveBeenCalledTimes(1));
    act(() => session.result.current.handleLocalTransform({
      roomId: 'dome-a',
      peerId: 'local-peer',
      seq: 2,
      position: [0, 90, -2_870],
      rotation: [0, 0, 0],
      animation: 'walk',
      sentAt: Date.now(),
    }));

    await waitFor(() => expect(session.result.current.selectedRoomId).toBe('dome-b'));
    expect(commitDomeTransition).toHaveBeenCalledTimes(1);
    expect(commitDomeTransition).toHaveBeenCalledWith(
      expect.objectContaining({ target_session_id: 'session-dome-b' }),
      [0, 90, 2_830],
      [0, 0, 0]
    );
    expect(abortDomeTransition).not.toHaveBeenCalled();
  });

  test('retries the same destination commit when the first acknowledgement is lost', async () => {
    const domeA = {
      ...room,
      room_id: 'dome-a',
      metaverse: createDefaultMetaverseRoomState(8, {
        roomId: 'dome-a',
        topicId: 'kukuri:topic:demo',
      }),
    };
    const domeB = {
      ...room,
      room_id: 'dome-b',
      title: 'Neighbor',
      metaverse: createDefaultMetaverseRoomState(8, {
        roomId: 'dome-b',
        topicId: 'kukuri:topic:demo',
      }),
    };
    const hosted = (instanceId: string): DomeHostingView => ({
      instance_id: instanceId,
      state: {
        kind: 'community_node_hosted',
        host: { kind: 'community_node', node_id: 'cn-1', api_base_url: 'https://cn.example' },
        lease_id: `lease-${instanceId}`,
        lease_epoch: 1,
        lease_expires_at: Date.now() + 60_000,
        session_id: `session-${instanceId}`,
        reason: null,
        last_heartbeat_at: Date.now(),
      },
      lease: null,
      signed_lease_json: null,
      signed_activation_json: null,
      signed_close_json: null,
      instance_manifest_json: '{}',
      preset_manifest_json: '{}',
      participants: 0,
      sleeping: false,
      resource_budget: {} as DomeHostingView['resource_budget'],
      resource_metrics: {} as DomeHostingView['resource_metrics'],
    });
    const prepareDomeTransition = vi.fn(async (request) => ({
      request,
      target_lease_epoch: 1,
      target_session_id: 'session-dome-b',
      expires_at: Date.now() + 15_000,
    }));
    const commitDomeTransition = vi.fn()
      .mockRejectedValueOnce(new Error('response lost after commit'))
      .mockResolvedValue(undefined);
    const abortDomeTransition = vi.fn().mockResolvedValue(undefined);
    const submitDomeSessionInput = vi.fn(async (
      _context: SpatialContextV1,
      instanceId: string,
      sequence: number
    ): Promise<DomePhysicsSnapshotV1> => ({
      instance_id: instanceId,
      instance_generation: 1,
      lease_epoch: 1,
      session_id: `session-${instanceId}`,
      host_pubkey: 'e'.repeat(64),
      sequence,
      simulated_at: Date.now(),
      sleeping: false,
      bodies: [{
        entity_id: `avatar:${'f'.repeat(64)}`,
        kind: 'avatar',
        position: [0, 0, 260],
        rotation: [0, 180, 0],
        linear_velocity: [0, 0, 0],
        animation: 'idle',
        grabbed_by: null,
        expires_at: null,
      }],
    }));
    const api: DesktopApi = {
      ...createDesktopMockApi(),
      listDomeConnectionTopology: vi.fn().mockResolvedValue({
        proposals: [],
        connections: [{
          record: {
            agreement: {
              connection_id: 'connection-1',
              proposal_id: 'proposal-1',
              spatial_context: domeA.metaverse.spatial_context,
              proposer: {
                instance_id: 'dome-a',
                instance_generation: 1,
                owner_pubkey: domeA.host_pubkey,
                direction: 'north',
              },
              receiver: {
                instance_id: 'dome-b',
                instance_generation: 1,
                owner_pubkey: domeB.host_pubkey,
                direction: 'south',
              },
              activation_generation: 1,
            },
            receiver_slot_generation: 1,
            observed_active_connection_ids: [],
            status: 'active',
            lifecycle_generation: 1,
            lifecycle_actor: null,
            lifecycle_reason: null,
            lifecycle_deadline_at: null,
          },
        }],
        resolution: {
          topology: {
            spatial_context: domeA.metaverse.spatial_context,
            components: [{
              root_instance_id: 'dome-a',
              instance_ids: ['dome-a', 'dome-b'],
              connection_ids: ['connection-1'],
              coordinates_cm: { 'dome-a': [0, 0, 0], 'dome-b': [0, 0, -5_700] },
            }],
            active_connection_ids: ['connection-1'],
            topology_digest: 'topology-1',
          },
          rejected_connections: [],
        },
      }),
      getDomeHosting: vi.fn(async (_context, instanceId) => hosted(instanceId)),
      submitDomeSessionInput,
      prepareDomeTransition,
      commitDomeTransition,
      abortDomeTransition,
    };
    const session = renderSession({
      api,
      rooms: [domeA, domeB],
      initialSelectedRoomId: 'dome-a',
    });
    await waitFor(() =>
      expect(session.result.current.transitionNeighbors[0]?.boundaryState).toBe('ready')
    );

    act(() => session.result.current.handleLocalTransform({
      roomId: 'dome-a',
      peerId: 'local-peer',
      seq: 1,
      position: [0, 90, -2_200],
      rotation: [0, 0, 0],
      animation: 'walk',
      sentAt: Date.now(),
    }));
    await waitFor(() => expect(prepareDomeTransition).toHaveBeenCalledTimes(1));
    act(() => session.result.current.handleLocalTransform({
      roomId: 'dome-a',
      peerId: 'local-peer',
      seq: 2,
      position: [0, 90, -2_870],
      rotation: [0, 0, 0],
      animation: 'walk',
      sentAt: Date.now(),
    }));

    await waitFor(() => expect(commitDomeTransition).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(session.result.current.selectedRoomId).toBe('dome-b'));
    expect(commitDomeTransition.mock.calls[1]).toEqual(commitDomeTransition.mock.calls[0]);
    expect(abortDomeTransition).not.toHaveBeenCalled();
    expect(submitDomeSessionInput).not.toHaveBeenCalledWith(
      expect.anything(),
      'dome-a',
      expect.anything(),
      expect.objectContaining({ type: 'abort_transition' }),
      1
    );
    expect(submitDomeSessionInput).toHaveBeenCalledWith(
      expect.anything(),
      'dome-a',
      expect.anything(),
      expect.objectContaining({ type: 'complete_transition' }),
      1
    );
  });
});
