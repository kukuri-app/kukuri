import type { FormEvent } from 'react';

import type { ChannelRef, GameRoomView, GameScoreView } from '@/lib/api';

import { removeRecordEntry, setRecordEntry } from '@/shell/stateUpdates';
import { createGameEditorDraft, messageFromError } from '@/shell/presentation';
import type { GameEditorDraft } from '@/shell/store';

import type {
  ActionsBaseParams,
  BoolStateDispatch,
  Setter,
} from './shared';
import { explainScopeLimit } from '@/shell/columnScopeLeases';

type LiveGameParams = ActionsBaseParams & {
  activeComposeChannel: ChannelRef;
  activeGameRooms: GameRoomView[];
  activeTopic: string;
  gameDescription: string;
  gameDrafts: Record<string, GameEditorDraft>;
  gameParticipantsInput: string;
  gameTitle: string;
  liveDescription: string;
  liveTitle: string;
  peerTicket: string;
  selectedThread: string | null;
  trackedTopics: string[];
  setPeerTicket: Setter<'peerTicket'>;
  setLiveTitle: Setter<'liveTitle'>;
  setLiveDescription: Setter<'liveDescription'>;
  setLiveError: Setter<'liveError'>;
  setLivePendingBySessionId: Setter<'livePendingBySessionId'>;
  setLiveCreatePending: Setter<'liveCreatePending'>;
  setShellChromeState: Setter<'shellChromeState'>;
  setGameTitle: Setter<'gameTitle'>;
  setGameDescription: Setter<'gameDescription'>;
  setGameParticipantsInput: Setter<'gameParticipantsInput'>;
  setGameError: Setter<'gameError'>;
  setGameDrafts: Setter<'gameDrafts'>;
  setGameSavingByRoomId: Setter<'gameSavingByRoomId'>;
  setGameCreatePending: Setter<'gameCreatePending'>;
  setError: Setter<'error'>;
  setLiveCreateDialogOpen: BoolStateDispatch;
  setGameCreateDialogOpen: BoolStateDispatch;
};

export function createLiveGameActions({
  api,
  translate,
  loadTopics,
  syncRoute,
  activeComposeChannel,
  activeGameRooms,
  activeTopic,
  gameDescription,
  gameDrafts,
  gameParticipantsInput,
  gameTitle,
  liveDescription,
  liveTitle,
  peerTicket,
  selectedThread,
  trackedTopics,
  setPeerTicket,
  setLiveTitle,
  setLiveDescription,
  setLiveError,
  setLivePendingBySessionId,
  setLiveCreatePending,
  setShellChromeState,
  setGameTitle,
  setGameDescription,
  setGameParticipantsInput,
  setGameError,
  setGameDrafts,
  setGameSavingByRoomId,
  setGameCreatePending,
  setError,
  setLiveCreateDialogOpen,
  setGameCreateDialogOpen,
}: LiveGameParams) {
  async function handleImportPeer() {
    if (!peerTicket.trim()) {
      return;
    }
    try {
      await api.importPeerTicket(peerTicket.trim());
      setPeerTicket('');
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (importError) {
      setError(
        importError instanceof Error
          ? importError.message
          : translate('common:errors.failedToImportPeer')
      );
    }
  }

  async function handleCreateLiveSession(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!liveTitle.trim()) {
      setLiveError(translate('live:errors.titleRequired'));
      return;
    }
    setLiveCreatePending(true);
    try {
      await api.createLiveSession(
        activeTopic,
        liveTitle.trim(),
        liveDescription.trim(),
        activeComposeChannel
      );
      setLiveTitle('');
      setLiveDescription('');
      setLiveError(null);
      setLiveCreateDialogOpen(false);
      setShellChromeState((current) => ({
        ...current,
      }));
      syncRoute('replace', {
        primarySection: 'live',
      });
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (liveCreateError) {
      setLiveError(messageFromError(liveCreateError, translate('live:errors.failedCreate')));
    } finally {
      setLiveCreatePending(false);
    }
  }

  async function handleJoinLiveSession(sessionId: string, topic: string = activeTopic) {
    setLivePendingBySessionId(setRecordEntry(sessionId, true));
    try {
      await explainScopeLimit(api.joinLiveSession(topic, sessionId));
      setLiveError(null);
      await loadTopics(trackedTopics, topic, selectedThread);
    } catch (joinError) {
      setLiveError(messageFromError(joinError, translate('live:errors.failedJoin')));
    } finally {
      setLivePendingBySessionId(removeRecordEntry(sessionId));
    }
  }

  async function handleLeaveLiveSession(sessionId: string, topic: string = activeTopic) {
    setLivePendingBySessionId(setRecordEntry(sessionId, true));
    try {
      await api.leaveLiveSession(topic, sessionId);
      setLiveError(null);
      await loadTopics(trackedTopics, topic, selectedThread);
    } catch (leaveError) {
      setLiveError(messageFromError(leaveError, translate('live:errors.failedLeave')));
    } finally {
      setLivePendingBySessionId(removeRecordEntry(sessionId));
    }
  }

  async function handleEndLiveSession(sessionId: string, topic: string = activeTopic) {
    setLivePendingBySessionId(setRecordEntry(sessionId, true));
    try {
      await api.endLiveSession(topic, sessionId);
      setLiveError(null);
      await loadTopics(trackedTopics, topic, selectedThread);
    } catch (endError) {
      setLiveError(messageFromError(endError, translate('live:errors.failedEnd')));
    } finally {
      setLivePendingBySessionId(removeRecordEntry(sessionId));
    }
  }

  async function handleCreateGameRoom(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const participants = Array.from(
      new Set(
        gameParticipantsInput
          .split(',')
          .map((value) => value.trim())
          .filter((value) => value.length > 0)
      )
    );
    if (!gameTitle.trim()) {
      setGameError(translate('game:errors.titleRequired'));
      return;
    }
    if (participants.length < 2) {
      setGameError(translate('game:errors.participantsRequired'));
      return;
    }
    setGameCreatePending(true);
    try {
      await api.createGameRoom(
        activeTopic,
        gameTitle.trim(),
        gameDescription.trim(),
        participants,
        activeComposeChannel
      );
      setGameTitle('');
      setGameDescription('');
      setGameParticipantsInput('');
      setGameError(null);
      setGameCreateDialogOpen(false);
      setShellChromeState((current) => ({
        ...current,
      }));
      syncRoute('replace', {
        primarySection: 'game',
      });
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (createError) {
      setGameError(messageFromError(createError, translate('game:errors.failedCreate')));
    } finally {
      setGameCreatePending(false);
    }
  }

  function updateGameDraft(roomId: string, update: (draft: GameEditorDraft) => GameEditorDraft) {
    setGameDrafts((current) => {
      const existingRoom = activeGameRooms.find((room) => room.room_id === roomId);
      const draft = current[roomId] ?? (existingRoom ? createGameEditorDraft(existingRoom) : null);
      if (!draft) {
        return current;
      }
      return {
        ...current,
        [roomId]: update(draft),
      };
    });
  }

  async function handleUpdateGameRoom(roomId: string) {
    const room = activeGameRooms.find((candidate) => candidate.room_id === roomId);
    if (!room) {
      return;
    }
    const draft = gameDrafts[room.room_id] ?? createGameEditorDraft(room);
    const scores: GameScoreView[] = [];
    for (const score of room.scores) {
      const rawScore = draft.scores[score.participant_id] ?? String(score.score);
      const parsed = Number.parseInt(rawScore, 10);
      if (Number.isNaN(parsed)) {
        setGameError(translate('game:errors.invalidScore', { label: score.label }));
        return;
      }
      scores.push({
        participant_id: score.participant_id,
        label: score.label,
        score: parsed,
      });
    }
    setGameSavingByRoomId(setRecordEntry(room.room_id, true));
    try {
      await api.updateGameRoom(
        activeTopic,
        room.room_id,
        draft.status,
        draft.phase_label.trim() || null,
        scores
      );
      setGameError(null);
      setGameDrafts(removeRecordEntry(room.room_id));
      await loadTopics(trackedTopics, activeTopic, selectedThread);
    } catch (updateError) {
      setGameError(messageFromError(updateError, translate('game:errors.failedUpdate')));
    } finally {
      setGameSavingByRoomId(removeRecordEntry(room.room_id));
    }
  }

  return {
    handleImportPeer,
    handleCreateLiveSession,
    handleJoinLiveSession,
    handleLeaveLiveSession,
    handleEndLiveSession,
    handleCreateGameRoom,
    updateGameDraft,
    handleUpdateGameRoom,
  };
}
