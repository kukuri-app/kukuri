import { describe, expect, it } from 'vitest';

import type { GameRoomView } from '@/lib/api';
import type { PeerPresence } from '../MetaverseSceneModel';
import type { DomeNeighborTransitionView } from './DomeTransitionModel';
import { domeOpeningPosition, spatialAudioRequest } from './SpatialAudioModel';

function dome(instanceId: string, owner: string): GameRoomView {
  return {
    room_id: instanceId,
    host_pubkey: owner,
    metaverse: {
      instance_id: instanceId,
      spatial_context: { kind: 'topic', topic_id: 'kukuri:topic:demo' },
      instance_generation: 3,
      session_id: instanceId,
    },
  } as unknown as GameRoomView;
}

function speaker(peer: string, pubkey: string, roomId: string, overrides: Partial<PeerPresence> = {}): PeerPresence {
  return {
    peerId: `${peer}:seed`,
    displayName: null,
    avatarAssetRef: null,
    joinedAt: 0,
    lastSeenAt: 1_000,
    authorPubkey: pubkey,
    roomId,
    microphoneEnabled: true,
    ...overrides,
  };
}

const current = dome('dome-a', 'owner-a');
const ready = {
  room: dome('dome-b', 'owner-b'),
  boundaryState: 'ready',
  direction: 'north',
  targetDirection: 'south',
} as unknown as DomeNeighborTransitionView;
const notReady = { ...ready, room: dome('dome-c', 'owner-c'), boundaryState: 'offline' } as DomeNeighborTransitionView;

describe('spatialAudioRequest', () => {
  it('passes the speakers that are sending audio, nearest first, without muted or stale ones', () => {
    const request = spatialAudioRequest({
      room: current,
      microphoneEnabled: true,
      listenerPosition: [0.4, 100, 0],
      neighbors: [ready, notReady],
      presences: {
        far: speaker('endpoint-far', 'far', 'dome-a'),
        near: speaker('endpoint-near', 'near', 'dome-a'),
        silent: speaker('endpoint-silent', 'silent', 'dome-a', { microphoneEnabled: false }),
        muted: speaker('endpoint-muted', 'muted', 'dome-a'),
        stale: speaker('endpoint-stale', 'stale', 'dome-a', { lastSeenAt: -20_000 }),
        neighbor: speaker('endpoint-neighbor', 'neighbor', 'dome-b'),
        offline: speaker('endpoint-offline', 'offline', 'dome-c'),
      },
      remoteTransforms: {
        'avatar:far': { roomId: 'dome-a', peerId: 'avatar:far', seq: 1, position: [0, 100, 1_000], rotation: [0, 0, 0], sentAt: 0 },
        'avatar:near': { roomId: 'dome-a', peerId: 'avatar:near', seq: 1, position: [0, 100, 100], rotation: [0, 0, 0], sentAt: 0 },
      } as never,
      mutedAuthorPubkeys: new Set(['muted']),
      now: 1_000,
    });
    expect(request.room).toEqual({
      spatial_context: { kind: 'topic', topic_id: 'kukuri:topic:demo' },
      owner_pubkey: 'owner-a',
      instance_id: 'dome-a',
      instance_generation: 3,
      session_id: 'dome-a',
    });
    expect(request.microphone_enabled).toBe(true);
    expect(request.listener_position).toEqual([0, 100, 0]);
    expect(request.speakers.map((entry) => entry.endpoint_id)).toEqual([
      'endpoint-near',
      'endpoint-far',
      'endpoint-neighbor',
    ]);
    // ready でない隣接 Dome は渡さない。ready な隣接は合成距離の opening を渡す。
    expect(request.neighbors).toEqual([{
      room: expect.objectContaining({ instance_id: 'dome-b', owner_pubkey: 'owner-b' }),
      speaker_opening: domeOpeningPosition('south'),
      listener_opening: domeOpeningPosition('north'),
    }]);
  });

  it('passes the same request when only unrelated peers grow tenfold', () => {
    const input = {
      room: current,
      microphoneEnabled: false,
      listenerPosition: [0, 100, 0] as [number, number, number],
      neighbors: [ready, notReady],
      remoteTransforms: {},
      mutedAuthorPubkeys: new Set(['muted']),
      now: 1_000,
    };
    // 音声を送っている 4 人に対し、無関係な peer(別の Dome・mic off・ready でない隣接・古い presence)を 10 倍にする。
    const presences = Object.fromEntries(['a', 'b', 'c'].map((name) => [name, speaker(`endpoint-${name}`, name, 'dome-a')])
      .concat([['neighbor', speaker('endpoint-neighbor', 'neighbor', 'dome-b')]]));
    const unrelated = Object.fromEntries(Array.from({ length: 10 }, (_, index) => [
      [`other-${index}`, speaker(`endpoint-other-${index}`, `other-${index}`, `dome-x-${index}`)],
      [`silent-${index}`, speaker(`endpoint-silent-${index}`, `silent-${index}`, 'dome-a', { microphoneEnabled: false })],
      [`offline-${index}`, speaker(`endpoint-offline-${index}`, `offline-${index}`, 'dome-c')],
      [`stale-${index}`, speaker(`endpoint-stale-${index}`, `stale-${index}`, 'dome-a', { lastSeenAt: -20_000 })],
    ]).flat());
    expect(spatialAudioRequest({ ...input, presences: { ...presences, ...unrelated } }))
      .toEqual(spatialAudioRequest({ ...input, presences }));
  });

  it('stops everything outside a Dome', () => {
    const request = spatialAudioRequest({
      room: null,
      microphoneEnabled: true,
      listenerPosition: [0, 0, 0],
      neighbors: [ready],
      presences: { near: speaker('endpoint-near', 'near', 'dome-a') },
      remoteTransforms: {},
      mutedAuthorPubkeys: new Set(),
      now: 1_000,
    });
    expect(request).toEqual({
      room: null,
      microphone_enabled: false,
      listener_position: [0, 0, 0],
      neighbors: [],
      speakers: [],
    });
  });
});
