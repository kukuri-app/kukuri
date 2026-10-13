import type {
  DomeDirection,
  GameRoomView,
  MetaverseAudioNeighbor,
  MetaverseAudioRoom,
  MetaverseAudioSpeaker,
  SetMetaverseAudioRequest,
} from '@/lib/api';
import type { AvatarTransform, PeerPresence } from '../MetaverseSceneModel';
import type { DomeNeighborTransitionView } from './DomeTransitionModel';
import { DOME_CONNECTION_BOUNDARY_OFFSET_CM } from './DomeSceneModel';

/** presence の有効期限(ADR 0043)。これより古い話者は購読の候補にしない。 */
export const SPATIAL_AUDIO_PRESENCE_TTL_MS = 10_000;

export function distanceCm(left: [number, number, number], right: [number, number, number]): number {
  return Math.hypot(left[0] - right[0], left[1] - right[1], left[2] - right[2]);
}

export function domeOpeningPosition(direction: DomeDirection): [number, number, number] {
  if (direction === 'north') return [0, 250, -DOME_CONNECTION_BOUNDARY_OFFSET_CM];
  if (direction === 'east') return [DOME_CONNECTION_BOUNDARY_OFFSET_CM, 250, 0];
  if (direction === 'south') return [0, 250, DOME_CONNECTION_BOUNDARY_OFFSET_CM];
  return [-DOME_CONNECTION_BOUNDARY_OFFSET_CM, 250, 0];
}

export function audioRoomOf(room: GameRoomView): MetaverseAudioRoom | null {
  const state = room.metaverse;
  if (!state) return null;
  return {
    spatial_context: state.spatial_context,
    owner_pubkey: room.host_pubkey,
    instance_id: state.instance_id,
    instance_generation: state.instance_generation,
    session_id: state.session_id,
  };
}

function roundedPosition(position: [number, number, number]): [number, number, number] {
  return [Math.round(position[0]), Math.round(position[1]), Math.round(position[2])];
}

type SpatialAudioRequestInput = {
  room: GameRoomView | null;
  microphoneEnabled: boolean;
  listenerPosition: [number, number, number];
  neighbors: DomeNeighborTransitionView[];
  presences: Record<string, PeerPresence>;
  remoteTransforms: Record<string, AvatarTransform>;
  /** mute していないと分かった author。分かるまでは渡さない(mute した相手を一度も流さない)。 */
  audibleAuthorPubkeys: ReadonlySet<string>;
  now: number;
};

/**
 * Rust の空間音声へ渡す状態(ADR 0067 §9)。音声を送っている話者を、current Dome は avatar の位置、ready な隣接 Dome は
 * opening までの距離で近い順に並べる。mute した(または mute の状態が分からない)話者・ready でない隣接 Dome・古い
 * presence は渡さない。
 */
export function spatialAudioRequest({
  room,
  microphoneEnabled,
  listenerPosition,
  neighbors,
  presences,
  remoteTransforms,
  audibleAuthorPubkeys,
  now,
}: SpatialAudioRequestInput): SetMetaverseAudioRequest {
  const listener = roundedPosition(listenerPosition);
  const current = room ? audioRoomOf(room) : null;
  if (!current) {
    return { room: null, microphone_enabled: false, listener_position: listener, neighbors: [], speakers: [] };
  }
  const readyNeighbors: MetaverseAudioNeighbor[] = [];
  const neighborDistance = new Map<string, number>();
  for (const neighbor of neighbors) {
    const neighborRoom = neighbor.boundaryState === 'ready' ? audioRoomOf(neighbor.room) : null;
    if (!neighborRoom) continue;
    const listenerOpening = domeOpeningPosition(neighbor.direction);
    readyNeighbors.push({
      room: neighborRoom,
      speaker_opening: domeOpeningPosition(neighbor.targetDirection),
      listener_opening: listenerOpening,
    });
    neighborDistance.set(neighborRoom.instance_id, distanceCm(listenerOpening, listener));
  }
  const ranked: { speaker: MetaverseAudioSpeaker; distance: number }[] = [];
  for (const presence of Object.values(presences)) {
    const roomId = presence.roomId;
    if (!presence.microphoneEnabled || !roomId || !presence.authorPubkey
      || !audibleAuthorPubkeys.has(presence.authorPubkey)
      || now - presence.lastSeenAt > SPATIAL_AUDIO_PRESENCE_TTL_MS) continue;
    let distance: number | undefined;
    if (roomId === current.instance_id) {
      const transform = remoteTransforms[`avatar:${presence.authorPubkey}`];
      distance = transform ? distanceCm(transform.position, listener) : Number.MAX_SAFE_INTEGER;
    } else {
      distance = neighborDistance.get(roomId);
    }
    if (distance === undefined) continue;
    ranked.push({
      speaker: { endpoint_id: presence.peerId.split(':')[0], instance_id: roomId },
      distance,
    });
  }
  ranked.sort((left, right) => left.distance - right.distance);
  return {
    room: current,
    microphone_enabled: microphoneEnabled,
    listener_position: listener,
    neighbors: readyNeighbors,
    speakers: ranked.map(({ speaker }) => speaker),
  };
}
