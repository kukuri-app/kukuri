import type { MetaverseAssetRef, SharedRoomObjectV1 } from '@/lib/api';

export type MetaverseVec3 = [number, number, number];

export const AVATAR_ANIMATION_STATES = ['idle', 'walk', 'sprint', 'jump', 'sitting'] as const;

export type AvatarAnimationState = (typeof AVATAR_ANIMATION_STATES)[number];

export function normalizeAvatarAnimationState(value: string | null | undefined): AvatarAnimationState {
  if (value === 'idle' || value === 'walk' || value === 'sprint' || value === 'jump' || value === 'sitting') {
    return value;
  }
  return 'idle';
}

export function avatarAnimationForInput(keys: Iterable<string>, sitting: boolean): AvatarAnimationState {
  if (sitting) {
    return 'sitting';
  }
  const keySet = new Set(Array.from(keys, (key) => key.toLowerCase()));
  const moving =
    keySet.has('w') ||
    keySet.has('a') ||
    keySet.has('s') ||
    keySet.has('d') ||
    keySet.has('arrowup') ||
    keySet.has('arrowleft') ||
    keySet.has('arrowdown') ||
    keySet.has('arrowright');
  if (!moving) {
    return 'idle';
  }
  return keySet.has('shift') ? 'sprint' : 'walk';
}

export type AvatarTransform = {
  roomId: string;
  peerId: string;
  seq: number;
  position: MetaverseVec3;
  rotation: MetaverseVec3;
  animation: AvatarAnimationState;
  sentAt: number;
};

export type AvatarPhysicsState = {
  verticalVelocity: number;
  grounded: boolean;
};

export type AvatarMovementStep = {
  position: MetaverseVec3;
  physics: AvatarPhysicsState;
};

export type PeerPresence = {
  peerId: string;
  displayName: string | null;
  avatarAssetRef: MetaverseAssetRef | null;
  avatarAssetUrl?: string | null;
  joinedAt: number;
  lastSeenAt: number;
  /** 署名した account の公開鍵・presence の Dome・音声を送っているか(空間音声の候補。ADR 0067)。 */
  authorPubkey?: string;
  roomId?: string;
  microphoneEnabled?: boolean;
};

export type RoomChatMessage = {
  roomId: string;
  messageId: string;
  authorPeerId: string;
  displayName?: string | null;
  body: string;
  createdAt: number;
};

export type LatestChatBubble = {
  peerId: string;
  displayName: string | null;
  body: string;
  createdAt: number;
  expiresAt: number;
};

export type MetaverseRoomConnectionState = 'live' | 'stale' | 'recovering' | 'offline';

export type MetaverseRoomEvent =
  | { type: 'presence.join'; presence: PeerPresence }
  | { type: 'presence.leave'; roomId: string; peerId: string; leftAt: number }
  | { type: 'chat.message'; message: RoomChatMessage };

export type AvatarAssetStatus = 'loading' | 'sample-vrm' | 'blob-vrm' | 'fallback-primitive';

export const DEFAULT_SHARED_OBJECT: SharedRoomObjectV1 = {
  object_id: 'mvp-object-1',
  asset_ref: null,
  primitive_fallback: 'cube',
  position: [0, 50, -240],
  rotation: [0, 0, 0],
  scale: [100, 100, 100],
  updated_by: '',
  updated_at: 0,
};

export const DEFAULT_AVATAR_ASSET_NAME = 'blumochichi.vrm';
export const DEFAULT_AVATAR_ASSET_URL = `/${DEFAULT_AVATAR_ASSET_NAME}`;
export const METAVERSE_CHAT_HISTORY_LIMIT = 100;
export const METAVERSE_CHAT_BUBBLE_TTL_MS = 8_000;
export const METAVERSE_ROOM_STALE_MS = 45_000;
export const METAVERSE_ROOM_HEARTBEAT_MS = 5_000;
export const METAVERSE_ROOM_RECOVERY_MS = 10_000;
export const METAVERSE_AVATAR_MOVING_SEND_INTERVAL_MS = 30;
export const METAVERSE_AVATAR_IDLE_SEND_INTERVAL_MS = 150;
export const METAVERSE_REMOTE_AVATAR_POSITION_SMOOTHING_SECONDS = 0.14;

export const AVATAR_GROUND_Y = 0;
export const AVATAR_JUMP_VELOCITY = 520;
export const AVATAR_GRAVITY = 1600;
export const AVATAR_VRM_FORWARD_YAW_OFFSET_DEGREES = 180;

export function avatarVrmVisualRootYawDegrees(baseYawDegrees: number): number {
  return baseYawDegrees + AVATAR_VRM_FORWARD_YAW_OFFSET_DEGREES;
}

export function initialAvatarTransform(
  roomId: string,
  localPeerId: string,
  spawnPosition?: MetaverseVec3,
  spawnRotation?: MetaverseVec3
): AvatarTransform {
  return {
    roomId,
    peerId: localPeerId,
    seq: 0,
    position: spawnPosition ?? [0, AVATAR_GROUND_Y, 260],
    rotation: spawnRotation ?? [0, 180, 0],
    animation: 'idle',
    sentAt: 0,
  };
}

export function stepAvatarJump(
  position: MetaverseVec3,
  physics: AvatarPhysicsState,
  deltaSeconds: number,
  jumpRequested: boolean,
  gravity = AVATAR_GRAVITY
): AvatarMovementStep {
  let verticalVelocity =
    jumpRequested && physics.grounded ? AVATAR_JUMP_VELOCITY : physics.verticalVelocity;
  let nextY = position[1] + verticalVelocity * deltaSeconds;
  verticalVelocity -= gravity * deltaSeconds;

  if (nextY <= AVATAR_GROUND_Y) {
    nextY = AVATAR_GROUND_Y;
    verticalVelocity = 0;
  }

  return {
    position: [position[0], Math.round(nextY), position[2]],
    physics: {
      verticalVelocity,
      grounded: nextY === AVATAR_GROUND_Y,
    },
  };
}

export function isNewerRemoteTransform(
  current: AvatarTransform | null | undefined,
  incoming: AvatarTransform
): boolean {
  if (!current) {
    return true;
  }
  if (incoming.seq !== current.seq) {
    return incoming.seq > current.seq;
  }
  return incoming.sentAt > current.sentAt;
}

export function remoteAvatarYawDegrees(rotation: MetaverseVec3): number {
  return rotation[1];
}

export function isNewerSharedObject(
  current: SharedRoomObjectV1 | null | undefined,
  incoming: SharedRoomObjectV1
): boolean {
  if (!current) {
    return true;
  }
  return incoming.updated_at > current.updated_at;
}

export function mergeRoomChatMessages(
  current: RoomChatMessage[],
  incoming: RoomChatMessage[]
): RoomChatMessage[] {
  const byId = new Map<string, RoomChatMessage>();
  for (const message of [...current, ...incoming]) {
    byId.set(message.messageId, message);
  }
  return Array.from(byId.values())
    .sort((left, right) => left.createdAt - right.createdAt || left.messageId.localeCompare(right.messageId))
    .slice(-METAVERSE_CHAT_HISTORY_LIMIT);
}
