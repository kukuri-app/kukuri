import type { SyncStatus } from '@/lib/api';
import { METAVERSE_CHAT_BUBBLE_TTL_MS } from '../MetaverseSceneModel';
import type { LatestChatBubble, RoomChatMessage } from '../MetaverseSceneModel';

export function chatMessageFromApi(message: {
  room_id: string;
  message_id: string;
  author_peer_id: string;
  display_name?: string | null;
  body: string;
  created_at: number;
}): RoomChatMessage {
  return {
    roomId: message.room_id,
    messageId: message.message_id,
    authorPeerId: message.author_peer_id,
    displayName: message.display_name ?? null,
    body: message.body,
    createdAt: message.created_at,
  };
}

export function topicDiagnosticFor(syncStatus: SyncStatus, topic: string) {
  return syncStatus.topic_diagnostics.find(
    (diagnostic) => diagnostic.topic === topic || diagnostic.topic === `hint/${topic}`
  );
}

export function latestChatBubbleFromMessage(
  message: RoomChatMessage,
  now = Date.now()
): LatestChatBubble {
  return {
    peerId: message.authorPeerId,
    displayName: message.displayName ?? null,
    body: message.body,
    createdAt: message.createdAt,
    expiresAt: now + METAVERSE_CHAT_BUBBLE_TTL_MS,
  };
}

/** keepalive の拒否のうち、退避が要るもの(block と access の失効)の理由。 */
export function keepAliveEvacuationReason(error: unknown): 'blocked' | 'access_revoked' | null {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes('BLOCKED')) return 'blocked';
  return message.includes('ACCESS_DENIED') || message.includes('ACCESS_REVOKED') ? 'access_revoked' : null;
}
