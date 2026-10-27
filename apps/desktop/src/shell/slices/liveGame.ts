import type { GameRoomStatus, GameRoomView, LiveSessionView, PostView } from '@/lib/api';

import type { AsyncPanelState } from '@/shell/slices/shared';

/// ライブセッション・ゲームルーム(WP-H6 PR3 のドメインスライス)。

export type GameEditorDraft = {
  status: GameRoomStatus;
  phase_label: string;
  scores: Record<string, string>;
};

export type LiveGameSliceState = {
  liveSessionsByScopeKey: Record<string, LiveSessionView[]>;
  gameRoomsByScopeKey: Record<string, GameRoomView[]>;
  liveError: string | null;
  livePanelStateByScopeKey: Record<string, AsyncPanelState>;
  livePendingBySessionId: Record<string, true>;
  selectedLiveSessionId: string | null;
  // Liveの開始postとPushで表示中のpost。表示面のobserverごとに持ち、通常のmedia/gate ownerと共用する。
  liveStagePosts: Record<string, PostView>;
  gameTitle: string;
  gameDescription: string;
  gameParticipantsInput: string;
  gameError: string | null;
  gameDrafts: Record<string, GameEditorDraft>;
  gamePanelStateByScopeKey: Record<string, AsyncPanelState>;
  gameCreatePending: boolean;
  gameSavingByRoomId: Record<string, true>;
  selectedGameRoomId: string | null;
};

export function createInitialLiveGameSlice(): LiveGameSliceState {
  return {
    liveSessionsByScopeKey: {},
    gameRoomsByScopeKey: {},
    liveError: null,
    livePanelStateByScopeKey: {},
    livePendingBySessionId: {},
    selectedLiveSessionId: null,
    liveStagePosts: {},
    gameTitle: '',
    gameDescription: '',
    gameParticipantsInput: '',
    gameError: null,
    gameDrafts: {},
    gamePanelStateByScopeKey: {},
    gameCreatePending: false,
    gameSavingByRoomId: {},
    selectedGameRoomId: null,
  };
}
