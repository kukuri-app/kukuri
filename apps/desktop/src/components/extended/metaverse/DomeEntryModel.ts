import type { TFunction } from 'i18next';

import type { GameRoomView, SpatialContextV1 } from '@/lib/api';
import { InvokeError } from '@/lib/api/invoke/error';

const LAST_VISITED_PREFIX = 'kukuri.metaverse.last-visited.v1';

/** 入室の失敗の表示。所有者の端末に接続できないときは code ではなく利用者向けの文言にする(ADR 0038 #1527)。 */
export function domeEntryErrorMessage(error: unknown, t: TFunction): string {
  if (error instanceof InvokeError && error.code === 'DOME_HOST_UNREACHABLE') {
    return t('entry.hostUnreachable');
  }
  return error instanceof Error ? error.message : 'Dome entry failed';
}

export function spatialContextKey(context: SpatialContextV1): string {
  return context.kind === 'channel'
    ? `channel:${context.topic_id}:${context.channel_id}`
    : `topic:${context.topic_id}`;
}

export function lastVisitedDomeStorageKey(
  authorPubkey: string,
  context: SpatialContextV1
): string {
  return `${LAST_VISITED_PREFIX}:${authorPubkey}:${spatialContextKey(context)}`;
}

export function readLastVisitedDome(
  authorPubkey: string,
  context: SpatialContextV1
): string | null {
  try {
    return globalThis.localStorage?.getItem(lastVisitedDomeStorageKey(authorPubkey, context)) ?? null;
  } catch {
    return null;
  }
}

export function writeLastVisitedDome(
  authorPubkey: string,
  context: SpatialContextV1,
  instanceId: string
): void {
  try {
    globalThis.localStorage?.setItem(lastVisitedDomeStorageKey(authorPubkey, context), instanceId);
  } catch {
    // Local visit history is optional and must never make admission fail.
  }
}

export function domeHasActiveHost(room: GameRoomView): boolean {
  return room.phase_label !== 'management_only' && (room.dome_hosting?.kind === 'owner_hosted'
    || room.dome_hosting?.kind === 'community_node_hosted');
}

type ResolveDomeEntryOrderInput = {
  rooms: GameRoomView[];
  localAuthorPubkey: string;
  lastVisitedInstanceId: string | null;
  configuredEntryInstanceId: string | null;
};

export function resolveDomeEntryOrder({
  rooms,
  localAuthorPubkey,
  lastVisitedInstanceId,
  configuredEntryInstanceId,
}: ResolveDomeEntryOrderInput): GameRoomView[] {
  const hosted = rooms
    .filter((room) => room.metaverse && domeHasActiveHost(room))
    .sort((left, right) => left.room_id.localeCompare(right.room_id));
  const result: GameRoomView[] = [];
  const add = (room: GameRoomView | undefined) => {
    if (room && !result.some((candidate) => candidate.room_id === room.room_id)) {
      result.push(room);
    }
  };
  const find = (id: string | null) => id
    ? hosted.find((room) => room.metaverse?.instance_id === id || room.room_id === id)
    : undefined;

  add(hosted.find((room) => room.host_pubkey === localAuthorPubkey));
  add(find(lastVisitedInstanceId));
  add(find(configuredEntryInstanceId));
  for (const room of hosted) add(room);
  return result;
}
