import { useCallback, useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from 'react';

import type { GameRoomView, MetaverseRoomEventView } from '@/lib/api';
import type {
  AvatarTransform,
  LatestChatBubble,
  PeerPresence,
  RoomChatMessage,
} from '../MetaverseSceneModel';
import { mergeRoomChatMessages } from '../MetaverseSceneModel';
import type { DomeNeighborTransitionView } from './DomeTransitionModel';
import type { MetaverseRoomActions } from './MetaverseRoomActions';
import { chatMessageFromApi, latestChatBubbleFromMessage } from './MetaverseRoomSessionSupport';
import {
  readClientResourceBudget,
  selectVisibleAvatarPeerIds,
} from './MetaverseResourceBudgetModel';

type UseMetaverseBackendEventsArgs = {
  actions: MetaverseRoomActions;
  selectedRoom: GameRoomView | null;
  transitionNeighbors: DomeNeighborTransitionView[];
  localPeerId: string;
  avatarFetchActive: boolean;
  remoteTransforms: Record<string, AvatarTransform>;
  setRemoteTransforms: Dispatch<SetStateAction<Record<string, AvatarTransform>>>;
};

type AvatarFetchEntry = {
  hash: string;
  attempts: number;
  state: 'idle' | 'loading' | 'resolved' | 'exhausted';
  retryAt: number | null;
};

const AVATAR_RETRY_DELAYS_MS = [5_000, 30_000] as const;
const AVATAR_MAX_ATTEMPTS = AVATAR_RETRY_DELAYS_MS.length + 1;
const AVATAR_FETCH_LEDGER_CAPACITY = 2_000;
const BACKEND_POLL_INTERVAL_MS = 180;
const BACKEND_POLL_MAX_INTERVAL_MS = 5_000;

export function mergePeerPresence(
  current: Record<string, PeerPresence>,
  presence: PeerPresence
): Record<string, PeerPresence> {
  const previous = current[presence.peerId];
  const sameAvatar = previous?.avatarAssetRef?.blob_hash
    === presence.avatarAssetRef?.blob_hash;
  return {
    ...current,
    [presence.peerId]: {
      ...previous,
      ...presence,
      avatarAssetUrl: sameAvatar ? previous?.avatarAssetUrl : undefined,
    },
  };
}

export function useMetaverseBackendEvents({
  actions,
  selectedRoom,
  transitionNeighbors,
  localPeerId,
  avatarFetchActive,
  remoteTransforms,
  setRemoteTransforms,
}: UseMetaverseBackendEventsArgs) {
  const [peerPresence, setPeerPresence] = useState<Record<string, PeerPresence>>({});
  const [messages, setMessages] = useState<RoomChatMessage[]>([]);
  const [latestChatByPeer, setLatestChatByPeer] = useState<Record<string, LatestChatBubble>>({});
  const [pollErrorCount, setPollErrorCount] = useState(0);
  const [lastRoomActivityAt, setLastRoomActivityAt] = useState(() => Date.now());
  const [, setAvatarFetchRevision] = useState(0);
  const cursorsRef = useRef(new Map<string, string>());
  const avatarFetchesRef = useRef(new Map<string, AvatarFetchEntry>());
  const clientResourceBudget = useMemo(
    () => readClientResourceBudget(typeof window === 'undefined' ? null : window.localStorage),
    []
  );
  const visibleAvatarPeerIds = useMemo(
    () => selectVisibleAvatarPeerIds(
      Object.keys(remoteTransforms),
      clientResourceBudget.max_rendered_avatars
    ),
    [clientResourceBudget.max_rendered_avatars, remoteTransforms]
  );

  const resetBackendEventCursor = useCallback(() => cursorsRef.current.clear(), []);
  const pollRoomIdsKey = JSON.stringify([
    selectedRoom?.room_id,
    ...transitionNeighbors
      .filter((neighbor) => neighbor.boundaryState === 'ready')
      .map((neighbor) => neighbor.room.room_id),
  ].filter((roomId): roomId is string => Boolean(roomId)).slice(0, 5));

  useEffect(() => {
    const roomIds = JSON.parse(pollRoomIdsKey) as string[];
    if (roomIds.length === 0) return;
    let cancelled = false;
    let timeoutId = 0;
    let consecutiveFailures = 0;
    const applyEvent = (view: MetaverseRoomEventView) => {
      const event = view.content.event;
      setLastRoomActivityAt(Date.now());
      if (event.type === 'presence_join' && event.presence.peer_id !== localPeerId) {
        const presence: PeerPresence = {
          peerId: event.presence.peer_id,
          displayName: event.presence.display_name ?? null,
          avatarAssetRef: event.presence.avatar_asset_ref ?? null,
          joinedAt: event.presence.joined_at,
          lastSeenAt: event.presence.last_seen_at,
          authorPubkey: String(view.envelope.pubkey),
          roomId: event.presence.room_id,
          microphoneEnabled: event.presence.microphone_enabled,
        };
        setPeerPresence((current) => mergePeerPresence(current, presence));
      } else if (event.type === 'presence_leave' && event.peer_id !== localPeerId) {
        setPeerPresence((current) => {
          const next = { ...current };
          delete next[event.peer_id];
          return next;
        });
        setRemoteTransforms((current) => {
          const next = { ...current };
          delete next[event.peer_id];
          return next;
        });
        setLatestChatByPeer((current) => {
          const next = { ...current };
          delete next[event.peer_id];
          return next;
        });
      } else if (event.type === 'chat_message') {
        const message = chatMessageFromApi(event.message);
        setMessages((current) => mergeRoomChatMessages(current, [message]));
        setLatestChatByPeer((current) => ({
          ...current,
          [message.authorPeerId]: latestChatBubbleFromMessage(message),
        }));
      }
    };
    const poll = async () => {
      try {
        const batches = await Promise.all(roomIds.map(async (roomId) => ({
          roomId,
          events: await actions.listRoomEvents(
            roomId,
            cursorsRef.current.get(roomId) ?? null,
            64
          ),
        })));
        if (!cancelled) {
          for (const batch of batches) {
            batch.events.forEach(applyEvent);
            const last = batch.events.at(-1);
            if (last) cursorsRef.current.set(batch.roomId, last.envelope_id);
          }
          setPollErrorCount(0);
          consecutiveFailures = 0;
        }
      } catch {
        if (!cancelled) {
          consecutiveFailures += 1;
          setPollErrorCount((current) => current + 1);
        }
      } finally {
        if (!cancelled) {
          const delay = consecutiveFailures === 0
            ? BACKEND_POLL_INTERVAL_MS
            : Math.min(
                BACKEND_POLL_INTERVAL_MS * (2 ** consecutiveFailures),
                BACKEND_POLL_MAX_INTERVAL_MS
              );
          timeoutId = window.setTimeout(() => void poll(), delay);
        }
      }
    };
    void poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timeoutId);
    };
  }, [actions, localPeerId, pollRoomIdsKey, setRemoteTransforms]);

  const avatarSessionKey = selectedRoom
    ? `${selectedRoom.room_id}:${selectedRoom.metaverse?.instance_generation ?? ''}:${localPeerId}`
    : '';
  useEffect(() => {
    const ledger = avatarFetchesRef.current;
    ledger.clear();
    return () => ledger.clear();
  }, [avatarSessionKey]);

  useEffect(() => {
    let timeoutId = 0;
    const visiblePeers = new Set(visibleAvatarPeerIds);
    const presentPeers = new Set(Object.keys(peerPresence));

    for (const [peerId, entry] of avatarFetchesRef.current) {
      const hash = peerPresence[peerId]?.avatarAssetRef?.blob_hash;
      if (!presentPeers.has(peerId) || hash !== entry.hash) {
        avatarFetchesRef.current.delete(peerId);
      }
    }

    if (!avatarFetchActive) {
      return;
    }

    const now = Date.now();
    let nextRetryAt: number | null = null;
    for (const peerId of visiblePeers) {
      const presence = peerPresence[peerId];
      const asset = presence?.avatarAssetRef;
      if (!asset) continue;

      let entry = avatarFetchesRef.current.get(peerId);
      if (!entry) {
        entry = {
          hash: asset.blob_hash,
          attempts: 0,
          state: presence.avatarAssetUrl ? 'resolved' : 'idle',
          retryAt: null,
        };
        avatarFetchesRef.current.set(peerId, entry);
      }
      if (entry.state === 'resolved' || entry.state === 'loading' || entry.state === 'exhausted') {
        continue;
      }
      if (entry.retryAt !== null && entry.retryAt > now) {
        nextRetryAt = nextRetryAt === null ? entry.retryAt : Math.min(nextRetryAt, entry.retryAt);
        continue;
      }

      entry.state = 'loading';
      entry.retryAt = null;
      entry.attempts += 1;
      const requestedEntry = entry;
      void actions.getBlobPreviewUrl(
        asset.blob_hash,
        asset.mime_type ?? 'model/vrm',
        asset.kind
      ).then((avatarAssetUrl) => {
        if (avatarFetchesRef.current.get(peerId) !== requestedEntry) return;
        if (avatarAssetUrl) {
          requestedEntry.state = 'resolved';
          setPeerPresence((current) => {
            const currentPresence = current[peerId];
            if (currentPresence?.avatarAssetRef?.blob_hash !== requestedEntry.hash) return current;
            return {
              ...current,
              [peerId]: { ...currentPresence, avatarAssetUrl },
            };
          });
          return;
        }
        requestedEntry.state = requestedEntry.attempts >= AVATAR_MAX_ATTEMPTS
          ? 'exhausted'
          : 'idle';
        requestedEntry.retryAt = requestedEntry.state === 'idle'
          ? Date.now() + AVATAR_RETRY_DELAYS_MS[requestedEntry.attempts - 1]
          : null;
      }).catch(() => {
        if (avatarFetchesRef.current.get(peerId) !== requestedEntry) return;
        requestedEntry.state = requestedEntry.attempts >= AVATAR_MAX_ATTEMPTS
          ? 'exhausted'
          : 'idle';
        requestedEntry.retryAt = requestedEntry.state === 'idle'
          ? Date.now() + AVATAR_RETRY_DELAYS_MS[requestedEntry.attempts - 1]
          : null;
      }).finally(() => {
        if (avatarFetchesRef.current.get(peerId) === requestedEntry) {
          setAvatarFetchRevision((current) => current + 1);
        }
      });
    }

    if (avatarFetchesRef.current.size > AVATAR_FETCH_LEDGER_CAPACITY) {
      for (const peerId of avatarFetchesRef.current.keys()) {
        if (!visiblePeers.has(peerId)) avatarFetchesRef.current.delete(peerId);
        if (avatarFetchesRef.current.size <= AVATAR_FETCH_LEDGER_CAPACITY) break;
      }
    }

    if (nextRetryAt !== null) {
      timeoutId = window.setTimeout(
        () => setAvatarFetchRevision((current) => current + 1),
        Math.max(0, nextRetryAt - now)
      );
    }
    return () => window.clearTimeout(timeoutId);
  }, [actions, avatarFetchActive, peerPresence, visibleAvatarPeerIds]);

  useEffect(() => {
    const liveRoomIds = new Set([
      selectedRoom?.room_id,
      ...transitionNeighbors
        .filter((neighbor) => neighbor.boundaryState === 'ready')
        .map((neighbor) => neighbor.room.room_id),
    ].filter((roomId): roomId is string => Boolean(roomId)));
    for (const roomId of cursorsRef.current.keys()) {
      if (!liveRoomIds.has(roomId)) cursorsRef.current.delete(roomId);
    }
  }, [selectedRoom?.room_id, transitionNeighbors]);

  useEffect(() => {
    const intervalId = window.setInterval(() => {
      const cutoff = Date.now() - 10_000;
      setPeerPresence((current) => Object.fromEntries(
        Object.entries(current).filter(([, presence]) => presence.lastSeenAt >= cutoff)
      ));
    }, 1_000);
    return () => window.clearInterval(intervalId);
  }, []);

  return {
    peerPresence,
    setPeerPresence,
    messages,
    setMessages,
    latestChatByPeer,
    setLatestChatByPeer,
    pollErrorCount,
    setPollErrorCount,
    lastRoomActivityAt,
    setLastRoomActivityAt,
    resetBackendEventCursor,
  };
}
