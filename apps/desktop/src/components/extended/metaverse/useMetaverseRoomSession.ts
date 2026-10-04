import { useCallback, useEffect, useId, useMemo, useRef, useState, type FormEvent } from 'react';
import { useTranslation } from 'react-i18next';

import type { SupportedLocale } from '@/i18n';
import type {
  DomeBoundaryStateV1,
  DomeDirection,
  GameRoomView,
  DomePhysicsSnapshotV1,
  DomeSessionInputKindV1,
  MetaverseAssetRef,
  MetaverseColliderV1,
  MetaverseInteractionKind,
  MetaversePersistentPropV1,
  SharedRoomObjectV1,
  SpatialContextV1,
  SyncStatus,
} from '@/lib/api';
import type { MetaverseRoomActions } from './MetaverseRoomActions';
import type { SessionPropView } from '../MetaverseScene';
import { createDomeInteractionInput, persistentPropAsSharedObject } from './DomeSceneModel';
import { useDomeTransitionNeighbors } from './useDomeTransitionNeighbors';
import { useDomeConnections } from './useDomeConnections';
import {
  DEFAULT_SHARED_OBJECT,
  METAVERSE_ROOM_HEARTBEAT_MS,
  METAVERSE_ROOM_RECOVERY_MS,
  METAVERSE_ROOM_STALE_MS,
  isNewerSharedObject,
  mergeRoomChatMessages,
  normalizeAvatarAnimationState,
  type AvatarTransform,
  type MetaverseRoomConnectionState,
  type MetaverseRoomEvent,
  type MetaverseVec3,
  type PeerPresence,
  type RoomChatMessage,
} from '../MetaverseSceneModel';
import {
  chatMessageFromApi,
  keepAliveEvacuationReason,
  latestChatBubbleFromMessage,
  topicDiagnosticFor,
} from './MetaverseRoomSessionSupport';
import { useSpatialAudio } from './useSpatialAudio';
import { mergePeerPresence, useMetaverseBackendEvents } from './useMetaverseBackendEvents';
import {
  domeEntryErrorMessage,
  readLastVisitedDome,
  resolveDomeEntryOrder,
  spatialContextKey,
  writeLastVisitedDome,
} from './DomeEntryModel';
import { loadAvatarCollider } from './AvatarColliderModel';
import { useDomeTransitionAttempt } from './useDomeTransitionAttempt';

type UseMetaverseRoomSessionArgs = {
  actions: MetaverseRoomActions;
  managementActive?: boolean;
  activeTopic: string;
  rooms: GameRoomView[];
  syncStatus: SyncStatus;
  locale: SupportedLocale;
  localDisplayName: string | null;
  localAvatarAssetRef: MetaverseAssetRef | null;
  localAvatarAssetUrl: string | null; avatarFetchActive?: boolean;
  mutedAuthorPubkeys?: ReadonlySet<string>;
  initialSelectedRoomId?: string | null;
  activeChannelId?: string | null;
  configuredEntryInstanceId?: string | null;
  onError: (message: string | null) => void;
};

export type DomeRecoveryStatus = {
  state: 'online' | 'offline' | 'evacuating' | 'closed' | 'no_candidate';
  secondsRemaining: number | null;
  reason: 'host_offline' | 'access_revoked' | 'blocked' | 'user_requested' | null;
  targetTitle: string | null;
};

export const ONLINE_DOME_RECOVERY: DomeRecoveryStatus = {
  state: 'online',
  secondsRemaining: null,
  reason: null,
  targetTitle: null,
};

const EMPTY_MUTED_AUTHOR_PUBKEYS = new Set<string>();

const EMPTY_ROOM_CHAT_HISTORY: NonNullable<
  NonNullable<GameRoomView['metaverse']>['chat_history']
> = [];

export function useMetaverseRoomSession({
  actions,
  managementActive = false,
  activeTopic,
  rooms,
  syncStatus,
  locale,
  localDisplayName,
  localAvatarAssetRef,
  localAvatarAssetUrl, avatarFetchActive = true,
  mutedAuthorPubkeys = EMPTY_MUTED_AUTHOR_PUBKEYS,
  initialSelectedRoomId = null,
  activeChannelId = null,
  configuredEntryInstanceId = null,
  onError,
}: UseMetaverseRoomSessionArgs) {
  const { t } = useTranslation('metaverse', { lng: locale });
  const admissionAttempt = useRef(0);
  useEffect(() => () => { ++admissionAttempt.current; }, [activeTopic, activeChannelId, syncStatus.local_author_pubkey, managementActive]);
  const [selectedRoomId, setSelectedRoomId] = useState<string | null>(initialSelectedRoomId);
  const [admissionClaimed, setAdmissionConfirmed] = useState(false);
  const [admittedIdentity, setAdmittedIdentity] = useState<string | null>(null);
  const [admissionStatus, setAdmissionStatus] = useState<'resolving' | 'admitting' | 'joined' | 'selection'>('resolving');
  const [joinedRoomIds, setJoinedRoomIds] = useState<Set<string>>(() => new Set());
  const [remoteTransforms, setRemoteTransforms] = useState<Record<string, AvatarTransform>>({});
  const [messageDraft, setMessageDraft] = useState('');
  const [sharedObject, setSharedObject] = useState<SharedRoomObjectV1>(DEFAULT_SHARED_OBJECT);
  const [sessionProps, setSessionProps] = useState<SessionPropView[]>([]);
  const [handoffTransform, setHandoffTransform] = useState<AvatarTransform | null>(null);
  const [lastSentSeq, setLastSentSeq] = useState(0);
  const [recoveringUntil, setRecoveringUntil] = useState(0);
  const [clockNow, setClockNow] = useState(() => Date.now());
  const [domeRecovery, setDomeRecovery] = useState<DomeRecoveryStatus>(ONLINE_DOME_RECOVERY);
  const [pendingEvacuationReason, setPendingEvacuationReason] = useState<
    'access_revoked' | 'blocked' | null
  >(null);
  const channelRef = useRef<BroadcastChannel | null>(null);
  const lastPhysicsSnapshotSequenceRef = useRef(0);
  const physicsTargetRef = useRef<{ id: string; generation: number } | null>(null);
  const physicsEpochRef = useRef(0);
  const bindPhysicsTarget = useCallback((room: GameRoomView) => {
    if (!room.metaverse) return;
    const target = { id: room.metaverse.instance_id, generation: room.metaverse.instance_generation };
    if (physicsTargetRef.current?.id !== target.id || physicsTargetRef.current.generation !== target.generation) {
      lastPhysicsSnapshotSequenceRef.current = 0; physicsEpochRef.current = 0;
    }
    physicsTargetRef.current = target;
  }, []);
  const lastRecoveryAtRef = useRef(0);
  const pendingCreatedRoomIdRef = useRef<string | null>(null);
  const sharedObjectRef = useRef<SharedRoomObjectV1>(DEFAULT_SHARED_OBJECT);
  const localPeerSeed = useId().replaceAll(':', '');
  const localPeerId = `${syncStatus.discovery.local_endpoint_id || syncStatus.local_author_pubkey || 'local'}:${localPeerSeed}`;
  const lastSentTransformRef = useRef<AvatarTransform | null>(null);
  const entryAttemptKeyRef = useRef<string | null>(null);
  const entryContextKeyRef = useRef<string | null>(null);
  const entryAutoDisabledRef = useRef(false);
  const evacuationRunningRef = useRef(false);
  const avatarColliderPromiseRef = useRef<{
    assetUrl: string | null;
    promise: Promise<MetaverseColliderV1 | null>;
  } | null>(null);
  const sessionSequenceByInstanceRef = useRef(new Map<string, number>());
  const lastReceivedAt = useMemo(() => {
    const values = Object.values(remoteTransforms).map((transform) => transform.sentAt);
    return values.length ? Math.max(...values) : null;
  }, [remoteTransforms]);
  const remoteAnimationSummary = useMemo(
    () =>
      Object.values(remoteTransforms)
        .map((transform) => `${transform.peerId.slice(0, 8)}:${transform.animation}`)
        .join(', '),
    [remoteTransforms]
  );

  const selectedRoom = selectedRoomId
    ? rooms.find((room) => room.room_id === selectedRoomId) ?? null
    : null;
  const selectedGeneration = selectedRoom?.metaverse?.instance_generation;
  const selectedInstanceId = selectedRoom?.metaverse?.instance_id;
  const selectedIdentity = selectedRoom ? JSON.stringify([syncStatus.local_author_pubkey, activeTopic, activeChannelId, selectedRoom.room_id, selectedRoom.metaverse?.instance_generation]) : null;
  const admissionConfirmed = admissionClaimed && admittedIdentity === selectedIdentity && selectedRoom?.phase_label !== 'management_only';
  const admittedRoom = admissionConfirmed ? selectedRoom : null;
  useEffect(() => {
    if (admissionClaimed && !admissionConfirmed) { setAdmissionConfirmed(false); setAdmittedIdentity(null); setJoinedRoomIds(new Set()); setAdmissionStatus('selection'); entryAttemptKeyRef.current = null; }
  }, [admissionClaimed, admissionConfirmed]);
  const entryContext = useMemo<SpatialContextV1>(() => activeChannelId
    ? { kind: 'channel', topic_id: activeTopic, channel_id: activeChannelId }
    : { kind: 'topic', topic_id: activeTopic }, [activeChannelId, activeTopic]);
  const entryCandidates = useMemo(() => resolveDomeEntryOrder({
    rooms,
    localAuthorPubkey: syncStatus.local_author_pubkey,
    lastVisitedInstanceId: readLastVisitedDome(syncStatus.local_author_pubkey, entryContext),
    configuredEntryInstanceId,
  }), [configuredEntryInstanceId, entryContext, rooms, syncStatus.local_author_pubkey]);
  const connections = useDomeConnections(actions, admittedRoom, syncStatus.local_author_pubkey);
  const [transitionNeighbors, setTransitionNeighbors] = useDomeTransitionNeighbors(
    actions,
    admittedRoom,
    rooms,
    syncStatus.local_author_pubkey,
    connections
  );

  const {
    microphoneEnabled,
    toggleMicrophone,
    disableMicrophone,
    playSpatialAudioFrame,
    stopSpatialAudio,
  } = useSpatialAudio({
    actions,
    selectedRoom: admittedRoom,
    localPeerId,
    listenerTransformRef: lastSentTransformRef,
    mutedAuthorPubkeys,
    transitionNeighbors,
  });
  const {
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
  } = useMetaverseBackendEvents({
    actions,
    selectedRoom: admittedRoom,
    transitionNeighbors,
    localPeerId, avatarFetchActive, remoteTransforms,
    playSpatialAudioFrame,
    setRemoteTransforms,
  });
  const selectedRoomRoomId = admittedRoom?.room_id ?? null;
  const selectedRoomSharedObject = admittedRoom?.metaverse
    ? persistentPropAsSharedObject(
        admittedRoom.metaverse.dome.customization.persistent_props[0],
        admittedRoom.host_pubkey,
        admittedRoom.updated_at
      )
    : null;
  const selectedRoomChatHistory = admittedRoom?.metaverse?.chat_history ?? EMPTY_ROOM_CHAT_HISTORY;
  const activeTopicDiagnostic = useMemo(
    () => topicDiagnosticFor(syncStatus, activeTopic),
    [activeTopic, syncStatus]
  );
  const roomConnectionState: MetaverseRoomConnectionState = useMemo(() => {
    if (!admittedRoom) {
      return 'offline';
    }
    if (recoveringUntil > clockNow) {
      return 'recovering';
    }
    const topicPeerCount = activeTopicDiagnostic?.peer_count ?? syncStatus.peer_count;
    const topicError = activeTopicDiagnostic?.last_error ?? syncStatus.last_error ?? null;
    if (
      !syncStatus.connected ||
      syncStatus.delivery_state === 'Offline' ||
      topicPeerCount === 0 ||
      pollErrorCount >= 3 ||
      topicError
    ) {
      return 'offline';
    }
    if (clockNow - lastRoomActivityAt > METAVERSE_ROOM_STALE_MS) {
      return 'stale';
    }
    return 'live';
  }, [
    activeTopicDiagnostic,
    clockNow,
    lastRoomActivityAt,
    pollErrorCount,
    recoveringUntil,
    admittedRoom,
    syncStatus,
  ]);
  const knownPeerCount = Object.keys(remoteTransforms).length;
  const nextSessionSequence = useCallback((instanceId: string, suggested = Date.now()) => {
    const next = Math.max(suggested, (sessionSequenceByInstanceRef.current.get(instanceId) ?? 0) + 1);
    sessionSequenceByInstanceRef.current.set(instanceId, next);
    return next;
  }, []);

  const submitInputForRoom = useCallback((
    room: GameRoomView,
    input: DomeSessionInputKindV1,
    suggestedSequence = Date.now()
  ) => {
    if (!room.metaverse) return Promise.reject(new Error('Dome room state is unavailable'));
    return actions.submitSessionInput(
      room.metaverse.spatial_context,
      room.metaverse.instance_id,
      nextSessionSequence(room.metaverse.instance_id, suggestedSequence),
      input,
      room.metaverse.instance_generation
    );
  }, [actions, nextSessionSequence]);

  const resolveLocalAvatarCollider = useCallback(() => {
    if (avatarColliderPromiseRef.current?.assetUrl !== localAvatarAssetUrl) {
      avatarColliderPromiseRef.current = {
        assetUrl: localAvatarAssetUrl,
        promise: loadAvatarCollider(localAvatarAssetUrl).catch(() => null),
      };
    }
    return avatarColliderPromiseRef.current.promise;
  }, [localAvatarAssetUrl]);

  useEffect(() => {
    sharedObjectRef.current = sharedObject;
  }, [sharedObject]);

  useEffect(() => {
    if (!selectedRoomId) {
      return;
    }
    if (rooms.some((room) => room.room_id === selectedRoomId)) {
      if (pendingCreatedRoomIdRef.current === selectedRoomId) {
        pendingCreatedRoomIdRef.current = null;
      }
      return;
    }
    if (pendingCreatedRoomIdRef.current !== selectedRoomId) {
      setSelectedRoomId(null);
      setAdmissionConfirmed(false);
      setAdmissionStatus('selection');
    }
  }, [rooms, selectedRoomId]);

  useEffect(() => {
    const intervalId = window.setInterval(() => {
      const now = Date.now();
      setClockNow(now);
      setLatestChatByPeer((current) => {
        const next = Object.fromEntries(
          Object.entries(current).filter(([, bubble]) => bubble.expiresAt > now)
        );
        return Object.keys(next).length === Object.keys(current).length ? current : next;
      });
    }, 1000);
    return () => {
      window.clearInterval(intervalId);
    };
  }, [setLatestChatByPeer]);

  useEffect(() => {
    physicsTargetRef.current = selectedInstanceId && selectedGeneration !== undefined ? { id: selectedInstanceId, generation: selectedGeneration } : null;
    physicsEpochRef.current = 0;
    sharedObjectRef.current = DEFAULT_SHARED_OBJECT;
    setSharedObject(DEFAULT_SHARED_OBJECT);
    setSessionProps([]);
    setRemoteTransforms({});
    setPeerPresence({});
    setMessages([]);
    setLatestChatByPeer({});
    setPollErrorCount(0);
    resetBackendEventCursor();
    lastPhysicsSnapshotSequenceRef.current = 0;
  }, [
    resetBackendEventCursor,
    selectedRoom?.room_id,
    selectedInstanceId,
    selectedGeneration,
    setLatestChatByPeer,
    setMessages,
    setPeerPresence,
    setPollErrorCount,
  ]);

  useEffect(() => {
    if (!selectedRoomRoomId) {
      return;
    }
    setLastRoomActivityAt(Date.now());
    if (typeof BroadcastChannel === 'undefined') {
      return;
    }
    const channel = new BroadcastChannel(`kukuri-metaverse-room:${selectedRoomRoomId}`);
    channelRef.current = channel;
    channel.onmessage = (event: MessageEvent<MetaverseRoomEvent>) => {
      const data = event.data;
      if (!data || !('type' in data)) {
        return;
      }
      setLastRoomActivityAt(Date.now());
      if (data.type === 'presence.join' && data.presence.peerId !== localPeerId) {
        setPeerPresence((current) => mergePeerPresence(current, data.presence));
      }
      if (data.type === 'presence.leave' && data.peerId !== localPeerId) {
        setPeerPresence((current) => {
          const next = { ...current };
          delete next[data.peerId];
          return next;
        });
        setRemoteTransforms((current) => {
          const next = { ...current };
          delete next[data.peerId];
          return next;
        });
        setLatestChatByPeer((current) => {
          const next = { ...current };
          delete next[data.peerId];
          return next;
        });
      }
      if (data.type === 'chat.message') {
        setMessages((current) => mergeRoomChatMessages(current, [data.message]));
        setLatestChatByPeer((current) => ({
          ...current,
          [data.message.authorPeerId]: latestChatBubbleFromMessage(data.message),
        }));
      }
    };
    return () => {
      channel.close();
      channelRef.current = null;
    };
  }, [
    localPeerId,
    resetBackendEventCursor,
    selectedRoomRoomId,
    setLastRoomActivityAt,
    setLatestChatByPeer,
    setMessages,
    setPeerPresence,
    setPollErrorCount,
  ]);

  useEffect(() => {
    const nextObject = selectedRoomSharedObject ?? DEFAULT_SHARED_OBJECT;
    setSharedObject((current) => {
      if (!isNewerSharedObject(current, nextObject)) {
        return current;
      }
      sharedObjectRef.current = nextObject;
      return nextObject;
    });
  }, [selectedRoomSharedObject]);

  useEffect(() => {
    const durableMessages = selectedRoomChatHistory.map(chatMessageFromApi);
    setMessages((current) => mergeRoomChatMessages(current, durableMessages));
  }, [selectedRoomChatHistory, selectedRoomRoomId, setMessages]);

  function emit(event: MetaverseRoomEvent) {
    channelRef.current?.postMessage(event);
  }

  const applyPhysicsSnapshot = useCallback((snapshot: DomePhysicsSnapshotV1) => {
    if (snapshot.instance_id !== physicsTargetRef.current?.id || snapshot.instance_generation !== physicsTargetRef.current.generation) return;
    if (snapshot.lease_epoch < physicsEpochRef.current) return;
    if (snapshot.lease_epoch > physicsEpochRef.current) { physicsEpochRef.current = snapshot.lease_epoch; lastPhysicsSnapshotSequenceRef.current = 0; }
    if (snapshot.sequence <= lastPhysicsSnapshotSequenceRef.current) return;
    lastPhysicsSnapshotSequenceRef.current = snapshot.sequence;
    const remote: Record<string, AvatarTransform> = {};
    for (const body of snapshot.bodies) {
      if (
        body.kind === 'avatar'
        && body.entity_id !== `avatar:${syncStatus.local_author_pubkey}`
      ) {
        remote[body.entity_id] = {
          roomId: snapshot.instance_id,
          peerId: body.entity_id,
          seq: snapshot.sequence,
          position: body.position,
          rotation: body.rotation,
          animation: normalizeAvatarAnimationState(body.animation),
          sentAt: snapshot.simulated_at,
        };
      }
    }
    setRemoteTransforms(remote);
    const propBodies = snapshot.bodies.filter(
      (body) => body.kind === 'persistent_prop' || body.kind === 'guest_prop'
    );
    setSessionProps(propBodies.map((body) => {
      const definition = admittedRoom?.metaverse?.dome.customization.persistent_props.find(
        (candidate) => candidate.prop_id === body.entity_id
      );
      return {
        kind: body.kind as SessionPropView['kind'],
        object: {
          object_id: body.entity_id,
          asset_ref: definition?.asset_ref ?? null,
          primitive_fallback: definition?.primitive_fallback ?? 'cube',
          position: body.position,
          rotation: body.rotation,
          scale: definition?.scale ?? [100, 100, 100],
          updated_by: snapshot.host_pubkey,
          updated_at: snapshot.simulated_at,
        },
        collider: definition?.collider ?? null,
      };
    }));
    const prop = propBodies.find((body) => body.kind === 'persistent_prop');
    if (prop) {
      setSharedObject((current) => {
        const next = {
          ...current,
          object_id: prop.entity_id,
          position: prop.position,
          rotation: prop.rotation,
          updated_by: snapshot.host_pubkey,
          updated_at: snapshot.simulated_at,
        };
        sharedObjectRef.current = next;
        return next;
      });
    }
    setLastRoomActivityAt(Date.now());
  }, [admittedRoom, setLastRoomActivityAt, syncStatus.local_author_pubkey]);

  const submitAuthoritativeInput = useCallback((input: DomeSessionInputKindV1, sequence = Date.now()) => {
    if (!admittedRoom?.metaverse || domeRecovery.state !== 'online') return;
    void submitInputForRoom(admittedRoom, input, sequence)
      .then(applyPhysicsSnapshot)
      .catch(() => {
        // Hosting may not have been started yet; presence/chat remain available.
      });
  }, [admittedRoom, applyPhysicsSnapshot, domeRecovery.state, submitInputForRoom]);

  // 一覧の取り直しで部屋の object が作り直されても、5 秒ごとの keepalive を止めない(ADR 0045、#1527)。
  const keepAliveRef = useRef<() => void>(() => undefined);
  useEffect(() => {
    keepAliveRef.current = () => {
      if (!admittedRoom?.metaverse) return;
      void submitInputForRoom(admittedRoom, { type: 'keep_alive' }).then(applyPhysicsSnapshot).catch((error: unknown) => {
        const reason = keepAliveEvacuationReason(error);
        if (reason) setPendingEvacuationReason(reason);
      });
    };
  });
  const admittedKey = admittedRoom?.metaverse ? `${admittedRoom.room_id}:${admittedRoom.metaverse.instance_generation}` : null;
  useEffect(() => {
    if (!admittedKey || domeRecovery.state !== 'online') return;
    const intervalId = window.setInterval(() => keepAliveRef.current(), 5_000);
    return () => window.clearInterval(intervalId);
  }, [admittedKey, domeRecovery.state]);

  useEffect(() => {
    if (!admittedRoom || domeRecovery.state !== 'online') {
      return;
    }
    const joinedAt = Date.now();
    const publishPresence = () => {
      const now = Date.now();
      const presence: PeerPresence = {
        peerId: localPeerId,
        displayName: localDisplayName,
        avatarAssetRef: localAvatarAssetRef,
        avatarAssetUrl: localAvatarAssetUrl,
        joinedAt,
        lastSeenAt: now,
      };
      emit({ type: 'presence.join', presence });
      void actions.publishRoomEvent(admittedRoom.room_id, localPeerId, now, {
        type: 'presence_join',
        presence: {
          room_id: admittedRoom.room_id,
          peer_id: localPeerId,
          display_name: localDisplayName,
          avatar_asset_ref: localAvatarAssetRef,
          joined_at: joinedAt,
          last_seen_at: now,
        },
      }).catch(() => {
        // Browser-only fallback is handled by the local scene.
      });
    };
    publishPresence();
    const intervalId = window.setInterval(publishPresence, METAVERSE_ROOM_HEARTBEAT_MS);
    return () => {
      window.clearInterval(intervalId);
    };
  }, [
    activeTopic,
    actions,
    localAvatarAssetRef,
    localAvatarAssetUrl,
    localDisplayName,
    localPeerId,
    admittedRoom,
    domeRecovery.state,
    submitAuthoritativeInput,
  ]);

  useEffect(() => {
    if (domeRecovery.state === 'online') return;
    disableMicrophone();
    stopSpatialAudio();
  }, [disableMicrophone, domeRecovery.state, stopSpatialAudio]);

  useEffect(() => stopSpatialAudio(), [admittedRoom?.room_id, stopSpatialAudio, transitionNeighbors]);

  useEffect(() => {
    if (!admittedRoom || (roomConnectionState !== 'stale' && roomConnectionState !== 'offline')) {
      return;
    }
    const now = Date.now();
    if (now - lastRecoveryAtRef.current < METAVERSE_ROOM_RECOVERY_MS) {
      return;
    }
    lastRecoveryAtRef.current = now;
    resetBackendEventCursor();
    lastPhysicsSnapshotSequenceRef.current = 0;
    if (roomConnectionState === 'stale') {
      setRecoveringUntil(now + 3_000);
    }
    void Promise.resolve(actions.refresh()).catch(() => {
      setPollErrorCount((current) => current + 1);
    });
  }, [actions, admittedRoom, resetBackendEventCursor, roomConnectionState, setPollErrorCount]);

  function resetRoomRuntimeState() {
    setRemoteTransforms({});
    setPeerPresence({});
    setLatestChatByPeer({});
    setPollErrorCount(0);
    setLastRoomActivityAt(Date.now());
    setRecoveringUntil(0);
    setLastSentSeq(0);
    setDomeRecovery(ONLINE_DOME_RECOVERY);
    lastSentTransformRef.current = null;
    resetBackendEventCursor();
  }

  const applyTransitionHandoff = useCallback((
    sourceRoom: GameRoomView, targetRoom: GameRoomView, targetTransform: AvatarTransform
  ) => {
    bindPhysicsTarget(targetRoom);
    setHandoffTransform(targetTransform);
    lastSentTransformRef.current = targetTransform;
    setLastSentSeq(0);
    setJoinedRoomIds((current) => {
      const next = new Set(current);
      next.delete(sourceRoom.room_id);
      next.add(targetRoom.room_id);
      return next;
    });
    setAdmittedIdentity(JSON.stringify([syncStatus.local_author_pubkey, activeTopic, activeChannelId, targetRoom.room_id, targetRoom.metaverse?.instance_generation]));
    setSelectedRoomId(targetRoom.room_id);
  }, [bindPhysicsTarget, activeTopic, activeChannelId, syncStatus.local_author_pubkey]);

  const { transitionPreparingDirections, requestTransitionAbort, handleTransitionTransform } = useDomeTransitionAttempt({
    actions, admittedRoom, localAuthorPubkey: syncStatus.local_author_pubkey, localPeerId,
    transitionNeighbors, setTransitionNeighbors, submitInputForRoom,
    onHandoff: applyTransitionHandoff, onError,
  });

  const transitionBoundaryStates = useMemo(() => {
    const states: Partial<Record<DomeDirection, DomeBoundaryStateV1>> = {};
    for (const neighbor of transitionNeighbors) {
      states[neighbor.direction] = domeRecovery.state === 'offline'
        ? 'offline'
        : transitionPreparingDirections.has(neighbor.direction)
        ? 'loading'
        : neighbor.boundaryState;
    }
    return states;
  }, [domeRecovery.state, transitionNeighbors, transitionPreparingDirections]);

  const joinRoom = useCallback(async (
    roomId: string,
    reportError = true,
    preserveCurrent = false
  ): Promise<boolean> => {
    const attempt = ++admissionAttempt.current;
    const room = rooms.find((candidate) => candidate.room_id === roomId);
    if (!room?.metaverse) return false;
    requestTransitionAbort();
    if (!preserveCurrent) {
      bindPhysicsTarget(room);
      setHandoffTransform(null);
      setSelectedRoomId(roomId);
      setAdmissionConfirmed(false);
    }
    setAdmissionStatus('admitting');
    try {
      const avatarCollider = await resolveLocalAvatarCollider();
      if (attempt !== admissionAttempt.current) return false;
      const snapshot = await submitInputForRoom(room, { type: 'join', avatar_collider: avatarCollider });
      if (attempt !== admissionAttempt.current) return false;
      if (snapshot.instance_id !== room.metaverse.instance_id || snapshot.instance_generation !== room.metaverse.instance_generation) throw new Error('DOME_ENTRY_STALE_INSTANCE');
      const body = snapshot.bodies.find(
        (candidate) => candidate.entity_id === `avatar:${syncStatus.local_author_pubkey}`
      );
      if (!body) throw new Error('DOME_ENTRY_CONFIRMATION_MISSING_AVATAR');
      const initialTransform: AvatarTransform = {
        roomId: room.room_id,
        peerId: localPeerId,
        seq: snapshot.sequence,
        position: body.position,
        rotation: body.rotation,
        animation: normalizeAvatarAnimationState(body.animation),
        sentAt: snapshot.simulated_at,
      };
      lastSentTransformRef.current = initialTransform;
      setHandoffTransform(initialTransform);
      bindPhysicsTarget(room);
      applyPhysicsSnapshot(snapshot);
      setJoinedRoomIds((current) => new Set(current).add(roomId));
      setAdmittedIdentity(JSON.stringify([syncStatus.local_author_pubkey, activeTopic, activeChannelId, room.room_id, room.metaverse.instance_generation]));
      setAdmissionConfirmed(true);
      setSelectedRoomId(roomId);
      setAdmissionStatus('joined');
      writeLastVisitedDome(
        syncStatus.local_author_pubkey,
        room.metaverse.spatial_context,
        room.metaverse.instance_id
      );
      onError(null);
      return true;
    } catch (entryError) {
      if (attempt !== admissionAttempt.current) return false;
      if (!preserveCurrent) {
        setAdmissionConfirmed(false);
        setAdmissionStatus('selection');
      } else {
        setAdmissionStatus('joined');
      }
      if (reportError) {
        onError(domeEntryErrorMessage(entryError, t));
      }
      return false;
    }
  }, [
    activeTopic,
    activeChannelId,
    bindPhysicsTarget,
    requestTransitionAbort,
    applyPhysicsSnapshot,
    localPeerId,
    onError,
    resolveLocalAvatarCollider,
    rooms,
    submitInputForRoom,
    syncStatus.local_author_pubkey,
    t,
  ]);

  const evacuate = useCallback(async (
    reason: 'host_offline' | 'access_revoked' | 'blocked' | 'user_requested'
  ): Promise<boolean> => {
    if (!admittedRoom || evacuationRunningRef.current) return false;
    evacuationRunningRef.current = true;
    const sourceRoom = admittedRoom;
    setDomeRecovery({ state: 'evacuating', secondsRemaining: null, reason, targetTitle: null });
    const adjacent = transitionNeighbors
      .filter((neighbor) => neighbor.boundaryState === 'ready')
      .map((neighbor) => neighbor.room);
    const ordered = reason === 'user_requested'
      ? [...entryCandidates, ...adjacent]
      : [...adjacent, ...entryCandidates];
    const seen = new Set<string>([sourceRoom.room_id]);
    const candidates = ordered.filter((candidate) => {
      if (seen.has(candidate.room_id)) return false;
      seen.add(candidate.room_id);
      return true;
    });
    try {
      for (const candidate of candidates) {
        setDomeRecovery({
          state: 'evacuating',
          secondsRemaining: null,
          reason,
          targetTitle: candidate.title,
        });
        if (!await joinRoom(candidate.room_id, false, true)) continue;
        await submitInputForRoom(sourceRoom, { type: 'leave' }).catch(() => undefined);
        setJoinedRoomIds((current) => {
          const next = new Set(current);
          next.delete(sourceRoom.room_id);
          next.add(candidate.room_id);
          return next;
        });
        setDomeRecovery({ state: 'online', secondsRemaining: null, reason: null, targetTitle: null });
        onError(t('recovery.moved', { target: candidate.title }));
        return true;
      }
      setSelectedRoomId(null);
      setAdmissionConfirmed(false);
      setAdmissionStatus('selection');
      entryAutoDisabledRef.current = true;
      disableMicrophone();
      stopSpatialAudio();
      setDomeRecovery({ state: 'no_candidate', secondsRemaining: null, reason, targetTitle: null });
      onError(t('recovery.noCandidate'));
      return false;
    } finally {
      evacuationRunningRef.current = false;
    }
  }, [
    admittedRoom,
    disableMicrophone,
    entryCandidates,
    joinRoom,
    onError,
    stopSpatialAudio,
    submitInputForRoom,
    t,
    transitionNeighbors,
  ]);

  useEffect(() => {
    if (!pendingEvacuationReason) return;
    const reason = pendingEvacuationReason;
    setPendingEvacuationReason(null);
    setDomeRecovery({ state: 'closed', secondsRemaining: 0, reason, targetTitle: null });
    void evacuate(reason);
  }, [evacuate, pendingEvacuationReason]);

  useEffect(() => {
    if (!admittedRoom?.metaverse) return;
    let cancelled = false;
    const pollHosting = async () => {
      try {
        const hosting = await actions.getHosting(
          admittedRoom.metaverse!.spatial_context,
          admittedRoom.metaverse!.instance_id
        );
        if (cancelled) return;
        if (hosting.state.kind === 'grace_period') {
          const deadline = (hosting.state.last_heartbeat_at ?? Date.now()) + 15_000;
          setDomeRecovery({
            state: 'offline',
            secondsRemaining: Math.max(0, Math.ceil((deadline - Date.now()) / 1_000)),
            reason: 'host_offline',
            targetTitle: null,
          });
        } else if (hosting.state.kind === 'closed' && hosting.state.reason === 'heartbeat_timeout') {
          setDomeRecovery({ state: 'closed', secondsRemaining: 0, reason: 'host_offline', targetTitle: null });
          void evacuate('host_offline');
        } else if (hosting.state.kind === 'owner_hosted' || hosting.state.kind === 'community_node_hosted') {
          setDomeRecovery({ state: 'online', secondsRemaining: null, reason: null, targetTitle: null });
        }
      } catch {
        setDomeRecovery((current) => current.state === 'online' ? {
          state: 'offline',
          secondsRemaining: 15,
          reason: 'host_offline',
          targetTitle: null,
        } : current);
      }
    };
    void pollHosting();
    const intervalId = window.setInterval(() => void pollHosting(), 1_000);
    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [actions, admittedRoom, evacuate]);

  useEffect(() => {
    const contextKey = spatialContextKey(entryContext);
    if (entryContextKeyRef.current !== contextKey) {
      entryContextKeyRef.current = contextKey;
      entryAttemptKeyRef.current = null;
      entryAutoDisabledRef.current = false;
      setAdmissionConfirmed(false);
      setSelectedRoomId(null);
    }
    if (managementActive || admissionConfirmed || entryAutoDisabledRef.current) return;
    const attemptKey = `${contextKey}:${entryCandidates.map((room) => `${room.room_id}:${room.metaverse?.instance_generation}`).join(',')}`;
    if (entryAttemptKeyRef.current === attemptKey) return;
    entryAttemptKeyRef.current = attemptKey;
    if (entryCandidates.length === 0) {
      setAdmissionStatus('selection');
      return;
    }
    setAdmissionStatus('resolving');
    let cancelled = false;
    void (async () => {
      for (const candidate of entryCandidates) {
        if (cancelled) return;
        if (await joinRoom(candidate.room_id, false)) return;
      }
      if (!cancelled) {
        setAdmissionStatus('selection');
        onError(t('entry.noAvailable'));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [managementActive, admissionConfirmed, entryCandidates, entryContext, joinRoom, onError, t]);

  function selectCreatedRoom(roomId: string) {
    pendingCreatedRoomIdRef.current = roomId;
    setSelectedRoomId(roomId);
    setAdmissionConfirmed(false);
    setAdmissionStatus('selection');
    if (rooms.some((room) => room.room_id === roomId)) {
      void joinRoom(roomId);
    }
  }

  function leaveRoom() {
    if (!admittedRoom) {
      setSelectedRoomId(null);
      return;
    }
    requestTransitionAbort();
    const roomId = admittedRoom.room_id;
    const leftAt = Date.now();
    emit({ type: 'presence.leave', roomId, peerId: localPeerId, leftAt });
    void actions.publishRoomEvent(roomId, localPeerId, leftAt, {
      type: 'presence_leave',
      room_id: roomId,
      peer_id: localPeerId,
      left_at: leftAt,
    }).catch((leaveError) => {
      onError(leaveError instanceof Error ? leaveError.message : t('errors.publishLeaveFailed'));
    });
    submitAuthoritativeInput({ type: 'leave' }, leftAt);
    setJoinedRoomIds((current) => {
      const next = new Set(current);
      next.delete(roomId);
      return next;
    });
    setSelectedRoomId(null);
    setAdmissionConfirmed(false);
    setAdmissionStatus('selection');
    entryAutoDisabledRef.current = true;
    disableMicrophone();
    stopSpatialAudio();
    resetRoomRuntimeState();
  }

  function handleLocalTransform(transform: AvatarTransform) {
    if (domeRecovery.state !== 'online') return;
    const previous = lastSentTransformRef.current;
    lastSentTransformRef.current = transform;
    setLastSentSeq(transform.seq);
    submitAuthoritativeInput({
      type: 'move',
      position: transform.position,
      rotation: transform.rotation,
      animation: transform.animation,
    }, transform.seq);
    handleTransitionTransform(transform, previous);
  }

  function handleSendMessage(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!admittedRoom || domeRecovery.state !== 'online' || !messageDraft.trim()) {
      return;
    }
    const message: RoomChatMessage = {
      roomId: admittedRoom.room_id,
      messageId: `${localPeerId}-${Date.now()}`,
      authorPeerId: localPeerId,
      displayName: localDisplayName,
      body: messageDraft.trim(),
      createdAt: Date.now(),
    };
    setMessages((current) => mergeRoomChatMessages(current, [message]));
    setLatestChatByPeer((current) => ({
      ...current,
      [message.authorPeerId]: latestChatBubbleFromMessage(message),
    }));
    setMessageDraft('');
    emit({ type: 'chat.message', message });
    void actions
      .publishRoomEvent(admittedRoom.room_id, localPeerId, Date.now(), {
        type: 'chat_message',
        message: {
          room_id: message.roomId,
          message_id: message.messageId,
          author_peer_id: message.authorPeerId,
          display_name: message.displayName,
          body: message.body,
          created_at: message.createdAt,
        },
      })
      .catch(() => {
        // Browser-only fallback is handled by BroadcastChannel.
      });
  }

  function moveSharedObject(delta: MetaverseVec3) {
    if (!admittedRoom) {
      return;
    }
    const current = sharedObjectRef.current;
    const nextObject: SharedRoomObjectV1 = {
      ...current,
      position: [
        current.position[0] + delta[0],
        current.position[1] + delta[1],
        current.position[2] + delta[2],
      ],
      updated_by: localPeerId,
      updated_at: Date.now(),
    };
    sharedObjectRef.current = nextObject;
    setSharedObject(nextObject);
    submitAuthoritativeInput({
      type: 'push',
      prop_id: current.object_id,
      impulse: delta,
    });
  }

  function interactWithProp(interaction: MetaverseInteractionKind) {
    if (!admittedRoom?.metaverse) return;
    const prop = admittedRoom.metaverse.dome.customization.persistent_props.find(
      (candidate) => candidate.prop_id === sharedObjectRef.current.object_id
    );
    if (!prop || prop.visual_only || !prop.interactions.includes(interaction)) return;

    const input = createDomeInteractionInput(interaction, prop.prop_id, localPeerId);
    const current = sharedObjectRef.current;
    if (input.type === 'sit') {
      const previous = lastSentTransformRef.current;
      handleLocalTransform({
        roomId: admittedRoom.room_id,
        peerId: localPeerId,
        seq: (previous?.seq ?? lastSentSeq) + 1,
        position: [current.position[0], current.position[1] + Math.ceil(current.scale[1] / 2), current.position[2]],
        rotation: previous?.rotation ?? [0, 0, 0],
        animation: 'idle',
        sentAt: input.issuedAt,
      });
      return;
    }

    const authoritativeInput: DomeSessionInputKindV1 = input.type === 'grab'
      ? { type: 'grab', prop_id: prop.prop_id }
      : input.type === 'throw'
        ? { type: 'throw', prop_id: prop.prop_id, impulse: [0, 100, -250] }
        : { type: 'push', prop_id: prop.prop_id, impulse: [0, 0, -50] };
    submitAuthoritativeInput(authoritativeInput, input.issuedAt);
  }

  const submitPropMutation = useCallback(async (input: DomeSessionInputKindV1) => {
    if (!admittedRoom?.metaverse) {
      throw new Error(t('errors.roomRequired'));
    }
    const snapshot = await actions.submitSessionInput(
      admittedRoom.metaverse.spatial_context,
      admittedRoom.metaverse.instance_id,
      Date.now(),
      input,
      admittedRoom.metaverse.instance_generation
    );
    applyPhysicsSnapshot(snapshot);
  }, [actions, admittedRoom, applyPhysicsSnapshot, t]);

  const newSessionProp = useCallback((kind: 'guest' | 'persistent'): MetaversePersistentPropV1 => ({
    prop_id: `${kind}-${localPeerId}-${Date.now()}`,
    asset_ref: null,
    primitive_fallback: 'cube',
    position: [0, 150, -250],
    rotation: [0, 0, 0],
    scale: [100, 100, 100],
    visual_only: false,
    interactions: ['grab', 'throw', 'push'],
    collider: {
      shape: 'cuboid',
      center: [0, 0, 0],
      half_extents: [50, 50, 50],
    },
  }), [localPeerId]);

  const spawnGuestProp = useCallback(
    () => submitPropMutation({
      type: 'spawn_guest_prop',
      prop: newSessionProp('guest'),
      expires_at: Date.now() + 5 * 60 * 1000,
    }),
    [newSessionProp, submitPropMutation]
  );

  const addPersistentProp = useCallback(
    () => submitPropMutation({
      type: 'upsert_persistent_prop',
      prop: newSessionProp('persistent'),
    }),
    [newSessionProp, submitPropMutation]
  );

  const deletePersistentProp = useCallback(() => {
    const propId = [...sessionProps]
      .reverse()
      .find((prop) => prop.kind === 'persistent_prop')?.object.object_id;
    if (!propId) {
      return Promise.resolve();
    }
    return submitPropMutation({ type: 'delete_persistent_prop', prop_id: propId });
  }, [sessionProps, submitPropMutation]);

  return {
    selectedRoomId,
    joinedRoomIds,
    selectedRoom,
    admittedRoom,
    admissionStatus,
    localPeerId,
    remoteTransforms,
    peerPresence,
    messages,
    latestChatByPeer,
    messageDraft,
    setMessageDraft,
    sharedObject,
    sessionProps,
    transitionNeighbors,
    transitionBoundaryStates,
    connections,
    handoffTransform,
    lastSentSeq,
    lastReceivedAt,
    remoteAnimationSummary,
    roomConnectionState,
    knownPeerCount,
    clockNow,
    domeRecovery,
    returnHome: () => void evacuate('user_requested'),
    joinRoom,
    selectCreatedRoom,
    leaveRoom,
    handleLocalTransform,
    handleSendMessage,
    moveSharedObject,
    interactWithProp,
    spawnGuestProp,
    addPersistentProp,
    deletePersistentProp,
    microphoneEnabled,
    toggleMicrophone,
  };
}
