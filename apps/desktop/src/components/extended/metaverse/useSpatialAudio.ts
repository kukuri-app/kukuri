import { useCallback, useEffect, useRef, useState, type RefObject } from 'react';

import type { GameRoomView, SetMetaverseAudioRequest } from '@/lib/api';
import type { AvatarTransform, PeerPresence } from '../MetaverseSceneModel';
import type { DomeNeighborTransitionView } from './DomeTransitionModel';
import type { MetaverseRoomActions } from './MetaverseRoomActions';
import { spatialAudioRequest } from './SpatialAudioModel';

/** 位置・話者・mic の変化を Rust の空間音声へ渡す間隔。 */
const SPATIAL_AUDIO_SYNC_INTERVAL_MS = 200;
/** 同じ内容を送り直す間隔。終わった購読(切断・話者の作り直し・満員)を Rust が試し直す(ADR 0067 §4)。 */
const SPATIAL_AUDIO_RESEND_INTERVAL_MS = 5_000;

const STOP_REQUEST: SetMetaverseAudioRequest = {
  room: null,
  microphone_enabled: false,
  listener_position: [0, 0, 0],
  neighbors: [],
  speakers: [],
};

type UseSpatialAudioArgs = {
  actions: MetaverseRoomActions;
  /** 参加中で online の Dome。`null` で取り込み・送信・購読・再生をすべて止める。 */
  selectedRoom: GameRoomView | null;
  listenerTransformRef: RefObject<AvatarTransform | null>;
  mutedAuthorPubkeys: ReadonlySet<string>;
  transitionNeighbors: DomeNeighborTransitionView[];
  peerPresence: Record<string, PeerPresence>;
  remoteTransforms: Record<string, AvatarTransform>;
};

/**
 * 空間音声(ADR 0067)。取り込み・圧縮・再生・距離減衰は Rust が行い、画面は mic の操作と、聞き手の位置・ready な
 * 隣接 Dome・音声を送っている話者(近い順)を渡す。mic は利用者の操作の後、機器を開けたときだけ on と示す。
 */
export function useSpatialAudio({
  actions,
  selectedRoom,
  listenerTransformRef,
  mutedAuthorPubkeys,
  transitionNeighbors,
  peerPresence,
  remoteTransforms,
}: UseSpatialAudioArgs) {
  const [microphoneRequested, setMicrophoneRequested] = useState(false);
  const [microphoneEnabled, setMicrophoneEnabled] = useState(false);
  const inputsRef = useRef({ transitionNeighbors, peerPresence, remoteTransforms, mutedAuthorPubkeys, microphoneRequested });
  const roomRef = useRef(selectedRoom);
  // actions を作り直しても(画面の他の操作)音声を止めない。
  const setAudioRef = useRef(actions.setAudio);
  // Rust へは送った順に 1 件ずつ反映する(前の Dome への遅れた要求が、後の Dome・停止を上書きしない)。
  const queueRef = useRef<Promise<unknown>>(Promise.resolve());
  // 周期の送信は描画の後の最新の入力を読む。
  useEffect(() => {
    inputsRef.current = { transitionNeighbors, peerPresence, remoteTransforms, mutedAuthorPubkeys, microphoneRequested };
    roomRef.current = selectedRoom;
    setAudioRef.current = actions.setAudio;
  });
  const audioAvailable = Boolean(actions.setAudio);
  const roomKey = selectedRoom?.metaverse
    ? `${selectedRoom.room_id}:${selectedRoom.metaverse.instance_generation}:${selectedRoom.metaverse.session_id}`
    : null;

  useEffect(() => {
    if (!audioAvailable || !roomKey) return;
    const send = (request: SetMetaverseAudioRequest) => {
      const sent = queueRef.current.then(() => setAudioRef.current?.(request));
      queueRef.current = sent.catch(() => undefined);
      return sent;
    };
    let cancelled = false;
    let inFlight = false;
    let lastSent = '';
    let lastSentAt = 0;
    const sync = async () => {
      if (inFlight) return;
      const now = Date.now();
      const inputs = inputsRef.current;
      const request = spatialAudioRequest({
        room: roomRef.current,
        microphoneEnabled: inputs.microphoneRequested,
        listenerPosition: listenerTransformRef.current?.position ?? [0, 100, 0],
        neighbors: inputs.transitionNeighbors,
        presences: inputs.peerPresence,
        remoteTransforms: inputs.remoteTransforms,
        mutedAuthorPubkeys: inputs.mutedAuthorPubkeys,
        now,
      });
      const key = JSON.stringify(request);
      if (key === lastSent && now - lastSentAt < SPATIAL_AUDIO_RESEND_INTERVAL_MS) return;
      inFlight = true;
      try {
        const status = await send(request);
        if (!status) return;
        lastSent = key;
        lastSentAt = now;
        if (cancelled) return;
        setMicrophoneEnabled(status.microphone === 'on');
        // 機器を開けなかったら(OS の許可を含む)、mic の要求を取り消す(開始を表示しない)。
        if (request.microphone_enabled && status.microphone !== 'on') setMicrophoneRequested(false);
      } catch {
        // 次の周期で送り直す。
      } finally {
        inFlight = false;
      }
    };
    void sync();
    const intervalId = window.setInterval(() => void sync(), SPATIAL_AUDIO_SYNC_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
      setMicrophoneEnabled(false);
      void send(STOP_REQUEST).catch(() => undefined);
    };
  }, [audioAvailable, listenerTransformRef, roomKey]);

  const toggleMicrophone = useCallback(() => setMicrophoneRequested((requested) => !requested), []);
  const disableMicrophone = useCallback(() => setMicrophoneRequested(false), []);

  return { microphoneEnabled, toggleMicrophone, disableMicrophone };
}
