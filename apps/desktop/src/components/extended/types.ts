import type {
  ChannelAudienceKind,
  GameRoomStatus,
  JoinedPrivateChannelView,
  LiveSessionView,
} from '@/lib/api';

export type ExtendedPanelStatus = 'loading' | 'ready' | 'error';

export type ProfileEditorFields = {
  displayName: string;
  name: string;
  about: string;
};

export type PrivateChannelPendingAction =
  | 'create'
  | 'join'
  | 'leave'
  | 'share'
  | 'take'
  | null;

export type InviteOutputLabel = 'invite' | 'grant' | 'share';

export type PrivateChannelListItemView = {
  channel: JoinedPrivateChannelView;
  active: boolean;
};

export type LiveSessionPendingMap = Record<string, true>;

export type GameDraftView = {
  status: GameRoomStatus;
  phaseLabel: string;
  scores: Record<string, string>;
};

export type GameRoomPendingMap = Record<string, true>;

export type LiveSessionListItemView = {
  session: LiveSessionView;
  isOwner: boolean;
  pending: boolean;
};

export type ChannelAudienceOption = {
  value: ChannelAudienceKind;
  label: string;
};
