import {
  type ChannelAudienceOption,
  type InviteOutputLabel,
  type PrivateChannelPendingAction,
} from '@/components/extended/types';
import type { JoinedPrivateChannelView } from '@/lib/api';

import {
  type AsyncPanelState,
  DEFAULT_ASYNC_PANEL_STATE,
  buildStarterTopicRecord,
} from '@/shell/slices/shared';

/// プライベートチャンネル(参加・作成・共有トークン)(WP-H6 PR3 のドメインスライス)。
export type PrivateChannelSliceState = {
  joinedChannelsByTopic: Record<string, JoinedPrivateChannelView[]>;
  // 読み込んだ一覧の続きの cursor(#1218 AC-4d)。null は最後まで読んだ、未設定はまだ読んでいない。
  joinedChannelsNextCursorByTopic: Record<string, string | null>;
  channelLabelInput: string;
  channelAudienceInput: ChannelAudienceOption['value'];
  inviteTokenInput: string;
  inviteOutput: string | null;
  inviteOutputLabel: InviteOutputLabel;
  channelError: string | null;
  channelPanelStateByTopic: Record<string, AsyncPanelState>;
  channelActionPending: PrivateChannelPendingAction;
};

export function createInitialPrivateChannelSlice(): PrivateChannelSliceState {
  return {
    joinedChannelsByTopic: buildStarterTopicRecord(() => [] as JoinedPrivateChannelView[]),
    joinedChannelsNextCursorByTopic: {},
    channelLabelInput: '',
    channelAudienceInput: 'invite_only',
    inviteTokenInput: '',
    inviteOutput: null,
    inviteOutputLabel: 'invite',
    channelError: null,
    channelPanelStateByTopic: buildStarterTopicRecord(() => ({ ...DEFAULT_ASYNC_PANEL_STATE })),
    channelActionPending: null,
  };
}
